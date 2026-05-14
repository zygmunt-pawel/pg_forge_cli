//! `pgforge dump --name <instance>` — stream a `pg_dump -Fc` of a live
//! instance to a portable, crash-safe, 0600 `.dump` file on the server.

use crate::docker::bollard_engine::BollardEngine;
use crate::docker::engine::DockerEngine;
use crate::domain::instance::Instance;
use crate::error::{PgForgeError, Result};
use crate::state::instance::InstanceState;
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

/// Given an instance's dump filenames and a keep-count `n`, return the
/// filenames to delete — everything except the newest `n`. Default dump
/// filenames embed a fixed-width timestamp, so lexicographic sort is
/// chronological. `n == 0` is treated as "keep all" (no pruning).
pub fn dumps_to_prune(files: &mut [String], n: usize) -> Vec<String> {
    if n == 0 || files.len() <= n {
        return Vec::new();
    }
    files.sort();
    let cutoff = files.len() - n;
    files[..cutoff].to_vec()
}

#[derive(Debug, Clone)]
pub struct DumpArgs {
    pub name: String,
    /// Full file path. `None` → default under `$HOME/pgforge-dumps/`.
    pub out: Option<PathBuf>,
    /// Overwrite the destination if a file already exists there.
    pub force: bool,
    /// Keep only the newest N dumps for this instance after a successful run.
    pub keep: Option<usize>,
    /// Hard cap on the dump in seconds.
    pub timeout_secs: u64,
    pub override_state_root: Option<PathBuf>,
}

/// RAII guard: removes the `.partial` file on drop unless `disarm()` was
/// called. Covers panics and every early-return error path with one object.
struct PartialGuard {
    path: PathBuf,
    armed: bool,
}

impl PartialGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PartialGuard {
    fn drop(&mut self) {
        if self.armed && self.path.exists()
            && let Err(e) = std::fs::remove_file(&self.path)
        {
            tracing::warn!(
                target: "pgforge::dump",
                "could not remove partial dump {}: {e}",
                self.path.display()
            );
        }
    }
}

/// Default dump directory: `$HOME/pgforge-dumps/`. `~` is not shell-expanded
/// by Rust — resolve `$HOME` explicitly.
fn default_dump_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| PgForgeError::Anyhow(anyhow::anyhow!("HOME not set")))?;
    Ok(PathBuf::from(home).join("pgforge-dumps"))
}

pub async fn run(args: DumpArgs) -> Result<PathBuf> {
    let state_root = args
        .override_state_root
        .clone()
        .unwrap_or_else(InstanceState::default_state_root);
    let docker = BollardEngine::connect()?;
    run_with_engine(args, &docker, state_root).await
}

pub async fn run_with_engine<E: DockerEngine>(
    args: DumpArgs,
    docker: &E,
    state_root: PathBuf,
) -> Result<PathBuf> {
    Instance::validate_name(&args.name)?;
    let state = InstanceState::load_under(&state_root, &args.name)?;

    let container = format!("pgforge_{}", state.instance.name);
    if !docker.container_running(&container).await? {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "instance {:?} is not running; start it first \
             (pgforge rotate) — pg_dump needs a live server.",
            args.name
        )));
    }

    // Resolve the dump dir + final path.
    // When `--out` is a bare filename (e.g. `billing.dump`), `parent()` returns
    // `Some("")` — an empty path, NOT `None` — so we must also guard for that.
    let dump_dir = match &args.out {
        Some(out) => match out.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        },
        None => default_dump_dir()?,
    };
    crate::util::fs::create_secret_dir(&dump_dir)?;
    let final_path = resolve_dump_path(
        args.out.clone(),
        &state.instance.name,
        &dump_dir,
        &crate::time::now_iso(),
    );
    if final_path.exists() && !args.force {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "dump already exists at {} — pass --force to overwrite, \
             or --out a different path.",
            final_path.display()
        )));
    }

    // Free-space precheck.
    let df_out = std::process::Command::new("df")
        .args(["-P", "-k"])
        .arg(&dump_dir)
        .output()
        .map_err(|e| PgForgeError::Anyhow(anyhow::anyhow!("df: {e}")))?;
    // If `df` itself fails or exits non-zero, stdout is empty -> parse returns
    // None -> the precheck is skipped (not a hard error): the dump's own write
    // path will still surface ENOSPC.
    if let Some(avail) = parse_df_available_kb(&String::from_utf8_lossy(&df_out.stdout))
        && avail < MIN_FREE_KB
    {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "only {} MiB free on the dump filesystem — refusing to start \
             a dump (need at least {} MiB). Free space or pass --out elsewhere.",
            avail / 1024,
            MIN_FREE_KB / 1024
        )));
    }

    // Sweep stale *.partial orphans (>24h) from prior killed runs.
    sweep_stale_partials(&dump_dir);

    // Per-pid-unique partial path, like util::fs::atomic_write.
    let partial = final_path.with_extension(format!("{}.partial", std::process::id()));
    let mut guard = PartialGuard::new(partial.clone());

    let started = std::time::Instant::now();
    tracing::info!(
        target: "pgforge::dump",
        "dumping {:?} -> {} (reads LIVE production data)",
        args.name,
        final_path.display()
    );

    // Stream pg_dump into the .partial file, bounded by the timeout.
    let cmd = [
        "pg_dump",
        "-Fc",
        "--lock-timeout=5000",
        "-U",
        state.instance.app_user.as_str(),
        "-h",
        "/var/run/postgresql",
        state.instance.db_name.as_str(),
    ];
    let exec = tokio::time::timeout(
        std::time::Duration::from_secs(args.timeout_secs),
        docker.exec_to_file(&container, &cmd, &partial),
    )
    .await
    .map_err(|_| {
        PgForgeError::Docker(format!(
            "pg_dump exceeded {}s; the instance may have a lock or a hung process.",
            args.timeout_secs
        ))
    })??;

    if exec.exit_code == 127 {
        return Err(PgForgeError::Docker(format!(
            "pg_dump not found in the instance image — recreate/rotate {:?}.",
            args.name
        )));
    }
    if exec.exit_code != 0 {
        return Err(PgForgeError::Docker(format!(
            "pg_dump failed (exit {}): {}",
            exec.exit_code,
            exec.stderr.trim()
        )));
    }

    // Verify before commit: non-empty + PGDMP custom-format header.
    let mut head = [0u8; 5];
    let valid = {
        use std::io::Read;
        std::fs::File::open(&partial)
            .and_then(|mut f| f.read(&mut head))
            .map(|n| is_valid_custom_dump(&head[..n]))
            .unwrap_or(false)
    };
    if !valid {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "pg_dump exited 0 but produced a truncated dump (no PGDMP header) — {:?}",
            args.name
        )));
    }

    // Commit: rename .partial -> final, fsync the directory.
    std::fs::rename(&partial, &final_path).map_err(|e| PgForgeError::Io {
        path: final_path.clone(),
        source: e,
    })?;
    crate::util::fs::fsync_dir(&dump_dir)?;
    guard.disarm();

    // Retention.
    if let Some(n) = args.keep {
        apply_retention(&dump_dir, &state.instance.name, n);
    }

    let size = std::fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0);
    tracing::info!(
        target: "pgforge::dump",
        "dump complete: {} ({} bytes, {}s)",
        final_path.display(),
        size,
        started.elapsed().as_secs()
    );
    eprintln!(
        "dump: {} ({:.1} MiB) — contains production data, delete after transfer.",
        final_path.display(),
        size as f64 / (1024.0 * 1024.0)
    );
    let total = dump_dir_total_bytes(&dump_dir);
    eprintln!(
        "dump: {} now holds {:.1} MiB of dumps total.",
        dump_dir.display(),
        total as f64 / (1024.0 * 1024.0)
    );
    Ok(final_path)
}

/// Remove `*.partial` files in `dir` older than 24h — orphans from prior
/// killed/crashed runs (the RAII guard cannot run after SIGKILL).
fn sweep_stale_partials(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let day = std::time::Duration::from_secs(24 * 3600);
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("partial") {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().map(|e| e > day).unwrap_or(false))
            .unwrap_or(false);
        if stale {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Best-effort sum of `*.dump` file sizes in `dir`, in bytes. Returns 0 on
/// any read error — this is only a cheap "you have N MiB of dumps" nudge.
fn dump_dir_total_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .filter(|e| {
            e.path().extension().and_then(|x| x.to_str()) == Some("dump")
        })
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// Apply `--keep N` retention for `instance` in `dir`.
fn apply_retention(dir: &Path, instance: &str, n: usize) {
    let prefix = format!("{instance}-");
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut dumps: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.starts_with(&prefix) && name.ends_with(".dump"))
        .collect();
    for name in dumps_to_prune(&mut dumps, n) {
        if let Err(e) = std::fs::remove_file(dir.join(&name)) {
            tracing::warn!(target: "pgforge::dump", "retention: could not remove {name}: {e}");
        }
    }
}
