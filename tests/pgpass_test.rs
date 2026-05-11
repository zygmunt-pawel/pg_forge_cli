use pgforge::pgbackrest::pgpass::generate_pgpass;

#[test]
fn pgpass_contains_pgbackrest_role_with_password() {
    let s = generate_pgpass("hunter2");
    assert!(s.contains(":pgbackrest:hunter2"));
}

#[test]
fn pgpass_is_wildcard_host_port_db() {
    let s = generate_pgpass("hunter2");
    assert!(s.starts_with("*:*:*:pgbackrest:"));
}

#[test]
fn pgpass_ends_with_newline() {
    let s = generate_pgpass("hunter2");
    assert!(s.ends_with('\n'), "pgpass must end with newline, got: {s:?}");
}

#[test]
fn pgpass_escapes_colons_and_backslashes() {
    let s = generate_pgpass(r"pa:ss\word");
    assert!(s.contains(r"pa\:ss\\word"), "expected escaped, got: {s}");
}
