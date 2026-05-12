//! Left pane — instance list, selection highlighted, color-coded by
//! running/stopped state.

use crate::tui::app::AppState;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};

pub fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let items: Vec<ListItem> = state.instances.iter().map(|inst| {
        let dot = if inst.running { Span::styled("●", Style::default().fg(Color::Green)) }
                  else            { Span::styled("○", Style::default().fg(Color::DarkGray)) };
        // CPU snippet on the right — pulled from the most recent status
        // poller refresh. None when the container is stopped or status
        // hasn't run yet; render a fixed-width placeholder so columns
        // don't jump as each row's data arrives at different ticks.
        let cpu = state.statuses.get(&inst.name)
            .and_then(|s| s.cpu_percent);
        let cpu_span = match cpu {
            Some(p) if p >= 80.0 => Span::styled(format!("{:>5.1}%", p), Style::default().fg(Color::Red)),
            Some(p) if p >= 50.0 => Span::styled(format!("{:>5.1}%", p), Style::default().fg(Color::Yellow)),
            Some(p)              => Span::raw(format!("{:>5.1}%", p)),
            None                 => Span::styled("    -%", Style::default().fg(Color::DarkGray)),
        };
        let line = Line::from(vec![
            Span::raw(format!("{:<16} ", trim(&inst.name, 16))),
            Span::raw(format!("PG{:<2} ", inst.pg_version)),
            Span::raw(format!(":{:<5} ", inst.host_port)),
            cpu_span,
            Span::raw(" "),
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
