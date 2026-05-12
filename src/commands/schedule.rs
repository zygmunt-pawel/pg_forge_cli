//! `pgforge schedule {install,uninstall,status}` — manage the macOS
//! launchd agent that fires `pgforge snapshot --due` periodically.
//!
//! Why a global "every 5 minutes" agent instead of N per-instance
//! agents at exact times: macOS launchd is fine with both, but the
//! per-instance variant means re-running launchctl bootstrap every
//! time the user changes a snapshot_hour. The tick agent is simpler —
//! it always runs, `snapshot --due` reads state.toml on each tick and
//! decides what (if anything) is overdue. The trade-off is that a
//! snapshot scheduled for 03:00 fires at 03:00-03:04 wall-clock; for
//! a daily backup this is invisible.

use crate::error::{PgForgeError, Result};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// launchctl soft-failure classification helpers
// ---------------------------------------------------------------------------

/// Returns true when `stderr` from `launchctl bootstrap` signals a headless-
/// domain failure — the plist is still on disk and launchd will load it at the
/// next user login, so the caller should downgrade to a warning rather than an
/// error.
///
/// Case-insensitive substring match to tolerate macOS-version wording changes.
pub fn launchctl_is_soft_install_failure(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("input/output error")
        || lower.contains("could not find domain")
        || lower.contains("domain target")
        || lower.contains("bootstrap failed: 5")
        || lower.contains("bootstrap failed: 112")
        || lower.contains("bootstrap failed: 113")
}

/// Returns true when `stderr` from `launchctl bootout` signals the service
/// was already absent — treat as idempotent success and continue to plist
/// removal.
pub fn launchctl_is_already_gone(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("could not find specified service")
        || lower.contains("no such process")
        || lower.contains("service is disabled and cannot be loaded")
        || lower.contains("esrch")
}

// ---------------------------------------------------------------------------
// Private helper — runs launchctl and captures stdout+stderr
// ---------------------------------------------------------------------------

fn run_launchctl(args: &[&str]) -> Result<std::process::Output> {
    std::process::Command::new("launchctl")
        .args(args)
        .output()
        .map_err(|e| PgForgeError::Anyhow(anyhow::anyhow!("launchctl {:?}: {e}", args)))
}

pub const AGENT_LABEL: &str = "dev.pgforge.snapshot-due";
const TICK_SECONDS: u32 = 300; // 5 min

#[derive(Debug, Clone)]
pub struct ScheduleStatus {
    /// True iff `~/Library/LaunchAgents/<label>.plist` exists.
    pub plist_present: bool,
    /// True iff `launchctl list` reports the label as loaded. Only
    /// meaningful when a GUI session exists (`launchctl print
    /// gui/<uid>` reachable). On headless boxes the plist gets picked
    /// up at next user login.
    pub loaded: bool,
    pub plist_path: PathBuf,
}

pub fn install() -> Result<PathBuf> {
    let exe = std::env::current_exe()
        .map_err(|e| PgForgeError::Anyhow(anyhow::anyhow!("current_exe: {e}")))?;
    let log_dir = log_dir()?;
    std::fs::create_dir_all(&log_dir).map_err(|e| PgForgeError::Io {
        path: log_dir.clone(),
        source: e,
    })?;
    let log_path = log_dir.join("schedule.log");
    let plist = render_plist(&exe.to_string_lossy(), &log_path.to_string_lossy());
    let plist_path = plist_path()?;
    if let Some(parent) = plist_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| PgForgeError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    std::fs::write(&plist_path, plist).map_err(|e| PgForgeError::Io {
        path: plist_path.clone(),
        source: e,
    })?;
    // Try to load it now. On headless macOS this can fail with
    // "Input/output error" / "Could not find domain" / "Bootstrap failed: 5"
    // because gui/<uid> is unreachable without a GUI session. The plist is
    // still on disk and launchd will pick it up at the next user login.
    // (Task 3.4 will prepend a defensive best-effort bootout here.)
    let domain = format!("gui/{}", uid_or_501());
    let plist_str = plist_path.to_string_lossy();
    let out = run_launchctl(&["bootstrap", &domain, &plist_str])?;
    if out.status.success() {
        return Ok(plist_path);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if launchctl_is_soft_install_failure(&stderr) {
        tracing::warn!(
            "launchctl bootstrap returned non-zero on a headless domain — \
             the plist is on disk at {} and will load at next user login. \
             stderr: {}",
            plist_path.display(),
            stderr.trim()
        );
        return Ok(plist_path);
    }
    Err(PgForgeError::Anyhow(anyhow::anyhow!(
        "launchctl bootstrap returned {}: {}",
        out.status,
        stderr.trim()
    )))
}

pub fn uninstall() -> Result<()> {
    let path = plist_path()?;
    let domain = format!("gui/{}", uid_or_501());
    let path_str = path.to_string_lossy();
    let out = run_launchctl(&["bootout", &domain, &path_str])?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if launchctl_is_already_gone(&stderr) {
            // Service was never loaded or already removed — still delete the
            // plist so the install state is clean.
            tracing::debug!(
                "launchctl bootout: service not found (idempotent). stderr: {}",
                stderr.trim()
            );
        } else {
            return Err(PgForgeError::Anyhow(anyhow::anyhow!(
                "launchctl bootout returned {}: {}",
                out.status,
                stderr.trim()
            )));
        }
    }
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| PgForgeError::Io {
            path: path.clone(),
            source: e,
        })?;
    }
    Ok(())
}

pub fn status() -> Result<ScheduleStatus> {
    let path = plist_path()?;
    let loaded = std::process::Command::new("launchctl")
        .args(["print"])
        .arg(format!("gui/{}/{}", uid_or_501(), AGENT_LABEL))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    Ok(ScheduleStatus {
        plist_present: path.exists(),
        loaded,
        plist_path: path,
    })
}

fn plist_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| {
        PgForgeError::Anyhow(anyhow::anyhow!("HOME not set"))
    })?;
    Ok(PathBuf::from(home)
        .join("Library/LaunchAgents")
        .join(format!("{AGENT_LABEL}.plist")))
}

fn log_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| {
        PgForgeError::Anyhow(anyhow::anyhow!("HOME not set"))
    })?;
    Ok(PathBuf::from(home).join("Library/Logs/pgforge"))
}

fn uid_or_501() -> u32 {
    #[cfg(unix)]
    unsafe { libc_getuid() }
    #[cfg(not(unix))]
    501
}

#[cfg(unix)]
unsafe fn libc_getuid() -> u32 {
    // libc isn't a direct dep yet; spawn `id -u` once to avoid pulling
    // it in just for this. Cheap (~1 ms) and only called by install /
    // uninstall / status.
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(501)
}

fn render_plist(exe: &str, log_path: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>snapshot</string>
        <string>--due</string>
    </array>
    <key>StartInterval</key>
    <integer>{tick}</integer>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
    </dict>
</dict>
</plist>
"#,
        label = AGENT_LABEL,
        exe = exe,
        tick = TICK_SECONDS,
        log = log_path,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_contains_label_and_program() {
        let p = render_plist("/usr/local/bin/pgforge", "/var/log/pgforge.log");
        assert!(p.contains("dev.pgforge.snapshot-due"));
        assert!(p.contains("<string>/usr/local/bin/pgforge</string>"));
        assert!(p.contains("<string>snapshot</string>"));
        assert!(p.contains("<string>--due</string>"));
        // StartInterval 5 minutes
        assert!(p.contains("<integer>300</integer>"));
    }
}
