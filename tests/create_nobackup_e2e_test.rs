//! E2E test for `pgforge create --no-backup`. Gated by PGFORGE_E2E=1 (needs
//! a real Docker engine) but unlike snapshot/clone E2E tests this one does
//! NOT need any S3 credentials.

use pgforge::commands::create::{CreateArgs, run_with_engine};
use pgforge::config::global::GlobalConfig;
use pgforge::docker::bollard_engine::BollardEngine;
use pgforge::domain::preset::Preset;
use std::time::Duration;
use tempfile::TempDir;

#[tokio::test]
async fn create_nobackup_tiny_instance() {
    if std::env::var("PGFORGE_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping: set PGFORGE_E2E=1 to run");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let docker = BollardEngine::connect().expect("docker engine reachable");
    // No S3 in global config — that's the whole point of --no-backup.
    // Use a high port range so the test doesn't collide with whatever the
    // developer already has bound on the default 5433-5500 range.
    let global = GlobalConfig {
        port_range_start: 5800,
        port_range_end: 5899,
        ..Default::default()
    };
    let name = format!("pgforge_e2e_nobackup_{}", uniq_suffix());

    let res = run_with_engine(
        CreateArgs {
            name: name.clone(),
            preset: Preset::Tiny,
            pg_version: 18,
            app_user: "leads".into(),
            app_password: "pw".into(),
            // Not used because no_backup=true skips pgbackrest entirely.
            pgbackrest_password: String::new(),
            override_state_root: Some(tmp.path().to_path_buf()),
            no_backup: true,
        retain_days: 30,
        },
        &docker,
        tmp.path().to_path_buf(),
        global,
        None,
    )
    .await;

    let container = format!("pgforge_{}", name);
    // On failure leave the container for inspection.
    if res.is_err() {
        eprintln!("FAILED — leaving container {container} for inspection. `docker logs {container}`");
    }
    let state = res.expect("create --no-backup should succeed without S3");
    assert!(state.instance.host_port >= 5433);
    assert!(
        !state.instance.backup_enabled,
        "state.toml must record backup_enabled=false for --no-backup instances"
    );

    poll_pg_ready(state.instance.host_port).await;

    // Cleanup AFTER the poll so we actually exercise the running instance.
    let _ = std::process::Command::new("docker")
        .args(["rm", "-f", &container])
        .output();
    let _ = std::process::Command::new("docker")
        .args(["volume", "rm", "-f", &format!("pgforge_data_{}", name)])
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
    for _ in 0..30 {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("port {port} did not accept TCP within 30s");
}
