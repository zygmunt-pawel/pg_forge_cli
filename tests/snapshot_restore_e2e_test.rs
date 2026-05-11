//! End-to-end: create source instance, snapshot, restore as a new instance,
//! verify it's reachable. Gated by PGFORGE_E2E=1. Requires real Docker + S3.

use pgforge::commands::create::{CreateArgs, run_with_engine as create_run};
use pgforge::commands::restore::{RestoreArgs, run_with_engine as restore_run};
use pgforge::commands::snapshot::{SnapshotArgs, run_with_engine as snapshot_run};
use pgforge::config::global::GlobalConfig;
use pgforge::docker::bollard_engine::BollardEngine;
use pgforge::domain::preset::Preset;
use pgforge::pgbackrest::conf::S3Settings;
use std::time::Duration;
use tempfile::TempDir;

fn fake_s3() -> S3Settings {
    S3Settings {
        bucket: std::env::var("PGFORGE_E2E_BUCKET").unwrap_or_else(|_| "pgforge-e2e".into()),
        region: std::env::var("PGFORGE_E2E_REGION").unwrap_or_else(|_| "eu-central-1".into()),
        endpoint: std::env::var("PGFORGE_E2E_ENDPOINT")
            .unwrap_or_else(|_| "s3.eu-central-1.amazonaws.com".into()),
        access_key: std::env::var("PGFORGE_E2E_S3_KEY").unwrap_or_else(|_| "AKIAFAKE".into()),
        secret_key: std::env::var("PGFORGE_E2E_S3_SECRET").unwrap_or_else(|_| "secret".into()),
    }
}

#[tokio::test]
async fn snapshot_then_restore_round_trip() {
    if std::env::var("PGFORGE_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping: set PGFORGE_E2E=1 to run");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let state_root = tmp.path().to_path_buf();
    let docker = BollardEngine::connect().expect("docker engine reachable");
    let s3 = fake_s3();
    let global = GlobalConfig { s3: Some(s3.clone()), ..Default::default() };

    let suffix = uniq_suffix();
    let src_name = format!("pgforge_e2e_src_{suffix}");
    let restored_name = format!("pgforge_e2e_rec_{suffix}");

    // 1. Create source.
    let src_state = create_run(
        CreateArgs {
            name: src_name.clone(),
            preset: Preset::Tiny,
            pg_version: 18,
            app_user: "leads".into(),
            app_password: "pw".into(),
            pgbackrest_password: "rpw".into(),
            override_state_root: Some(state_root.clone()),
        },
        &docker,
        state_root.clone(),
        global.clone(),
        s3.clone(),
    )
    .await
    .expect("create source");

    poll_tcp_ready(src_state.instance.host_port, 30).await;

    // 2. Snapshot.
    let rec = snapshot_run(
        SnapshotArgs {
            instance: src_name.clone(),
            user_label: Some("e2e".into()),
            override_state_root: Some(state_root.clone()),
        },
        &docker,
        state_root.clone(),
    )
    .await
    .expect("snapshot");
    eprintln!("snapshot label: {} taken_at: {}", rec.label, rec.taken_at);

    // 3. Restore as a new instance (latest, no target-time).
    let restored = restore_run(
        RestoreArgs {
            source: src_name.clone(),
            as_name: restored_name.clone(),
            target_time: None,
            override_state_root: Some(state_root.clone()),
        },
        &docker,
        state_root.clone(),
        global,
        s3,
        src_state,
    )
    .await;

    // Always cleanup before assert.
    cleanup(&src_name);
    cleanup(&restored_name);

    let restored = restored.expect("restore should succeed");
    assert_ne!(
        restored.instance.host_port,
        0,
        "restored instance must have a real port"
    );
    poll_tcp_ready(restored.instance.host_port, 600).await; // up to 10 min
}

fn uniq_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos().to_string()
}

fn cleanup(name: &str) {
    let _ = std::process::Command::new("docker")
        .args(["rm", "-f", &format!("pgforge_{name}")])
        .output();
    let _ = std::process::Command::new("docker")
        .args(["volume", "rm", "-f", &format!("pgforge_data_{name}")])
        .output();
}

async fn poll_tcp_ready(port: u16, seconds: u64) {
    use tokio::net::TcpStream;
    for _ in 0..seconds {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("port {port} did not accept TCP within {seconds}s");
}
