use pgforge::docker::restore_entrypoint::generate_restore_entrypoint;

#[test]
fn entrypoint_runs_pgbackrest_restore_with_target_time() {
    let script = generate_restore_entrypoint(Some("2026-05-10T14:23:00Z"));
    assert!(script.contains("pgbackrest --stanza=main"));
    assert!(script.contains("restore"));
    assert!(script.contains("--target=\"2026-05-10T14:23:00Z\""));
    assert!(script.contains("--type=time"));
    assert!(script.contains("--target-action=promote"));
}

#[test]
fn entrypoint_restores_latest_when_no_target_time() {
    let script = generate_restore_entrypoint(None);
    assert!(script.contains("pgbackrest --stanza=main"));
    assert!(script.contains("restore"));
    assert!(!script.contains("--target="));
    // Critical: must auto-promote, otherwise PG sits in paused recovery.
    assert!(script.contains("--target-action=promote"),
            "default restore must include --target-action=promote, got:\n{script}");
}

#[test]
fn entrypoint_skips_restore_if_pgdata_already_populated() {
    let script = generate_restore_entrypoint(None);
    assert!(
        script.contains("PG_VERSION") || script.contains("postmaster.pid") || script.contains("ls -A"),
        "expected a 'is PGDATA empty?' check, got:\n{script}"
    );
}

#[test]
fn entrypoint_execs_official_postgres_entrypoint_with_bindmount_config_flags() {
    // Without -c config_file / -c hba_file, postgres reads only PGDATA's
    // initdb-defaults — our bind-mounted hardened postgresql.conf and
    // pg_hba.conf are silently ignored.
    let script = generate_restore_entrypoint(None);
    assert!(script.contains("exec docker-entrypoint.sh postgres"));
    assert!(
        script.contains("config_file=/etc/postgresql/postgresql.conf"),
        "must pass config_file flag, got:\n{script}"
    );
    assert!(
        script.contains("hba_file=/etc/postgresql/pg_hba.conf"),
        "must pass hba_file flag, got:\n{script}"
    );
}

#[test]
fn entrypoint_is_a_shebang_script() {
    let script = generate_restore_entrypoint(None);
    assert!(script.starts_with("#!/"));
}
