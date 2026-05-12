use pgforge::commands::snapshot::{is_snapshot_due, redact_pgbackrest_output, pgbackrest_indicates_failure};

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

#[test]
fn never_snapshotted_with_zero_hour_does_not_panic() {
    let _ = is_snapshot_due(0, None);
}

#[test]
fn unparseable_last_is_not_due() {
    // The previous behavior returned true on parse failure → storm.
    // We now treat unparseable as "skip this tick" so the scheduler
    // doesn't loop on a corrupt state.toml.
    assert!(!is_snapshot_due(0, Some("garbage")),
        "garbage timestamp must NOT trigger a re-snapshot");
}
