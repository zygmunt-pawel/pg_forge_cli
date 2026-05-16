//! `pgforge schedule {install,uninstall,status}` — manage the systemd user
//! timer that fires `pgforge snapshot --due` every 5 minutes.
//!
//! User units live under `~/.config/systemd/user/`. `systemctl --user`
//! manages them without root. For the typical headless-server case, the
//! operator must run `sudo loginctl enable-linger $USER` once so the timer
//! fires when no user session is active — this is checked at `install` time
//! and surfaced as a loud stderr warning if missing.

use crate::error::{PgForgeError, Result};
use std::path::PathBuf;
use std::process::Output;

pub const AGENT_LABEL: &str = "dev.pgforge.snapshot-due";

/// Status snapshot returned by `pgforge schedule status`.
#[derive(Debug, Clone)]
pub struct ScheduleStatus {
    /// Path of the `.timer` file.
    pub unit_path: PathBuf,
    /// True iff both unit files exist on disk.
    pub unit_present: bool,
    /// `systemctl --user is-enabled` reports `enabled`.
    pub enabled: bool,
    /// `systemctl --user is-active` reports `active`.
    pub active: bool,
    /// Next firing time as systemd reports it (`NextElapseUSecRealtime`),
    /// formatted as a human string. `None` when systemd reports unknown
    /// or parsing fails.
    pub next_run: Option<String>,
    /// `loginctl show-user $USER -p Linger` reports `Linger=yes`.
    pub linger_enabled: bool,
}

// ---------------------------------------------------------------------------
// Pure generators — unit-tested
// ---------------------------------------------------------------------------

/// Render the `.service` unit. `exe` must be an absolute path to the pgforge
/// binary (the systemd-spawned process won't inherit `$PATH`-shell lookups).
pub fn render_service(exe: &str) -> String {
    format!(
        "[Unit]\n\
         Description=pgforge: snapshot every backup-enabled instance whose hour is due\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart={exe} snapshot --due\n\
         Environment=PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin\n",
    )
}

/// Render the `.timer` unit. Fires 2 minutes after boot, then every 5 minutes.
///
/// Note on `OnUnitActiveSec`: the next firing is 5 min after the timer last
/// *activated*, not after the previous run *finished*. For a `snapshot --due`
/// tick that normally completes in seconds this is irrelevant; for a slow S3
/// push the next activation can overlap. Acceptable for a daily-snapshot
/// trigger; documented here so it isn't read later as a bug.
pub fn render_timer() -> String {
    format!(
        "[Unit]\n\
         Description=Run pgforge snapshot --due every 5 minutes\n\
         \n\
         [Timer]\n\
         OnBootSec=2min\n\
         OnUnitActiveSec=5min\n\
         Unit={AGENT_LABEL}.service\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n",
    )
}

// ---------------------------------------------------------------------------
// Filesystem layout
// ---------------------------------------------------------------------------

fn unit_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| PgForgeError::Anyhow(anyhow::anyhow!("HOME not set")))?;
    Ok(PathBuf::from(home).join(".config/systemd/user"))
}

fn service_path() -> Result<PathBuf> {
    Ok(unit_dir()?.join(format!("{AGENT_LABEL}.service")))
}

fn timer_path() -> Result<PathBuf> {
    Ok(unit_dir()?.join(format!("{AGENT_LABEL}.timer")))
}

// ---------------------------------------------------------------------------
// Shell-out helpers
// ---------------------------------------------------------------------------

fn run_systemctl(args: &[&str]) -> Result<Output> {
    std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .map_err(|e| PgForgeError::Anyhow(anyhow::anyhow!("systemctl --user {:?}: {e}", args)))
}

fn run_loginctl(args: &[&str]) -> Result<Output> {
    std::process::Command::new("loginctl")
        .args(args)
        .output()
        .map_err(|e| PgForgeError::Anyhow(anyhow::anyhow!("loginctl {:?}: {e}", args)))
}

/// True iff `loginctl show-user $USER -p Linger` reports `Linger=yes`.
fn linger_enabled() -> bool {
    let user = match std::env::var("USER") {
        Ok(u) => u,
        Err(_) => return false,
    };
    let Ok(out) = run_loginctl(&["show-user", &user, "-p", "Linger"]) else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|l| l.trim() == "Linger=yes")
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Install the user timer. Returns the path of the `.timer` file.
pub fn install() -> Result<PathBuf> {
    let exe = std::env::current_exe()
        .map_err(|e| PgForgeError::Anyhow(anyhow::anyhow!("current_exe: {e}")))?;
    let exe_str = exe.to_string_lossy().into_owned();

    let dir = unit_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| PgForgeError::Io {
        path: dir.clone(),
        source: e,
    })?;

    let svc_path = service_path()?;
    let tmr_path = timer_path()?;
    std::fs::write(&svc_path, render_service(&exe_str)).map_err(|e| PgForgeError::Io {
        path: svc_path.clone(),
        source: e,
    })?;
    std::fs::write(&tmr_path, render_timer()).map_err(|e| PgForgeError::Io {
        path: tmr_path.clone(),
        source: e,
    })?;

    let reload = run_systemctl(&["daemon-reload"])?;
    if !reload.status.success() {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "systemctl --user daemon-reload returned {}: {}",
            reload.status,
            String::from_utf8_lossy(&reload.stderr).trim()
        )));
    }
    let enable = run_systemctl(&["enable", "--now", &format!("{AGENT_LABEL}.timer")])?;
    if !enable.status.success() {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "systemctl --user enable --now returned {}: {}",
            enable.status,
            String::from_utf8_lossy(&enable.stderr).trim()
        )));
    }

    if !linger_enabled() {
        eprintln!(
            "WARNING: linger is not enabled for $USER — the timer will only fire \
             while you are logged in. Run `sudo loginctl enable-linger $USER` to \
             have it fire on a headless server."
        );
    }

    Ok(tmr_path)
}

pub fn uninstall() -> Result<()> {
    // Best-effort disable; tolerate "not loaded" / "doesn't exist".
    let _ = run_systemctl(&["disable", "--now", &format!("{AGENT_LABEL}.timer")]);

    let svc = service_path()?;
    let tmr = timer_path()?;
    for p in [&tmr, &svc] {
        if p.exists() {
            std::fs::remove_file(p).map_err(|e| PgForgeError::Io {
                path: p.clone(),
                source: e,
            })?;
        }
    }

    let _ = run_systemctl(&["daemon-reload"]);
    Ok(())
}

pub fn status() -> Result<ScheduleStatus> {
    let svc = service_path()?;
    let tmr = timer_path()?;
    let unit_present = svc.exists() && tmr.exists();
    let timer_unit = format!("{AGENT_LABEL}.timer");

    let is_enabled = run_systemctl(&["is-enabled", &timer_unit])
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "enabled")
        .unwrap_or(false);
    let is_active = run_systemctl(&["is-active", &timer_unit])
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "active")
        .unwrap_or(false);

    let next_run = run_systemctl(&[
        "show",
        &timer_unit,
        "-p",
        "NextElapseUSecRealtime",
        "--value",
    ])
    .ok()
    .and_then(|o| {
        if o.status.success() {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() || s == "n/a" { None } else { Some(s) }
        } else {
            None
        }
    });

    Ok(ScheduleStatus {
        unit_path: tmr,
        unit_present,
        enabled: is_enabled,
        active: is_active,
        next_run,
        linger_enabled: linger_enabled(),
    })
}
