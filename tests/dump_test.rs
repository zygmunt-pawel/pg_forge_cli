use pgforge::commands::dump::resolve_dump_path;
use std::path::PathBuf;

#[test]
fn resolve_dump_path_default_uses_dump_dir_instance_and_timestamp() {
    let p = resolve_dump_path(
        None,
        "billing",
        &PathBuf::from("/home/pawel/pgforge-dumps"),
        "2026-05-14T09:30:00Z",
    );
    assert_eq!(
        p,
        PathBuf::from("/home/pawel/pgforge-dumps/billing-20260514-093000.dump")
    );
}

#[test]
fn resolve_dump_path_out_override_is_used_verbatim() {
    let p = resolve_dump_path(
        Some(PathBuf::from("/tmp/mine.dump")),
        "billing",
        &PathBuf::from("/home/pawel/pgforge-dumps"),
        "2026-05-14T09:30:00Z",
    );
    assert_eq!(p, PathBuf::from("/tmp/mine.dump"));
}
