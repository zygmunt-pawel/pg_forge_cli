//! pgforge smart {install, check, status, uninstall}.
//!
//! `install` and `uninstall` shell out to sudo/systemctl (orchestrated in
//! `crate::smart::install`). `check` and `status` are pure read paths
//! (smartctl subprocess + cache read).

use crate::error::{PgForgeError, Result};
use crate::smart::cache::{STALE_AFTER_HOURS, default_cache_path, read_cache, write_cache};
use crate::smart::check::{SudoMode, check_all};
use crate::smart::install::{InstallOpts, install_all, postinstall_summary, uninstall_all};
use crate::smart::installed::{default_installed_path, read_installed};
use crate::smart::types::SmartStatus;

pub async fn run_install(force: bool) -> Result<()> {
    let health = install_all(InstallOpts { force })
        .await
        .map_err(|e| PgForgeError::Anyhow(anyhow::anyhow!("{e}")))?;
    println!("{}", postinstall_summary(&health));
    println!("Cache: {}", default_cache_path().display());
    Ok(())
}

pub async fn run_uninstall() -> Result<()> {
    uninstall_all()
        .await
        .map_err(|e| PgForgeError::Anyhow(anyhow::anyhow!("{e}")))?;
    println!("Removed sudoers fragment, systemd-user timer/service, and cache.");
    Ok(())
}

pub async fn run_check(write_cache_flag: bool) -> Result<()> {
    use std::io::IsTerminal;
    let installed = read_installed(&default_installed_path());
    // TTY + no --write-cache → user is at the keyboard, interactive sudo OK.
    let sudo_mode = if !write_cache_flag && std::io::stdout().is_terminal() {
        SudoMode::Interactive
    } else {
        SudoMode::NonInteractive
    };
    let health = check_all(installed.as_ref(), sudo_mode).await;
    println!("SMART check ({}):", health.checked_at);
    println!("{}", postinstall_summary(&health));
    if write_cache_flag {
        write_cache(&default_cache_path(), &health)
            .map_err(|e| PgForgeError::Anyhow(anyhow::anyhow!("{e}")))?;
    }
    Ok(())
}

pub async fn run_status() -> Result<()> {
    use crate::smart::types::SmartUnknownReason;
    let path = default_cache_path();
    let now = jiff::Timestamp::now();
    let health = read_cache(&path, now, STALE_AFTER_HOURS);
    println!("SMART status (cache: {})", path.display());
    // Special-case NoCache so we don't print "Last checked: <now> (0h 0m
    // ago)" when there's no cache at all (Unknown(NoCache) carries
    // checked_at=Timestamp::now() to keep the field non-Optional).
    if health.unknown_reason == Some(SmartUnknownReason::NoCache) {
        println!("  No cache file. Run `pgforge smart install` first, or");
        println!("  `pgforge smart check` for an ad-hoc check.");
        return Ok(());
    }
    let age = now
        .since(health.checked_at)
        .ok()
        .map(|s| {
            // Use integer truncation so "5h 34m" is exactly 5h 34m and never
            // "6h 34m" via float rounding of total_hours.
            let total_secs = s.total(jiff::Unit::Second).unwrap_or(0.0) as i64;
            let hours = total_secs / 3600;
            let minutes = (total_secs % 3600) / 60;
            format!("{hours}h {minutes}m ago")
        })
        .unwrap_or_else(|| "unknown".into());
    println!("  Last checked: {} ({})", health.checked_at, age);
    if health.status == SmartStatus::Unknown {
        println!(
            "  Status: SMART ? ({})",
            health
                .unknown_reason
                .map(|r| r.to_string())
                .unwrap_or_else(|| "no devices".into())
        );
    } else {
        println!("{}", postinstall_summary(&health));
    }
    Ok(())
}
