//! `pgforge smart install` / `uninstall` orchestration + the pure rendering
//! helpers for the sudoers fragment and the systemd-user unit files.
//!
//! Renderers are deterministic, no I/O — easy to snapshot-test. Orchestration
//! shells out to `sudo`, `visudo`, `install(1)`, and `systemctl --user`.

use jiff::Timestamp;
use std::path::{Path, PathBuf};

use crate::smart::cache::{default_cache_path, write_cache};
use crate::smart::check::{SudoMode, check_all};
use crate::smart::installed::{InstalledState, default_installed_path, read_installed, write_installed};
use crate::smart::types::SmartHealth;

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("no devices to install for (empty discovery)")]
    NoDevices,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("subprocess failed: {0}")]
    Subprocess(String),
    #[error("visudo validation failed: {0}")]
    SudoersValidation(String),
}

/// Render the sudoers fragment that grants the given user NOPASSWD on
/// `smartctl_path -H -A -j /dev/X` for each enumerated device.
///
/// - One rule per line (intentional: line-by-device diffs are readable).
/// - Refuses empty `devices` → `InstallError::NoDevices` (defense in depth
///   against installs that grant nothing).
pub fn render_sudoers_fragment(
    user: &str,
    smartctl_path: &Path,
    devices: &[std::path::PathBuf],
) -> Result<String, InstallError> {
    if devices.is_empty() {
        return Err(InstallError::NoDevices);
    }
    let installed_at = Timestamp::now();
    let mut s = String::new();
    s.push_str("# pgforge SMART disk health checks\n");
    s.push_str("#\n");
    s.push_str(&format!("# Installed by `pgforge smart install` on {installed_at}.\n"));
    s.push_str("# Allows the pgforge-smart.timer (systemd-user) to read SMART data from\n");
    s.push_str("# the disks discovered at install time. Each line is one exact device path\n");
    s.push_str("# (no wildcards) so adding a new disk requires `pgforge smart install --force`.\n");
    s.push_str("#\n");
    s.push_str("# Remove with: pgforge smart uninstall\n\n");
    for dev in devices {
        s.push_str(&format!(
            "{user} ALL=(root) NOPASSWD: {} -H -A -j {}\n",
            smartctl_path.display(),
            dev.display(),
        ));
    }
    Ok(s)
}

pub fn render_timer_unit() -> String {
    "[Unit]\n\
     Description=pgforge daily SMART disk health check\n\
     \n\
     [Timer]\n\
     OnCalendar=daily\n\
     RandomizedDelaySec=1h\n\
     Persistent=true\n\
     Unit=pgforge-smart.service\n\
     \n\
     [Install]\n\
     WantedBy=timers.target\n"
        .to_string()
}

pub fn render_service_unit(pgforge_path: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=pgforge SMART disk health check (writes cache)\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart={} smart check --write-cache\n",
        pgforge_path.display(),
    )
}

pub struct InstallOpts {
    pub force: bool,
}

/// Full install pipeline. Idempotent re-runs without --force are fine when
/// the rendered sudoers fragment is byte-identical to the existing one.
pub async fn install_all(opts: InstallOpts) -> Result<SmartHealth, InstallError> {
    // Step 1: smartctl present?
    let smartctl_path = which_smartctl().ok_or_else(|| InstallError::Subprocess(
        "smartctl not found — sudo apt install smartmontools".to_string(),
    ))?;

    // Step 2: discover disks
    let discovered = crate::smart::check::discover_disks().await;
    if discovered.is_empty() {
        return Err(InstallError::NoDevices);
    }
    let devices: Vec<PathBuf> = discovered.iter().map(|d| d.path.clone()).collect();

    // Step 3: render + validate in a tempfile
    let user = whoami_string()?;
    let fragment = render_sudoers_fragment(&user, &smartctl_path, &devices)?;
    let tmp = tempfile::NamedTempFile::new()
        .map_err(|e| InstallError::Subprocess(format!("tempfile: {e}")))?;
    std::fs::write(tmp.path(), fragment.as_bytes())
        .map_err(InstallError::Io)?;
    let out = std::process::Command::new("sudo")
        .args(["visudo", "-c", "-f"])
        .arg(tmp.path())
        .output()
        .map_err(InstallError::Io)?;
    if !out.status.success() {
        return Err(InstallError::SudoersValidation(
            String::from_utf8_lossy(&out.stderr).to_string(),
        ));
    }

    // Step 4: idempotency check
    let final_path = PathBuf::from("/etc/sudoers.d/pgforge-smart");
    let existing = std::fs::read_to_string(&final_path).ok();
    let needs_install = match (&existing, opts.force) {
        (Some(e), false) if e == &fragment => false,
        (Some(_), false) => {
            return Err(InstallError::Subprocess(
                "/etc/sudoers.d/pgforge-smart exists with different content — pass --force to overwrite".to_string(),
            ));
        }
        _ => true,
    };
    if needs_install {
        let out = std::process::Command::new("sudo")
            .args(["install", "-m", "0440", "-o", "root", "-g", "root"])
            .arg(tmp.path())
            .arg(&final_path)
            .output()
            .map_err(InstallError::Io)?;
        if !out.status.success() {
            return Err(InstallError::Subprocess(format!(
                "sudo install: {}", String::from_utf8_lossy(&out.stderr)
            )));
        }
    }

    // Step 5: write InstalledState
    let installed = InstalledState {
        smartctl_path: smartctl_path.clone(),
        user,
        devices: devices.clone(),
        installed_at: Timestamp::now(),
    };
    write_installed(&default_installed_path(), &installed)?;

    // Step 6: write systemd-user units
    let pgforge_path = std::env::current_exe().ok().unwrap_or_else(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        home.join(".local/bin/pgforge")
    });
    let units_dir = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| InstallError::Subprocess("HOME not set".to_string()))?
        .join(".config/systemd/user");
    std::fs::create_dir_all(&units_dir)?;
    std::fs::write(units_dir.join("pgforge-smart.timer"), render_timer_unit())?;
    std::fs::write(
        units_dir.join("pgforge-smart.service"),
        render_service_unit(&pgforge_path),
    )?;

    // Step 7: reload + enable + start
    run_systemctl_user(&["daemon-reload"])?;
    run_systemctl_user(&["enable", "--now", "pgforge-smart.timer"])?;

    // Step 8: first check now
    let installed_read = read_installed(&default_installed_path());
    let health = check_all(installed_read.as_ref(), SudoMode::NonInteractive).await;
    let _ = write_cache(&default_cache_path(), &health);

    Ok(health)
}

/// Reverse of install. Idempotent.
pub async fn uninstall_all() -> Result<(), InstallError> {
    // Best-effort — keep going on any single failure.
    let _ = run_systemctl_user(&["disable", "--now", "pgforge-smart.timer"]);
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        let units = home.join(".config/systemd/user");
        let _ = std::fs::remove_file(units.join("pgforge-smart.timer"));
        let _ = std::fs::remove_file(units.join("pgforge-smart.service"));
    }
    let _ = run_systemctl_user(&["daemon-reload"]);
    let _ = std::process::Command::new("sudo")
        .args(["rm", "-f", "/etc/sudoers.d/pgforge-smart"])
        .status();
    let _ = std::fs::remove_file(default_installed_path());
    let _ = std::fs::remove_file(default_cache_path());
    Ok(())
}

pub fn postinstall_summary(health: &SmartHealth) -> String {
    use crate::smart::types::SmartStatus;
    let mut out = String::new();
    for d in &health.drives {
        let label = d.status.label();
        let reasons = if d.reasons.is_empty() {
            String::new()
        } else {
            format!(" — {}", d.reasons.join(", "))
        };
        out.push_str(&format!(
            "  {} ({}): SMART {}{}\n",
            d.device, d.model, label, reasons,
        ));
    }
    let summary = match health.status {
        SmartStatus::Ok       => format!("Overall: SMART ok across {} disk(s).", health.drives.len()),
        SmartStatus::Warn     => format!("Overall: SMART warn (worst: {}).", health.worst_device.as_deref().unwrap_or("?")),
        SmartStatus::Critical => format!("Overall: SMART FAIL (worst: {}). Replace drive.", health.worst_device.as_deref().unwrap_or("?")),
        SmartStatus::Unknown  => {
            let all_unsupported = health.drives.iter().all(|d| {
                d.unknown_reason == Some(crate::smart::types::SmartUnknownReason::DeviceNotSupported)
            });
            if !health.drives.is_empty() && all_unsupported {
                "\u{26A0} Install completed, but no disk exposes SMART data (typical on VPS \
                 without passthrough). Status will be 'SMART ?' indefinitely. Capacity \
                 monitoring (Disk N% used) continues to work. To remove: pgforge smart uninstall.".to_string()
            } else {
                format!("Overall: SMART ? ({}).", health.unknown_reason
                    .map(|r| r.to_string()).unwrap_or_else(|| "no devices".to_string()))
            }
        }
    };
    out.push_str(&summary);
    out
}

fn which_smartctl() -> Option<PathBuf> {
    let out = std::process::Command::new("which").arg("smartctl").output().ok()?;
    if !out.status.success() { return None; }
    let s = String::from_utf8(out.stdout).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() { return None; }
    Some(PathBuf::from(trimmed))
}

fn whoami_string() -> Result<String, InstallError> {
    let out = std::process::Command::new("whoami").output()
        .map_err(InstallError::Io)?;
    if !out.status.success() {
        return Err(InstallError::Subprocess("whoami exit non-zero".to_string()));
    }
    let s = String::from_utf8(out.stdout)
        .map_err(|_| InstallError::Subprocess("whoami non-utf8".to_string()))?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(InstallError::Subprocess("whoami empty".to_string()));
    }
    Ok(trimmed.to_string())
}

fn run_systemctl_user(args: &[&str]) -> Result<(), InstallError> {
    let out = std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .map_err(InstallError::Io)?;
    if !out.status.success() {
        return Err(InstallError::Subprocess(format!(
            "systemctl --user {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr),
        )));
    }
    Ok(())
}
