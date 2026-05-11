use pgforge::pgbackrest::parse::parse_backup_label;

#[test]
fn extracts_label_from_full_output() {
    let output = "\
P00   INFO: backup begin: 20260511-141259F
P00   INFO: backup stop archive = 0/2000028, lsn = 0/2000060
P00   INFO: new backup label = 20260511-141259F
P00   INFO: backup command end: completed successfully (1024ms)
";
    let label = parse_backup_label(output).unwrap();
    assert_eq!(label, "20260511-141259F");
}

#[test]
fn extracts_diff_backup_label() {
    let output = "P00   INFO: new backup label = 20260512-020000F_20260513-020000D\n";
    assert_eq!(parse_backup_label(output).unwrap(), "20260512-020000F_20260513-020000D");
}

#[test]
fn returns_none_when_no_label_line() {
    assert!(parse_backup_label("unrelated text\n").is_none());
}

#[test]
fn ignores_whitespace_after_equals() {
    let output = "P00   INFO: new backup label =  20260511-141259F  \n";
    assert_eq!(parse_backup_label(output).unwrap(), "20260511-141259F");
}
