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
