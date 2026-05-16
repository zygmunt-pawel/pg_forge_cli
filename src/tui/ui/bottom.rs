//! Bottom bar — priority: error > running op > flash > default keybinds.
//! The bottom-right corner always shows the pgforge version (dim).

use crate::tui::app::AppState;
use crate::tui::events::FlashKind;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Paragraph;

const SPINNER: &[char] = &['⠋','⠙','⠹','⠸','⠼','⠴','⠦','⠧'];

pub fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let version = format!(" v{} ", env!("CARGO_PKG_VERSION"));
    let disk = format_disk_zone(state.disk_health.as_ref());
    let [content_area, disk_area, version_area] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(disk.label.chars().count() as u16),
        Constraint::Length(version.chars().count() as u16),
    ])
    .areas(area);
    render_content(f, content_area, state);
    f.render_widget(
        Paragraph::new(disk.label).style(disk.style),
        disk_area,
    );
    f.render_widget(
        Paragraph::new(version).style(Style::default().add_modifier(Modifier::DIM)),
        version_area,
    );
}

struct DiskZone { label: String, style: Style }

fn format_disk_zone(h: Option<&crate::disk::health::DiskHealth>) -> DiskZone {
    use crate::disk::health::DiskStatus;
    let Some(h) = h else {
        return DiskZone {
            label: " Disk ? ".to_string(),
            style: Style::default().add_modifier(Modifier::DIM),
        };
    };
    let (label, style) = match h.status {
        DiskStatus::Unknown  => (" Disk ? ".to_string(),
                                 Style::default().add_modifier(Modifier::DIM)),
        DiskStatus::Ok       => (format!(" Disk {}% ", h.worst_pct),
                                 Style::default().add_modifier(Modifier::DIM)),
        DiskStatus::Warn     => (format!(" Disk {}% ", h.worst_pct),
                                 Style::default().fg(Color::Yellow)),
        DiskStatus::Critical => (format!(" Disk {}% ", h.worst_pct),
                                 Style::default().fg(Color::Red)),
    };
    DiskZone { label, style }
}

fn render_content(f: &mut Frame, area: Rect, state: &AppState) {
    if let Some(e) = &state.last_op_error {
        let text = format!(
            "✗ {} {} failed: {}  [?] details  [esc] clear",
            e.kind.label(), e.instance, truncate(&e.msg, 80)
        );
        f.render_widget(Paragraph::new(text).style(Style::default().fg(Color::Red)), area);
        return;
    }
    if !state.in_progress.is_empty() {
        // Prefer the selected instance's op; fall back to any.
        let chosen = state.selected_name()
            .and_then(|n| state.in_progress.get(n).map(|r| (n.to_string(), r.clone())))
            .or_else(|| state.in_progress.iter().next().map(|(k,v)| (k.clone(), v.clone())));
        if let Some((name, op)) = chosen {
            let elapsed = state.now.saturating_duration_since(op.started_at).as_secs();
            let frame = SPINNER[(elapsed as usize) % SPINNER.len()];
            let suffix = if state.in_progress.len() > 1 {
                format!("  (+{} more)", state.in_progress.len() - 1)
            } else { String::new() };
            let text = format!("{} {} on {} ({}s)…{}  [q]uit", frame, op.kind.label(), name, elapsed, suffix);
            f.render_widget(Paragraph::new(text).style(Style::default().fg(Color::Yellow)), area);
            return;
        }
    }
    if let Some(fl) = &state.flash {
        let style = match fl.kind {
            FlashKind::Success => Style::default().fg(Color::Green),
            FlashKind::Info    => Style::default().fg(Color::Cyan),
        };
        let prefix = match fl.kind { FlashKind::Success => "✓ ", FlashKind::Info => "• " };
        f.render_widget(Paragraph::new(format!("{prefix}{}", fl.msg)).style(style), area);
        return;
    }
    f.render_widget(
        Paragraph::new(
            "[n]ew [a]ctions [?]help [↵] uri [q]uit"
        ),
        area,
    );
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() }
    else { s.chars().take(n.saturating_sub(1)).collect::<String>() + "…" }
}
