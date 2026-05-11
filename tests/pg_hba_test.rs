use pgforge::postgres::hba::generate_pg_hba;

#[test]
fn hba_allows_local_app_user_trust() {
    // Inside the container the app user (created via POSTGRES_USER) is the only
    // superuser role. Trust over unix socket lets `docker exec ... psql` work
    // without password handling for admin tasks.
    let hba = generate_pg_hba("billing", "leads");
    assert!(
        hba.contains("local   all             leads                                   trust"),
        "got:\n{hba}"
    );
}

#[test]
fn hba_uses_scram_for_app_user_over_network() {
    let hba = generate_pg_hba("billing", "leads");
    assert!(hba.contains("billing"));
    assert!(hba.contains("leads"));
    assert!(hba.contains("scram-sha-256"));
}

#[test]
fn hba_grants_local_replication_to_pgbackrest_via_trust() {
    // `pgbackrest archive-push` runs inside this container as the postgres OS
    // user; trust on the unix socket avoids needing a .pgpass file.
    let hba = generate_pg_hba("billing", "leads");
    assert!(hba.contains("local   replication     pgbackrest                              trust"));
    assert!(hba.contains("local   all             pgbackrest                              trust"));
}

#[test]
fn hba_does_not_reference_nonexistent_postgres_role() {
    // The official postgres image with POSTGRES_USER set does NOT create a
    // `postgres` superuser. Make sure we don't grant trust to a phantom role.
    let hba = generate_pg_hba("billing", "leads");
    assert!(!hba.contains("postgres                                trust"),
            "must not reference nonexistent postgres role:\n{hba}");
}

#[test]
fn hba_rejects_default_anything_else() {
    let hba = generate_pg_hba("billing", "leads");
    assert!(hba.contains("host    all             all             all                     reject"));
}

#[test]
fn hba_grants_host_replication_to_pgbackrest_over_samenet() {
    // Clone uses pg_basebackup from a sibling container — host replication
    // over the docker bridge is required.
    let hba = generate_pg_hba("billing", "leads");
    assert!(
        hba.contains("host    replication     pgbackrest      samenet                 scram-sha-256"),
        "must allow host replication from samenet, got:\n{hba}"
    );
}
