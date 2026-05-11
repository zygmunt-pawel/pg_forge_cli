use pgforge::commands::ls::{InstanceSummary, render_table};

fn row(name: &str, backups: bool, running: bool) -> InstanceSummary {
    InstanceSummary {
        name: name.into(),
        pg_version: 18,
        preset_label: "tiny".into(),
        host_port: 5433,
        backup_enabled: backups,
        running,
    }
}

#[test]
fn render_table_empty_prints_helpful_message() {
    let s = render_table(&[]);
    assert!(s.contains("No instances"), "got:\n{s}");
    assert!(s.contains("pgforge create"), "should hint at next step, got:\n{s}");
}

#[test]
fn render_table_has_header_and_one_row_per_instance() {
    let s = render_table(&[row("billing", true, true), row("analytics", false, false)]);
    assert!(s.contains("NAME"));
    assert!(s.contains("PG"));
    assert!(s.contains("BACKUPS"));
    assert!(s.contains("RUNNING"));
    assert!(s.contains("billing"));
    assert!(s.contains("analytics"));
}

#[test]
fn render_table_shows_yes_no_for_running_and_backups() {
    let s = render_table(&[
        row("alive_with_backups", true, true),
        row("dead_no_backup", false, false),
    ]);
    // Both yes and no must appear.
    assert!(s.contains("yes"));
    assert!(s.contains("no"));
}

#[test]
fn render_table_handles_long_name_with_ellipsis() {
    let s = render_table(&[row("this_is_a_very_long_instance_name_that_should_get_truncated", true, true)]);
    assert!(s.contains("…"), "expected ellipsis in truncated name, got:\n{s}");
}
