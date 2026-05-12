use pgforge::time::{canonicalize_target_time, now_iso, parse_target_time};

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

#[test]
fn canonicalizes_space_separator_as_utc() {
    assert_eq!(
        canonicalize_target_time("2026-05-12 14:00:00").unwrap(),
        "2026-05-12T14:00:00Z"
    );
}

#[test]
fn canonicalizes_offset_to_utc() {
    assert_eq!(
        canonicalize_target_time("2026-05-12T14:00:00+02:00").unwrap(),
        "2026-05-12T12:00:00Z"
    );
}

#[test]
fn passes_through_z_form() {
    assert_eq!(
        canonicalize_target_time("2026-05-12T14:00:00Z").unwrap(),
        "2026-05-12T14:00:00Z"
    );
}

#[test]
fn rejects_garbage() {
    assert!(canonicalize_target_time("not a time").is_err());
}
