use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pgforge::tui::app::AppState;
use pgforge::tui::events::{Event, Modal};

fn k(c: char) -> Event { Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)) }

fn state_with_instance() -> AppState {
    let mut s = AppState::default();
    s.apply_event(Event::InstancesListed(vec![
        pgforge::commands::ls::InstanceSummary {
            name: "billing".into(),
            pg_version: 18,
            preset_label: "tiny".into(),
            host_port: 5433,
            backup_enabled: true,
            running: true,
            backup_failing: false,
        }
    ]));
    s
}

#[test]
fn s_at_top_level_flashes_hint_and_opens_no_modal() {
    let mut s = state_with_instance();
    s.apply_event(k('s'));
    assert!(s.flash.is_some(), "expected flash hint");
    assert!(s.modal.is_none(), "expected no modal; got {:?}", s.modal);
}

#[test]
fn each_moved_key_flashes_hint_and_opens_no_modal() {
    for c in ['c', 'R', 'p', 't', 'r', 'd', 'u', 'e'] {
        let mut s = state_with_instance();
        s.apply_event(k(c));
        assert!(s.flash.is_some(), "{c}: expected flash");
        assert!(s.modal.is_none(), "{c}: expected no modal; got {:?}", s.modal);
    }
}

#[test]
fn d_capital_also_flashes() {
    let mut s = state_with_instance();
    s.apply_event(Event::Key(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT)));
    assert!(s.flash.is_some());
    assert!(s.modal.is_none());
}

#[test]
fn a_still_opens_actions_menu() {
    let mut s = state_with_instance();
    s.apply_event(k('a'));
    assert!(matches!(s.modal, Some(Modal::ActionsMenu { .. })));
}
