//! Plan 5 — interactive ratatui dashboard.

pub mod app;
pub mod clipboard;
pub mod events;
pub mod ops;
pub mod refresh;
pub mod ui;

use crate::error::Result;
use crate::tui::app::AppState;
use crate::tui::events::Event;
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

pub async fn run() -> Result<()> {
    install_panic_hook();
    let mut stdout = io::stdout();
    enable_raw_mode().map_err(map_term_err)?;
    execute!(stdout, EnterAlternateScreen).map_err(map_term_err)?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend).map_err(map_term_err)?;

    let result = run_loop(&mut term).await;

    disable_raw_mode().ok();
    execute!(io::stdout(), LeaveAlternateScreen).ok();
    term.show_cursor().ok();
    result
}

async fn run_loop(term: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Event>();
    let (names_tx, names_rx) = watch::channel::<Vec<String>>(Vec::new());

    refresh::spawn_pollers(tx.clone(), names_rx, None);
    spawn_key_reader(tx.clone());

    let mut state = AppState::default();

    loop {
        if state.should_quit { break; }
        term.draw(|f| ui::render(f, &state)).map_err(map_term_err)?;
        let ev = tokio::select! {
            biased;
            Some(e) = rx.recv()                              => e,
            _ = tokio::time::sleep(Duration::from_millis(250)) => Event::Tick,
        };
        if let Event::InstancesListed(rows) = &ev {
            let names: Vec<String> = rows.iter().map(|r| r.name.clone()).collect();
            let _ = names_tx.send(names);
        }
        state.apply_event(ev);

        for (encoded, kind) in std::mem::take(&mut state.pending_ops) {
            ops::spawn(kind, encoded, tx.clone(), None);
        }
        // [Enter] on the instance list pushes a name into pending_clipboard;
        // we open a Modal::ConnectionString with the URI text instead of
        // trying to copy it via OSC52 / arboard. Terminal-clipboard support
        // varies (iTerm2 default-off, tmux passthrough, …) so showing the
        // URI and letting the user select-with-mouse + Cmd+C is the
        // simplest "it just works" path.
        for n in std::mem::take(&mut state.pending_clipboard) {
            match build_post_create_uri(&n) {
                Ok(uri) => {
                    state.modal = Some(crate::tui::events::Modal::ConnectionString { name: n, uri });
                }
                Err(e) => {
                    state.last_op_error = Some(crate::tui::events::OpError {
                        instance: n,
                        kind: crate::tui::events::OpKind::Clipboard,
                        msg: format!("load state for connection string: {e}"),
                        at: std::time::Instant::now(),
                    });
                }
            }
        }
        for n in std::mem::take(&mut state.refresh_requests) {
            refresh::refresh_one(n, tx.clone(), None);
        }
        for req in std::mem::take(&mut state.pending_creates) {
            ops::spawn_create(req, tx.clone(), None);
        }
        // After a successful Create, build the URI from the freshly-written
        // state.toml and open the CreatedSuccess modal so the user can
        // copy the password before it disappears into 0600 storage.
        for n in std::mem::take(&mut state.pending_show_created) {
            match build_post_create_uri(&n) {
                Ok(uri) => {
                    state.modal = Some(crate::tui::events::Modal::CreatedSuccess { name: n, uri });
                }
                Err(e) => {
                    tracing::warn!(target: "pgforge::tui", "post-create URI build for {n} failed: {e}");
                }
            }
        }
    }
    Ok(())
}

fn build_post_create_uri(instance_name: &str) -> Result<String> {
    let root = crate::state::instance::InstanceState::default_state_root();
    let st = crate::state::instance::InstanceState::load_under(&root, instance_name)?;
    Ok(clipboard::build_connection_uri(&st))
}

// `do_clipboard` previously fired OSC52 + arboard for [Enter]; removed
// in favour of showing the URI in a modal (Modal::ConnectionString).
// The clipboard::copy_to_clipboard helper is kept for potential future
// CLI hooks but no longer wired into the TUI keymap.

fn spawn_key_reader(tx: mpsc::UnboundedSender<Event>) {
    tokio::task::spawn_blocking(move || {
        loop {
            if crossterm::event::poll(Duration::from_millis(500)).unwrap_or(false) {
                if let Ok(crossterm::event::Event::Key(k)) = crossterm::event::read() {
                    if tx.send(Event::Key(k)).is_err() { return; }
                }
            }
        }
    });
}

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        prev(info);
    }));
}

fn map_term_err(e: io::Error) -> crate::error::PgForgeError {
    crate::error::PgForgeError::Anyhow(anyhow::anyhow!("terminal: {e}"))
}
