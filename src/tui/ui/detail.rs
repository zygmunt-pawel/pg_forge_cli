//! Right pane — selected instance: header + status + truncated
//! snapshot list + PITR window. Stale fields tagged "(stale)".

use crate::tui::app::AppState;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Sparkline, Wrap};

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
        Span::raw(format!("  PG{}  :{}  ", inst.pg_version, inst.host_port)),
        header_dot,
    ]));
    // Preset summary — spell out RAM / max_connections / shared_buffers
    // so the user doesn't have to look up what `medium` means.
    if let Some(preset) = parse_preset(&inst.preset_label) {
        let t = preset.tuning();
        lines.push(Line::styled(
            format!(
                "Preset: {} — {}GB RAM · {} conn · {}MB shared_buffers",
                inst.preset_label,
                t.ram_mb / 1024,
                t.max_connections,
                t.shared_buffers_mb,
            ),
            Style::default().fg(Color::DarkGray),
        ));
    }
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
        // Heartbeat / uptime row.
        let mut hb: Vec<Span> = Vec::new();
        if let Some(up) = s.uptime_seconds {
            hb.push(Span::raw(format!("Uptime: {}", crate::commands::status::humanize_uptime(up))));
        }
        if let Some(rc) = s.restart_count && rc > 0 {
            if !hb.is_empty() { hb.push(Span::raw("   ")); }
            hb.push(Span::styled(format!("Restarts: {rc}"), Style::default().fg(Color::Yellow)));
        }
        if let Some(resp) = s.db_responsive {
            if !hb.is_empty() { hb.push(Span::raw("   ")); }
            if resp {
                hb.push(Span::styled("DB ✓ responsive", Style::default().fg(Color::Green)));
            } else {
                hb.push(Span::styled("DB ✗ not responding", Style::default().fg(Color::Red)));
            }
        }
        if !hb.is_empty() { lines.push(Line::from(hb)); }
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

    // Split inner: text on top, 2-row sparkline at the bottom.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)])
        .split(inner);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), chunks[0]);

    // CPU sparkline — last ~60s of samples for the selected instance.
    let hist: Vec<u64> = state
        .cpu_history
        .get(&inst.name)
        .map(|v| v.iter().copied().collect())
        .unwrap_or_default();
    if !hist.is_empty() {
        let max_sample = hist.iter().copied().max().unwrap_or(1000).max(100);
        let spark = Sparkline::default()
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .title(Line::from(vec![
                        Span::styled(" CPU last ~60s ", Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            format!("max {:.1}%", (max_sample as f64) / 10.0),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ])),
            )
            .data(&hist[..])
            .max(max_sample)
            .style(Style::default().fg(Color::Cyan));
        f.render_widget(spark, chunks[1]);
    }
}

fn parse_preset(label: &str) -> Option<crate::domain::preset::Preset> {
    use std::str::FromStr;
    crate::domain::preset::Preset::from_str(label).ok()
}

fn human_bytes(b: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut n = b as f64; let mut u = 0;
    while n >= 1024.0 && u < units.len() - 1 { n /= 1024.0; u += 1; }
    format!("{n:.1}{}", units[u])
}
