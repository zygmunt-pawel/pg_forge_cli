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
    });
    assert!(s.contains("State: running"));
    assert!(s.contains("CPU:"));
    assert!(s.contains("12.50%"));
    assert!(s.contains("256MB / 1024MB"));
    assert!(s.contains("2 active, 5 idle, 7 total"));
    assert!(s.contains("DB:"));
    assert!(s.contains("PGDATA:"));
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
    });
    assert!(s.contains("State: running"));
    assert!(!s.contains("CPU:"));
    assert!(s.contains("0 active, 0 idle, 0 total"));
    assert!(s.contains("DB:"));
}
