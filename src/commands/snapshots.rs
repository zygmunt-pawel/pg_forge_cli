use crate::docker::engine::DockerEngine;
use crate::error::{PgForgeError, Result};
use crate::state::instance::InstanceState;
use crate::state::snapshots::SnapshotsFile;
use std::path::PathBuf;

/// Coarse PITR window derived from `pgbackrest info`. start/stop are
/// RFC3339-ish strings emitted by pgbackrest (epoch seconds converted).
/// `None` for either side if pgbackrest hasn't produced a backup yet
/// (fresh instance, no `pgforge snapshot` ever ran).
#[derive(Debug, Clone, Default)]
pub struct PitrWindow {
    /// Start of the earliest full backup — earliest target_time you can
    /// PITR to.
    pub earliest: Option<String>,
    /// Stop of the latest backup / latest archived WAL — latest
    /// reliably-replayable target_time.
    pub latest: Option<String>,
}

pub fn run(
    instance: &str,
    override_state_root: Option<PathBuf>,
) -> Result<Vec<crate::domain::snapshot::SnapshotRecord>> {
    let state_root = override_state_root
        .clone()
        .unwrap_or_else(InstanceState::default_state_root);
    // Ensures instance exists; errors if not
    let _ = InstanceState::load_under(&state_root, instance)?;
    let file = SnapshotsFile::load_for(&state_root, instance)?;
    Ok(file.snapshots)
}

/// Query `pgbackrest info` inside the instance's container to derive the
/// effective PITR window. Skips and returns an empty window for instances
/// without backups (`backup_enabled = false`) so the caller doesn't need
/// a separate code path for the no-backup case.
pub async fn pitr_window<E: DockerEngine>(
    instance: &str,
    docker: &E,
    state_root: &std::path::Path,
) -> Result<PitrWindow> {
    let state = InstanceState::load_under(state_root, instance)?;
    if !state.instance.backup_enabled {
        return Ok(PitrWindow::default());
    }
    let container = format!("pgforge_{}", instance);
    if !docker.container_running(&container).await? {
        // Can't run pgbackrest info if the container is down.
        return Ok(PitrWindow::default());
    }
    let out = docker
        .exec(
            &container,
            &[
                "su", "-", "postgres", "-c",
                "pgbackrest --stanza=main --output=json info",
            ],
        )
        .await?;
    if out.exit_code != 0 {
        return Err(PgForgeError::Docker(format!(
            "pgbackrest info failed (exit {}): stderr={:?}",
            out.exit_code, out.stderr
        )));
    }
    Ok(parse_pitr_window(&out.stdout))
}

/// Parse the JSON emitted by `pgbackrest --output=json info` and project it
/// down to `(earliest, latest)` ISO-8601 strings. The expected shape:
///
/// ```json
/// [
///   {
///     "name": "main",
///     "backup": [
///       { "timestamp": { "start": 1715000000, "stop": 1715000300 } },
///       …
///     ]
///   }
/// ]
/// ```
///
/// `earliest` is the smallest `start` across all backups; `latest` is the
/// largest `stop`. Both are formatted as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn parse_pitr_window(json: &str) -> PitrWindow {
    let v: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return PitrWindow::default(),
    };
    let stanzas = match v.as_array() {
        Some(a) => a,
        None => return PitrWindow::default(),
    };
    let mut earliest: Option<i64> = None;
    let mut latest: Option<i64> = None;
    for stanza in stanzas {
        let Some(backups) = stanza.get("backup").and_then(|b| b.as_array()) else { continue };
        for b in backups {
            if let Some(start) = b.pointer("/timestamp/start").and_then(|n| n.as_i64()) {
                earliest = Some(earliest.map_or(start, |cur| cur.min(start)));
            }
            if let Some(stop) = b.pointer("/timestamp/stop").and_then(|n| n.as_i64()) {
                latest = Some(latest.map_or(stop, |cur| cur.max(stop)));
            }
        }
    }
    PitrWindow {
        earliest: earliest.map(epoch_to_iso),
        latest: latest.map(epoch_to_iso),
    }
}

fn epoch_to_iso(epoch_seconds: i64) -> String {
    // chrono is not a dependency yet; format manually. Days/months precision
    // is enough for showing a PITR window — users will copy this back into
    // `pgforge restore --target-time` which itself accepts RFC3339.
    let secs = epoch_seconds;
    // Compute Y/M/D/h/m/s from epoch using a standard civil-from-days routine.
    let days = secs.div_euclid(86_400);
    let time_of_day = secs.rem_euclid(86_400);
    let hh = time_of_day / 3600;
    let mm = (time_of_day % 3600) / 60;
    let ss = time_of_day % 60;
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Howard Hinnant's days-to-Y/M/D civil_from_days, public-domain pseudocode
/// translated to Rust. Avoids pulling in chrono just for one formatter.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}
