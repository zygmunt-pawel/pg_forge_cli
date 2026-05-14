use pgforge::commands::status::{InstanceStatus, render};

#[test]
fn render_stopped_instance_just_shows_state() {
    let s = render(&InstanceStatus {
        name: "billing".into(),
        running: false,
        host_port: 5433,
        ..Default::default()
    });
    assert!(s.contains("Instance: billing"));
    assert!(s.contains("State: stopped"));
    assert!(!s.contains("CPU"), "stopped instance shouldn't list runtime metrics");
}

#[test]
fn render_running_instance_with_full_metrics() {
    let s = render(&InstanceStatus {
        name: "billing".into(),
        running: true,
        host_port: 5433,
        cpu_percent: Some(12.5),
        mem_used_mb: Some(256),
        mem_limit_mb: Some(1024),
        connections_active: Some(2),
        connections_idle: Some(5),
        connections_total: Some(7),
        db_size_bytes: Some(1_234_567),
        pgdata_bytes: Some(50_000_000),
        uptime_seconds: Some(125),
        restart_count: Some(0),
        db_responsive: Some(true),
        backup_enabled: true,
        backup_failing: false,
        last_snapshot_at: Some("2026-05-14T03:00:00Z".into()),
    });
    assert!(s.contains("State: running"));
    assert!(s.contains("CPU:"));
    assert!(s.contains("12.50%"));
    assert!(s.contains("256MB / 1024MB"));
    assert!(s.contains("2 active, 5 idle, 7 total"));
    assert!(s.contains("DB:"));
    assert!(s.contains("PGDATA:"));
    assert!(s.contains("Uptime:"));
    assert!(s.contains("responsive ✓"));
}

#[test]
fn render_running_instance_with_partial_metrics() {
    // When docker stats fails (e.g. CLI not on PATH) we still want a useful
    // dump of what we did get from postgres.
    let s = render(&InstanceStatus {
        name: "billing".into(),
        running: true,
        host_port: 5433,
        cpu_percent: None,
        mem_used_mb: None,
        mem_limit_mb: None,
        connections_active: Some(0),
        connections_idle: Some(0),
        connections_total: Some(0),
        db_size_bytes: Some(1024),
        pgdata_bytes: None,
        uptime_seconds: None,
        restart_count: None,
        db_responsive: Some(true),
        backup_enabled: true,
        backup_failing: false,
        last_snapshot_at: None,
    });
    assert!(s.contains("State: running"));
    assert!(!s.contains("CPU:"));
    assert!(s.contains("0 active, 0 idle, 0 total"));
    assert!(s.contains("DB:"));
}

#[test]
fn render_flags_failing_backups_even_when_stopped() {
    // Backup health must be visible regardless of container run state — a
    // stopped instance with broken backups is exactly what the operator
    // needs to notice.
    let s = render(&InstanceStatus {
        name: "billing".into(),
        running: false,
        host_port: 5433,
        backup_enabled: true,
        backup_failing: true,
        last_snapshot_at: Some("2026-05-10T03:00:00Z".into()),
        ..Default::default()
    });
    assert!(
        s.contains("FAILING"),
        "failing backups must be flagged in status output, got:\n{s}"
    );
    assert!(
        s.contains("2026-05-10T03:00:00Z"),
        "operator needs to see when the last good backup was, got:\n{s}"
    );
}
