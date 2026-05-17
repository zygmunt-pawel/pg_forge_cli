use pgforge::smart::types::{
    DriveSmart, SmartHealth, SmartStatus, SmartUnknownReason,
};
use pgforge::tui::ui::bottom::{SmartZone, format_smart_zone};
use ratatui::style::{Color, Modifier, Style};

fn h(status: SmartStatus) -> SmartHealth {
    SmartHealth {
        status,
        worst_device: Some("/dev/nvme0n1".into()),
        worst_reasons: vec![],
        unknown_reason: if status == SmartStatus::Unknown {
            Some(SmartUnknownReason::Stale)
        } else { None },
        drives: vec![DriveSmart {
            device: "/dev/nvme0n1".into(),
            model: "X".into(),
            transport: "nvme".into(),
            status,
            reasons: vec![],
            unknown_reason: None,
        }],
        checked_at: jiff::Timestamp::from_second(1_715_000_000).unwrap(),
    }
}

fn dim() -> Style { Style::default().add_modifier(Modifier::DIM) }
fn yellow() -> Style { Style::default().fg(Color::Yellow) }
fn red()    -> Style { Style::default().fg(Color::Red) }

#[test]
fn none_renders_dim_question_mark() {
    let z: SmartZone = format_smart_zone(None);
    assert_eq!(z.label, " SMART ? ");
    assert_eq!(z.style, dim());
}

#[test]
fn ok_renders_dim() {
    let z = format_smart_zone(Some(&h(SmartStatus::Ok)));
    assert_eq!(z.label, " SMART ok ");
    assert_eq!(z.style, dim());
}

#[test]
fn warn_renders_yellow() {
    let z = format_smart_zone(Some(&h(SmartStatus::Warn)));
    assert_eq!(z.label, " SMART warn ");
    assert_eq!(z.style, yellow());
}

#[test]
fn critical_renders_red() {
    let z = format_smart_zone(Some(&h(SmartStatus::Critical)));
    assert_eq!(z.label, " SMART fail ");
    assert_eq!(z.style, red());
}

#[test]
fn unknown_renders_dim_question_mark() {
    let z = format_smart_zone(Some(&h(SmartStatus::Unknown)));
    assert_eq!(z.label, " SMART ? ");
    assert_eq!(z.style, dim());
}
