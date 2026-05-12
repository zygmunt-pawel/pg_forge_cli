//! E2E test for `pgforge rotate`. Builds on the --no-backup flow so the
//! test doesn't need S3 credentials. Gated by PGFORGE_E2E=1.

use pgforge::commands::create::{CreateArgs, run_with_engine as create_run};
use pgforge::commands::rotate::{RotateArgs, run_with_engine as rotate_run};
use pgforge::config::global::GlobalConfig;
use pgforge::docker::bollard_engine::BollardEngine;
use pgforge::docker::engine::DockerEngine;
use pgforge::domain::preset::Preset;
use std::time::Duration;
use tempfile::TempDir;

#[tokio::test]
async fn rotate_recreates_container_keeps_volume() {
    if std::env::var("PGFORGE_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping: set PGFORGE_E2E=1 to run");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let docker = BollardEngine::connect().expect("docker engine reachable");
    let global = GlobalConfig {
        port_range_start: 5810,
        port_range_end: 5899,
        ..Default::default()
    };
    let name = format!("pgforge_e2e_rotate_{}", uniq_suffix());

    // 1. Create a no-backup instance.
    let state = create_run(
        CreateArgs {
            name: name.clone(),
            preset: Preset::Tiny,
            pg_version: 18,
            app_user: "leads".into(),
            app_password: "pw".into(),
            pgbackrest_password: String::new(),
            override_state_root: Some(tmp.path().to_path_buf()),
            no_backup: true,
        retain_days: 30,
        snapshot_hour: None,
        },
        &docker,
        tmp.path().to_path_buf(),
        global.clone(),
        None,
    )
    .await
    .expect("create --no-backup must succeed");

    poll_pg_ready(state.instance.host_port).await;
    let container = format!("pgforge_{}", name);

    // 1b. Write a marker row so step 4 can prove volume (data) is preserved.
    let write = docker
        .exec(
            &container,
            &[
                "su", "-", "postgres", "-c",
                &format!(
                    "psql -tA -U leads -d {name} -c \"CREATE TABLE rotate_marker(v int); INSERT INTO rotate_marker VALUES (42);\""
                ),
            ],
        )
        .await
        .expect("seed marker");
    assert_eq!(write.exit_code, 0, "seed failed: stderr={:?}", write.stderr);

    // Record the container ID — after rotate it must be a different ID.
    let id_before = docker_container_id(&container);

    // 2. Rotate.
    let rotate_res = rotate_run(
        RotateArgs {
            name: name.clone(),
            override_state_root: Some(tmp.path().to_path_buf()),
        },
        &docker,
        tmp.path().to_path_buf(),
        global,
    )
    .await;

    if let Err(e) = &rotate_res {
        eprintln!("rotate failed: {e}. Leaving container {container} for inspection.");
    }
    rotate_res.expect("rotate must succeed");

    // 3. Container must be replaced (new ID) but on the same port + same volume.
    let id_after = docker_container_id(&container);
    assert_ne!(id_before, id_after, "rotate must produce a new container ID");

    poll_pg_ready(state.instance.host_port).await;

    // 4. Postgres must now be reading the bind-mounted config — verify by
    // asking SHOW config_file. Connect as the app user (the only role on
    // a --no-backup instance) via local socket trust.
    let exec = docker
        .exec(
            &container,
            &[
                "su", "-", "postgres", "-c",
                &format!("psql -tA -U leads -d {name} -c \"SHOW config_file;\""),
            ],
        )
        .await
        .expect("exec SHOW config_file");
    assert!(
        exec.stdout.contains("/etc/postgresql/postgresql.conf"),
        "rotate must produce a container with config_file pointing at bind-mount; got stdout={:?} stderr={:?}",
        exec.stdout, exec.stderr
    );

    // 5. Volume retention — the seeded row must still be present.
    let read = docker
        .exec(
            &container,
            &[
                "su", "-", "postgres", "-c",
                &format!(
                    "psql -tA -U leads -d {name} -c \"SELECT v FROM rotate_marker;\""
                ),
            ],
        )
        .await
        .expect("read marker");
    assert_eq!(
        read.stdout.trim(),
        "42",
        "rotate must preserve volume data; got stdout={:?} stderr={:?}",
        read.stdout, read.stderr
    );

    // Cleanup
    let _ = std::process::Command::new("docker")
        .args(["rm", "-f", &container])
        .output();
    let _ = std::process::Command::new("docker")
        .args(["volume", "rm", "-f", &format!("pgforge_data_{}", name)])
        .output();
}

fn docker_container_id(name: &str) -> String {
    let out = std::process::Command::new("docker")
        .args(["inspect", "-f", "{{.Id}}", name])
        .output()
        .expect("docker inspect");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
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
