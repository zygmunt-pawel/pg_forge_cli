pub mod bottom;
pub mod detail;
pub mod list;
pub mod modal;

use crate::tui::app::AppState;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

pub fn render(f: &mut Frame, state: &AppState) {
    let outer = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(outer);
    let main = chunks[0];
    let bar  = chunks[1];

    let two = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(42), Constraint::Min(1)])
        .split(main);
    list::render(f, two[0], state);
    detail::render(f, two[1], state);
    bottom::render(f, bar, state);

    if let Some(m) = &state.modal {
        modal::render(f, outer, m);
    }
}
