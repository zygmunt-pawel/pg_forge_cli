//! End-to-end test: actually create a tiny PG instance via pgforge,
//! connect to it, drop it. Gated by `PGFORGE_E2E=1` so CI doesn't run it.
//!
//! Requires: a running Docker engine (Docker Desktop, OrbStack, etc.) and
//! S3 creds in the global config OR a stubbed config in the temp dir.

use pgforge::commands::create::{CreateArgs, run_with_engine};
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
async fn create_tiny_instance_then_cleanup() {
    if std::env::var("PGFORGE_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping: set PGFORGE_E2E=1 to run");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let docker = BollardEngine::connect().expect("docker engine reachable");
    let global = GlobalConfig {
        s3: Some(fake_s3()),
        ..Default::default()
    };
    let name = format!("pgforge_e2e_{}", uniq_suffix());
    let res = run_with_engine(
        CreateArgs {
            name: name.clone(),
            preset: Preset::Tiny,
            pg_version: 18,
            app_user: "leads".into(),
            app_password: "pw".into(),
            pgbackrest_password: "rpw".into(),
            override_state_root: Some(tmp.path().to_path_buf()),
        },
        &docker,
        tmp.path().to_path_buf(),
        global,
        fake_s3(),
    )
    .await;

    // Cleanup before assert so a failure leaves no residue.
    let container = format!("pgforge_{}", name);
    let _ = std::process::Command::new("docker")
        .args(["rm", "-f", &container])
        .output();
    let _ = std::process::Command::new("docker")
        .args(["volume", "rm", "-f", &format!("pgforge_data_{}", name)])
        .output();

    let state = res.expect("create should succeed");
    assert_eq!(state.instance.preset, Preset::Tiny);
    assert!(state.instance.host_port >= 5433);

    // Poll the host port until PG accepts connections.
    poll_pg_ready(state.instance.host_port).await;
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
    panic!("postgres on port {port} did not accept TCP within 30s");
}
