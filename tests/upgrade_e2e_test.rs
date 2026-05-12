//! E2E for `pgforge upgrade`. Builds a 17→18 upgrade against real Docker.
//! Gated by PGFORGE_E2E=1; uses --no-backup so no S3 is required.
//!
//! Why 17→18 specifically: pgdg hasn't published `postgresql-19` as of
//! 2026-05, so the highest version we can install is 18. The test pair
//! just needs `to > from` to exercise the upgrade machinery — switch to
//! 18→19 once pgdg ships pg 19.

use pgforge::commands::create::{CreateArgs, run_with_engine as create_run};
use pgforge::commands::upgrade::{UpgradeArgs, run_with_engine as upgrade_run};
use pgforge::config::global::GlobalConfig;
use pgforge::docker::bollard_engine::BollardEngine;
use pgforge::docker::engine::DockerEngine;
use pgforge::domain::preset::Preset;
use pgforge::state::instance::InstanceState;
use std::time::Duration;
use tempfile::TempDir;

#[tokio::test]
async fn upgrade_17_to_18_preserves_data() {
    if std::env::var("PGFORGE_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping: set PGFORGE_E2E=1 to run");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let state_root = tmp.path().to_path_buf();
    let docker = BollardEngine::connect().expect("docker engine reachable");
    let global = GlobalConfig {
        port_range_start: 5820,
        port_range_end: 5899,
        ..Default::default()
    };
    let name = format!("pgforge_e2e_upgrade_{}", uniq_suffix());

    // 1. Create no-backup at PG 17.
    let state = create_run(
        CreateArgs {
            name: name.clone(),
            preset: Preset::Tiny,
            pg_version: 17,
            app_user: "leads".into(),
            app_password: "pw".into(),
            pgbackrest_password: String::new(),
            override_state_root: Some(state_root.clone()),
            no_backup: true,
        retain_days: 30,
        snapshot_hour: None,
        },
        &docker,
        state_root.clone(),
        global.clone(),
        None,
    )
    .await
    .expect("create at pg 17 must succeed");
    let container = format!("pgforge_{}", name);
    poll_pg_ready(state.instance.host_port).await;

    // 2. Seed data.
    let seed = docker
        .exec(
            &container,
            &[
                "su", "-", "postgres", "-c",
                &format!(
                    "psql -tA -U leads -d {name} -c \"\
                     CREATE TABLE upgrade_marker(v int); \
                     INSERT INTO upgrade_marker VALUES (17);\""
                ),
            ],
        )
        .await
        .expect("seed");
    assert_eq!(seed.exit_code, 0, "seed failed: stderr={:?}", seed.stderr);

    // 3. Upgrade to PG 18. This builds the upgrade image (apt-installs
    // postgresql-17 over postgres:18-bookworm) — first run takes minutes.
    let upgrade_res = upgrade_run(
        UpgradeArgs {
            name: name.clone(),
            to_version: 18,
            override_state_root: Some(state_root.clone()),
        },
        &docker,
        state_root.clone(),
        global,
    )
    .await;

    if let Err(e) = &upgrade_res {
        eprintln!("upgrade failed: {e}. Containers/volumes left for inspection.");
    }
    upgrade_res.expect("upgrade must succeed");

    // 4. state.toml must reflect new version + new volume override.
    let new_state = InstanceState::load_under(&state_root, &name).expect("reload state");
    assert_eq!(new_state.instance.pg_version, 18, "pg_version must bump to 18");
    assert!(
        new_state.instance.volume_name_override.is_some(),
        "volume_name_override must be set post-upgrade"
    );
    let new_vol = new_state.instance.volume_name();
    assert!(new_vol.contains("_v18"), "new vol name must encode v18, got {new_vol}");

    // 5. The upgraded instance must accept connections and retain the
    // marker row.
    poll_pg_ready(state.instance.host_port).await;
    let read = docker
        .exec(
            &container,
            &[
                "su", "-", "postgres", "-c",
                &format!(
                    "psql -tA -U leads -d {name} -c \"SELECT v FROM upgrade_marker;\""
                ),
            ],
        )
        .await
        .expect("read marker");
    assert_eq!(read.stdout.trim(), "17", "data must survive upgrade");

    // 6. Server reports the new major version.
    let ver = docker
        .exec(
            &container,
            &[
                "su", "-", "postgres", "-c",
                &format!(
                    "psql -tA -U leads -d {name} -c \"SHOW server_version_num;\""
                ),
            ],
        )
        .await
        .expect("show version");
    assert!(
        ver.stdout.trim().starts_with("18"),
        "server_version_num must report 18*, got {:?}",
        ver.stdout
    );

    // Cleanup
    let _ = std::process::Command::new("docker")
        .args(["rm", "-f", &container])
        .output();
    let _ = std::process::Command::new("docker")
        .args(["volume", "rm", "-f", &format!("pgforge_data_{}", name)])
        .output();
    let _ = std::process::Command::new("docker")
        .args(["volume", "rm", "-f", &new_vol])
        .output();
}

fn uniq_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .to_string()
}

async fn poll_pg_ready(port: u16) {
    use tokio::net::TcpStream;
    for _ in 0..60 {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("port {port} did not accept TCP within 60s");
}
