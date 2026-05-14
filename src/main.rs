use clap::Parser;
use pgforge::cli::{Cli, dispatch};
use pgforge::error::Result;
use std::io::IsTerminal;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    // TUI mode = no subcommand + stdout is a real tty. In that case route
    // tracing to a file under ~/Library/Logs/pgforge/ so warn!/error! logs
    // from background pollers don't shred the ratatui layout. CLI subcommands
    // keep tracing on stderr as before.
    let tui_mode = cli.command.is_none() && std::io::stdout().is_terminal();
    if tui_mode {
        if let Some(path) = log_file_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                tracing_subscriber::fmt()
                    .with_env_filter(
                        EnvFilter::try_from_default_env()
                            .unwrap_or_else(|_| EnvFilter::new("warn")),
                    )
                    .with_target(false)
                    .with_writer(file)
                    .with_ansi(false)
                    .init();
            }
            // If the file open failed silently, no subscriber is installed;
            // tracing macros become no-ops. Better than corrupting the TUI.
        }
    } else {
        // CLI mode: diagnostic logs go to STDERR, never stdout —
        // `tracing_subscriber::fmt()` defaults to stdout, which would
        // pollute commands whose stdout is a contract (e.g. `pgforge dump`
        // prints exactly the dump path on stdout for `make`/`scp` glue).
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
            )
            .with_target(false)
            .with_writer(std::io::stderr)
            .init();
    }
    dispatch(cli).await
}

fn log_file_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        std::path::PathBuf::from(home)
            .join("Library/Logs/pgforge")
            .join("tui.log"),
    )
}
