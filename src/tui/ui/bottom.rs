//! Bottom bar — priority: error > running op > flash > default keybinds.

use crate::tui::app::AppState;
use crate::tui::events::FlashKind;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Paragraph;

const SPINNER: &[char] = &['⠋','⠙','⠹','⠸','⠼','⠴','⠦','⠧'];

pub fn render(f: &mut Frame, area: Rect, state: &AppState) {
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
            "[s]nap [c]lone [R]otate [u]pgrade [r]estore [d]estroy [↵] copy [q]uit"
        ),
        area,
    );
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() }
    else { s.chars().take(n.saturating_sub(1)).collect::<String>() + "…" }
}
