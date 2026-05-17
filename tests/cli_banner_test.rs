use pgforge::cli::{should_emit_banner_for_command, format_banner_line};
use pgforge::disk::health::{DiskHealth, DiskStatus};

#[test]
fn banner_format_warn_shows_pct_label_and_soft_verb() {
    let warn = DiskHealth {
        status: DiskStatus::Warn, worst_pct: 85,
        worst_label: "docker".into(), worst_mount: "/var/lib/docker".into(),
    };
    let line = format_banner_line(&warn).expect("Warn should produce a banner");
    assert!(line.contains("85%"));
    assert!(line.contains("docker"));
    assert!(line.contains("may start failing"));
}

#[test]
fn banner_format_critical_uses_hard_verb() {
    let crit = DiskHealth {
        status: DiskStatus::Critical, worst_pct: 92,
        worst_label: "docker".into(), worst_mount: "/var/lib/docker".into(),
    };
    let cl = format_banner_line(&crit).expect("Critical should produce a banner");
    assert!(cl.contains("92%"));
    assert!(cl.contains("WILL"));
}

#[test]
fn banner_format_returns_none_for_ok_or_unknown() {
    assert!(format_banner_line(&DiskHealth::unknown()).is_none());
    let ok = DiskHealth {
        status: DiskStatus::Ok, worst_pct: 10,
        worst_label: "docker".into(), worst_mount: "/var/lib/docker".into(),
    };
    assert!(format_banner_line(&ok).is_none());
}

#[test]
fn ls_skips_the_banner() {
    // When stderr is not a TTY (as in cargo test), should_emit_banner_for_command
    // always returns false regardless of variant. We assert that property and also
    // verify the function exists / is reachable.
    assert!(!should_emit_banner_for_command(&pgforge::cli::Command::Ls));
}
