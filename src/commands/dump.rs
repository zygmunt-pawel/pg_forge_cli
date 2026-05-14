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

/// True iff `head` (the first bytes of a file) is the start of a pg_dump
/// custom-format archive. A clean `pg_dump` exit with a 0-byte or truncated
/// file is still a failed dump; the `PGDMP` magic is the cheapest reliable
/// "this is a real dump" gate before we rename `.partial` into place.
pub fn is_valid_custom_dump(head: &[u8]) -> bool {
    head.starts_with(b"PGDMP")
}

/// Parse the "Available" column (1K blocks) from `df -P -k <dir>` output.
/// POSIX (`-P`) format guarantees one data line, columns:
/// Filesystem, 1024-blocks, Used, Available, Capacity, Mounted-on.
pub fn parse_df_available_kb(df_output: &str) -> Option<u64> {
    let data_line = df_output.lines().nth(1)?;
    data_line.split_whitespace().nth(3)?.parse::<u64>().ok()
}

/// Minimum free space (KiB) required before starting a dump. 5 GiB — the
/// dump dir shares the disk with live PG data volumes.
pub const MIN_FREE_KB: u64 = 5 * 1024 * 1024;
