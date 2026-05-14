//! `pgforge status <name>` — one-shot snapshot of a managed instance's
//! runtime metrics: docker resource usage, postgres connection counts,
//! database size, on-disk PGDATA size.
//!
//! Backend used by the Plan 5 TUI for its per-instance detail pane. The
//! CLI prints a simple human-readable summary; the TUI builds its own view
//! from the `InstanceStatus` struct.

use crate::docker::bollard_engine::BollardEngine;
use crate::docker::engine::DockerEngine;
use crate::error::{PgForgeError, Result};
use crate::state::instance::InstanceState;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct StatusArgs {
    pub name: String,
    pub override_state_root: Option<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct InstanceStatus {
    pub name: String,
    pub running: bool,
    pub host_port: u16,
    /// Container CPU usage % (one-shot sample via `docker stats`). None
    /// when the container isn't running.
    pub cpu_percent: Option<f64>,
    pub mem_used_mb: Option<u64>,
    pub mem_limit_mb: Option<u64>,
    /// Connections by state — only populated when postgres is reachable.
    pub connections_active: Option<u64>,
    pub connections_idle: Option<u64>,
    pub connections_total: Option<u64>,
    pub db_size_bytes: Option<u64>,
    /// On-disk size of $PGDATA inside the container (via `du -sb`).
    pub pgdata_bytes: Option<u64>,
    /// Seconds since the *current* container run started. None when the
    /// container isn't running or `docker inspect` couldn't parse
    /// State.StartedAt. Use `humanize_uptime` for display.
    pub uptime_seconds: Option<u64>,
    /// How many times the Docker restart policy has had to relaunch
    /// this container since first create. >0 = it crashed at least
    /// once and was recovered automatically — a soft warning signal.
    pub restart_count: Option<u32>,
    /// True iff postgres responded to a SELECT within this status
    /// refresh (i.e. `connections_total` was populated). When the
    /// container is running but this is `Some(false)`, postgres is
    /// not yet reachable (still booting, or hung).
    pub db_responsive: Option<bool>,
    /// Whether this instance has pgbackrest backups configured.
    pub backup_enabled: bool,
    /// True when the last snapshot attempt is newer than the last success —
    /// backups are broken and need operator attention.
    pub backup_failing: bool,
    /// RFC3339 timestamp of the last *successful* snapshot, if any.
    pub last_snapshot_at: Option<String>,
}

pub async fn run(args: StatusArgs) -> Result<InstanceStatus> {
    let state_root = args
        .override_state_root
        .clone()
        .unwrap_or_else(InstanceState::default_state_root);
    let docker = BollardEngine::connect()?;
    run_with_engine(args, &docker, state_root).await
}

pub async fn run_with_engine<E: DockerEngine>(
    args: StatusArgs,
    docker: &E,
    state_root: PathBuf,
) -> Result<InstanceStatus> {
    let state = InstanceState::load_under(&state_root, &args.name)?;
    let container = format!("pgforge_{}", state.instance.name);
    let mut out = InstanceStatus {
        name: state.instance.name.clone(),
        running: false,
        host_port: state.instance.host_port,
        // Backup health is independent of container run state, so populate it
        // before the early `!running` return below.
        backup_enabled: state.instance.backup_enabled,
        backup_failing: state.instance.backup_failing(),
        last_snapshot_at: state.instance.last_snapshot_at.clone(),
        ..Default::default()
    };
    let inspect = docker.inspect_container(&container).await?;
    out.running = inspect.running;
    out.restart_count = Some(inspect.restart_count);
    if out.running {
        if let Some(ref ts) = inspect.started_at {
            out.uptime_seconds = parse_rfc3339_uptime_secs(ts);
        }
    }
    if !out.running {
        return Ok(out);
    }

    // 1. Docker stats — one-shot via the docker CLI. bollard exposes a
    // streaming endpoint but a single sample is enough for a one-shot
    // status command (the TUI will poll on tick instead).
    if let Some((cpu, used_mb, limit_mb)) = read_docker_stats(&container) {
        out.cpu_percent = Some(cpu);
        out.mem_used_mb = Some(used_mb);
        out.mem_limit_mb = Some(limit_mb);
    }

    // 2. Connection breakdown — connect as the app user (only role on
    // --no-backup instances; backup-enabled instances also have it).
    let pgsql_count = docker
        .exec(
            &container,
            &[
                "su", "-", "postgres", "-c",
                &format!(
                    "psql -tA -U {user} -d {db} -c \"SELECT \
                     coalesce(sum((state = 'active')::int), 0), \
                     coalesce(sum((state = 'idle')::int), 0), \
                     count(*) \
                     FROM pg_stat_activity WHERE datname IS NOT NULL;\"",
                    user = state.instance.app_user,
                    db = state.instance.db_name,
                ),
            ],
        )
        .await?;
    if pgsql_count.exit_code == 0 {
        // psql -tA output: "active|idle|total" (default field separator '|')
        let line = pgsql_count.stdout.lines().next().unwrap_or("").trim();
        let fields: Vec<&str> = line.split('|').collect();
        if fields.len() == 3 {
            out.connections_active = fields[0].parse().ok();
            out.connections_idle = fields[1].parse().ok();
            out.connections_total = fields[2].parse().ok();
        }
    }
    // db_responsive is the "did the SELECT come back?" signal — container
    // running != postgres ready (post-restart it can take seconds to
    // accept connections; if pg crashed inside the container it can
    // stay down indefinitely).
    out.db_responsive = Some(out.connections_total.is_some());

    // 3. Database size.
    let pgsql_size = docker
        .exec(
            &container,
            &[
                "su", "-", "postgres", "-c",
                &format!(
                    "psql -tA -U {user} -d {db} -c \"SELECT pg_database_size('{db}');\"",
                    user = state.instance.app_user,
                    db = state.instance.db_name,
                ),
            ],
        )
        .await?;
    if pgsql_size.exit_code == 0 {
        out.db_size_bytes = pgsql_size.stdout.lines().next().unwrap_or("").trim().parse().ok();
    }

    // 4. On-disk PGDATA size — useful when the DB has lots of WAL/temp,
    // because pg_database_size only counts the database itself.
    let du = docker
        .exec(
            &container,
            &["du", "-sb", "/var/lib/postgresql/data/pgdata"],
        )
        .await?;
    if du.exit_code == 0 {
        if let Some(first_field) = du.stdout.split_whitespace().next() {
            out.pgdata_bytes = first_field.parse().ok();
        }
    }

    Ok(out)
}

fn read_docker_stats(container: &str) -> Option<(f64, u64, u64)> {
    let out = std::process::Command::new("docker")
        .args([
            "stats",
            "--no-stream",
            "--format",
            "{{.CPUPerc}} {{.MemUsage}}",
            container,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let line = line.lines().next()?.trim();
    // Format: "12.34% 256MiB / 1GiB"
    let mut parts = line.split_whitespace();
    let cpu = parts.next()?.trim_end_matches('%').parse::<f64>().ok()?;
    let used = parts.next()?;
    let _slash = parts.next();
    let limit = parts.next()?;
    Some((cpu, parse_size_to_mb(used)?, parse_size_to_mb(limit)?))
}

/// Parse strings like "256MiB", "1.5GiB", "512KiB", "2GB" into a megabyte count.
fn parse_size_to_mb(s: &str) -> Option<u64> {
    let (num, unit) = split_size_unit(s)?;
    let n: f64 = num.parse().ok()?;
    let mb = match unit {
        "B" | "" => n / (1024.0 * 1024.0),
        "KiB" | "KB" | "kB" | "kiB" => n / 1024.0,
        "MiB" | "MB" => n,
        "GiB" | "GB" => n * 1024.0,
        "TiB" | "TB" => n * 1024.0 * 1024.0,
        _ => return None,
    };
    Some(mb as u64)
}

fn split_size_unit(s: &str) -> Option<(&str, &str)> {
    let idx = s.find(|c: char| c.is_alphabetic())?;
    Some((&s[..idx], &s[idx..]))
}

pub fn render(status: &InstanceStatus) -> String {
    let mut s = format!("Instance: {}\n", status.name);
    s.push_str(&format!("Port: {}\n", status.host_port));
    s.push_str(&format!(
        "State: {}\n",
        if status.running { "running" } else { "stopped" }
    ));
    // Backup health — independent of container run state, shown for both
    // running and stopped instances so a silent backup outage is visible.
    if status.backup_enabled {
        if status.backup_failing {
            s.push_str("Backups: ✗ FAILING — last attempt is newer than last success\n");
        } else {
            s.push_str("Backups: ✓ ok\n");
        }
        match &status.last_snapshot_at {
            Some(ts) => s.push_str(&format!("Last snapshot: {ts}\n")),
            None => s.push_str("Last snapshot: never\n"),
        }
    } else {
        s.push_str("Backups: disabled (--no-backup)\n");
    }
    if !status.running {
        return s;
    }
    if let (Some(cpu), Some(used), Some(limit)) = (status.cpu_percent, status.mem_used_mb, status.mem_limit_mb) {
        s.push_str(&format!("CPU:    {cpu:.2}%\n"));
        s.push_str(&format!("Memory: {used}MB / {limit}MB\n"));
    }
    if let (Some(active), Some(idle), Some(total)) = (
        status.connections_active,
        status.connections_idle,
        status.connections_total,
    ) {
        s.push_str(&format!("Conns:  {active} active, {idle} idle, {total} total\n"));
    }
    if let Some(db) = status.db_size_bytes {
        s.push_str(&format!("DB:     {} ({db} B)\n", human_bytes(db)));
    }
    if let Some(pg) = status.pgdata_bytes {
        s.push_str(&format!("PGDATA: {} ({pg} B)\n", human_bytes(pg)));
    }
    if let Some(up) = status.uptime_seconds {
        s.push_str(&format!("Uptime: {}\n", humanize_uptime(up)));
    }
    if let Some(rc) = status.restart_count {
        if rc > 0 {
            s.push_str(&format!("Restarts: {rc} (container crashed and was auto-restarted)\n"));
        }
    }
    if let Some(resp) = status.db_responsive {
        s.push_str(&format!(
            "DB:     {}\n",
            if resp { "responsive ✓" } else { "container up, postgres not responding ✗" }
        ));
    }
    s
}

/// Parse Docker's RFC3339 `started_at` into seconds-since-now using a
/// small bespoke parser. We avoid pulling chrono — jiff is already a
/// dependency and handles fractional seconds + `Z` / `+00:00` offsets.
pub(crate) fn parse_rfc3339_uptime_secs(s: &str) -> Option<u64> {
    use std::str::FromStr;
    let started = <jiff::Timestamp as FromStr>::from_str(s).ok()?;
    let now = jiff::Timestamp::now();
    let secs = started.duration_until(now).as_secs();
    if secs < 0 { Some(0) } else { Some(secs as u64) }
}

/// "5s", "2m", "1h 14m", "3d 4h", "12d". Tweaked for at-a-glance
/// reading in the TUI detail pane — never shows more than two units.
pub fn humanize_uptime(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if d > 0 {
        if h > 0 { format!("{d}d {h}h") } else { format!("{d}d") }
    } else if h > 0 {
        if m > 0 { format!("{h}h {m}m") } else { format!("{h}h") }
    } else if m > 0 {
        if s > 0 { format!("{m}m {s}s") } else { format!("{m}m") }
    } else {
        format!("{s}s")
    }
}

fn human_bytes(b: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut n = b as f64;
    let mut u = 0;
    while n >= 1024.0 && u < units.len() - 1 {
        n /= 1024.0;
        u += 1;
    }
    format!("{n:.1}{}", units[u])
}

#[allow(dead_code)]
fn _unused_err(s: String) -> PgForgeError {
    PgForgeError::Docker(s)
}
