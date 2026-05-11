use pgforge::postgres::hba::generate_pg_hba;

#[test]
fn hba_allows_local_postgres_socket_trust() {
    let hba = generate_pg_hba("billing", "leads");
    assert!(hba.contains("local   all             postgres                                trust"));
}

#[test]
fn hba_uses_scram_for_app_user_over_network() {
    let hba = generate_pg_hba("billing", "leads");
    assert!(hba.contains("billing"), "should reference db name");
    assert!(hba.contains("leads"), "should reference app user");
    assert!(hba.contains("scram-sha-256"));
}

#[test]
fn hba_grants_local_replication_to_pgbackrest() {
    let hba = generate_pg_hba("billing", "leads");
    assert!(hba.contains("local   replication     pgbackrest                              scram-sha-256"));
}

#[test]
fn hba_rejects_default_anything_else() {
    let hba = generate_pg_hba("billing", "leads");
    assert!(hba.contains("host    all             all             all                     reject"),
            "must end with default-reject row");
}
