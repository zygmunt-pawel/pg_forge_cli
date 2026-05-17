//! `pgforge smart install` / `uninstall` orchestration + the pure rendering
//! helpers for the sudoers fragment and the systemd-user unit files.
//!
//! Renderers are deterministic, no I/O — easy to snapshot-test. Orchestration
//! shells out to `sudo`, `visudo`, `install(1)`, and `systemctl --user`.

use jiff::Timestamp;
use std::path::Path;

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
