//! Right pane — selected instance: header + status + truncated
//! snapshot list + PITR window. Stale fields tagged "(stale)".

use crate::tui::app::AppState;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

pub fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default().borders(Borders::ALL);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(inst) = state.selected_instance() else {
        f.render_widget(
            Paragraph::new("No instance selected").style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    };

    let mut lines: Vec<Line> = Vec::new();
    let header_dot = if inst.running { Span::styled("● running", Style::default().fg(Color::Green)) }
                     else            { Span::styled("○ stopped", Style::default().fg(Color::DarkGray)) };
    lines.push(Line::from(vec![
        Span::styled(inst.name.clone(), Style::default().fg(Color::Cyan)),
        Span::raw(format!("  PG{} {} :{}  ", inst.pg_version, inst.preset_label, inst.host_port)),
        header_dot,
    ]));
    lines.push(Line::raw(""));

    let stale = state.stale_status.contains(&inst.name);
    let stale_tag = if stale { "  (stale)" } else { "" };

    if let Some(s) = state.statuses.get(&inst.name) {
        if let (Some(cpu), Some(used), Some(limit)) = (s.cpu_percent, s.mem_used_mb, s.mem_limit_mb) {
            lines.push(Line::raw(format!("CPU:    {:.2}%        Mem: {} / {} MiB{}", cpu, used, limit, stale_tag)));
        }
        if let (Some(a), Some(i), Some(t)) = (s.connections_active, s.connections_idle, s.connections_total) {
            lines.push(Line::raw(format!("Conns:  {} active / {} idle / {} total", a, i, t)));
        }
        if let Some(b) = s.db_size_bytes {
            lines.push(Line::raw(format!("DB:     {}", human_bytes(b))));
        }
        if let Some(b) = s.pgdata_bytes {
            lines.push(Line::raw(format!("PGDATA: {}", human_bytes(b))));
        }
    } else {
        lines.push(Line::styled("(no status yet)", Style::default().fg(Color::DarkGray)));
    }

    lines.push(Line::raw(""));
    if let Some(v) = state.snapshots.get(&inst.name) {
        lines.push(Line::raw(format!("Snapshots ({})", v.list.len())));
        for s in v.list.iter().take(6) {
            let kind = format!("{:?}", s.kind).to_lowercase();
            let label = s.user_label.as_deref().unwrap_or("-");
            lines.push(Line::raw(format!("  {}  {}  {}", s.taken_at, kind, label)));
        }
        if v.list.len() > 6 {
            lines.push(Line::raw(format!("  … {} more  ([e] to expand)", v.list.len() - 6)));
        }
        if let (Some(a), Some(b)) = (&v.pitr.earliest, &v.pitr.latest) {
            lines.push(Line::raw(format!("PITR window: {} .. {}", a, b)));
        }
    }

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn human_bytes(b: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut n = b as f64; let mut u = 0;
    while n >= 1024.0 && u < units.len() - 1 { n /= 1024.0; u += 1; }
    format!("{n:.1}{}", units[u])
}
