//! Plan 5 — clipboard support for the TUI. Builds postgres URIs from
//! InstanceState (with real password from state.toml) and copies them
//! to the system pasteboard via arboard.

use crate::error::{PgForgeError, Result};
use crate::state::instance::InstanceState;

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
    use arboard::Clipboard;
    let mut cb = Clipboard::new().map_err(|e| PgForgeError::Anyhow(anyhow::anyhow!("clipboard: {e}")))?;
    cb.set_text(s.to_string()).map_err(|e| PgForgeError::Anyhow(anyhow::anyhow!("clipboard set_text: {e}")))?;
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
