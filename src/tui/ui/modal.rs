//! Modal overlay — six variants. Centered Block on top of the main UI.

use crate::tui::events::Modal;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

pub fn render(f: &mut Frame, full: Rect, modal: &Modal) {
    let (w, h) = match modal {
        Modal::CloneAs { .. } | Modal::UpgradeTo { .. } => (60, 9),
        Modal::RestoreAs { .. } => (70, 13),
        Modal::Confirm { .. } => (60, 7),
        Modal::ErrorDetail { .. } => (80, 15),
        Modal::Snapshots { .. } => (80, 20),
    };
    let area = centered_rect(w, h, full);
    f.render_widget(Clear, area);
    match modal {
        Modal::CloneAs { source, input } => single_input(f, area, &format!("Clone {source} as"), &input.buf, input.cursor),
        Modal::UpgradeTo { source, input } => single_input(f, area, &format!("Upgrade {source} — target version"), &input.buf, input.cursor),
        Modal::RestoreAs { source, as_input, target_time, focus } => {
            let block = Block::default().title(format!(" Restore {source} ")).borders(Borders::ALL);
            f.render_widget(block, area);
            let inner = area.inner(Margin{ horizontal: 1, vertical: 1 });
            let chunks = Layout::default().direction(Direction::Vertical)
                .constraints([Constraint::Length(2), Constraint::Length(2), Constraint::Min(1)])
                .split(inner);
            f.render_widget(field_para("New instance name:", &as_input.buf, *focus == 0), chunks[0]);
            f.render_widget(field_para("Target time (RFC3339, optional):", &target_time.buf, *focus == 1), chunks[1]);
            f.render_widget(
                Paragraph::new("[Tab] switch field   [Enter] continue   [Esc] cancel")
                    .style(Style::default().fg(Color::DarkGray)),
                chunks[2],
            );
        }
        Modal::Confirm { prompt, .. } => {
            let block = Block::default().title(" Confirm ").borders(Borders::ALL);
            f.render_widget(block, area);
            let inner = area.inner(Margin{ horizontal: 1, vertical: 1 });
            let lines = vec![
                Line::raw(prompt.as_str()),
                Line::raw(""),
                Line::styled("[y/Enter] yes   [n/Esc] no", Style::default().fg(Color::DarkGray)),
            ];
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
        Modal::ErrorDetail { msg } => {
            let block = Block::default().title(" Error ").borders(Borders::ALL).style(Style::default().fg(Color::Red));
            f.render_widget(block, area);
            let inner = area.inner(Margin{ horizontal: 1, vertical: 1 });
            f.render_widget(Paragraph::new(msg.as_str()).wrap(Wrap { trim: true }), inner);
        }
        Modal::Snapshots { name, view } => {
            let block = Block::default().title(format!(" Snapshots — {name} ")).borders(Borders::ALL);
            f.render_widget(block, area);
            let inner = area.inner(Margin{ horizontal: 1, vertical: 1 });
            let mut lines: Vec<Line> = Vec::with_capacity(view.list.len() + 2);
            for s in &view.list {
                let kind = format!("{:?}", s.kind).to_lowercase();
                let label = s.user_label.as_deref().unwrap_or("-");
                lines.push(Line::raw(format!("{}  {:<6}  {}", s.taken_at, kind, label)));
            }
            if let (Some(a), Some(b)) = (&view.pitr.earliest, &view.pitr.latest) {
                lines.push(Line::raw(""));
                lines.push(Line::raw(format!("PITR window: {} .. {}", a, b)));
            }
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
    }
}

fn single_input(f: &mut Frame, area: Rect, title: &str, buf: &str, _cursor: usize) {
    let block = Block::default().title(format!(" {title} ")).borders(Borders::ALL);
    f.render_widget(block, area);
    let inner = area.inner(Margin{ horizontal: 1, vertical: 1 });
    let chunks = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)])
        .split(inner);
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled("▌ ", Style::default().fg(Color::Cyan)), Span::raw(buf.to_string())])),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new("[Enter] continue   [Esc] cancel").style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

fn field_para(label: &str, buf: &str, focused: bool) -> Paragraph<'static> {
    let marker = if focused { "▌ " } else { "  " };
    Paragraph::new(vec![
        Line::styled(label.to_string(), Style::default().fg(Color::DarkGray)),
        Line::from(vec![
            Span::styled(marker, Style::default().fg(if focused { Color::Cyan } else { Color::DarkGray })),
            Span::raw(buf.to_string()),
        ]),
    ])
}

fn centered_rect(w: u16, h: u16, area: Rect) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect { x, y, width: w, height: h }
}
