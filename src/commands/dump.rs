//! `pgforge dump --name <instance>` — stream a `pg_dump -Fc` of a live
//! instance to a portable, crash-safe, 0600 `.dump` file on the server.

use std::path::{Path, PathBuf};

/// Decide the final dump file path. `out` (if given) is used verbatim — it
/// is always a full file path. Otherwise the file lands in `dump_dir` as
/// `<instance>-<YYYYMMDD-HHMMSS>.dump`, where the timestamp is derived from
/// `now_iso` (a `YYYY-MM-DDTHH:MM:SSZ` string from `crate::time::now_iso`).
pub fn resolve_dump_path(
    out: Option<PathBuf>,
    instance: &str,
    dump_dir: &Path,
    now_iso: &str,
) -> PathBuf {
    if let Some(out) = out {
        return out;
    }
    // "2026-05-14T09:30:00Z" -> "20260514-093000"
    let compact: String = now_iso
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>();
    let stamp = if compact.len() >= 14 {
        format!("{}-{}", &compact[0..8], &compact[8..14])
    } else {
        compact
    };
    dump_dir.join(format!("{instance}-{stamp}.dump"))
}
