use pgforge::commands::snapshot::{redact_pgbackrest_output, pgbackrest_indicates_failure};

#[test]
fn detects_error_marker_in_stderr() {
    assert!(pgbackrest_indicates_failure("WARN: …\nERROR: [056]: file missing"));
    assert!(pgbackrest_indicates_failure("ABORTED: shutting down"));
}

#[test]
fn clean_output_is_not_a_failure() {
    assert!(!pgbackrest_indicates_failure("INFO: backup begin\nINFO: full backup size = 12MB"));
}

#[test]
fn redact_strips_repo1_s3_key_lines() {
    let s = "repo1-s3-key=AKIAEXAMPLE\nrepo1-s3-key-secret=verysecret\nINFO: ok";
    let r = redact_pgbackrest_output(s);
    assert!(!r.contains("AKIAEXAMPLE"));
    assert!(!r.contains("verysecret"));
    assert!(r.contains("INFO: ok"));
}
