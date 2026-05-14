use pgforge::commands::dump::dumps_to_prune;
use pgforge::commands::dump::is_valid_custom_dump;
use pgforge::commands::dump::parse_df_available_kb;
use pgforge::commands::dump::resolve_dump_path;
use std::path::PathBuf;

#[test]
fn resolve_dump_path_default_uses_dump_dir_instance_and_timestamp() {
    let p = resolve_dump_path(
        None,
        "billing",
        &PathBuf::from("/home/pawel/pgforge-dumps"),
        "2026-05-14T09:30:00Z",
    );
    assert_eq!(
        p,
        PathBuf::from("/home/pawel/pgforge-dumps/billing-20260514-093000.dump")
    );
}

#[test]
fn resolve_dump_path_out_override_is_used_verbatim() {
    let p = resolve_dump_path(
        Some(PathBuf::from("/tmp/mine.dump")),
        "billing",
        &PathBuf::from("/home/pawel/pgforge-dumps"),
        "2026-05-14T09:30:00Z",
    );
    assert_eq!(p, PathBuf::from("/tmp/mine.dump"));
}

#[test]
fn valid_custom_dump_recognises_pgdmp_header() {
    assert!(is_valid_custom_dump(b"PGDMP\x01\x0e\x00"));
}

#[test]
fn valid_custom_dump_rejects_empty_and_truncated() {
    assert!(!is_valid_custom_dump(b""));
    assert!(!is_valid_custom_dump(b"PGD"));
    assert!(!is_valid_custom_dump(b"-- plain sql dump\n"));
}

#[test]
fn parse_df_reads_available_column_from_posix_output() {
    // `df -P -k <dir>` output: header line, then one data line.
    let out = "Filesystem 1024-blocks      Used Available Capacity Mounted on\n\
               /dev/disk3s1s1 482797652 12222540 458123880       3% /\n";
    assert_eq!(parse_df_available_kb(out), Some(458_123_880));
}

#[test]
fn parse_df_returns_none_on_garbage() {
    assert_eq!(parse_df_available_kb(""), None);
    assert_eq!(parse_df_available_kb("just a header\n"), None);
}

#[test]
fn dumps_to_prune_keeps_newest_n_by_filename() {
    // Default filenames sort chronologically (timestamp is fixed-width).
    let mut files = vec![
        "billing-20260514-093000.dump".to_string(),
        "billing-20260512-093000.dump".to_string(),
        "billing-20260513-093000.dump".to_string(),
    ];
    let prune = dumps_to_prune(&mut files, 2);
    assert_eq!(prune, vec!["billing-20260512-093000.dump".to_string()]);
}

#[test]
fn dumps_to_prune_keeps_all_when_count_within_limit() {
    let mut files = vec!["billing-20260514-093000.dump".to_string()];
    assert!(dumps_to_prune(&mut files, 2).is_empty());
    let mut none: Vec<String> = vec![];
    assert!(dumps_to_prune(&mut none, 0).is_empty());
}

use async_trait::async_trait;
use pgforge::commands::dump::{run_with_engine, DumpArgs};
use pgforge::docker::engine::{
    BuildImageSpec, ContainerInspect, CreateContainerSpec, DockerEngine, ExecOutput,
    ExecToFileOutput,
};
use pgforge::domain::instance::Instance;
use pgforge::domain::preset::Preset;
use pgforge::error::Result as PgResult;
use pgforge::state::instance::InstanceState;
use std::time::Duration;
use tempfile::TempDir;

struct DumpMockEngine {
    running: bool,
}

#[async_trait]
impl DockerEngine for DumpMockEngine {
    async fn container_running(&self, _: &str) -> PgResult<bool> {
        Ok(self.running)
    }
    async fn build_image(&self, _: &BuildImageSpec) -> PgResult<()> { unimplemented!() }
    async fn ensure_network(&self, _: &str) -> PgResult<()> { unimplemented!() }
    async fn create_container(&self, _: &CreateContainerSpec) -> PgResult<String> { unimplemented!() }
    async fn start_container(&self, _: &str) -> PgResult<()> { unimplemented!() }
    async fn container_exists(&self, _: &str) -> PgResult<bool> { unimplemented!() }
    async fn exec(&self, _: &str, _: &[&str]) -> PgResult<ExecOutput> { unimplemented!() }
    async fn exec_as(&self, _: &str, _: &str, _: &[&str]) -> PgResult<ExecOutput> { unimplemented!() }
    async fn exec_with_stdin(&self, _: &str, _: &[&str], _: &str) -> PgResult<ExecOutput> { unimplemented!() }
    async fn exec_to_file(&self, _: &str, _: &[&str], _: &std::path::Path) -> PgResult<ExecToFileOutput> {
        unimplemented!()
    }
    async fn stop_container(&self, _: &str) -> PgResult<()> { unimplemented!() }
    async fn wait_for_container_running(&self, _: &str, _: Duration) -> PgResult<()> { unimplemented!() }
    async fn wait_for_container_exit(&self, _: &str, _: Duration) -> PgResult<i64> { unimplemented!() }
    async fn remove_container(&self, _: &str, _: bool) -> PgResult<()> { unimplemented!() }
    async fn remove_volume(&self, _: &str) -> PgResult<()> { unimplemented!() }
    async fn inspect_container(&self, _: &str) -> PgResult<ContainerInspect> { unimplemented!() }
    async fn logs(&self, _: &str) -> PgResult<String> { unimplemented!() }
}

fn write_instance(state_root: &std::path::Path, name: &str) {
    InstanceState {
        instance: Instance {
            name: name.into(),
            db_name: name.into(),
            app_user: "leads".into(),
            app_password: "pw".into(),
            pgbackrest_password: "rpw".into(),
            preset: Preset::Tiny,
            pg_version: 18,
            host_port: 5433,
            backup_enabled: true,
            volume_name_override: None,
            retain_days: 30,
            snapshot_hour: Some(3),
            last_snapshot_at: None,
            last_snapshot_attempt_at: None,
            full_backup_day: 0,
        },
        created_at: "2026-05-12T10:00:00Z".into(),
    }
    .save_under(state_root)
    .unwrap();
}

fn args(name: &str, root: &std::path::Path, out: Option<std::path::PathBuf>) -> DumpArgs {
    DumpArgs {
        name: name.into(),
        out,
        force: false,
        keep: None,
        timeout_secs: 1800,
        override_state_root: Some(root.to_path_buf()),
    }
}

#[tokio::test]
async fn run_with_engine_errors_when_instance_missing() {
    let tmp = TempDir::new().unwrap();
    let eng = DumpMockEngine { running: true };
    let err = run_with_engine(args("ghost", tmp.path(), None), &eng, tmp.path().to_path_buf())
        .await
        .unwrap_err();
    assert!(matches!(err, pgforge::error::PgForgeError::InstanceNotFound(_)));
}

#[tokio::test]
async fn run_with_engine_errors_when_container_not_running() {
    let tmp = TempDir::new().unwrap();
    write_instance(tmp.path(), "billing");
    let eng = DumpMockEngine { running: false };
    let err = run_with_engine(args("billing", tmp.path(), None), &eng, tmp.path().to_path_buf())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not running"));
}

#[tokio::test]
async fn run_with_engine_errors_when_destination_exists_without_force() {
    let tmp = TempDir::new().unwrap();
    write_instance(tmp.path(), "billing");
    let existing = tmp.path().join("already.dump");
    std::fs::write(&existing, b"old").unwrap();
    let eng = DumpMockEngine { running: true };
    let err = run_with_engine(
        args("billing", tmp.path(), Some(existing.clone())),
        &eng,
        tmp.path().to_path_buf(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("already exists"));
    // The pre-existing file must be untouched.
    assert_eq!(std::fs::read(&existing).unwrap(), b"old");
}

use pgforge::commands::create::{run_with_engine as create_run, CreateArgs};
use pgforge::commands::destroy::{run_with_engine as destroy_run, DestroyArgs};
use pgforge::config::global::GlobalConfig;
use pgforge::docker::bollard_engine::BollardEngine;

#[tokio::test]
async fn dump_e2e_produces_a_restorable_file() {
    if std::env::var("PGFORGE_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping: set PGFORGE_E2E=1 to run");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let state_root = tmp.path().to_path_buf();
    let docker = BollardEngine::connect().expect("docker reachable");
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let name = format!("pgfdump_e2e_{suffix}");

    // 1. Create a throwaway --no-backup instance (no S3 needed for a dump test).
    create_run(
        CreateArgs {
            name: name.clone(),
            preset: Preset::Tiny,
            pg_version: 18,
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
        GlobalConfig::default(),
        None,
    )
    .await
    .expect("create");

    // 2. Dump it.
    let out = tmp.path().join("e2e.dump");
    let path = run_with_engine(
        DumpArgs {
            name: name.clone(),
            out: Some(out.clone()),
            force: false,
            keep: None,
            timeout_secs: 600,
            override_state_root: Some(state_root.clone()),
        },
        &docker,
        state_root.clone(),
    )
    .await
    .expect("dump");

    // 3. Verify: file exists, non-empty, PGDMP header, 0600, no leftover .partial,
    //    and pg_restore --list can read it.
    assert_eq!(path, out);
    let meta = std::fs::metadata(&out).expect("dump file exists");
    assert!(meta.len() > 0, "dump must be non-empty");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }
    let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().to_string_lossy().contains(".partial"))
        .collect();
    assert!(leftovers.is_empty(), "no .partial should remain: {leftovers:?}");
    let listed = std::process::Command::new("pg_restore")
        .arg("--list")
        .arg(&out)
        .output()
        .expect("pg_restore on PATH");
    assert!(listed.status.success(), "pg_restore --list must accept the dump");

    // 4. Cleanup.
    destroy_run(
        DestroyArgs {
            name: name.clone(),
            delete_backups: false,
            override_state_root: Some(state_root.clone()),
        },
        &docker,
        state_root.clone(),
    )
    .await
    .expect("destroy");
}
