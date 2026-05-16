mod tui_render_helpers;
use tui_render_helpers::{buffer_contains, draw_into};

use pgforge::disk::health::{DiskHealth, DiskStatus};
use pgforge::tui::app::AppState;
use pgforge::tui::ui::bottom;

fn state_with(h: Option<DiskHealth>) -> AppState {
    AppState { disk_health: h, ..AppState::default() }
}

#[test]
fn footer_shows_disk_pct_when_known() {
    let state = state_with(Some(DiskHealth {
        status: DiskStatus::Ok, worst_pct: 42,
        worst_label: "docker".into(), worst_mount: "/var/lib/docker".into(),
    }));
    let buf = draw_into(80, 3, |f| {
        let area = ratatui::layout::Rect { x: 0, y: 0, width: 80, height: 1 };
        bottom::render(f, area, &state);
    });
    assert!(buffer_contains(&buf, "Disk 42%"), "footer = {:?}",
        tui_render_helpers::buffer_to_string(&buf));
}

#[test]
fn footer_shows_question_mark_when_none() {
    let state = state_with(None);
    let buf = draw_into(80, 3, |f| {
        let area = ratatui::layout::Rect { x: 0, y: 0, width: 80, height: 1 };
        bottom::render(f, area, &state);
    });
    assert!(buffer_contains(&buf, "Disk ?"));
}

#[test]
fn footer_shows_question_mark_when_unknown() {
    let state = state_with(Some(DiskHealth::unknown()));
    let buf = draw_into(80, 3, |f| {
        let area = ratatui::layout::Rect { x: 0, y: 0, width: 80, height: 1 };
        bottom::render(f, area, &state);
    });
    assert!(buffer_contains(&buf, "Disk ?"));
}

#[test]
fn footer_default_lists_minimal_keys() {
    let state = state_with(None);
    let buf = draw_into(80, 3, |f| {
        let area = ratatui::layout::Rect { x: 0, y: 0, width: 80, height: 1 };
        bottom::render(f, area, &state);
    });
    let text = tui_render_helpers::buffer_to_string(&buf);
    assert!(text.contains("[n]ew"), "missing [n]ew: {text}");
    assert!(text.contains("[a]ctions"), "missing [a]ctions: {text}");
    assert!(text.contains("[?]help"), "missing [?]help: {text}");
    assert!(text.contains("[q]uit") || text.contains("[q] uit"), "missing q");
}
