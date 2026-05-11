use pgforge::docker::clone_entrypoint::generate_clone_entrypoint;

#[test]
fn entrypoint_runs_pg_basebackup_from_source_as_pgreplica() {
    // The clone connects to the source over TCP and must authenticate as
    // the non-SUPERUSER `pgreplica` role, not as `pgbackrest`.
    let script = generate_clone_entrypoint("pgforge_billing");
    assert!(script.contains("pg_basebackup"));
    assert!(script.contains("-h pgforge_billing"));
    assert!(script.contains("-U pgreplica"));
    assert!(
        !script.contains("-U pgbackrest"),
        "must NOT use SUPERUSER role for TCP replication"
    );
    // -D may use either the literal path or the $PGDATA shell variable set
    // at the top of the script; we accept either form.
    assert!(
        script.contains("-D /var/lib/postgresql/data/pgdata") || script.contains("-D $PGDATA"),
        "pg_basebackup must target PGDATA, got:\n{script}"
    );
    assert!(
        script.contains(r#"PGDATA="/var/lib/postgresql/data/pgdata""#),
        "PGDATA must be defined to the standard path"
    );
    assert!(script.contains("-X stream"));
}

#[test]
fn entrypoint_uses_marker_file_not_pg_version_for_resume() {
    // PG_VERSION can be written by a partial pg_basebackup that then crashes,
    // leaving a corrupt cluster. We use a separate marker file written AFTER
    // pg_basebackup completes — so on retry we re-do the basebackup rather
    // than booting on corruption.
    let script = generate_clone_entrypoint("pgforge_billing");
    assert!(
        script.contains(".pgforge-clone-complete"),
        "expected marker-file check, got:\n{script}"
    );
}

#[test]
fn entrypoint_clears_partial_pgdata_before_basebackup() {
    // pg_basebackup refuses to write to a non-empty target. Partial state
    // from a previous failed attempt must be cleared.
    let script = generate_clone_entrypoint("pgforge_billing");
    assert!(
        script.contains("find") && script.contains("$PGDATA"),
        "expected PGDATA clear before basebackup, got:\n{script}"
    );
}

#[test]
fn entrypoint_copies_pgpass_into_container_owned_location() {
    // The bind-mounted .pgpass is owned by the HOST user UID, which may not
    // be readable by the postgres user inside the container (macOS Docker
    // Desktop FUSE remapping). Copy it to a postgres-owned path before use.
    let script = generate_clone_entrypoint("pgforge_billing");
    assert!(script.contains("/tmp/pgforge.pgpass"), "got:\n{script}");
    assert!(script.contains("chown postgres"), "got:\n{script}");
    assert!(script.contains("chmod 0600"), "got:\n{script}");
}

#[test]
fn entrypoint_execs_official_postgres_entrypoint_at_end() {
    let script = generate_clone_entrypoint("pgforge_billing");
    assert!(script.contains("exec docker-entrypoint.sh postgres"));
}

#[test]
fn entrypoint_starts_with_shebang_and_set_eu() {
    let script = generate_clone_entrypoint("pgforge_billing");
    assert!(script.starts_with("#!/"));
    assert!(script.contains("set -eu"), "must fail-fast on errors");
}
