#[path = "tui_render_helpers.rs"]
mod tui_render_helpers;
use tui_render_helpers::{buffer_contains, draw_into};

use pgforge::tui::events::Modal;
use pgforge::tui::ui::modal;

#[test]
fn actions_menu_lists_all_keys() {
    let m = Modal::ActionsMenu { instance_name: "billing".into() };
    let buf = draw_into(80, 24, |f| {
        let full = ratatui::layout::Rect { x: 0, y: 0, width: 80, height: 24 };
        modal::render(f, full, &m);
    });
    for needle in &["billing", "[s] Snapshot", "[c] Clone", "[R] Rotate",
                    "[p] Preset", "[t]", "[r] Restore", "[d] Destroy",
                    "[u] Upgrade", "[e]", "[esc]"] {
        assert!(buffer_contains(&buf, needle),
            "missing {needle:?}\n--- buffer ---\n{}",
            tui_render_helpers::buffer_to_string(&buf));
    }
}

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pgforge::tui::app::AppState;
use pgforge::tui::events::Event;

fn k(c: char) -> Event { Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)) }
fn esc() -> Event { Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)) }

fn row(name: &str) -> pgforge::commands::ls::InstanceSummary {
    pgforge::commands::ls::InstanceSummary {
        name: name.into(),
        pg_version: 18,
        preset_label: "tiny".into(),
        host_port: 5433,
        backup_enabled: true,
        backup_failing: false,
        running: true,
    }
}

#[test]
fn pressing_a_with_selection_opens_actions_menu() {
    let mut s = AppState::default();
    s.apply_event(Event::InstancesListed(vec![row("billing")]));
    s.apply_event(k('a'));
    assert!(matches!(s.modal, Some(Modal::ActionsMenu { .. })));
}

#[test]
fn esc_in_actions_menu_closes_it() {
    let mut s = AppState {
        modal: Some(Modal::ActionsMenu { instance_name: "x".into() }),
        ..AppState::default()
    };
    s.apply_event(esc());
    assert!(s.modal.is_none());
}

#[test]
fn help_modal_lists_global_and_per_instance_keys() {
    let m = Modal::Help;
    let buf = draw_into(80, 24, |f| {
        let full = ratatui::layout::Rect { x: 0, y: 0, width: 80, height: 24 };
        modal::render(f, full, &m);
    });
    for needle in &["pgforge — keybinds",
                    "[n]ew", "[a]ctions", "[?]", "[q]uit",
                    "[s] Snapshot", "[c] Clone",
                    "[esc]"] {
        assert!(buffer_contains(&buf, needle),
            "missing {needle:?}\n{}", tui_render_helpers::buffer_to_string(&buf));
    }
}

use pgforge::tui::events::OpError;
use std::time::Instant;

#[test]
fn question_mark_without_error_opens_help_modal() {
    let mut s = AppState::default();
    s.apply_event(k('?'));
    assert!(matches!(s.modal, Some(Modal::Help)),
        "expected Help; got {:?}", s.modal);
}

#[test]
fn question_mark_with_error_opens_error_detail_modal() {
    let mut s = AppState {
        last_op_error: Some(OpError {
            instance: "x".into(),
            kind: pgforge::tui::events::OpKind::Snapshot,
            msg: "boom".into(),
            at: Instant::now(),
        }),
        ..AppState::default()
    };
    s.apply_event(k('?'));
    assert!(matches!(s.modal, Some(Modal::ErrorDetail { .. })),
        "expected ErrorDetail; got {:?}", s.modal);
}

#[test]
fn d_in_actions_menu_opens_destroy_options() {
    let mut s = AppState::default();
    s.apply_event(Event::InstancesListed(vec![row("billing")]));
    s.modal = Some(Modal::ActionsMenu { instance_name: "billing".into() });
    s.apply_event(k('d'));
    // ActionsMenu should have closed, then DestroyOptions modal opened
    assert!(matches!(s.modal, Some(Modal::DestroyOptions { delete_backups: false, .. })),
        "expected DestroyOptions; got {:?}", s.modal);
}
