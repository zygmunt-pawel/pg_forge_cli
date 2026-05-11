use pgforge::docker::clone_entrypoint::generate_clone_entrypoint;

#[test]
fn entrypoint_runs_pg_basebackup_from_source() {
    let script = generate_clone_entrypoint("pgforge_billing");
    assert!(script.contains("pg_basebackup"));
    assert!(script.contains("-h pgforge_billing"));
    assert!(script.contains("-U pgbackrest"));
    assert!(script.contains("-D /var/lib/postgresql/data/pgdata"));
    assert!(script.contains("-X stream"));
}

#[test]
fn entrypoint_skips_basebackup_if_pgdata_populated() {
    let script = generate_clone_entrypoint("pgforge_billing");
    assert!(
        script.contains("PG_VERSION"),
        "expected a 'is PGDATA empty?' check, got:\n{script}"
    );
}

#[test]
fn entrypoint_sets_pgpassfile_to_mounted_path() {
    let script = generate_clone_entrypoint("pgforge_billing");
    assert!(script.contains("PGPASSFILE=/var/lib/postgresql/.pgpass"));
}

#[test]
fn entrypoint_execs_official_postgres_entrypoint_at_end() {
    let script = generate_clone_entrypoint("pgforge_billing");
    assert!(script.contains("exec docker-entrypoint.sh postgres"));
}

#[test]
fn entrypoint_starts_with_shebang() {
    let script = generate_clone_entrypoint("pgforge_billing");
    assert!(script.starts_with("#!/"));
}
