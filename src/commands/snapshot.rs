use crate::docker::bollard_engine::BollardEngine;
use crate::docker::engine::DockerEngine;
use crate::domain::snapshot::{SnapshotKind, SnapshotRecord};
use crate::error::{PgForgeError, Result};
use crate::pgbackrest::parse::parse_backup_label;
use crate::state::instance::InstanceState;
use crate::state::snapshots::SnapshotsFile;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct SnapshotArgs {
    pub instance: String,
    pub user_label: Option<String>,
    pub override_state_root: Option<PathBuf>,
}

/// Iterate every managed instance, snapshot the ones whose scheduled
/// hour is past for today AND that haven't been snapshotted yet today.
/// Designed to be called every few minutes by the launchd agent
/// installed via `pgforge schedule install`. Returns the count of
/// instances that were actually snapshotted (0 if nothing was due).
pub async fn run_due(override_state_root: Option<PathBuf>) -> Result<usize> {
    let state_root = override_state_root
        .unwrap_or_else(crate::state::instance::InstanceState::default_state_root);
    let names = crate::state::instance::InstanceState::list_under(&state_root)?;
    let mut count = 0usize;
    for name in names {
        let Ok(state) = crate::state::instance::InstanceState::load_under(&state_root, &name)
        else {
            continue;
        };
        if !state.instance.backup_enabled {
            continue;
        }
        let Some(hour) = state.instance.snapshot_hour else {
            continue;
        };
        if !is_snapshot_due(
            hour,
            state.instance.last_snapshot_at.as_deref(),
            state.instance.last_snapshot_attempt_at.as_deref(),
        ) {
            continue;
        }
        tracing::info!(target: "pgforge::snapshot::due",
            "running due snapshot for {name} (hour={hour})");
        if let Err(e) = run(SnapshotArgs {
            instance: name.clone(),
            user_label: Some("auto-scheduled".into()),
            override_state_root: Some(state_root.clone()),
        }).await {
            tracing::error!(target: "pgforge::snapshot::due",
                "due snapshot for {name} failed: {e}");
            if let Ok(_lock) = crate::util::fs::LockedStateRoot::acquire(&state_root)
                && let Ok(mut s) = crate::state::instance::InstanceState::load_under(&state_root, &name) {
                s.instance.last_snapshot_attempt_at = Some(crate::time::now_iso());
                let _ = s.save_under(&state_root);
            }
            continue;
        }
        count += 1;
    }
    Ok(count)
}

/// True iff:
///   1. the hour has already passed today in local time, AND
///   2. last_snapshot_at is None OR it's older than today's window-open time, AND
///   3. last_snapshot_attempt_at is None OR it's older than 1 hour (backoff after failures).
pub fn is_snapshot_due(
    hour: u8,
    last_ok: Option<&str>,
    last_attempt: Option<&str>,
) -> bool {
    use std::str::FromStr;
    let zoned_now = jiff::Zoned::now();
    let today = zoned_now.date();
    let now_secs = zoned_now.hour() as i64 * 3600
        + zoned_now.minute() as i64 * 60
        + zoned_now.second() as i64;
    let hour_secs = (hour as i64) * 3600;
    if now_secs < hour_secs {
        return false; // window hasn't opened yet today
    }
    if let Some(last) = last_ok {
        if let Ok(ts) = jiff::Timestamp::from_str(last) {
            if ts.to_zoned(zoned_now.time_zone().clone()).date() == today {
                return false; // already covered today
            }
        } else {
            tracing::warn!(target: "pgforge::snapshot::due",
                "unparseable last_snapshot_at {last:?}; skipping");
            return false;
        }
    }
    if let Some(att) = last_attempt
        && let Ok(ts) = jiff::Timestamp::from_str(att) {
        let age = (jiff::Timestamp::now().as_second() - ts.as_second()).max(0);
        if age < 3600 {
            tracing::info!(target: "pgforge::snapshot::due",
                "recent failed attempt {att:?} ({age}s ago); backing off");
            return false;
        }
    }
    true
}

pub async fn run(args: SnapshotArgs) -> Result<SnapshotRecord> {
    let state_root = args
        .override_state_root
        .clone()
        .unwrap_or_else(InstanceState::default_state_root);
    let s = InstanceState::load_under(&state_root, &args.instance)?;
    if !s.instance.backup_enabled {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "instance {:?} was created with --no-backup; pgbackrest is not \
             configured and `pgforge snapshot` cannot operate on it.",
            args.instance
        )));
    }
    let docker = BollardEngine::connect()?;
    run_with_engine(args, &docker, state_root).await
}

/// Belt-and-braces scan of pgbackrest output. Even when exit code is 0,
/// surface output that contains the canonical pgbackrest error markers.
pub fn pgbackrest_indicates_failure(s: &str) -> bool {
    s.lines().any(|l| {
        let l = l.trim_start();
        l.starts_with("ERROR:") || l.starts_with("ABORTED:")
    })
}

/// Remove lines that may carry secret material (S3 keys, passwords).
pub fn redact_pgbackrest_output(s: &str) -> String {
    s.lines()
        .filter(|l| {
            let lc = l.to_ascii_lowercase();
            !lc.contains("repo1-s3-key")
                && !lc.contains("repo1-cipher-pass")
                && !lc.contains("password")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub async fn run_with_engine<E: DockerEngine>(
    args: SnapshotArgs,
    docker: &E,
    state_root: PathBuf,
) -> Result<SnapshotRecord> {
    let container = format!("pgforge_{}", args.instance);
    if !docker.container_running(&container).await? {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "container {container:?} is not running. Start it with `docker start {container}` and retry."
        )));
    }

    let s = InstanceState::load_under(&state_root, &args.instance)?;

    let weekday_idx: u8 = match jiff::Zoned::now().weekday() {
        jiff::civil::Weekday::Sunday => 0,
        jiff::civil::Weekday::Monday => 1,
        jiff::civil::Weekday::Tuesday => 2,
        jiff::civil::Weekday::Wednesday => 3,
        jiff::civil::Weekday::Thursday => 4,
        jiff::civil::Weekday::Friday => 5,
        jiff::civil::Weekday::Saturday => 6,
    };
    let has_prior_full = SnapshotsFile::load_for(&state_root, &args.instance)
        .map(|f| f.snapshots.iter().any(|sn| matches!(sn.kind, SnapshotKind::Full)))
        .unwrap_or(false); // missing snapshots.toml → no prior → treat as first-ever → full
    let kind = if !has_prior_full || weekday_idx == s.instance.full_backup_day {
        SnapshotKind::Full
    } else {
        SnapshotKind::Diff
    };
    let type_flag = match kind {
        SnapshotKind::Full => "--type=full",
        SnapshotKind::Diff => "--type=diff",
    };

    let out = docker
        .exec_as(
            &container,
            "postgres",
            &["pgbackrest", "--stanza=main", type_flag, "backup"],
        )
        .await?;
    if out.exit_code != 0
        || pgbackrest_indicates_failure(&out.stderr)
        || pgbackrest_indicates_failure(&out.stdout)
    {
        return Err(PgForgeError::Docker(format!(
            "pgbackrest backup failed (exit {}): {}",
            out.exit_code,
            redact_pgbackrest_output(&out.stderr)
        )));
    }
    let label = parse_backup_label(&out.stdout).ok_or_else(|| {
        PgForgeError::Anyhow(anyhow::anyhow!(
            "pgbackrest backup succeeded but no label found in stdout — output: {}",
            redact_pgbackrest_output(&out.stdout)
        ))
    })?;

    let _lock = crate::util::fs::LockedStateRoot::acquire(&state_root)?;
    let mut file = SnapshotsFile::load_for(&state_root, &args.instance)?;
    let record = SnapshotRecord {
        label: label.clone(),
        kind,
        user_label: args.user_label,
        taken_at: crate::time::now_iso(),
    };
    file.snapshots.push(record.clone());
    file.save_for(&state_root, &args.instance)?;

    // Re-load InstanceState INSIDE the lock so we don't clobber concurrent
    // edits (e.g. user changing snapshot_hour via TUI [t] while we run).
    let mut state =
        crate::state::instance::InstanceState::load_under(&state_root, &args.instance)?;
    state.instance.last_snapshot_at = Some(record.taken_at.clone());
    state.instance.last_snapshot_attempt_at = Some(record.taken_at.clone());
    state.save_under(&state_root)?;
    drop(_lock);

    Ok(record)
}
