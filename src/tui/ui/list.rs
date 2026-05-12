//! Left pane — instance list, selection highlighted, color-coded by
//! running/stopped state.

use crate::tui::app::AppState;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};

pub fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let items: Vec<ListItem> = state.instances.iter().enumerate().map(|(_i, inst)| {
        let dot = if inst.running { Span::styled("●", Style::default().fg(Color::Green)) }
                  else            { Span::styled("○", Style::default().fg(Color::DarkGray)) };
        let line = Line::from(vec![
            Span::raw(format!("{:<22} ", trim(&inst.name, 22))),
            Span::raw(format!("PG{:<3} ", inst.pg_version)),
            dot,
        ]);
        ListItem::new(line)
    }).collect();

    let title = format!(" Instances ({}) ", state.instances.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL);

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::BOLD).bg(Color::DarkGray))
        .highlight_symbol("> ");
    let mut ls = ListState::default();
    ls.select(Some(state.selected));
    f.render_stateful_widget(list, area, &mut ls);
}

fn trim(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() }
    else { s.chars().take(n.saturating_sub(1)).collect::<String>() + "…" }
}
