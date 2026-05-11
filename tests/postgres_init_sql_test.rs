use pgforge::postgres::init_sql::generate_init_sql;

#[test]
fn creates_pgbackrest_role_with_replication_and_superuser() {
    let sql = generate_init_sql("hunter2");
    assert!(sql.contains("CREATE ROLE pgbackrest"));
    assert!(sql.contains("LOGIN"));
    assert!(sql.contains("REPLICATION"));
    assert!(sql.contains("SUPERUSER"));
}

#[test]
fn creates_pgreplica_role_with_replication_login_no_superuser() {
    // pgreplica is the role used by `pgforge clone`'s pg_basebackup over TCP.
    // It must NOT be SUPERUSER — that would expose RCE-as-postgres to any
    // sibling container that learns the password.
    let sql = generate_init_sql("hunter2");
    let line = sql
        .lines()
        .find(|l| l.contains("CREATE ROLE pgreplica"))
        .unwrap_or_else(|| panic!("missing pgreplica role line in:\n{sql}"));
    assert!(line.contains("LOGIN"), "pgreplica needs LOGIN: {line}");
    assert!(line.contains("REPLICATION"), "pgreplica needs REPLICATION: {line}");
    assert!(!line.contains("SUPERUSER"), "pgreplica must NOT be SUPERUSER: {line}");
}

#[test]
fn embeds_password_quoted() {
    let sql = generate_init_sql("hunter2");
    assert!(sql.contains("PASSWORD 'hunter2'"));
}

#[test]
fn escapes_single_quotes_in_password() {
    let sql = generate_init_sql("o'reilly");
    // PG SQL-escapes ' as '' inside string literals.
    assert!(sql.contains("PASSWORD 'o''reilly'"), "got:\n{sql}");
}

#[test]
fn output_is_valid_psql_script_shape() {
    let sql = generate_init_sql("pw");
    assert!(sql.trim_end().ends_with(';'), "must end with semicolon, got:\n{sql}");
}
