//! Plan 5 — clipboard support for the TUI. Builds postgres URIs from
//! InstanceState and copies them via two strategies:
//!
//! - **OSC52** — terminal escape sequence interpreted by the *local*
//!   terminal (the one the user is looking at), making this work
//!   correctly over SSH where arboard cannot reach the user's clipboard.
//! - **arboard** — direct system pasteboard. Works on a local Mac/Linux
//!   desktop session; fails on headless macOS (no GUI/Aqua session).
//!
//! We try OSC52 first when `SSH_CONNECTION` is set (the user is remote
//! and OSC52 is the only thing that can reach their clipboard), and
//! arboard first when local. The other one is the fallback. Either
//! succeeding is reported as success.

use crate::error::{PgForgeError, Result};
use crate::state::instance::InstanceState;
use base64::Engine;

/// Build a postgres connection URI for an instance, using the real
/// password from state.toml (mode 0600). The TUI never *prints* this —
/// it's clipboard-only; the bottom-bar flash says "copied" without
/// echoing the URI.
pub fn build_connection_uri(state: &InstanceState) -> String {
    let i = &state.instance;
    format!(
        "postgresql://{user}:{pw}@127.0.0.1:{port}/{db}",
        user = i.app_user, pw = i.app_password,
        port = i.host_port, db = i.db_name,
    )
}

pub fn copy_to_clipboard(s: &str) -> Result<()> {
    let remote = std::env::var_os("SSH_CONNECTION").is_some()
        || std::env::var_os("SSH_TTY").is_some();
    let strategies: [fn(&str) -> std::result::Result<(), String>; 2] = if remote {
        [copy_via_osc52, copy_via_arboard]
    } else {
        [copy_via_arboard, copy_via_osc52]
    };
    let mut errors = Vec::with_capacity(2);
    for strat in strategies {
        match strat(s) {
            Ok(()) => return Ok(()),
            Err(e) => errors.push(e),
        }
    }
    Err(PgForgeError::Anyhow(anyhow::anyhow!(
        "no clipboard backend worked: {}",
        errors.join("; ")
    )))
}

/// OSC52: print escape sequence to stdout. The terminal emulator
/// intercepts `\x1b]52;c;<base64>\x07` and copies the decoded payload
/// into the user's local clipboard, regardless of where the process
/// itself is running.
fn copy_via_osc52(s: &str) -> std::result::Result<(), String> {
    use std::io::Write;
    let b64 = base64::engine::general_purpose::STANDARD.encode(s.as_bytes());
    let mut out = std::io::stdout().lock();
    write!(out, "\x1b]52;c;{}\x07", b64).map_err(|e| format!("OSC52 write: {e}"))?;
    out.flush().map_err(|e| format!("OSC52 flush: {e}"))?;
    Ok(())
}

fn copy_via_arboard(s: &str) -> std::result::Result<(), String> {
    use arboard::Clipboard;
    let mut cb = Clipboard::new().map_err(|e| format!("arboard new: {e}"))?;
    cb.set_text(s.to_string())
        .map_err(|e| format!("arboard set_text: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::instance::Instance;
    use crate::domain::preset::Preset;

    fn mk_state(user: &str, pw: &str, port: u16, db: &str) -> InstanceState {
        InstanceState {
            instance: Instance {
                name: "alpha".into(),
                db_name: db.into(),
                app_user: user.into(),
                app_password: pw.into(),
                pgbackrest_password: String::new(),
                preset: Preset::Tiny,
                pg_version: 18,
                host_port: port,
                backup_enabled: false,
                volume_name_override: None,
            },
            created_at: "2026-05-12T10:00:00Z".into(),
        }
    }

    #[test]
    fn uri_embeds_real_password() {
        let s = mk_state("leads", "s3cret", 5433, "leads");
        assert_eq!(
            build_connection_uri(&s),
            "postgresql://leads:s3cret@127.0.0.1:5433/leads"
        );
    }

    #[test]
    fn uri_with_special_chars_in_password_unescaped() {
        // arboard takes a raw string; consumers paste verbatim. We do NOT
        // url-encode — the user can paste into psql/JDBC which both accept
        // raw chars in the password component (psql tolerates URL-decoding
        // on its own when the URI has %XX, but our state.toml uses raw).
        let s = mk_state("leads", "p@ss/w*rd", 5432, "leads");
        let uri = build_connection_uri(&s);
        assert!(uri.contains("p@ss/w*rd"));
    }
}
