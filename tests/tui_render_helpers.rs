//! Shared helpers for ratatui render tests. First TUI render-test
//! pattern in this repo — keep helpers minimal so tests stay obvious.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

pub fn draw_into<F>(width: u16, height: u16, draw_fn: F) -> Buffer
where
    F: FnOnce(&mut ratatui::Frame),
{
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| draw_fn(f)).unwrap();
    terminal.backend().buffer().clone()
}

/// Returns true if the given text appears anywhere in the rendered buffer.
pub fn buffer_contains(buf: &Buffer, needle: &str) -> bool {
    buffer_to_string(buf).contains(needle)
}

pub fn buffer_to_string(buf: &Buffer) -> String {
    let mut out = String::new();
    let area = buf.area();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}
