use pgforge::commands::snapshots::parse_pitr_window;

#[test]
fn empty_json_returns_empty_window() {
    let w = parse_pitr_window("");
    assert!(w.earliest.is_none() && w.latest.is_none());
}

#[test]
fn malformed_json_returns_empty_window() {
    let w = parse_pitr_window("{not json");
    assert!(w.earliest.is_none() && w.latest.is_none());
}

#[test]
fn stanza_without_backups_returns_empty_window() {
    let w = parse_pitr_window(r#"[{"name": "main", "backup": []}]"#);
    assert!(w.earliest.is_none() && w.latest.is_none());
}

#[test]
fn single_backup_emits_window_with_iso_timestamps() {
    // start = 1_715_000_000  (2024-05-06T12:53:20Z UTC)
    // stop  = 1_715_000_300  (2024-05-06T12:58:20Z UTC)
    let w = parse_pitr_window(
        r#"[{"name":"main","backup":[{"timestamp":{"start":1715000000,"stop":1715000300}}]}]"#,
    );
    assert_eq!(w.earliest.as_deref(), Some("2024-05-06T12:53:20Z"));
    assert_eq!(w.latest.as_deref(), Some("2024-05-06T12:58:20Z"));
}

#[test]
fn multiple_backups_take_min_start_and_max_stop() {
    let w = parse_pitr_window(
        r#"[{"name":"main","backup":[
            {"timestamp":{"start":1715000000,"stop":1715000300}},
            {"timestamp":{"start":1700000000,"stop":1700000400}},
            {"timestamp":{"start":1720000000,"stop":1720000999}}
        ]}]"#,
    );
    // earliest = 1_700_000_000 (2023-11-14T22:13:20Z)
    // latest   = 1_720_000_999 (2024-07-03T07:23:19Z)
    assert!(
        w.earliest.as_deref().unwrap().starts_with("2023-11-"),
        "earliest should be from 2023-11, got {:?}",
        w.earliest
    );
    assert!(
        w.latest.as_deref().unwrap().starts_with("2024-07-"),
        "latest should be from 2024-07, got {:?}",
        w.latest
    );
}

#[test]
fn missing_stanza_array_returns_empty_window() {
    // Top-level object instead of array — older pgbackrest versions
    // emitted this shape on errors.
    let w = parse_pitr_window(r#"{"error": "stanza not found"}"#);
    assert!(w.earliest.is_none() && w.latest.is_none());
}
