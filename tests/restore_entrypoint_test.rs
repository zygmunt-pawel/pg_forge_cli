use pgforge::docker::restore_entrypoint::generate_restore_entrypoint;

#[test]
fn entrypoint_runs_pgbackrest_restore_with_target_time() {
    let script = generate_restore_entrypoint(Some("2026-05-10T14:23:00Z"));
    assert!(script.contains("pgbackrest --stanza=main"));
    assert!(script.contains("restore"));
    // Value must NOT be interpolated literally — it should reference env var.
    assert!(
        !script.contains("--target=\"2026-05-10T14:23:00Z\""),
        "target time must not be interpolated as a literal string"
    );
    assert!(script.contains("--target=$PGFORGE_TARGET"), "expected --target=$PGFORGE_TARGET");
    assert!(script.contains("--type=time"));
    assert!(script.contains("--target-action=promote"));
}

#[test]
fn entrypoint_restores_latest_when_no_target_time() {
    let script = generate_restore_entrypoint(None);
    assert!(script.contains("pgbackrest --stanza=main"));
    assert!(script.contains("restore"));
    // When no target time, --type=time and --target=$PGFORGE_TARGET must not appear.
    assert!(!script.contains("--type=time"), "no --type=time without target");
    assert!(!script.contains("--target=$PGFORGE_TARGET"), "no --target var without target");
    // Must auto-promote, otherwise PG sits in recovery — but pgbackrest
    // REJECTS --target-action unless --type is also given ("option
    // 'target-action' not valid without option 'type'"). A no-target
    // restore must omit it entirely: postgres promotes on its own once
    // archive recovery reaches end-of-WAL (pgbackrest writes recovery.signal,
    // not standby.signal).
    assert!(!script.contains("--target-action"),
            "no-target restore must not pass --target-action — pgbackrest \
             errors without a matching --type, got:\n{script}");
}

#[test]
fn target_time_passed_via_env_not_interpolated() {
    let s = generate_restore_entrypoint(Some("2026-05-12T14:00:00Z"));
    // Target value should not appear inside double-quoted shell string.
    assert!(
        !s.contains(r#"--target="2026-05-12T14:00:00Z""#),
        "target time must not be interpolated into the script; pass via env"
    );
    // Should reference an env var instead.
    assert!(s.contains("PGFORGE_TARGET"), "expected PGFORGE_TARGET env var as the target carrier");
}

#[test]
fn entrypoint_skips_restore_if_pgdata_already_populated() {
    let script = generate_restore_entrypoint(None);
    // Guard is now a marker file, not PG_VERSION (PG_VERSION can exist after a
    // partial restore, which would cause a broken cluster to boot without retrying).
    assert!(
        script.contains(".pgforge-restore-complete") || script.contains("postmaster.pid") || script.contains("ls -A"),
        "expected a 're-entry guard' check, got:\n{script}"
    );
}

#[test]
fn restore_script_uses_marker_not_pg_version() {
    let s = generate_restore_entrypoint(None);
    assert!(s.contains(".pgforge-restore-complete"),
        "marker missing — script will re-skip restore on PG_VERSION from partial restore");
    assert!(!s.contains("[ ! -f \"$PGDATA/PG_VERSION\" ]"),
        "PG_VERSION guard still present");
}

#[test]
fn restore_script_writes_marker_after_pgbackrest() {
    let s = generate_restore_entrypoint(None);
    let marker_idx = s.find("touch \"$MARKER\"").expect("marker write missing");
    let pgbackrest_idx = s.find("pgbackrest").expect("pgbackrest missing");
    assert!(pgbackrest_idx < marker_idx,
        "marker must be written AFTER pgbackrest restore, not before");
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

#[test]
fn no_target_entrypoint_does_not_reference_unbound_target_var() {
    // The script runs under `set -u`. restore.rs only injects the
    // PGFORGE_TARGET container env var when a --target-time is given, so the
    // no-target script must not reference $PGFORGE_TARGET at all — otherwise
    // it aborts with "unbound variable" before pgbackrest ever runs.
    let s = generate_restore_entrypoint(None);
    assert!(
        !s.contains("PGFORGE_TARGET"),
        "no-target restore script must not reference PGFORGE_TARGET \
         (unbound under set -u):\n{s}"
    );
}

#[test]
fn target_entrypoint_passes_target_var_into_su_environment() {
    // `su -` resets the environment, so the target value must be explicitly
    // carried into the postgres user's command — but only in the target case.
    let s = generate_restore_entrypoint(Some("2026-05-12T14:00:00Z"));
    assert!(
        s.contains("PGFORGE_TARGET=$PGFORGE_TARGET pgbackrest"),
        "target restore must forward PGFORGE_TARGET into the su command:\n{s}"
    );
}
