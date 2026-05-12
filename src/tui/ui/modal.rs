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
        Modal::Create { .. } => (72, 17),
        Modal::CreatedSuccess { .. } => (90, 11),
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
            // Place caret on the focused field. See Modal::Create arm for the same idiom.
            let (field, idx) = if *focus == 0 { (as_input, 0usize) } else { (target_time, 1usize) };
            let chunk = chunks[idx];
            let visual_col = field.buf[..field.cursor].chars().count() as u16;
            f.set_cursor_position(ratatui::layout::Position {
                x: chunk.x + 2 + visual_col,
                y: chunk.y + 1,
            });
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
        Modal::Create { name, app_user, pg_version, preset, no_backup, focus, .. } => {
            let block = Block::default().title(" Create new instance ").borders(Borders::ALL);
            f.render_widget(block, area);
            let inner = area.inner(Margin{ horizontal: 1, vertical: 1 });
            let chunks = Layout::default().direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(2), // name
                    Constraint::Length(2), // app_user
                    Constraint::Length(2), // pg_version
                    Constraint::Length(2), // preset
                    Constraint::Length(2), // backup toggle
                    Constraint::Min(1),    // footer
                ])
                .split(inner);
            f.render_widget(field_para("Instance name:", &name.buf, *focus == 0), chunks[0]);
            f.render_widget(field_para("App user:", &app_user.buf, *focus == 1), chunks[1]);
            f.render_widget(field_para("Postgres version:", &pg_version.buf, *focus == 2), chunks[2]);
            f.render_widget(
                cycle_para(
                    "Preset (← →):",
                    &preset_label(*preset),
                    *focus == 3,
                ),
                chunks[3],
            );
            f.render_widget(
                cycle_para(
                    "Backups (space):",
                    if *no_backup { "off — local dev / no S3" } else { "on — requires S3 in config.toml" },
                    *focus == 4,
                ),
                chunks[4],
            );
            // Help footer — styled spans so the keys stand out from the
            // surrounding prose. Three lines because [n]ew is the most
            // discovery-heavy keybind and users were missing them.
            let key = Style::default().fg(Color::Cyan);
            let dim = Style::default().fg(Color::DarkGray);
            f.render_widget(
                Paragraph::new(vec![
                    Line::raw(""),
                    Line::from(vec![
                        Span::styled("[Tab]", key), Span::styled(" next field   ", dim),
                        Span::styled("[Shift+Tab]", key), Span::styled(" previous", dim),
                    ]),
                    Line::from(vec![
                        Span::styled("[Space] / [← →]", key),
                        Span::styled(" cycle preset & backups toggle", dim),
                    ]),
                    Line::from(vec![
                        Span::styled("[Enter]", key), Span::styled(" create instance   ", dim),
                        Span::styled("[Esc]", key), Span::styled(" cancel", dim),
                    ]),
                    Line::styled(
                        "Password is auto-generated and shown once after [Enter].",
                        Style::default().fg(Color::DarkGray),
                    ),
                ]),
                chunks[5],
            );
            // Place the terminal's blinking caret at the active text
            // field. ratatui hides the cursor in the alternate screen
            // by default; calling set_cursor_position re-shows it and
            // places it where the next typed char would land.
            // Layout: field_para is 2 lines: label on chunk.y, input
            // on chunk.y+1. The "▌ " marker is 2 columns wide; the
            // visual cursor offset is `chars().count()` of buf up to
            // the byte-cursor (correct for ASCII; close enough for
            // multi-byte names which aren't expected here).
            if let Some(field) = match *focus {
                0 => Some(&*name),
                1 => Some(&*app_user),
                2 => Some(&*pg_version),
                _ => None,
            } {
                let chunk = chunks[*focus as usize];
                let visual_col = field.buf[..field.cursor].chars().count() as u16;
                f.set_cursor_position(ratatui::layout::Position {
                    x: chunk.x + 2 + visual_col,
                    y: chunk.y + 1,
                });
            }
        }
        Modal::CreatedSuccess { name, uri } => {
            let block = Block::default()
                .title(format!(" Instance '{name}' ready "))
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Green));
            f.render_widget(block, area);
            let inner = area.inner(Margin { horizontal: 1, vertical: 1 });
            let chunks = Layout::default().direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(2), // heading
                    Constraint::Length(2), // URI line
                    Constraint::Length(1), // spacer
                    Constraint::Length(2), // warning
                    Constraint::Min(1),    // footer
                ])
                .split(inner);
            f.render_widget(
                Paragraph::new(Line::styled(
                    "Save this connection URI now. The generated password is also in state.toml (0600).",
                    Style::default().fg(Color::White),
                ))
                .wrap(Wrap { trim: false }),
                chunks[0],
            );
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("▌ ", Style::default().fg(Color::Cyan)),
                    Span::styled(uri.as_str(), Style::default().fg(Color::Yellow)),
                ])),
                chunks[1],
            );
            f.render_widget(
                Paragraph::new(Line::styled(
                    "After dismiss, retrieve again with [Enter] on the instance row.",
                    Style::default().fg(Color::DarkGray),
                )).wrap(Wrap { trim: true }),
                chunks[3],
            );
            f.render_widget(
                Paragraph::new("[c]/[Enter] copy to clipboard   [Esc] dismiss")
                    .style(Style::default().fg(Color::DarkGray)),
                chunks[4],
            );
        }
    }
}

fn single_input(f: &mut Frame, area: Rect, title: &str, buf: &str, cursor: usize) {
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
    // Blinking caret on the input line.
    let visual_col = buf[..cursor].chars().count() as u16;
    f.set_cursor_position(ratatui::layout::Position {
        x: chunks[0].x + 2 + visual_col,
        y: chunks[0].y,
    });
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

/// One-line summary of a Preset's tuning parameters, shown inline in the
/// Create wizard so the user sees what they're picking at a glance:
/// "small — 2GB RAM · 100 conn · 512MB shared_buffers"
fn preset_label(p: crate::domain::preset::Preset) -> String {
    let name = format!("{:?}", p).to_lowercase();
    let t = p.tuning();
    format!(
        "{name} — {}GB RAM · {} conn · {}MB shared_buffers",
        t.ram_mb / 1024,
        t.max_connections,
        t.shared_buffers_mb,
    )
}

fn cycle_para(label: &str, value: &str, focused: bool) -> Paragraph<'static> {
    let marker = if focused { "▌ " } else { "  " };
    Paragraph::new(vec![
        Line::styled(label.to_string(), Style::default().fg(Color::DarkGray)),
        Line::from(vec![
            Span::styled(marker, Style::default().fg(if focused { Color::Cyan } else { Color::DarkGray })),
            Span::styled(
                value.to_string(),
                Style::default().fg(if focused { Color::Yellow } else { Color::White }),
            ),
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
