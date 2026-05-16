use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pgforge::tui::app::AppState;
use pgforge::tui::events::{Event, Modal, PendingDestructiveOp};

fn key(code: KeyCode) -> Event { Event::Key(KeyEvent::new(code, KeyModifiers::NONE)) }

fn instance_summary(name: &str) -> pgforge::commands::ls::InstanceSummary {
    pgforge::commands::ls::InstanceSummary {
        name: name.into(),
        pg_version: 18,
        preset_label: "tiny".into(),
        host_port: 5433,
        backup_enabled: true,
        running: true,
        backup_failing: false,
    }
}

#[test]
fn open_destroy_for_selected_opens_destroy_options() {
    let mut s = AppState::default();
    s.apply_event(Event::InstancesListed(vec![instance_summary("x")]));
    s.open_destroy_for_selected();
    assert!(matches!(s.modal,
        Some(Modal::DestroyOptions { delete_backups: false, .. })));
}

#[test]
fn space_toggles_delete_backups() {
    let mut s = AppState {
        modal: Some(Modal::DestroyOptions { name: "x".into(), delete_backups: false }),
        ..AppState::default()
    };
    s.apply_event(key(KeyCode::Char(' ')));
    assert!(matches!(s.modal,
        Some(Modal::DestroyOptions { delete_backups: true, .. })));
    s.apply_event(key(KeyCode::Char(' ')));
    assert!(matches!(s.modal,
        Some(Modal::DestroyOptions { delete_backups: false, .. })));
}

#[test]
fn enter_advances_to_confirm_with_chosen_delete_backups() {
    let mut s = AppState {
        modal: Some(Modal::DestroyOptions { name: "x".into(), delete_backups: true }),
        ..AppState::default()
    };
    s.apply_event(key(KeyCode::Enter));
    assert!(matches!(s.modal,
        Some(Modal::Confirm { kind: PendingDestructiveOp::Destroy { delete_backups: true, .. }, .. })));
}

#[test]
fn esc_cancels() {
    let mut s = AppState {
        modal: Some(Modal::DestroyOptions { name: "x".into(), delete_backups: true }),
        ..AppState::default()
    };
    s.apply_event(key(KeyCode::Esc));
    assert!(s.modal.is_none());
}
