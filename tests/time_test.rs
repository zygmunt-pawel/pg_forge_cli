use pgforge::time::{now_iso, parse_target_time};

#[test]
fn now_iso_returns_20_char_z_string() {
    let s = now_iso();
    assert_eq!(s.len(), 20);
    assert!(s.ends_with('Z'));
    assert!(s.starts_with('2'));
}

#[test]
fn parse_target_time_accepts_full_rfc3339() {
    let t = parse_target_time("2026-05-10T14:23:00Z").unwrap();
    assert_eq!(t.to_string(), "2026-05-10T14:23:00Z");
}

#[test]
fn parse_target_time_accepts_space_separator() {
    let t = parse_target_time("2026-05-10 14:23:00").unwrap();
    assert!(t.to_string().starts_with("2026-05-10T14:23:00"));
}

#[test]
fn parse_target_time_accepts_offset() {
    let t = parse_target_time("2026-05-10T14:23:00+02:00").unwrap();
    assert!(t.to_string().contains("12:23:00"));
}

#[test]
fn parse_target_time_rejects_garbage() {
    assert!(parse_target_time("not a date").is_err());
    assert!(parse_target_time("").is_err());
}
