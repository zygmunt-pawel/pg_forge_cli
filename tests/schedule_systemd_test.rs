use pgforge::commands::schedule::{render_service, render_timer, AGENT_LABEL};

#[test]
fn service_unit_contains_oneshot_and_absolute_exec_path() {
    let s = render_service("/home/pawel/.local/bin/pgforge");
    assert!(s.contains("[Unit]"));
    assert!(s.contains("[Service]"));
    assert!(s.contains("Type=oneshot"));
    assert!(s.contains("ExecStart=\"/home/pawel/.local/bin/pgforge\" snapshot --due"));
    // PATH so systemd-spawned process can find docker/pgbackrest tooling.
    assert!(s.contains("Environment=PATH="));
}

#[test]
fn timer_unit_fires_every_5_minutes_under_timers_target() {
    let t = render_timer();
    assert!(t.contains("[Unit]"));
    assert!(t.contains("[Timer]"));
    assert!(t.contains("OnBootSec=2min"));
    assert!(t.contains("OnUnitActiveSec=5min"));
    // Bound to the service unit by short name; full name embeds AGENT_LABEL.
    assert!(t.contains(&format!("Unit={AGENT_LABEL}.service")));
    assert!(t.contains("[Install]"));
    assert!(t.contains("WantedBy=timers.target"));
}

#[test]
fn agent_label_is_the_service_basename() {
    // The .service / .timer files share the AGENT_LABEL stem.
    assert_eq!(AGENT_LABEL, "dev.pgforge.snapshot-due");
}

#[test]
fn service_exec_start_quotes_path_with_spaces() {
    // systemd tokenises ExecStart on whitespace — paths with spaces must
    // be double-quoted or the timer silently does nothing.
    let s = render_service("/home/my user/.local/bin/pgforge");
    assert!(s.contains("ExecStart=\"/home/my user/.local/bin/pgforge\" snapshot --due"),
        "ExecStart must quote the exe path so spaces don't tokenise:\n{s}");
}
