# Disk SMART Health Check — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add SMART hardware-health monitoring to pgforge (predictive failure detection — reallocated/pending sectors, NVMe critical_warning, available_spare, percentage_used, OVERALL_HEALTH). A daily systemd-user timer runs `sudo smartctl -H -A -j` across all sata/sas/nvme disks discovered via `lsblk` and writes a JSON snapshot to `~/.local/state/pgforge/disk-smart.json`. The TUI footer reads that cache every 60 s (a new zone left of the existing `Disk N% used`), and `pgforge` CLI reads it pre-dispatch to emit a red stderr banner when status is Critical.

**Architecture:** New `src/smart/` module with one file per responsibility: `types.rs` (`SmartStatus`/`DriveSmart`/`SmartHealth`/`SmartUnknownReason`), `cache.rs` (atomic JSON read/write + stale + clock-skew handling), `installed.rs` (record of what `pgforge smart install` set up), `check.rs` (discover via lsblk → run smartctl → parse → aggregate), `install.rs` (sudoers + systemd-user timer + first-check orchestration). Privilege escalation happens once per day inside the timer, never in the TUI or CLI hot path; sudoers fragment enumerates exact device paths. TUI/CLI just read the cache file. New `pgforge smart {install,check,status,uninstall}` subcommand tree exposes the operator surface. Mirrors the existing `src/disk/` capacity-health module structurally.

**Tech Stack:** Rust 2024, tokio (process + time), serde_json, jiff (Timestamp), tempfile (atomic writes — already in tree), ratatui 0.29 (TUI), `sudo`/`visudo`/`systemctl --user` invoked via `tokio::process::Command`. No new Rust dependencies.

**Spec:** `docs/superpowers/specs/2026-05-17-disk-smart-check-design.md`

---

## Top-of-plan operational rules

- Each commit ends with `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`.
- Run `cargo test` before each commit; never commit with failures.
- Run `cargo clippy --all-targets -- -D warnings` before each commit; never commit with warnings.
- Behaviour-change tasks (T9 install, T10 CLI surface, T11 CLI banner, T13 TUI footer, T14 README) MUST call out the user-visible change in the commit message body so the eventual changelog catches it.
- TDD: write the failing test, RUN IT, see it fail with the expected message, THEN implement, then run, then commit. Skipping "see it fail" is a plan violation — the test must be proven to actually exercise the code.
- The `src/smart/` module subscribes to the same panic-free contract as `src/disk/`: file-level `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]`. Every fallible step degrades to `SmartHealth::unknown(<reason>)`, never panics.
- Long-running E2E tests (T15) are GATED by `PGFORGE_E2E=1` and never auto-run in interactive sessions (per the project's `feedback_long_tasks` convention). Verify compile + gated skip path; user runs the gated body manually on db-server.

---

## File structure (locked before tasks)

| Path | Status | Responsibility |
|---|---|---|
| `Cargo.toml` | unchanged | All deps already present (tempfile, serde_json, jiff, nix, tokio with `process` feature). |
| `src/lib.rs` | modify | Add `pub mod smart;` next to existing `pub mod disk;`. |
| `src/smart/mod.rs` | create | Declare `pub mod {types, cache, installed, check, install};` + `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]`. |
| `src/smart/types.rs` | create | `SmartStatus`, `SmartUnknownReason`, `DriveSmart`, `SmartHealth` + aggregate + is_stale helpers. |
| `src/smart/installed.rs` | create | `InstalledState` record persisted at `~/.local/state/pgforge/smart-installed.json`; `default_installed_path`, `read_installed`, `write_installed`. |
| `src/smart/cache.rs` | create | `default_cache_path`, `read_cache`, `write_cache`, `STALE_AFTER_HOURS` constant. Atomic writes via `tempfile::NamedTempFile::new_in(parent)` + `persist()`. |
| `src/smart/check.rs` | create | `discover_disks` (lsblk wrap), `run_smartctl` (sudo wrap, `Interactive`/`NonInteractive`), `parse_smartctl_json` dispatching on `device.protocol`, `parse_sata`, `parse_nvme`, `check_all` orchestration. Threshold constants `SATA_TEMP_WARN_C=60`, `NVME_TEMP_WARN_C=70`, `NVME_WEAR_WARN_PCT=80`. |
| `src/smart/install.rs` | create | `install_all`, `uninstall_all`, `render_sudoers_fragment(user, smartctl_path, devices) -> Result<String, InstallError>`, `render_timer_unit() -> String`, `render_service_unit(pgforge_path) -> String`, `postinstall_summary(&SmartHealth) -> String`, `InstallError` enum. |
| `src/commands/smart.rs` | create | Dispatcher: `pgforge smart {install --force, check --write-cache (hidden), status, uninstall}`. Argument parsing + human-readable output. |
| `src/cli.rs` | modify | Add `Command::Smart { #[command(subcommand)] action: SmartAction }` (SmartAction enum); extend `should_emit_banner_for_command` skip-list with `| Command::Smart { .. }`; extend `dispatch` with pre-dispatch SMART-banner block BEFORE existing capacity banner. New `format_smart_banner_line(&SmartHealth) -> Option<String>`. |
| `src/tui/events.rs` | modify | Add `Event::SmartRefreshed(SmartHealth)`. |
| `src/tui/app.rs` | modify | Add `smart_health: Option<SmartHealth>` field; `apply_event` arm for `SmartRefreshed`. |
| `src/tui/refresh.rs` | modify | New `spawn_smart_reader(tx, cache_path) -> JoinHandle<()>` with 60s tick + eager first read; wire into `spawn_pollers`. |
| `src/tui/ui/bottom.rs` | modify | Extend `Layout::horizontal` from 3 zones to 4: `[content, smart, disk, version]`. New `format_smart_zone(Option<&SmartHealth>) -> SmartZone` mirroring `format_disk_zone`. |
| `tests/smart_types_test.rs` | create | Threshold rank, aggregate (empty/single/mix/Critical-dominates), is_stale (boundary + clock-skew). |
| `tests/smart_cache_test.rs` | create | Round-trip; missing file → `Unknown(NoCache)`; corrupt → `Unknown(ParseError)`; 47h59m fresh; 48h boundary → Stale; future timestamp → Stale. |
| `tests/smart_installed_test.rs` | create | `InstalledState` round-trip; missing file → None. |
| `tests/smart_parsing_test.rs` | create | Load every fixture in `tests/fixtures/smart/`, assert `DriveSmart`. |
| `tests/smart_install_test.rs` | create | `render_sudoers_fragment` (happy + empty-devices `Err`); `render_timer_unit()` and `render_service_unit()` snapshots; no actual sudo / systemctl. |
| `tests/cli_smart_banner_test.rs` | create | `format_smart_banner_line` returns `Some` only for Critical; format contains device + reasons. |
| `tests/tui_smart_zone_test.rs` | create | `format_smart_zone(None)` → ` SMART ? ` dim; `Some(Ok)` → dim; `Warn` → yellow; `Critical` → red; `Unknown` → dim. |
| `tests/smart_e2e_test.rs` | create | Gated by `PGFORGE_E2E=1`. End-to-end install → check --write-cache → uninstall on a real Linux host. |
| `tests/fixtures/smart/` | create | JSON fixtures (sanitized real `smartctl -H -A -j` output): see T6 step 1 for full file list. |
| `README.md` | modify | Replace the existing `### Disk pressure` subsection (inside `## How it works`) — delete it; add new top-level `## Disk health` section (between `## TUI mode` and `## How it works`) covering both capacity recap and SMART. |

---

## Task 0: Branch + module skeleton

**Files:**
- Modify: `src/lib.rs`
- Create: `src/smart/mod.rs`

- [ ] **Step 1: Verify branch**

Run: `git rev-parse --abbrev-ref HEAD`
Expected: `feat/disk-smart` (already created during brainstorm). If not, `git checkout -b feat/disk-smart`.

- [ ] **Step 2: Create the module skeleton**

Create `src/smart/mod.rs`:

```rust
//! Host SMART hardware-health monitoring. Sibling to `src/disk/` (capacity).
//! Cache-and-read architecture: a daily systemd-user timer writes
//! `~/.local/state/pgforge/disk-smart.json`; TUI/CLI read that cache.
//!
//! This subsystem MUST NEVER panic — every fallible step degrades to
//! `SmartHealth::unknown(<reason>)`. Enforced by the file-level deny lints
//! below (matching `src/disk/mod.rs`).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

pub mod cache;
pub mod check;
pub mod install;
pub mod installed;
pub mod types;
```

- [ ] **Step 3: Wire into the crate**

In `src/lib.rs`, find the existing `pub mod disk;` line and add directly below it:

```rust
pub mod smart;
```

- [ ] **Step 4: Create stub files so `cargo check` passes**

Create `src/smart/types.rs`:

```rust
//! Stub — filled in by Task 1.
```

Create `src/smart/cache.rs`:

```rust
//! Stub — filled in by Task 3.
```

Create `src/smart/installed.rs`:

```rust
//! Stub — filled in by Task 2.
```

Create `src/smart/check.rs`:

```rust
//! Stub — filled in by Tasks 4–7.
```

Create `src/smart/install.rs`:

```rust
//! Stub — filled in by Tasks 8–9.
```

- [ ] **Step 5: Build to confirm the skeleton compiles**

Run: `cargo build`
Expected: success; new `smart` module declared.

- [ ] **Step 6: Commit**

```bash
git add src/lib.rs src/smart/
git commit -m "$(cat <<'EOF'
chore(smart): scaffold src/smart module

Empty submodule files for the upcoming SMART disk-health feature. Mirrors
the src/disk/ layout. Each file gets its real contents in subsequent
tasks; this commit just declares the module so the rest of the plan can
land one task per file without re-touching lib.rs.

Spec: docs/superpowers/specs/2026-05-17-disk-smart-check-design.md

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 1: SmartStatus, SmartUnknownReason, DriveSmart, SmartHealth (types + aggregate + is_stale)

**Files:**
- Modify: `src/smart/types.rs`
- Create: `tests/smart_types_test.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/smart_types_test.rs`:

```rust
use jiff::{Span, Timestamp};
use pgforge::smart::types::{
    DriveSmart, SmartHealth, SmartStatus, SmartUnknownReason,
};

fn drive(status: SmartStatus, device: &str, reasons: &[&str]) -> DriveSmart {
    DriveSmart {
        device: device.into(),
        model: "X".into(),
        transport: "nvme".into(),
        status,
        reasons: reasons.iter().map(|s| s.to_string()).collect(),
        unknown_reason: if status == SmartStatus::Unknown {
            Some(SmartUnknownReason::DeviceNotSupported)
        } else {
            None
        },
    }
}

fn now() -> Timestamp { Timestamp::from_second(1_715_000_000).unwrap() }

#[test]
fn empty_aggregates_to_no_devices_found() {
    let h = SmartHealth::aggregate(vec![], now());
    assert_eq!(h.status, SmartStatus::Unknown);
    assert_eq!(h.unknown_reason, Some(SmartUnknownReason::NoDevicesFound));
    assert_eq!(h.worst_device, None);
    assert!(h.worst_reasons.is_empty());
}

#[test]
fn single_ok_aggregates_to_ok() {
    let h = SmartHealth::aggregate(vec![drive(SmartStatus::Ok, "/dev/nvme0n1", &[])], now());
    assert_eq!(h.status, SmartStatus::Ok);
    assert_eq!(h.worst_device.as_deref(), Some("/dev/nvme0n1"));
}

#[test]
fn critical_dominates_warn_and_ok() {
    let h = SmartHealth::aggregate(vec![
        drive(SmartStatus::Ok,       "/dev/sda",     &[]),
        drive(SmartStatus::Warn,     "/dev/sdb",     &["Temperature=62"]),
        drive(SmartStatus::Critical, "/dev/nvme0n1", &["Reallocated_Sector_Ct=3"]),
    ], now());
    assert_eq!(h.status, SmartStatus::Critical);
    assert_eq!(h.worst_device.as_deref(), Some("/dev/nvme0n1"));
    assert_eq!(h.worst_reasons, vec!["Reallocated_Sector_Ct=3".to_string()]);
}

#[test]
fn ok_wins_over_unknown() {
    // "A real measurement wins over 'we don't know'." Documented aggregate
    // semantics — do not change without re-reading the spec.
    let h = SmartHealth::aggregate(vec![
        drive(SmartStatus::Unknown, "/dev/vda", &[]),
        drive(SmartStatus::Ok,      "/dev/sda", &[]),
    ], now());
    assert_eq!(h.status, SmartStatus::Ok);
    assert_eq!(h.worst_device.as_deref(), Some("/dev/sda"));
}

#[test]
fn all_unknown_carries_first_reason() {
    let mut a = drive(SmartStatus::Unknown, "/dev/vda", &[]);
    a.unknown_reason = Some(SmartUnknownReason::DeviceNotSupported);
    let mut b = drive(SmartStatus::Unknown, "/dev/vdb", &[]);
    b.unknown_reason = Some(SmartUnknownReason::ParseError);
    let h = SmartHealth::aggregate(vec![a, b], now());
    assert_eq!(h.status, SmartStatus::Unknown);
    assert_eq!(h.unknown_reason, Some(SmartUnknownReason::DeviceNotSupported));
}

#[test]
fn is_stale_at_48h_boundary() {
    let mut h = SmartHealth::aggregate(vec![drive(SmartStatus::Ok, "/dev/sda", &[])], now());
    h.checked_at = now().checked_sub(Span::new().hours(48)).unwrap();
    assert!(h.is_stale(now(), 48));
}

#[test]
fn is_not_stale_at_47h59m() {
    let mut h = SmartHealth::aggregate(vec![drive(SmartStatus::Ok, "/dev/sda", &[])], now());
    h.checked_at = now()
        .checked_sub(Span::new().hours(47).minutes(59))
        .unwrap();
    assert!(!h.is_stale(now(), 48));
}

#[test]
fn future_checked_at_is_stale() {
    // Clock-skew fail-safe: a checked_at in the future means we can't trust
    // the snapshot (NTP step backward, container with frozen clock, etc.).
    let mut h = SmartHealth::aggregate(vec![drive(SmartStatus::Ok, "/dev/sda", &[])], now());
    h.checked_at = now().checked_add(Span::new().minutes(10)).unwrap();
    assert!(h.is_stale(now(), 48));
}

#[test]
fn status_label_strings() {
    assert_eq!(SmartStatus::Ok.label(),       "ok");
    assert_eq!(SmartStatus::Warn.label(),     "warn");
    assert_eq!(SmartStatus::Critical.label(), "fail");
    assert_eq!(SmartStatus::Unknown.label(),  "?");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test smart_types_test`
Expected: FAIL with `unresolved import 'pgforge::smart::types'` items.

- [ ] **Step 3: Implement the types**

Replace `src/smart/types.rs`:

```rust
//! Public types shared across the smart module. Mirrors the shape of
//! `src/disk/health.rs` so the two health surfaces aggregate the same way.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmartStatus {
    Ok,
    Warn,
    Critical,
    Unknown,
}

impl SmartStatus {
    /// Severity ordering: Unknown < Ok < Warn < Critical.
    /// Matches `DiskStatus::rank` in `src/disk/health.rs` so aggregate logic
    /// is consistent across the two health surfaces.
    fn rank(self) -> u8 {
        match self {
            SmartStatus::Unknown  => 0,
            SmartStatus::Ok       => 1,
            SmartStatus::Warn     => 2,
            SmartStatus::Critical => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SmartStatus::Ok       => "ok",
            SmartStatus::Warn     => "warn",
            SmartStatus::Critical => "fail",
            SmartStatus::Unknown  => "?",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmartUnknownReason {
    NotInstalled,
    NoSudoers,
    NoInstalledState,
    NoDevicesFound,
    DeviceNotSupported,
    DeviceMissing,
    Stale,
    NoCache,
    ParseError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveSmart {
    pub device: String,
    pub model: String,
    pub transport: String,
    pub status: SmartStatus,
    pub reasons: Vec<String>,
    pub unknown_reason: Option<SmartUnknownReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartHealth {
    pub status: SmartStatus,
    pub worst_device: Option<String>,
    pub worst_reasons: Vec<String>,
    pub unknown_reason: Option<SmartUnknownReason>,
    pub drives: Vec<DriveSmart>,
    pub checked_at: jiff::Timestamp,
}

impl SmartHealth {
    pub fn unknown(reason: SmartUnknownReason) -> Self {
        SmartHealth {
            status: SmartStatus::Unknown,
            worst_device: None,
            worst_reasons: Vec::new(),
            unknown_reason: Some(reason),
            drives: Vec::new(),
            checked_at: jiff::Timestamp::now(),
        }
    }

    /// Worst-of aggregate. Empty -> Unknown(NoDevicesFound). Otherwise pick
    /// by `SmartStatus::rank`; ties broken by first occurrence (lsblk order).
    /// A mix of one Ok drive and many Unknown drives reports Ok — "a real
    /// measurement wins over 'we don't know'." If every drive is Unknown,
    /// surface the first drive's unknown_reason.
    pub fn aggregate(drives: Vec<DriveSmart>, now: jiff::Timestamp) -> Self {
        if drives.is_empty() {
            let mut h = Self::unknown(SmartUnknownReason::NoDevicesFound);
            h.checked_at = now;
            return h;
        }
        let worst_idx = drives
            .iter()
            .enumerate()
            .max_by_key(|(_, d)| d.status.rank())
            .map(|(i, _)| i);
        let worst_idx = match worst_idx {
            Some(i) => i,
            None    => 0,
        };
        let worst = match drives.get(worst_idx) {
            Some(d) => d.clone(),
            None    => return Self::unknown(SmartUnknownReason::ParseError),
        };
        let all_unknown = drives.iter().all(|d| d.status == SmartStatus::Unknown);
        let unknown_reason = if all_unknown {
            drives.first().and_then(|d| d.unknown_reason)
        } else {
            None
        };
        SmartHealth {
            status: if all_unknown { SmartStatus::Unknown } else { worst.status },
            worst_device: Some(worst.device),
            worst_reasons: worst.reasons,
            unknown_reason,
            drives,
            checked_at: now,
        }
    }

    /// True if the snapshot is too old (older than `max_age_hours`) OR if
    /// `checked_at` is in the future relative to `now` (clock-skew /
    /// NTP step backward / container with frozen-in-future clock).
    pub fn is_stale(&self, now: jiff::Timestamp, max_age_hours: u32) -> bool {
        if now < self.checked_at {
            return true;
        }
        match now.since(self.checked_at) {
            Ok(span) => {
                let hours = span.total(jiff::Unit::Hour).unwrap_or(0.0);
                hours > max_age_hours as f64
            }
            Err(_) => true, // unrepresentable -> treat as stale
        }
    }
}
```

- [ ] **Step 4: Run the tests and confirm green**

Run: `cargo test --test smart_types_test`
Expected: all 9 tests pass.

- [ ] **Step 5: Lint**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/smart/types.rs tests/smart_types_test.rs
git commit -m "$(cat <<'EOF'
feat(smart): types + aggregate + clock-skew-safe staleness

SmartStatus / SmartUnknownReason / DriveSmart / SmartHealth, mirroring
the shape of src/disk/health.rs. aggregate() is worst-of with the same
rank as DiskStatus (Unknown < Ok < Warn < Critical) — a real measurement
wins over "we don't know." is_stale() fail-safes on future checked_at to
protect against clock skew / NTP backward step.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: InstalledState (record of what install did)

**Files:**
- Modify: `src/smart/installed.rs`
- Create: `tests/smart_installed_test.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/smart_installed_test.rs`:

```rust
use jiff::Timestamp;
use pgforge::smart::installed::{InstalledState, read_installed, write_installed};
use std::path::PathBuf;

#[test]
fn round_trip() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("smart-installed.json");
    let state = InstalledState {
        smartctl_path: PathBuf::from("/usr/sbin/smartctl"),
        user: "pawel".into(),
        devices: vec![PathBuf::from("/dev/nvme0n1"), PathBuf::from("/dev/sda")],
        installed_at: Timestamp::from_second(1_715_000_000).unwrap(),
    };
    write_installed(&path, &state).unwrap();
    let back = read_installed(&path).unwrap();
    assert_eq!(back.smartctl_path, state.smartctl_path);
    assert_eq!(back.user, state.user);
    assert_eq!(back.devices, state.devices);
    assert_eq!(back.installed_at, state.installed_at);
}

#[test]
fn missing_file_returns_none() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("absent.json");
    assert!(read_installed(&path).is_none());
}

#[test]
fn corrupt_file_returns_none() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("smart-installed.json");
    std::fs::write(&path, b"not json {{{").unwrap();
    assert!(read_installed(&path).is_none());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test smart_installed_test`
Expected: FAIL with `unresolved import` for `InstalledState`/`read_installed`/`write_installed`.

- [ ] **Step 3: Implement the module**

Replace `src/smart/installed.rs`:

```rust
//! Record of what `pgforge smart install` set up — used at runtime by
//! `run_smartctl` to know which absolute `smartctl` binary path the sudoers
//! rule grants, and by `pgforge smart status` to surface install age.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledState {
    pub smartctl_path: PathBuf,
    pub user: String,
    pub devices: Vec<PathBuf>,
    pub installed_at: jiff::Timestamp,
}

/// Default path under XDG_STATE_HOME. Falls back to $HOME/.local/state.
pub fn default_installed_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("pgforge").join("smart-installed.json")
}

/// Best-effort read. Missing or corrupt → None (caller treats as
/// `SmartUnknownReason::NoInstalledState`).
pub fn read_installed(path: &Path) -> Option<InstalledState> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Atomic write via tempfile + persist (same-fs rename).
pub fn write_installed(path: &Path, state: &InstalledState) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent")
    })?;
    std::fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(tmp.as_file_mut(), state)
        .map_err(|e| std::io::Error::other(format!("serialize: {e}")))?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --test smart_installed_test`
Expected: 3/3 pass.

- [ ] **Step 5: Lint**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/smart/installed.rs tests/smart_installed_test.rs
git commit -m "$(cat <<'EOF'
feat(smart): InstalledState — record of what `smart install` set up

Persists smartctl_path + user + devices + installed_at to
~/.local/state/pgforge/smart-installed.json (XDG_STATE_HOME aware).
Atomic write via tempfile + persist. Read is best-effort: missing or
corrupt → None (caller treats as Unknown(NoInstalledState)).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Cache (read/write + stale + clock-skew)

**Files:**
- Modify: `src/smart/cache.rs`
- Create: `tests/smart_cache_test.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/smart_cache_test.rs`:

```rust
use jiff::{Span, Timestamp};
use pgforge::smart::cache::{STALE_AFTER_HOURS, read_cache, write_cache};
use pgforge::smart::types::{
    DriveSmart, SmartHealth, SmartStatus, SmartUnknownReason,
};

fn now() -> Timestamp { Timestamp::from_second(1_715_000_000).unwrap() }

fn sample_health(checked_at: Timestamp) -> SmartHealth {
    let drive = DriveSmart {
        device: "/dev/nvme0n1".into(),
        model: "X".into(),
        transport: "nvme".into(),
        status: SmartStatus::Ok,
        reasons: vec![],
        unknown_reason: None,
    };
    let mut h = SmartHealth::aggregate(vec![drive], checked_at);
    h.checked_at = checked_at;
    h
}

#[test]
fn round_trip_fresh() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("disk-smart.json");
    let h = sample_health(now());
    write_cache(&path, &h).unwrap();
    let back = read_cache(&path, now(), STALE_AFTER_HOURS);
    assert_eq!(back.status, SmartStatus::Ok);
    assert_eq!(back.worst_device.as_deref(), Some("/dev/nvme0n1"));
    assert_eq!(back.unknown_reason, None);
}

#[test]
fn missing_file_returns_no_cache() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("absent.json");
    let back = read_cache(&path, now(), STALE_AFTER_HOURS);
    assert_eq!(back.status, SmartStatus::Unknown);
    assert_eq!(back.unknown_reason, Some(SmartUnknownReason::NoCache));
}

#[test]
fn corrupt_json_returns_parse_error() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("disk-smart.json");
    std::fs::write(&path, b"not json {{{").unwrap();
    let back = read_cache(&path, now(), STALE_AFTER_HOURS);
    assert_eq!(back.status, SmartStatus::Unknown);
    assert_eq!(back.unknown_reason, Some(SmartUnknownReason::ParseError));
}

#[test]
fn boundary_48h_is_stale() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("disk-smart.json");
    let checked = now().checked_sub(Span::new().hours(48)).unwrap();
    write_cache(&path, &sample_health(checked)).unwrap();
    let back = read_cache(&path, now(), STALE_AFTER_HOURS);
    assert_eq!(back.status, SmartStatus::Unknown);
    assert_eq!(back.unknown_reason, Some(SmartUnknownReason::Stale));
}

#[test]
fn just_under_48h_is_fresh() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("disk-smart.json");
    let checked = now().checked_sub(Span::new().hours(47).minutes(59)).unwrap();
    write_cache(&path, &sample_health(checked)).unwrap();
    let back = read_cache(&path, now(), STALE_AFTER_HOURS);
    assert_eq!(back.status, SmartStatus::Ok);
    assert_eq!(back.unknown_reason, None);
}

#[test]
fn future_checked_at_is_stale() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("disk-smart.json");
    let checked = now().checked_add(Span::new().minutes(10)).unwrap();
    write_cache(&path, &sample_health(checked)).unwrap();
    let back = read_cache(&path, now(), STALE_AFTER_HOURS);
    assert_eq!(back.status, SmartStatus::Unknown);
    assert_eq!(back.unknown_reason, Some(SmartUnknownReason::Stale));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test smart_cache_test`
Expected: FAIL with `unresolved import 'pgforge::smart::cache::{...}'`.

- [ ] **Step 3: Implement the module**

Replace `src/smart/cache.rs`:

```rust
//! JSON cache of the most recent SMART check. Written once a day by the
//! systemd-user timer's `pgforge smart check --write-cache`; read every
//! 60 s by the TUI poller and pre-dispatch by the CLI banner.
//!
//! All I/O is SYNCHRONOUS by design — the cache file is a few-KB JSON,
//! reads take microseconds, and using `tokio::fs` here would just add
//! task-switch overhead with no latency benefit. The TUI reader poller is
//! aware of this (see `src/tui/refresh.rs::spawn_smart_reader`).

use crate::smart::types::{SmartHealth, SmartUnknownReason};
use std::path::{Path, PathBuf};

pub const STALE_AFTER_HOURS: u32 = 48;

/// `$XDG_STATE_HOME/pgforge/disk-smart.json` (fallback
/// `$HOME/.local/state/pgforge/disk-smart.json`).
pub fn default_cache_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("pgforge").join("disk-smart.json")
}

/// Best-effort read with explicit failure-reason mapping.
///
/// - File missing → `Unknown(NoCache)`.
/// - File present but unparseable → `Unknown(ParseError)`.
/// - File parses but `checked_at` is older than `max_age_hours` OR in the
///   future (clock skew / NTP step backward) → `Unknown(Stale)`.
/// - Otherwise the deserialized snapshot.
pub fn read_cache(
    path: &Path,
    now: jiff::Timestamp,
    max_age_hours: u32,
) -> SmartHealth {
    let bytes = match std::fs::read(path) {
        Ok(b)  => b,
        Err(_) => return SmartHealth::unknown(SmartUnknownReason::NoCache),
    };
    let h: SmartHealth = match serde_json::from_slice(&bytes) {
        Ok(h)  => h,
        Err(_) => return SmartHealth::unknown(SmartUnknownReason::ParseError),
    };
    if h.is_stale(now, max_age_hours) {
        return SmartHealth::unknown(SmartUnknownReason::Stale);
    }
    h
}

/// Atomic write: tempfile in the SAME parent directory + `persist()`. Same
/// filesystem → rename is atomic. Tempfile mode is 0600 via NamedTempFile.
pub fn write_cache(path: &Path, health: &SmartHealth) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent")
    })?;
    std::fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(tmp.as_file_mut(), health)
        .map_err(|e| std::io::Error::other(format!("serialize: {e}")))?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --test smart_cache_test`
Expected: 6/6 pass.

- [ ] **Step 5: Lint**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/smart/cache.rs tests/smart_cache_test.rs
git commit -m "$(cat <<'EOF'
feat(smart): JSON cache with stale + clock-skew fail-safes

read_cache maps every failure to a distinct SmartUnknownReason
(NoCache/ParseError/Stale). is_stale guards against future checked_at
(NTP step backward, container with frozen clock). write_cache uses
tempfile in the same parent dir for atomic rename.

STALE_AFTER_HOURS=48 (2× daily cadence — one missed run is tolerated,
two is suspicious).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Discovery via `lsblk`

**Files:**
- Modify: `src/smart/check.rs`

- [ ] **Step 1: Write the failing test (inline in check.rs)**

Append to `src/smart/check.rs` (replacing the stub):

```rust
//! SMART check pipeline: discover physical disks via lsblk → run smartctl
//! per disk (sudo) → parse JSON → aggregate. The whole pipeline is
//! best-effort: anything that fails maps to a `DriveSmart` with
//! `SmartStatus::Unknown` and a specific `SmartUnknownReason`.

use crate::smart::types::SmartUnknownReason;
use serde::Deserialize;
use std::path::PathBuf;

/// One physical disk discovered on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredDisk {
    pub path: PathBuf,        // "/dev/nvme0n1"
    pub transport: String,    // "sata" | "sas" | "nvme"
    pub model: String,        // best-effort model string from lsblk
}

#[derive(Debug, Deserialize)]
struct LsblkRoot {
    #[serde(default)]
    blockdevices: Vec<LsblkDevice>,
}

#[derive(Debug, Deserialize)]
struct LsblkDevice {
    name: String,
    #[serde(default, rename = "type")]
    dev_type: Option<String>,
    #[serde(default)]
    tran: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

/// Parse `lsblk -d -o NAME,TYPE,TRAN,MODEL -J` JSON output into the
/// discovered-disk list. Filters: type=="disk" AND tran in {sata,sas,nvme}.
/// Tolerant of missing optional fields; any device whose JSON entry fails
/// to deserialize is silently skipped (lsblk -J had quoting bugs through
/// util-linux 2.38).
pub fn parse_lsblk_json(json: &[u8]) -> Vec<DiscoveredDisk> {
    let root: LsblkRoot = match serde_json::from_slice(json) {
        Ok(r)  => r,
        Err(_) => return Vec::new(),
    };
    root.blockdevices
        .into_iter()
        .filter_map(|d| {
            let dt = d.dev_type.as_deref()?;
            if dt != "disk" { return None; }
            let tran = d.tran?;
            if !matches!(tran.as_str(), "sata" | "sas" | "nvme") {
                return None;
            }
            Some(DiscoveredDisk {
                path: PathBuf::from(format!("/dev/{}", d.name)),
                transport: tran,
                model: d.model.unwrap_or_default(),
            })
        })
        .collect()
}

/// Discover physical disks by invoking `lsblk -d -o NAME,TYPE,TRAN,MODEL -J`.
/// Failure (lsblk missing, exit non-zero, JSON broken) → empty Vec; caller
/// degrades to `SmartUnknownReason::NoDevicesFound`.
pub async fn discover_disks() -> Vec<DiscoveredDisk> {
    let out = tokio::process::Command::new("lsblk")
        .args(["-d", "-o", "NAME,TYPE,TRAN,MODEL", "-J"])
        .output()
        .await;
    let stdout = match out {
        Ok(o) if o.status.success() => o.stdout,
        Ok(o) => {
            tracing::warn!(target: "pgforge::smart",
                "lsblk exit {}: {}", o.status,
                String::from_utf8_lossy(&o.stderr));
            return Vec::new();
        }
        Err(e) => {
            tracing::warn!(target: "pgforge::smart", "lsblk spawn failed: {e}");
            return Vec::new();
        }
    };
    parse_lsblk_json(&stdout)
}

// Silence unused-import lint until later tasks use the reason enum.
#[allow(dead_code)]
fn _touch_reason_enum() -> SmartUnknownReason { SmartUnknownReason::NoDevicesFound }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_to_physical_disks() {
        let json = br#"{
            "blockdevices": [
                {"name":"nvme0n1","type":"disk","tran":"nvme","model":"SK hynix"},
                {"name":"sda","type":"disk","tran":"sata","model":"Samsung 870"},
                {"name":"loop0","type":"loop","tran":null,"model":null},
                {"name":"dm-0","type":"crypt","tran":null,"model":null},
                {"name":"sdb1","type":"part","tran":null,"model":null},
                {"name":"vda","type":"disk","tran":"virtio","model":""}
            ]
        }"#;
        let disks = parse_lsblk_json(json);
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].path, std::path::PathBuf::from("/dev/nvme0n1"));
        assert_eq!(disks[0].transport, "nvme");
        assert_eq!(disks[1].path, std::path::PathBuf::from("/dev/sda"));
        assert_eq!(disks[1].transport, "sata");
    }

    #[test]
    fn empty_on_no_blockdevices() {
        let json = br#"{"blockdevices":[]}"#;
        assert!(parse_lsblk_json(json).is_empty());
    }

    #[test]
    fn empty_on_garbage_json() {
        assert!(parse_lsblk_json(b"not json").is_empty());
    }

    #[test]
    fn skips_devices_missing_required_fields() {
        let json = br#"{
            "blockdevices":[
                {"name":"sda"},
                {"name":"sdb","type":"disk","tran":"sata","model":"X"}
            ]
        }"#;
        let disks = parse_lsblk_json(json);
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].path, std::path::PathBuf::from("/dev/sdb"));
    }
}
```

- [ ] **Step 2: Run the inline tests**

Run: `cargo test --lib smart::check::tests`
Expected: 4/4 pass.

- [ ] **Step 3: Lint**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add src/smart/check.rs
git commit -m "$(cat <<'EOF'
feat(smart): discover physical disks via lsblk -d -o ... -J

parse_lsblk_json filters to type=disk AND tran in {sata,sas,nvme} —
drops loop, dm-crypt, partitions, virtio (the latter not because virtio
is bad, but because virtio disks almost never expose SMART so they
contribute nothing). Tolerant of optional missing fields. discover_disks
shells out to lsblk and degrades to empty Vec on any failure.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: smartctl invocation (sudo wrap with Interactive / NonInteractive modes)

**Files:**
- Modify: `src/smart/check.rs`

- [ ] **Step 1: Write the failing test (extend the inline test module)**

In `src/smart/check.rs`, ADD to the existing `mod tests`:

```rust
    #[test]
    fn classify_smartctl_stderr_no_such_file() {
        let stderr = "Smartctl open device: /dev/sda failed: No such file or directory";
        assert_eq!(
            classify_smartctl_failure(stderr),
            SmartUnknownReason::DeviceMissing,
        );
    }

    #[test]
    fn classify_smartctl_stderr_not_supported() {
        let stderr = "Device does not support SMART";
        assert_eq!(
            classify_smartctl_failure(stderr),
            SmartUnknownReason::DeviceNotSupported,
        );
    }

    #[test]
    fn classify_smartctl_stderr_unknown_usb_bridge() {
        let stderr = "Smartctl open device: /dev/sdc failed: Unknown USB bridge";
        assert_eq!(
            classify_smartctl_failure(stderr),
            SmartUnknownReason::DeviceNotSupported,
        );
    }

    #[test]
    fn classify_sudo_password_required() {
        let stderr = "sudo: a password is required";
        assert_eq!(
            classify_smartctl_failure(stderr),
            SmartUnknownReason::NoSudoers,
        );
    }

    #[test]
    fn classify_command_not_found() {
        let stderr = "sudo: smartctl: command not found";
        assert_eq!(
            classify_smartctl_failure(stderr),
            SmartUnknownReason::NotInstalled,
        );
    }

    #[test]
    fn classify_anything_else_is_parse_error() {
        let stderr = "weird unexpected message";
        assert_eq!(
            classify_smartctl_failure(stderr),
            SmartUnknownReason::ParseError,
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib smart::check::tests::classify`
Expected: FAIL with `cannot find function 'classify_smartctl_failure'`.

- [ ] **Step 3: Implement `run_smartctl` + helpers**

In `src/smart/check.rs`, REMOVE the placeholder `_touch_reason_enum` helper and APPEND below the existing functions:

```rust
/// Whether to run sudo non-interactively (`-n`, used by the timer where we
/// can't prompt) or interactively (used by `pgforge smart check` from a
/// TTY without --write-cache, where the user is sitting at the keyboard).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SudoMode {
    NonInteractive,
    Interactive,
}

/// Spawn sudo + smartctl on one device, return stdout bytes on success or
/// a classified `SmartUnknownReason` on failure. 5-second per-call timeout.
pub async fn run_smartctl(
    smartctl_path: &std::path::Path,
    device: &std::path::Path,
    mode: SudoMode,
) -> Result<Vec<u8>, SmartUnknownReason> {
    let mut cmd = tokio::process::Command::new("sudo");
    if mode == SudoMode::NonInteractive {
        cmd.arg("-n");
    }
    cmd.arg(smartctl_path)
        .args(["-H", "-A", "-j"])
        .arg(device);

    let output = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        cmd.output(),
    ).await {
        Ok(Ok(o))  => o,
        Ok(Err(e)) => {
            tracing::warn!(target: "pgforge::smart",
                "spawn sudo smartctl {device:?}: {e}");
            return Err(SmartUnknownReason::NotInstalled);
        }
        Err(_) => {
            tracing::warn!(target: "pgforge::smart",
                "smartctl {device:?} timed out after 5s");
            return Err(SmartUnknownReason::ParseError);
        }
    };

    // Non-zero exit code may still produce valid JSON (smartctl uses its
    // exit code as a bitfield; OVERALL_HEALTH=FAILED returns nonzero but
    // emits JSON). Trust stdout if it parses; only fall back to stderr
    // classification when stdout is empty.
    if !output.stdout.is_empty() {
        return Ok(output.stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(classify_smartctl_failure(&stderr))
}

/// Map a smartctl / sudo stderr blob to the most specific
/// `SmartUnknownReason`. Order matters — match the more specific patterns
/// first.
pub fn classify_smartctl_failure(stderr: &str) -> SmartUnknownReason {
    if stderr.contains("a password is required") {
        return SmartUnknownReason::NoSudoers;
    }
    if stderr.contains("command not found") {
        return SmartUnknownReason::NotInstalled;
    }
    if stderr.contains("No such file or directory") {
        return SmartUnknownReason::DeviceMissing;
    }
    if stderr.contains("Unknown USB bridge")
        || stderr.contains("does not support SMART")
    {
        return SmartUnknownReason::DeviceNotSupported;
    }
    SmartUnknownReason::ParseError
}
```

- [ ] **Step 4: Run the inline tests**

Run: `cargo test --lib smart::check::tests`
Expected: 10/10 pass (4 from T4 + 6 new).

- [ ] **Step 5: Lint**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/smart/check.rs
git commit -m "$(cat <<'EOF'
feat(smart): run_smartctl wrap + stderr classification

5s-timeout subprocess wrapper around `sudo [-n] smartctl -H -A -j`.
NonInteractive mode for the timer path; Interactive for TTY check.
classify_smartctl_failure maps stderr blobs to specific
SmartUnknownReason variants (NoSudoers, NotInstalled, DeviceMissing,
DeviceNotSupported, ParseError fallback) so the operator gets a
specific 'why is this Unknown' instead of a generic shrug.

Non-zero exit codes with non-empty stdout are passed through unchanged —
smartctl exit codes are a bitfield and OVERALL_HEALTH=FAILED still
produces valid JSON.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: parse_smartctl_json + parse_sata + parse_nvme + fixtures

**Files:**
- Modify: `src/smart/check.rs`
- Create: `tests/smart_parsing_test.rs`
- Create: `tests/fixtures/smart/*.json` (13 fixtures)

- [ ] **Step 1: Write the failing test**

Create `tests/smart_parsing_test.rs`:

```rust
use pgforge::smart::check::parse_smartctl_json;
use pgforge::smart::types::{SmartStatus, SmartUnknownReason};

fn load(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/smart").join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("read {p:?}: {e}"))
}

#[test]
fn sata_ok() {
    let d = parse_smartctl_json(&load("sata_ok.json"));
    assert_eq!(d.status, SmartStatus::Ok);
    assert_eq!(d.transport, "ATA");
    assert!(d.reasons.is_empty());
}

#[test]
fn sata_reallocated_is_critical() {
    let d = parse_smartctl_json(&load("sata_reallocated_3.json"));
    assert_eq!(d.status, SmartStatus::Critical);
    assert!(d.reasons.iter().any(|r| r.contains("Reallocated_Sector_Ct=3")));
}

#[test]
fn sata_pending_is_critical() {
    let d = parse_smartctl_json(&load("sata_pending_1.json"));
    assert_eq!(d.status, SmartStatus::Critical);
    assert!(d.reasons.iter().any(|r| r.contains("Current_Pending_Sector=1")));
}

#[test]
fn sata_offline_uncorrectable_is_critical() {
    let d = parse_smartctl_json(&load("sata_offline_uncorrectable_1.json"));
    assert_eq!(d.status, SmartStatus::Critical);
    assert!(d.reasons.iter().any(|r| r.contains("Offline_Uncorrectable=1")));
}

#[test]
fn sata_temp_65_is_warn() {
    let d = parse_smartctl_json(&load("sata_temp_65.json"));
    assert_eq!(d.status, SmartStatus::Warn);
    assert!(d.reasons.iter().any(|r| r.contains("Temperature=65")));
}

#[test]
fn sata_temp_55_is_ok() {
    let d = parse_smartctl_json(&load("sata_temp_55.json"));
    assert_eq!(d.status, SmartStatus::Ok);
}

#[test]
fn sas_attached_sata_dispatches_to_sata_parser() {
    // device.protocol = "ATA" even though lsblk reports tran=sas; the parser
    // must trust device.protocol, not whatever was in the discovery call.
    let d = parse_smartctl_json(&load("sas_attached_sata_ok.json"));
    assert_eq!(d.status, SmartStatus::Ok);
    assert_eq!(d.transport, "ATA");
}

#[test]
fn nvme_ok() {
    let d = parse_smartctl_json(&load("nvme_ok.json"));
    assert_eq!(d.status, SmartStatus::Ok);
    assert_eq!(d.transport, "NVMe");
}

#[test]
fn nvme_critical_warning_spare() {
    let d = parse_smartctl_json(&load("nvme_critical_warning_spare.json"));
    assert_eq!(d.status, SmartStatus::Critical);
    assert!(d.reasons.iter().any(|r| r.contains("available_spare_below_threshold")));
}

#[test]
fn nvme_media_errors_is_critical() {
    let d = parse_smartctl_json(&load("nvme_media_errors_5.json"));
    assert_eq!(d.status, SmartStatus::Critical);
    assert!(d.reasons.iter().any(|r| r.contains("media_errors=5")));
}

#[test]
fn nvme_spare_below_threshold_is_critical() {
    let d = parse_smartctl_json(&load("nvme_spare_below_threshold.json"));
    assert_eq!(d.status, SmartStatus::Critical);
    assert!(d.reasons.iter().any(|r| r.contains("available_spare=5") && r.contains("threshold=10")));
}

#[test]
fn nvme_percentage_used_82_is_warn() {
    let d = parse_smartctl_json(&load("nvme_percentage_used_82.json"));
    assert_eq!(d.status, SmartStatus::Warn);
    assert!(d.reasons.iter().any(|r| r.contains("percentage_used=82")));
}

#[test]
fn nvme_temp_75_celsius_is_warn() {
    let d = parse_smartctl_json(&load("nvme_temp_75_celsius.json"));
    assert_eq!(d.status, SmartStatus::Warn);
    assert!(d.reasons.iter().any(|r| r.contains("Temperature=75")));
}

#[test]
fn nvme_temp_kelvin_fallback_is_warn() {
    // smartmontools omits top-level temperature.current; parse_nvme must
    // fall back to nvme_smart_health_information_log.temperature (kelvin)
    // and convert. 348 K = 75 °C → Warn.
    let d = parse_smartctl_json(&load("nvme_temp_only_kelvin.json"));
    assert_eq!(d.status, SmartStatus::Warn);
    assert!(d.reasons.iter().any(|r| r.contains("Temperature=75")));
}

#[test]
fn empty_bytes_is_parse_error() {
    let d = parse_smartctl_json(b"");
    assert_eq!(d.status, SmartStatus::Unknown);
    assert_eq!(d.unknown_reason, Some(SmartUnknownReason::ParseError));
}

#[test]
fn unknown_protocol_is_parse_error() {
    let json = br#"{"device":{"protocol":"SAT"},"smart_status":{"passed":true}}"#;
    let d = parse_smartctl_json(json);
    assert_eq!(d.status, SmartStatus::Unknown);
    assert_eq!(d.unknown_reason, Some(SmartUnknownReason::ParseError));
}
```

- [ ] **Step 2: Create the fixtures**

Create the `tests/fixtures/smart/` directory and write these 14 files. Each is a minimal but valid `smartctl -H -A -j` JSON; we deliberately keep only the fields parse_smartctl_json reads, with everything else stripped.

Create `tests/fixtures/smart/sata_ok.json`:

```json
{
  "device": { "protocol": "ATA", "name": "/dev/sda" },
  "model_name": "Samsung SSD 870 EVO 500GB",
  "smart_status": { "passed": true },
  "ata_smart_attributes": {
    "table": [
      { "id": 5,   "name": "Reallocated_Sector_Ct",   "raw": { "value": 0 } },
      { "id": 194, "name": "Temperature_Celsius",     "raw": { "value": 35 } },
      { "id": 197, "name": "Current_Pending_Sector",  "raw": { "value": 0 } },
      { "id": 198, "name": "Offline_Uncorrectable",   "raw": { "value": 0 } }
    ]
  }
}
```

Create `tests/fixtures/smart/sata_reallocated_3.json`:

```json
{
  "device": { "protocol": "ATA", "name": "/dev/sda" },
  "model_name": "Samsung SSD 870 EVO 500GB",
  "smart_status": { "passed": true },
  "ata_smart_attributes": {
    "table": [
      { "id": 5,   "name": "Reallocated_Sector_Ct",   "raw": { "value": 3 } },
      { "id": 194, "name": "Temperature_Celsius",     "raw": { "value": 35 } },
      { "id": 197, "name": "Current_Pending_Sector",  "raw": { "value": 0 } },
      { "id": 198, "name": "Offline_Uncorrectable",   "raw": { "value": 0 } }
    ]
  }
}
```

Create `tests/fixtures/smart/sata_pending_1.json`:

```json
{
  "device": { "protocol": "ATA", "name": "/dev/sda" },
  "model_name": "Samsung SSD 870 EVO 500GB",
  "smart_status": { "passed": true },
  "ata_smart_attributes": {
    "table": [
      { "id": 5,   "name": "Reallocated_Sector_Ct",   "raw": { "value": 0 } },
      { "id": 194, "name": "Temperature_Celsius",     "raw": { "value": 35 } },
      { "id": 197, "name": "Current_Pending_Sector",  "raw": { "value": 1 } },
      { "id": 198, "name": "Offline_Uncorrectable",   "raw": { "value": 0 } }
    ]
  }
}
```

Create `tests/fixtures/smart/sata_offline_uncorrectable_1.json`:

```json
{
  "device": { "protocol": "ATA", "name": "/dev/sda" },
  "model_name": "Samsung SSD 870 EVO 500GB",
  "smart_status": { "passed": true },
  "ata_smart_attributes": {
    "table": [
      { "id": 5,   "name": "Reallocated_Sector_Ct",   "raw": { "value": 0 } },
      { "id": 194, "name": "Temperature_Celsius",     "raw": { "value": 35 } },
      { "id": 197, "name": "Current_Pending_Sector",  "raw": { "value": 0 } },
      { "id": 198, "name": "Offline_Uncorrectable",   "raw": { "value": 1 } }
    ]
  }
}
```

Create `tests/fixtures/smart/sata_temp_65.json`:

```json
{
  "device": { "protocol": "ATA", "name": "/dev/sda" },
  "model_name": "Samsung SSD 870 EVO 500GB",
  "smart_status": { "passed": true },
  "ata_smart_attributes": {
    "table": [
      { "id": 5,   "name": "Reallocated_Sector_Ct",   "raw": { "value": 0 } },
      { "id": 194, "name": "Temperature_Celsius",     "raw": { "value": 65 } },
      { "id": 197, "name": "Current_Pending_Sector",  "raw": { "value": 0 } },
      { "id": 198, "name": "Offline_Uncorrectable",   "raw": { "value": 0 } }
    ]
  }
}
```

Create `tests/fixtures/smart/sata_temp_55.json`:

```json
{
  "device": { "protocol": "ATA", "name": "/dev/sda" },
  "model_name": "Samsung SSD 870 EVO 500GB",
  "smart_status": { "passed": true },
  "ata_smart_attributes": {
    "table": [
      { "id": 5,   "name": "Reallocated_Sector_Ct",   "raw": { "value": 0 } },
      { "id": 194, "name": "Temperature_Celsius",     "raw": { "value": 55 } },
      { "id": 197, "name": "Current_Pending_Sector",  "raw": { "value": 0 } },
      { "id": 198, "name": "Offline_Uncorrectable",   "raw": { "value": 0 } }
    ]
  }
}
```

Create `tests/fixtures/smart/sas_attached_sata_ok.json` (this is a SATA drive sitting on a SAS backplane — `device.protocol` is `ATA`, even though lsblk would have called the transport `sas`):

```json
{
  "device": { "protocol": "ATA", "name": "/dev/sdx" },
  "model_name": "WDC WD40EFAX-68JH4N1",
  "smart_status": { "passed": true },
  "ata_smart_attributes": {
    "table": [
      { "id": 5,   "name": "Reallocated_Sector_Ct",   "raw": { "value": 0 } },
      { "id": 194, "name": "Temperature_Celsius",     "raw": { "value": 32 } },
      { "id": 197, "name": "Current_Pending_Sector",  "raw": { "value": 0 } },
      { "id": 198, "name": "Offline_Uncorrectable",   "raw": { "value": 0 } }
    ]
  }
}
```

Create `tests/fixtures/smart/nvme_ok.json`:

```json
{
  "device": { "protocol": "NVMe", "name": "/dev/nvme0n1" },
  "model_name": "SK hynix BC901 HFS512GEJ9X108N",
  "smart_status": { "passed": true },
  "temperature": { "current": 38 },
  "nvme_smart_health_information_log": {
    "critical_warning": 0,
    "available_spare": 100,
    "available_spare_threshold": 10,
    "percentage_used": 3,
    "media_errors": 0
  }
}
```

Create `tests/fixtures/smart/nvme_critical_warning_spare.json`:

```json
{
  "device": { "protocol": "NVMe", "name": "/dev/nvme0n1" },
  "model_name": "SK hynix BC901 HFS512GEJ9X108N",
  "smart_status": { "passed": true },
  "temperature": { "current": 40 },
  "nvme_smart_health_information_log": {
    "critical_warning": 1,
    "available_spare": 8,
    "available_spare_threshold": 10,
    "percentage_used": 20,
    "media_errors": 0
  }
}
```

Create `tests/fixtures/smart/nvme_media_errors_5.json`:

```json
{
  "device": { "protocol": "NVMe", "name": "/dev/nvme0n1" },
  "model_name": "SK hynix BC901 HFS512GEJ9X108N",
  "smart_status": { "passed": true },
  "temperature": { "current": 38 },
  "nvme_smart_health_information_log": {
    "critical_warning": 0,
    "available_spare": 100,
    "available_spare_threshold": 10,
    "percentage_used": 3,
    "media_errors": 5
  }
}
```

Create `tests/fixtures/smart/nvme_spare_below_threshold.json`:

```json
{
  "device": { "protocol": "NVMe", "name": "/dev/nvme0n1" },
  "model_name": "SK hynix BC901 HFS512GEJ9X108N",
  "smart_status": { "passed": true },
  "temperature": { "current": 38 },
  "nvme_smart_health_information_log": {
    "critical_warning": 0,
    "available_spare": 5,
    "available_spare_threshold": 10,
    "percentage_used": 30,
    "media_errors": 0
  }
}
```

Create `tests/fixtures/smart/nvme_percentage_used_82.json`:

```json
{
  "device": { "protocol": "NVMe", "name": "/dev/nvme0n1" },
  "model_name": "SK hynix BC901 HFS512GEJ9X108N",
  "smart_status": { "passed": true },
  "temperature": { "current": 40 },
  "nvme_smart_health_information_log": {
    "critical_warning": 0,
    "available_spare": 100,
    "available_spare_threshold": 10,
    "percentage_used": 82,
    "media_errors": 0
  }
}
```

Create `tests/fixtures/smart/nvme_temp_75_celsius.json`:

```json
{
  "device": { "protocol": "NVMe", "name": "/dev/nvme0n1" },
  "model_name": "SK hynix BC901 HFS512GEJ9X108N",
  "smart_status": { "passed": true },
  "temperature": { "current": 75 },
  "nvme_smart_health_information_log": {
    "critical_warning": 0,
    "available_spare": 100,
    "available_spare_threshold": 10,
    "percentage_used": 3,
    "media_errors": 0
  }
}
```

Create `tests/fixtures/smart/nvme_temp_only_kelvin.json` (older smartmontools — no top-level `temperature` block; the only temperature field is the raw kelvin value inside the smart-health-information-log):

```json
{
  "device": { "protocol": "NVMe", "name": "/dev/nvme0n1" },
  "model_name": "SK hynix BC901 HFS512GEJ9X108N",
  "smart_status": { "passed": true },
  "nvme_smart_health_information_log": {
    "critical_warning": 0,
    "temperature": 348,
    "available_spare": 100,
    "available_spare_threshold": 10,
    "percentage_used": 3,
    "media_errors": 0
  }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test --test smart_parsing_test`
Expected: FAIL with `cannot find function 'parse_smartctl_json'`.

- [ ] **Step 4: Implement parsing**

In `src/smart/check.rs`, APPEND the parser functions:

```rust
use crate::smart::types::{DriveSmart, SmartStatus};

pub const SATA_TEMP_WARN_C:   i64 = 60;
pub const NVME_TEMP_WARN_C:   i64 = 70;
pub const NVME_WEAR_WARN_PCT: u32 = 80;

/// Top-level structural decoder. We only deserialize what we actually need;
/// `#[serde(default)]` on every nested field so the parser tolerates older /
/// newer smartmontools schema variations.
#[derive(Deserialize)]
struct SmartctlJson {
    #[serde(default)]
    device: SmartctlDevice,
    #[serde(default)]
    model_name: Option<String>,
    #[serde(default)]
    smart_status: SmartctlSmartStatus,
    #[serde(default)]
    temperature: Option<SmartctlTemperature>,
    #[serde(default)]
    ata_smart_attributes: Option<AtaSmartAttributes>,
    #[serde(default)]
    nvme_smart_health_information_log: Option<NvmeSmartLog>,
}

#[derive(Default, Deserialize)]
struct SmartctlDevice {
    #[serde(default)]
    protocol: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Default, Deserialize)]
struct SmartctlSmartStatus {
    #[serde(default)]
    passed: Option<bool>,
}

#[derive(Deserialize)]
struct SmartctlTemperature {
    #[serde(default)]
    current: Option<i64>,
}

#[derive(Deserialize)]
struct AtaSmartAttributes {
    #[serde(default)]
    table: Vec<AtaAttribute>,
}

#[derive(Deserialize)]
struct AtaAttribute {
    id: u8,
    #[serde(default)]
    name: Option<String>,
    raw: AtaRaw,
}

#[derive(Deserialize)]
struct AtaRaw {
    value: i64,
}

#[derive(Deserialize)]
struct NvmeSmartLog {
    #[serde(default)] critical_warning:          Option<u32>,
    #[serde(default)] available_spare:           Option<u32>,
    #[serde(default)] available_spare_threshold: Option<u32>,
    #[serde(default)] percentage_used:           Option<u32>,
    #[serde(default)] media_errors:              Option<u64>,
    #[serde(default)] temperature:               Option<i64>, // kelvin (older smartmontools)
}

/// Parse one device's `smartctl -H -A -j` JSON output. Dispatches on
/// `device.protocol` (NOT the lsblk transport — SAS-attached SATA reports
/// protocol="ATA" even though lsblk called the transport "sas"). Any failure
/// produces a `DriveSmart` with `SmartStatus::Unknown` and a specific
/// `SmartUnknownReason`.
pub fn parse_smartctl_json(json: &[u8]) -> DriveSmart {
    let parsed: SmartctlJson = match serde_json::from_slice(json) {
        Ok(p)  => p,
        Err(_) => return unknown_drive("?", "ParseError", SmartUnknownReason::ParseError, "json parse failed"),
    };
    let device = parsed.device.name.clone().unwrap_or_else(|| "?".to_string());
    let model  = parsed.model_name.clone().unwrap_or_default();
    let protocol = parsed.device.protocol.clone().unwrap_or_default();

    match protocol.as_str() {
        "ATA" | "SCSI" => parse_sata(&parsed, &device, &model, &protocol),
        "NVMe"         => parse_nvme(&parsed, &device, &model, &protocol),
        other => unknown_drive(
            &device, &protocol,
            SmartUnknownReason::ParseError,
            &format!("unsupported device.protocol={other}"),
        ),
    }
}

fn unknown_drive(device: &str, transport: &str, reason: SmartUnknownReason, msg: &str) -> DriveSmart {
    DriveSmart {
        device: device.to_string(),
        model: String::new(),
        transport: transport.to_string(),
        status: SmartStatus::Unknown,
        reasons: vec![msg.to_string()],
        unknown_reason: Some(reason),
    }
}

fn parse_sata(p: &SmartctlJson, device: &str, model: &str, transport: &str) -> DriveSmart {
    let mut reasons: Vec<String> = Vec::new();
    let mut status = SmartStatus::Ok;
    let mut bump = |s: SmartStatus, r: String, status: &mut SmartStatus, reasons: &mut Vec<String>| {
        if (s as u8) > (*status as u8) {
            *status = s;
        }
        reasons.push(r);
    };

    if p.smart_status.passed == Some(false) {
        bump(SmartStatus::Critical, "OVERALL_HEALTH=FAILED".to_string(), &mut status, &mut reasons);
    }

    let table: &[AtaAttribute] = p
        .ata_smart_attributes
        .as_ref()
        .map(|a| a.table.as_slice())
        .unwrap_or(&[]);
    for attr in table {
        let name = attr.name.clone().unwrap_or_default();
        match attr.id {
            5  if attr.raw.value > 0 => bump(
                SmartStatus::Critical,
                format!("Reallocated_Sector_Ct={}", attr.raw.value),
                &mut status, &mut reasons,
            ),
            197 if attr.raw.value > 0 => bump(
                SmartStatus::Critical,
                format!("Current_Pending_Sector={}", attr.raw.value),
                &mut status, &mut reasons,
            ),
            198 if attr.raw.value > 0 => bump(
                SmartStatus::Critical,
                format!("Offline_Uncorrectable={}", attr.raw.value),
                &mut status, &mut reasons,
            ),
            190 | 194 if attr.raw.value > SATA_TEMP_WARN_C => bump(
                SmartStatus::Warn,
                format!("Temperature={}", attr.raw.value),
                &mut status, &mut reasons,
            ),
            _ => { let _ = name; }
        }
    }

    DriveSmart {
        device: device.to_string(),
        model: model.to_string(),
        transport: transport.to_string(),
        status,
        reasons,
        unknown_reason: None,
    }
}

/// Decode a NVMe critical_warning bitmap into human-readable bit names per the
/// NVMe spec. Empty Vec when no bits are set.
fn decode_nvme_critical_warning(bits: u32) -> Vec<String> {
    let names = [
        (0, "available_spare_below_threshold"),
        (1, "temperature_above_threshold"),
        (2, "nvm_reliability_degraded"),
        (3, "media_read_only"),
        (4, "volatile_memory_backup_failed"),
        (5, "persistent_memory_region_unreliable"),
    ];
    names.iter()
        .filter(|(bit, _)| (bits >> bit) & 1 == 1)
        .map(|(_, name)| (*name).to_string())
        .collect()
}

fn parse_nvme(p: &SmartctlJson, device: &str, model: &str, transport: &str) -> DriveSmart {
    let mut reasons: Vec<String> = Vec::new();
    let mut status = SmartStatus::Ok;
    let mut bump = |s: SmartStatus, r: String, status: &mut SmartStatus, reasons: &mut Vec<String>| {
        if (s as u8) > (*status as u8) {
            *status = s;
        }
        reasons.push(r);
    };

    if p.smart_status.passed == Some(false) {
        bump(SmartStatus::Critical, "OVERALL_HEALTH=FAILED".to_string(), &mut status, &mut reasons);
    }

    let log = match &p.nvme_smart_health_information_log {
        Some(l) => l,
        None => {
            return unknown_drive(
                device, transport,
                SmartUnknownReason::ParseError,
                "missing nvme_smart_health_information_log",
            );
        }
    };

    if let Some(cw) = log.critical_warning && cw != 0 {
        let decoded = decode_nvme_critical_warning(cw);
        let joined = if decoded.is_empty() {
            format!("critical_warning={cw}")
        } else {
            format!("critical_warning: {}", decoded.join(","))
        };
        bump(SmartStatus::Critical, joined, &mut status, &mut reasons);
    }
    if let Some(me) = log.media_errors && me > 0 {
        bump(SmartStatus::Critical, format!("media_errors={me}"), &mut status, &mut reasons);
    }
    if let (Some(spare), Some(thr)) = (log.available_spare, log.available_spare_threshold)
        && spare < thr
    {
        bump(
            SmartStatus::Critical,
            format!("available_spare={spare}% < threshold={thr}%"),
            &mut status, &mut reasons,
        );
    }
    if let Some(pct) = log.percentage_used && pct >= NVME_WEAR_WARN_PCT {
        bump(SmartStatus::Warn, format!("percentage_used={pct}%"), &mut status, &mut reasons);
    }

    // Prefer the normalised top-level Celsius. Fall back to kelvin in
    // nvme_smart_health_information_log.temperature (older smartmontools).
    let temp_c: Option<i64> = p.temperature.as_ref().and_then(|t| t.current)
        .or_else(|| log.temperature.map(|k| k - 273));
    if let Some(c) = temp_c && c > NVME_TEMP_WARN_C {
        bump(SmartStatus::Warn, format!("Temperature={c}"), &mut status, &mut reasons);
    }

    DriveSmart {
        device: device.to_string(),
        model: model.to_string(),
        transport: transport.to_string(),
        status,
        reasons,
        unknown_reason: None,
    }
}
```

- [ ] **Step 5: Run the parsing tests**

Run: `cargo test --test smart_parsing_test`
Expected: 16/16 pass.

- [ ] **Step 6: Re-run all unit tests + clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: green, clean.

- [ ] **Step 7: Commit**

```bash
git add src/smart/check.rs tests/smart_parsing_test.rs tests/fixtures/smart/
git commit -m "$(cat <<'EOF'
feat(smart): parse_smartctl_json + SATA/NVMe predictive attribute parsing

Dispatch on device.protocol (NOT lsblk transport) so SAS-attached SATA
drives go through the SATA parser. SATA rules: OVERALL_HEALTH=FAILED,
Reallocated_Sector_Ct / Current_Pending_Sector / Offline_Uncorrectable >0
are Critical; Temperature > 60°C is Warn. NVMe rules: OVERALL=FAILED,
any critical_warning bit set (decoded to human names), media_errors >0,
available_spare < available_spare_threshold are Critical;
percentage_used >= 80% or temperature > 70°C are Warn.

Temperature reads from top-level temperature.current Celsius when present;
falls back to kelvin in nvme_smart_health_information_log.temperature
(older smartmontools) and converts. Tolerant of missing optional fields
via serde defaults — unknown schema variants degrade to Unknown
(ParseError) instead of panicking.

Fixtures cover every parsing path including SAS-attached SATA and the
NVMe-temperature-only-kelvin fallback.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: check_all orchestration

**Files:**
- Modify: `src/smart/check.rs`

- [ ] **Step 1: Implement `check_all`**

In `src/smart/check.rs`, APPEND:

```rust
use crate::smart::installed::InstalledState;
use crate::smart::types::SmartHealth;

/// Wire discover → run_smartctl → parse → aggregate. Returns a fully
/// populated `SmartHealth` ready to write to the cache. Never fails — every
/// per-device error produces a `DriveSmart` with `SmartStatus::Unknown`.
///
/// `installed` is the persisted record from `pgforge smart install` (used
/// for the smartctl absolute path). If None → degrade every drive to
/// `Unknown(NoInstalledState)`.
pub async fn check_all(
    installed: Option<&InstalledState>,
    sudo_mode: SudoMode,
) -> SmartHealth {
    let now = jiff::Timestamp::now();
    let discovered = discover_disks().await;
    if discovered.is_empty() {
        let mut h = SmartHealth::unknown(SmartUnknownReason::NoDevicesFound);
        h.checked_at = now;
        return h;
    }
    let Some(state) = installed else {
        let drives = discovered.into_iter().map(|d| DriveSmart {
            device: d.path.display().to_string(),
            model: d.model,
            transport: d.transport,
            status: SmartStatus::Unknown,
            reasons: vec!["pgforge smart install has not been run".to_string()],
            unknown_reason: Some(SmartUnknownReason::NoInstalledState),
        }).collect::<Vec<_>>();
        return SmartHealth::aggregate(drives, now);
    };

    let mut drives: Vec<DriveSmart> = Vec::with_capacity(discovered.len());
    for disk in discovered {
        let result = run_smartctl(&state.smartctl_path, &disk.path, sudo_mode).await;
        let drive = match result {
            Ok(bytes) => {
                let mut d = parse_smartctl_json(&bytes);
                // Preserve the lsblk-known model if smartctl didn't echo one.
                if d.model.is_empty() { d.model = disk.model.clone(); }
                // Always overwrite device with the canonical lsblk path
                // (smartctl sometimes echoes a different alias).
                d.device = disk.path.display().to_string();
                d
            }
            Err(reason) => DriveSmart {
                device: disk.path.display().to_string(),
                model: disk.model,
                transport: disk.transport,
                status: SmartStatus::Unknown,
                reasons: vec![format!("{reason:?}")],
                unknown_reason: Some(reason),
            },
        };
        drives.push(drive);
    }
    SmartHealth::aggregate(drives, now)
}
```

- [ ] **Step 2: Build + clippy + test**

Run: `cargo build && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add src/smart/check.rs
git commit -m "$(cat <<'EOF'
feat(smart): check_all orchestration (discover → smartctl → parse → aggregate)

Wires the three lower-level primitives. Per-device errors degrade to a
Unknown drive with the specific reason (NoInstalledState, NoSudoers,
DeviceMissing, etc.) so the aggregate retains the failure mode. No
sudo_mode decision in here — caller picks Interactive or NonInteractive
based on whether they're running from a TTY or the timer.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Install renderers (pure functions, snapshot tests)

**Files:**
- Modify: `src/smart/install.rs`
- Create: `tests/smart_install_test.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/smart_install_test.rs`:

```rust
use pgforge::smart::install::{
    InstallError, render_service_unit, render_sudoers_fragment, render_timer_unit,
};
use std::path::PathBuf;

#[test]
fn sudoers_happy_path() {
    let out = render_sudoers_fragment(
        "pawel",
        std::path::Path::new("/usr/sbin/smartctl"),
        &[PathBuf::from("/dev/nvme0n1"), PathBuf::from("/dev/sda")],
    ).expect("render");
    assert!(out.contains("# pgforge SMART disk health checks"));
    assert!(out.contains("pawel ALL=(root) NOPASSWD: /usr/sbin/smartctl -H -A -j /dev/nvme0n1"));
    assert!(out.contains("pawel ALL=(root) NOPASSWD: /usr/sbin/smartctl -H -A -j /dev/sda"));
    // One rule per line — count newlines beginning with the user prefix.
    let count = out.lines().filter(|l| l.starts_with("pawel ")).count();
    assert_eq!(count, 2);
}

#[test]
fn sudoers_empty_devices_is_err() {
    let result = render_sudoers_fragment(
        "pawel",
        std::path::Path::new("/usr/sbin/smartctl"),
        &[],
    );
    assert!(matches!(result, Err(InstallError::NoDevices)));
}

#[test]
fn timer_unit_has_daily_persistent_randomized() {
    let unit = render_timer_unit();
    assert!(unit.contains("OnCalendar=daily"));
    assert!(unit.contains("RandomizedDelaySec=1h"));
    assert!(unit.contains("Persistent=true"));
    assert!(unit.contains("Unit=pgforge-smart.service"));
    assert!(unit.contains("WantedBy=timers.target"));
}

#[test]
fn service_unit_uses_absolute_pgforge_path() {
    let unit = render_service_unit(std::path::Path::new("/home/pawel/.local/bin/pgforge"));
    assert!(unit.contains("Type=oneshot"));
    assert!(unit.contains("ExecStart=/home/pawel/.local/bin/pgforge smart check --write-cache"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test smart_install_test`
Expected: FAIL with `unresolved import 'pgforge::smart::install::{...}'`.

- [ ] **Step 3: Implement the renderers**

Replace `src/smart/install.rs`:

```rust
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
```

Note: the `thiserror` crate may already be in tree — verify with `grep '^thiserror' Cargo.toml`. If not present, fall back to a hand-written impl (Display + std::error::Error). The rest of this plan assumes thiserror is available; adjust if not.

- [ ] **Step 4: Add thiserror dep if missing**

Run: `grep '^thiserror' Cargo.toml`
- If present → skip this step.
- If absent → add `thiserror = "1"` to `Cargo.toml` `[dependencies]` (alphabetical insertion), commit with the install.rs work.

- [ ] **Step 5: Run the renderer tests**

Run: `cargo test --test smart_install_test`
Expected: 4/4 pass.

- [ ] **Step 6: Lint**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/smart/install.rs tests/smart_install_test.rs Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
feat(smart): install/uninstall rendering helpers (snapshot-tested)

Pure functions for the sudoers fragment, the systemd-user timer unit,
and the systemd-user service unit. Renderers do no I/O — all the moving
sudo / systemctl / file-write parts come in Task 9. render_sudoers_fragment
refuses empty device lists (defense in depth against installs that
grant nothing). service ExecStart uses the absolute pgforge binary path
(resolved by the caller via std::env::current_exe), NOT %h/.local/bin —
no surprises when the binary moves.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Install/uninstall orchestration (tempfile → visudo → sudo install → systemd → first check)

**Files:**
- Modify: `src/smart/install.rs`

This task does NOT have a TDD test — the orchestration shells out to sudo, visudo, systemctl, and touches /etc. Pure-function pieces were covered in T8; the orchestration is exercised end-to-end by the gated E2E test in T15. Code review + the E2E gate is the safety net.

- [ ] **Step 1: Add the orchestration code**

APPEND to `src/smart/install.rs`:

```rust
use crate::smart::cache::{default_cache_path, write_cache};
use crate::smart::check::{SudoMode, check_all};
use crate::smart::installed::{InstalledState, default_installed_path, read_installed, write_installed};
use crate::smart::types::SmartHealth;
use std::path::PathBuf;

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
                "⚠ Install completed, but no disk exposes SMART data (typical on VPS \
                 without passthrough). Status will be 'SMART ?' indefinitely. Capacity \
                 monitoring (Disk N% used) continues to work. To remove: pgforge smart uninstall.".to_string()
            } else {
                format!("Overall: SMART ? ({}).", health.unknown_reason
                    .map(|r| format!("{r:?}")).unwrap_or_else(|| "no devices".to_string()))
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
```

- [ ] **Step 2: Build + clippy**

Run: `cargo build && cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 3: Re-run all tests to confirm nothing else broke**

Run: `cargo test`
Expected: all existing tests still pass; no new tests added in this task (orchestration is exercised by T15 E2E).

- [ ] **Step 4: Commit**

```bash
git add src/smart/install.rs
git commit -m "$(cat <<'EOF'
feat(smart): install_all / uninstall_all orchestration

Pipeline: which smartctl → discover_disks → render fragment → write to
tempfile → sudo visudo -c -f → sudo install -m 0440 -o root -g root
(NOT sudo tee — install is atomic AND sets mode/owner in one op so
there's never a window where /etc/sudoers.d/pgforge-smart is
wrong-mode). Then write InstalledState, write systemd-user units with
the absolute pgforge binary path (std::env::current_exe), daemon-reload,
enable --now the timer, run first check, write cache.

Uninstall is best-effort and idempotent — keeps going on any single
failure so a half-installed state can be cleaned up by re-running.

User-visible: installs /etc/sudoers.d/pgforge-smart, ~/.config/systemd
/user/pgforge-smart.{service,timer}, ~/.local/state/pgforge/
{disk-smart,smart-installed}.json. All four are removed by `pgforge smart
uninstall`.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: CLI subcommand surface (`pgforge smart {install,check,status,uninstall}`)

**Files:**
- Create: `src/commands/smart.rs`
- Modify: `src/commands/mod.rs`
- Modify: `src/cli.rs`

- [ ] **Step 1: Add the dispatcher module**

Create `src/commands/smart.rs`:

```rust
//! pgforge smart {install, check, status, uninstall}.
//!
//! `install` and `uninstall` shell out to sudo/systemctl (orchestrated in
//! `crate::smart::install`). `check` and `status` are pure read paths
//! (smartctl subprocess + cache read).

use crate::smart::cache::{STALE_AFTER_HOURS, default_cache_path, read_cache, write_cache};
use crate::smart::check::{SudoMode, check_all};
use crate::smart::install::{InstallOpts, install_all, postinstall_summary, uninstall_all};
use crate::smart::installed::{default_installed_path, read_installed};
use crate::smart::types::SmartStatus;
use anyhow::Result;

pub async fn run_install(force: bool) -> Result<()> {
    let health = install_all(InstallOpts { force }).await?;
    println!("{}", postinstall_summary(&health));
    println!("Cache: {}", default_cache_path().display());
    Ok(())
}

pub async fn run_uninstall() -> Result<()> {
    uninstall_all().await?;
    println!("Removed sudoers fragment, systemd-user timer/service, and cache.");
    Ok(())
}

pub async fn run_check(write_cache_flag: bool) -> Result<()> {
    let installed = read_installed(&default_installed_path());
    // TTY + no --write-cache → user is at the keyboard, interactive sudo OK.
    let sudo_mode = if !write_cache_flag && std::io::stdout_is_tty() {
        SudoMode::Interactive
    } else {
        SudoMode::NonInteractive
    };
    let health = check_all(installed.as_ref(), sudo_mode).await;
    println!("SMART check ({}):", health.checked_at);
    println!("{}", postinstall_summary(&health));
    if write_cache_flag {
        write_cache(&default_cache_path(), &health)?;
    }
    Ok(())
}

pub async fn run_status() -> Result<()> {
    let path = default_cache_path();
    let now = jiff::Timestamp::now();
    let health = read_cache(&path, now, STALE_AFTER_HOURS);
    println!("SMART status (cache: {})", path.display());
    let age = now.since(health.checked_at).ok()
        .map(|s| format!("{:.0}h {:.0}m ago", s.total(jiff::Unit::Hour).unwrap_or(0.0),
                         s.total(jiff::Unit::Minute).unwrap_or(0.0) % 60.0))
        .unwrap_or_else(|| "unknown".into());
    println!("  Last checked: {} ({})", health.checked_at, age);
    if health.status == SmartStatus::Unknown {
        println!("  Status: SMART ? ({})", health.unknown_reason
            .map(|r| format!("{r:?}")).unwrap_or_else(|| "no devices".into()));
    } else {
        println!("{}", postinstall_summary(&health));
    }
    Ok(())
}

// Helper because std::io::IsTerminal is a trait we need to bring into scope.
mod io_ext {
    use std::io::IsTerminal;
    pub fn stdout_is_tty() -> bool { std::io::stdout().is_terminal() }
}
pub use io_ext::stdout_is_tty;
```

Then in `src/commands/mod.rs`, add `pub mod smart;` next to the other modules.

- [ ] **Step 2: Add the `Command::Smart` variant to cli.rs**

In `src/cli.rs`, find the `enum Command` definition and add a new variant. Position it after the existing `SelfUpdate { ... }` variant (or wherever lexically appropriate):

```rust
    /// SMART hardware health monitoring (predictive disk failure detection)
    Smart {
        #[command(subcommand)]
        action: SmartAction,
    },
```

Then add the action enum (near the other action enums like `ScheduleAction` if present, otherwise at the bottom of the file's enum section):

```rust
#[derive(clap::Subcommand)]
pub enum SmartAction {
    /// Set up sudoers + systemd-user timer for daily SMART check
    Install {
        /// Overwrite existing /etc/sudoers.d/pgforge-smart even when content differs
        #[arg(long)]
        force: bool,
    },
    /// Run smartctl now and print human-readable status
    Check {
        /// Internal — used by the systemd-user timer to refresh the cache
        #[arg(long, hide = true)]
        write_cache: bool,
    },
    /// Read the cache and print (no smartctl call)
    Status,
    /// Remove sudoers, timer, and cache files
    Uninstall,
}
```

Then in `pub async fn dispatch(cli: Cli) -> Result<()>`'s main match, add a new arm (in alphabetical order with the others):

```rust
        Some(Command::Smart { action }) => match action {
            SmartAction::Install   { force }       => crate::commands::smart::run_install(force).await,
            SmartAction::Check     { write_cache } => crate::commands::smart::run_check(write_cache).await,
            SmartAction::Status                    => crate::commands::smart::run_status().await,
            SmartAction::Uninstall                 => crate::commands::smart::run_uninstall().await,
        },
```

- [ ] **Step 3: Extend the banner skip-list**

In `src/cli.rs`, find the existing `pub fn should_emit_banner_for_command(cmd: &Command) -> bool` and edit the `!matches!(...)` macro arm to add `| Command::Smart { .. }`:

```rust
    !matches!(
        cmd,
        Command::Ls
            | Command::Status { .. }
            | Command::Snapshots { .. }
            | Command::Dump { .. }
            | Command::Snapshot { due: true, .. }
            | Command::Smart { .. }
    )
```

- [ ] **Step 4: Build to verify the surface**

Run: `cargo build`
Expected: success.

- [ ] **Step 5: Smoke-test the subcommand registration**

Run: `cargo run -- smart --help`
Expected: clap prints the four subcommands (install, check, status, uninstall).

Run: `cargo run -- smart check --help`
Expected: shows `--write-cache` flag as either hidden (no mention) or visible — clap should NOT show it because of `hide = true`.

- [ ] **Step 6: Lint**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/cli.rs src/commands/mod.rs src/commands/smart.rs
git commit -m "$(cat <<'EOF'
feat(cli): pgforge smart {install, check, status, uninstall}

Four operator-facing subcommands plus the hidden --write-cache flag on
check (used by the systemd-user timer). install/uninstall delegate to
crate::smart::install; check delegates to crate::smart::check::check_all
with a TTY-aware sudo mode (interactive when at the keyboard, non-
interactive when piped or via --write-cache); status reads the cache.

Also: Command::Smart{..} added to should_emit_banner_for_command
skip-list so `pgforge smart status` doesn't print the SMART banner
above its own output.

User-visible: four new subcommands, all required to use the SMART
feature.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: CLI pre-dispatch banner (Critical only)

**Files:**
- Modify: `src/cli.rs`
- Create: `tests/cli_smart_banner_test.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/cli_smart_banner_test.rs`:

```rust
use pgforge::cli::format_smart_banner_line;
use pgforge::smart::types::{
    DriveSmart, SmartHealth, SmartStatus, SmartUnknownReason,
};

fn health(status: SmartStatus, device: &str, reasons: &[&str]) -> SmartHealth {
    SmartHealth {
        status,
        worst_device: Some(device.into()),
        worst_reasons: reasons.iter().map(|s| s.to_string()).collect(),
        unknown_reason: None,
        drives: vec![DriveSmart {
            device: device.into(),
            model: "X".into(),
            transport: "nvme".into(),
            status,
            reasons: reasons.iter().map(|s| s.to_string()).collect(),
            unknown_reason: None,
        }],
        checked_at: jiff::Timestamp::from_second(1_715_000_000).unwrap(),
    }
}

#[test]
fn critical_produces_banner_with_device_and_reasons() {
    let h = health(SmartStatus::Critical, "/dev/nvme0n1", &["Reallocated_Sector_Ct=3", "Current_Pending_Sector=1"]);
    let line = format_smart_banner_line(&h).expect("Some line for Critical");
    assert!(line.starts_with("\u{26A0}"));
    assert!(line.contains("SMART CRITICAL"));
    assert!(line.contains("/dev/nvme0n1"));
    assert!(line.contains("Reallocated_Sector_Ct=3"));
    assert!(line.contains("Current_Pending_Sector=1"));
}

#[test]
fn ok_returns_none() {
    let h = health(SmartStatus::Ok, "/dev/nvme0n1", &[]);
    assert!(format_smart_banner_line(&h).is_none());
}

#[test]
fn warn_returns_none() {
    // Warn is shown in TUI only — no CLI banner (anti-desensitization).
    let h = health(SmartStatus::Warn, "/dev/nvme0n1", &["percentage_used=82%"]);
    assert!(format_smart_banner_line(&h).is_none());
}

#[test]
fn unknown_returns_none() {
    let h = SmartHealth::unknown(SmartUnknownReason::NoCache);
    assert!(format_smart_banner_line(&h).is_none());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test cli_smart_banner_test`
Expected: FAIL with `cannot find function 'format_smart_banner_line'`.

- [ ] **Step 3: Add the formatter to cli.rs**

In `src/cli.rs`, add directly below the existing `format_banner_line` function:

```rust
/// Format a one-line warning banner for `stderr` from a SMART snapshot.
///
/// Returns `None` for any status except `Critical`. Warn is shown only in
/// the TUI (anti-desensitization — Warn in CLI would fire on every
/// "percentage_used=82%" NVMe drive, training operators to ignore it).
pub fn format_smart_banner_line(h: &crate::smart::types::SmartHealth) -> Option<String> {
    use crate::smart::types::SmartStatus;
    if h.status != SmartStatus::Critical {
        return None;
    }
    let device = h.worst_device.as_deref().unwrap_or("?");
    let reasons = h.worst_reasons.join(", ");
    Some(format!(
        "\u{26A0} SMART CRITICAL on {device}: {reasons}. Replace drive before Postgres data corruption."
    ))
}
```

- [ ] **Step 4: Wire the banner into dispatch (BEFORE the existing capacity banner)**

In `src/cli.rs`'s `pub async fn dispatch(cli: Cli) -> Result<()>`, find the existing capacity-banner block and add a new SMART block IMMEDIATELY ABOVE it:

```rust
    // Pre-dispatch SMART banner — Critical only. Reads the cache (no
    // smartctl call). Fires before the capacity banner so SMART (hardware)
    // appears above Capacity (fixable by cleanup) on stderr.
    if let Some(cmd) = &cli.command
        && should_emit_banner_for_command(cmd)
    {
        let sh = crate::smart::cache::read_cache(
            &crate::smart::cache::default_cache_path(),
            jiff::Timestamp::now(),
            crate::smart::cache::STALE_AFTER_HOURS,
        );
        if let Some(line) = format_smart_banner_line(&sh) {
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "{line}");
        }
    }

    // (existing capacity banner block follows unchanged)
```

- [ ] **Step 5: Run the banner tests + full suite**

Run: `cargo test --test cli_smart_banner_test && cargo test`
Expected: all green.

- [ ] **Step 6: Lint**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/cli.rs tests/cli_smart_banner_test.rs
git commit -m "$(cat <<'EOF'
feat(cli): pre-dispatch SMART banner on Critical

format_smart_banner_line returns Some only when status is Critical (Warn
shows only in TUI — keeping CLI banner reserved for "drop everything"
events so operators don't train themselves to ignore it). Read is from
the disk cache (no smartctl in the hot path) and lives ABOVE the
existing capacity banner block.

Gating is the existing should_emit_banner_for_command — same skip-list
(Ls, Status, Snapshots, Dump, Snapshot --due, Smart {..}).

User-visible: interactive pgforge commands now print one extra red
stderr line when the cached SMART status is Critical, before the
existing yellow/red Disk capacity banner. Pipes/cron/--due paths
unaffected.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: TUI Event + AppState + reader poller

**Files:**
- Modify: `src/tui/events.rs`
- Modify: `src/tui/app.rs`
- Modify: `src/tui/refresh.rs`

- [ ] **Step 1: Add the event variant**

In `src/tui/events.rs`, find the `pub enum Event` definition and add a new variant near the existing `DiskHealthRefreshed`:

```rust
    SmartRefreshed(crate::smart::types::SmartHealth),
```

- [ ] **Step 2: Add the AppState field + apply_event arm**

In `src/tui/app.rs`, find the `AppState` struct and add `pub smart_health: Option<crate::smart::types::SmartHealth>,` directly below the existing `pub disk_health: Option<...>` field. Initialize to `None` in any `Default` impl / `new()` constructor.

Then in the `apply_event` function, find the existing arm for `Event::DiskHealthRefreshed(h) => { self.disk_health = Some(h); }` and add directly below it:

```rust
            Event::SmartRefreshed(h) => {
                self.smart_health = Some(h);
            }
```

- [ ] **Step 3: Implement the reader poller**

In `src/tui/refresh.rs`, append the new poller (mirroring the existing `spawn_disk_health` style but with eager-first-read + 60s tick):

```rust
const SMART_READ_PERIOD: std::time::Duration = std::time::Duration::from_secs(60);

/// 60-second poller that reads the SMART cache file (no smartctl invocation,
/// no sudo, no Docker call — pure file read). Eager first read so the TUI
/// footer doesn't show `SMART ?` for a full minute on startup when there's
/// a valid cache sitting on disk.
pub fn spawn_smart_reader(
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
    cache_path: std::path::PathBuf,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Eager first read.
        let h = crate::smart::cache::read_cache(
            &cache_path,
            jiff::Timestamp::now(),
            crate::smart::cache::STALE_AFTER_HOURS,
        );
        let _ = tx.send(Event::SmartRefreshed(h));

        let mut iv = tokio::time::interval(SMART_READ_PERIOD);
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // The first iv.tick() fires immediately — consume it since we
        // already did the eager read above.
        iv.tick().await;
        loop {
            iv.tick().await;
            let h = crate::smart::cache::read_cache(
                &cache_path,
                jiff::Timestamp::now(),
                crate::smart::cache::STALE_AFTER_HOURS,
            );
            let _ = tx.send(Event::SmartRefreshed(h));
        }
    })
}
```

- [ ] **Step 4: Wire the poller into `spawn_pollers`**

Still in `src/tui/refresh.rs`, find the `pub fn spawn_pollers(...)` body and add at the bottom (after the existing disk-health spawn block):

```rust
    spawn_smart_reader(tx, crate::smart::cache::default_cache_path());
```

(`tx` is the existing `UnboundedSender<Event>` argument — pass `tx` by value here since this is the last consumer in the function; if there's any later consumer downstream, clone instead.)

- [ ] **Step 5: Build + clippy + test**

Run: `cargo build && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add src/tui/events.rs src/tui/app.rs src/tui/refresh.rs
git commit -m "$(cat <<'EOF'
feat(tui): background SMART cache reader (60s, eager first read)

spawn_smart_reader polls the SMART cache file every 60s and emits
Event::SmartRefreshed into the main TUI event channel. Pure file read
in the hot path — no smartctl, no sudo, no Docker. Eager pre-read at
poller startup keeps the footer from showing `SMART ?` for a full
minute when there's a valid cache on disk.

AppState gains smart_health: Option<SmartHealth>, fed by the new
apply_event arm.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: TUI footer 4-zone layout + format_smart_zone

**Files:**
- Modify: `src/tui/ui/bottom.rs`
- Create: `tests/tui_smart_zone_test.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/tui_smart_zone_test.rs`:

```rust
use pgforge::smart::types::{
    DriveSmart, SmartHealth, SmartStatus, SmartUnknownReason,
};
use pgforge::tui::ui::bottom::{SmartZone, format_smart_zone};
use ratatui::style::{Color, Modifier, Style};

fn h(status: SmartStatus) -> SmartHealth {
    SmartHealth {
        status,
        worst_device: Some("/dev/nvme0n1".into()),
        worst_reasons: vec![],
        unknown_reason: if status == SmartStatus::Unknown {
            Some(SmartUnknownReason::Stale)
        } else { None },
        drives: vec![DriveSmart {
            device: "/dev/nvme0n1".into(),
            model: "X".into(),
            transport: "nvme".into(),
            status,
            reasons: vec![],
            unknown_reason: None,
        }],
        checked_at: jiff::Timestamp::from_second(1_715_000_000).unwrap(),
    }
}

fn dim() -> Style { Style::default().add_modifier(Modifier::DIM) }
fn yellow() -> Style { Style::default().fg(Color::Yellow) }
fn red()    -> Style { Style::default().fg(Color::Red) }

#[test]
fn none_renders_dim_question_mark() {
    let z: SmartZone = format_smart_zone(None);
    assert_eq!(z.label, " SMART ? ");
    assert_eq!(z.style, dim());
}

#[test]
fn ok_renders_dim() {
    let z = format_smart_zone(Some(&h(SmartStatus::Ok)));
    assert_eq!(z.label, " SMART ok ");
    assert_eq!(z.style, dim());
}

#[test]
fn warn_renders_yellow() {
    let z = format_smart_zone(Some(&h(SmartStatus::Warn)));
    assert_eq!(z.label, " SMART warn ");
    assert_eq!(z.style, yellow());
}

#[test]
fn critical_renders_red() {
    let z = format_smart_zone(Some(&h(SmartStatus::Critical)));
    assert_eq!(z.label, " SMART fail ");
    assert_eq!(z.style, red());
}

#[test]
fn unknown_renders_dim_question_mark() {
    let z = format_smart_zone(Some(&h(SmartStatus::Unknown)));
    assert_eq!(z.label, " SMART ? ");
    assert_eq!(z.style, dim());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test tui_smart_zone_test`
Expected: FAIL with `cannot find function 'format_smart_zone'` / `cannot find type 'SmartZone'`.

- [ ] **Step 3: Add format_smart_zone + SmartZone + extend the 3-zone layout to 4**

In `src/tui/ui/bottom.rs`, find the existing `struct DiskZone { label: String, style: Style }` and add directly below it:

```rust
pub struct SmartZone { pub label: String, pub style: Style }
```

Then find the existing `fn format_disk_zone(...)` and add directly below it:

```rust
pub fn format_smart_zone(h: Option<&crate::smart::types::SmartHealth>) -> SmartZone {
    use crate::smart::types::SmartStatus;
    let Some(h) = h else {
        return SmartZone {
            label: " SMART ? ".to_string(),
            style: Style::default().add_modifier(Modifier::DIM),
        };
    };
    let label = format!(" SMART {} ", h.status.label());
    let style = match h.status {
        SmartStatus::Ok       => Style::default().add_modifier(Modifier::DIM),
        SmartStatus::Warn     => Style::default().fg(Color::Yellow),
        SmartStatus::Critical => Style::default().fg(Color::Red),
        SmartStatus::Unknown  => Style::default().add_modifier(Modifier::DIM),
    };
    let label = if matches!(h.status, SmartStatus::Unknown) {
        " SMART ? ".to_string()
    } else { label };
    SmartZone { label, style }
}
```

(The double assignment for `label` is intentional and clearer than a nested match — Unknown overrides the label format because we want `?` not `unknown`.)

Then in the existing `pub fn render(f: &mut Frame, area: Rect, state: &AppState)` function, find the existing 3-zone layout and replace with 4:

```rust
pub fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let version = format!(" v{} ", env!("CARGO_PKG_VERSION"));
    let disk  = format_disk_zone(state.disk_health.as_ref());
    let smart = format_smart_zone(state.smart_health.as_ref());
    let [content_area, smart_area, disk_area, version_area] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(smart.label.chars().count() as u16),
        Constraint::Length(disk.label.chars().count() as u16),
        Constraint::Length(version.chars().count() as u16),
    ])
    .areas(area);
    render_content(f, content_area, state);
    f.render_widget(
        Paragraph::new(smart.label).style(smart.style),
        smart_area,
    );
    f.render_widget(
        Paragraph::new(disk.label).style(disk.style),
        disk_area,
    );
    f.render_widget(
        Paragraph::new(version).style(Style::default().add_modifier(Modifier::DIM)),
        version_area,
    );
}
```

- [ ] **Step 4: Run the zone test**

Run: `cargo test --test tui_smart_zone_test`
Expected: 5/5 pass.

- [ ] **Step 5: Run all tests + clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: green, clean.

- [ ] **Step 6: Commit**

```bash
git add src/tui/ui/bottom.rs tests/tui_smart_zone_test.rs
git commit -m "$(cat <<'EOF'
feat(tui): 4-zone footer — SMART zone left of Disk N% used

format_smart_zone mirrors format_disk_zone (struct + per-status colour).
Labels: 'SMART ok' dim, 'SMART warn' yellow, 'SMART fail' red, 'SMART ?'
dim (used for both None and Unknown — the operator doesn't need
'unknown' as the rendered text, ? is enough).

User-visible: the bottom footer now shows two health indicators (SMART
+ Disk) side by side. No new keybinds — the SMART zone is read-only
display, consistent with feedback_tui_no_implicit_keys.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: README — new top-level "Disk health" section

**Files:**
- Modify: `README.md`

This task is documentation; no Rust tests. Verification is a careful read.

- [ ] **Step 1: Delete the existing `### Disk pressure` subsection**

In `README.md`, find `### Disk pressure` (currently inside `## How it works`) and delete the heading plus its body paragraph(s). The new top-level section below fully supersedes it.

- [ ] **Step 2: Add the new top-level `## Disk health` section**

Insert this section between `## TUI mode` (ends around line 202) and `## How it works` (starts around line 204):

````markdown
## Disk health

pgforge surfaces two independent disk-health signals in the TUI footer and as
optional pre-dispatch CLI banners. They are independent because they answer
different questions and require different remediations.

### Capacity (`Disk N% used`)

Shown continuously in the TUI footer. Read every 15s via `statvfs` across the
filesystems holding Docker volumes, the pgforge state directory, and the
pgforge dumps directory. Aggregate = worst-of.

Thresholds: < 80% Ok (dim), 80–89% Warn (yellow), ≥ 90% Critical (red). When
Warn or Critical, every interactive `pgforge` CLI command (except `ls`,
`status`, `snapshots`, `dump`, `snapshot --due`) prints a one-line stderr
banner before its output.

Remediation: free space, resize the volume, prune snapshots, or move the data
directory.

### SMART hardware monitoring

Catches a physically failing drive (reallocated sectors, NVMe wear,
available-spare exhaustion, OVERALL_HEALTH=FAILED) *before* the drive starts
refusing writes. Independent from the capacity signal — a 30%-full drive can
still be dying.

#### Setup (one-time)

```bash
# Install smartmontools if missing
sudo apt install smartmontools   # Debian/Ubuntu

# Set up pgforge SMART monitoring
pgforge smart install
```

`pgforge smart install` does:
1. Auto-discovers physical disks via `lsblk` (filters to sata/sas/nvme).
2. Writes a tightly-scoped sudoers fragment at `/etc/sudoers.d/pgforge-smart`
   that allows the pgforge user to run *only* `smartctl -H -A -j /dev/{disk}`
   as root. You'll be asked for your sudo password once.
3. Validates the fragment with `visudo -c` before declaring success (so a
   malformed write can never lock you out of sudo).
4. Installs a daily systemd-user timer (`pgforge-smart.timer`) that runs the
   check at roughly the same time each day (jittered up to 1h).
5. Runs the first check immediately and prints what it found.

If no disk on your host exposes SMART (typical on VPS without passthrough),
the install completes with a clear warning — the feature is a no-op there and
the TUI/CLI will show `SMART ?` indefinitely. Capacity monitoring still works.

#### What's monitored

For each detected disk, on each daily tick:

**SATA/SAS** — Critical if any of:
- OVERALL_HEALTH=FAILED (smart_status.passed == false)
- Reallocated_Sector_Ct > 0 (drive remapped a bad sector)
- Current_Pending_Sector > 0 (sector failed to read but not yet remapped)
- Offline_Uncorrectable > 0 (uncorrectable error during offline scan)

Warn if: temperature > 60 °C.

**NVMe** — Critical if any of:
- OVERALL_HEALTH=FAILED
- `critical_warning != 0` (NVMe spec bitmap: spare-below-threshold,
  temp-critical, reliability-degraded, read-only, backup-failed, etc.)
- `media_errors > 0`
- `available_spare < available_spare_threshold`

Warn if: `percentage_used >= 80%` (drive wear), temperature > 70 °C.

#### Where the result lives

`~/.local/state/pgforge/disk-smart.json` — a small JSON snapshot rewritten by
each daily run. The TUI reads it every 60 s (no smartctl invocation in the hot
path); the CLI banner reads it pre-dispatch.

If the cache is older than 48 h (two missed daily runs), status degrades to
`SMART ?` so you can tell when monitoring has silently stopped.

#### Status meanings

| TUI label   | Color  | What it means |
|-------------|--------|---------------|
| `SMART ok`  | dim    | All disks healthy; no flagged attributes |
| `SMART warn`| yellow | One or more disks have a Warn-level signal (high temp, high NVMe wear). Plan a replacement window. |
| `SMART fail`| red    | A Critical attribute fired (bad sectors, NVMe critical_warning, OVERALL_HEALTH=FAILED). Replace the drive ASAP. CLI also prints a stderr banner. |
| `SMART ?`   | dim    | Unknown — drive doesn't expose SMART, cache is stale, smartctl not installed, sudoers missing, or first check hasn't run yet. Run `pgforge smart status` for the specific reason. |

#### Day-to-day commands

```bash
pgforge smart check    # run smartctl now, print human-readable status
pgforge smart status   # read cache + print (no smartctl call); shows when last checked
pgforge smart uninstall  # remove sudoers + timer + cache
```

#### Troubleshooting

- **`SMART ?` and you haven't run `pgforge smart install` yet** — run it.
  The TUI shows `?` until the first check populates the cache.
- **`SMART ?` after a successful install** — run `pgforge smart status` for
  the specific reason. Common: `NoSudoers` (re-run `pgforge smart install`),
  `Stale` (timer didn't fire — `systemctl --user status pgforge-smart.timer`),
  `DeviceNotSupported` (your host doesn't expose SMART; expected on most VPS),
  `DeviceMissing` (a disk listed in the sudoers fragment was hot-unplugged;
  re-run `pgforge smart install --force`).
- **Timer didn't fire across a reboot** — `Persistent=true` means missed runs
  fire on next boot, BUT only after the timer has fired at least once
  previously. The first install runs an immediate check explicitly to avoid
  this. If a timer that has fired before still doesn't catch up after reboot:
  `systemctl --user list-timers pgforge-smart.timer` and check linger is
  enabled (`loginctl show-user $USER | grep Linger`).
- **Added a new disk** — re-run `pgforge smart install --force` to refresh
  the sudoers fragment with the new device path.
- **smartctl path is unusual on your distro** — `pgforge smart install`
  detects it via `which smartctl` at install time and persists the absolute
  path in `~/.local/state/pgforge/smart-installed.json`. Re-run install if it
  moves (e.g., after migrating between distros, or after a smartmontools
  package update that changed the binary location).
````

- [ ] **Step 3: Verify the rendered markdown**

Run: `mdcat README.md 2>/dev/null | head -300` (or any other markdown previewer). Confirm the new section appears between `## TUI mode` and `## How it works`, and the old `### Disk pressure` subsection is gone.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "$(cat <<'EOF'
docs(readme): new top-level Disk health section (capacity + SMART)

Removes the brief `### Disk pressure` subsection from `## How it works`
and replaces it with a dedicated top-level `## Disk health` section
that covers both signals: the already-shipped capacity (Disk N% used)
and the new SMART hardware monitoring. Includes setup steps, what's
monitored per transport, where the cache lives, the status-meaning
table, day-to-day commands, and a troubleshooting list (pre-install,
post-install, reboot, hot-unplug, smartctl-relocation cases).

User-visible: README finally tells you how to turn on SMART monitoring
and how to interpret the four-state indicator.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 15: Gated E2E smoke test

**Files:**
- Create: `tests/smart_e2e_test.rs`

The body of the E2E test is GATED by `PGFORGE_E2E=1`. Without the env var the test no-ops in <1s (per the project's `feedback_long_tasks` convention). The user runs the gated body manually on db-server.

- [ ] **Step 1: Write the gated test**

Create `tests/smart_e2e_test.rs`:

```rust
//! End-to-end SMART feature smoke test. Gated by `PGFORGE_E2E=1` because
//! it shells out to sudo, modifies /etc/sudoers.d, and writes systemd-user
//! units — not safe to auto-run in CI or local dev. Run manually on a
//! Linux box where you're OK setting up + tearing down the sudoers rule.
//!
//!     PGFORGE_E2E=1 cargo test --test smart_e2e_test -- --nocapture

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_check_uninstall_round_trip() {
    if std::env::var("PGFORGE_E2E").is_err() {
        eprintln!("skipping: PGFORGE_E2E not set");
        return;
    }
    // 1. install
    let health = pgforge::smart::install::install_all(
        pgforge::smart::install::InstallOpts { force: true }
    ).await.expect("install_all");
    assert!(!health.drives.is_empty(), "discovered at least one disk");

    // 2. check --write-cache (simulating timer)
    pgforge::commands::smart::run_check(true).await.expect("run_check");

    // 3. cache exists and is fresh
    let path = pgforge::smart::cache::default_cache_path();
    assert!(path.exists(), "cache file at {path:?}");
    let now = jiff::Timestamp::now();
    let h = pgforge::smart::cache::read_cache(
        &path, now, pgforge::smart::cache::STALE_AFTER_HOURS,
    );
    assert_ne!(h.unknown_reason, Some(pgforge::smart::types::SmartUnknownReason::NoCache));
    assert_ne!(h.unknown_reason, Some(pgforge::smart::types::SmartUnknownReason::Stale));

    // 4. uninstall
    pgforge::smart::install::uninstall_all().await.expect("uninstall_all");
    assert!(!path.exists(), "cache cleared by uninstall");
    assert!(!std::path::Path::new("/etc/sudoers.d/pgforge-smart").exists(),
            "sudoers fragment cleared by uninstall");
}
```

- [ ] **Step 2: Verify compile + skip path (cheap, fast)**

Run: `cargo test --test smart_e2e_test`
Expected: 1 test runs, prints "skipping: PGFORGE_E2E not set", passes in <1s.

- [ ] **Step 3: Lint**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add tests/smart_e2e_test.rs
git commit -m "$(cat <<'EOF'
test(smart): gated E2E install→check→uninstall round-trip

PGFORGE_E2E=1 cargo test --test smart_e2e_test -- --nocapture

Runs the full install pipeline on a real Linux host: writes sudoers,
installs systemd-user timer, runs first check, verifies cache, then
uninstalls everything. Default test run skips (the same convention as
the upgrade_e2e_test) so CI and local dev stay fast.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 16: Manual TUI checkpoint on db-server (NO commit — operator gate)

**Files:** none (pure verification step)

The user runs pgforge on db-server (ssh alias `db-server`) and confirms each of the following. This is the ONE remaining human-in-loop checkpoint per the user's `feedback_full_delegation_review_loop` memory.

- [ ] **Step 1: Copy the freshly built binary to db-server**

```bash
cargo build --release
scp target/release/pgforge db-server:.local/bin/pgforge
```

- [ ] **Step 2: Run `pgforge smart install` over SSH**

```bash
ssh -t db-server "pgforge smart install"
```

Expected:
- Prompts once for sudo password.
- Prints discovered disks (at least `/dev/nvme0n1`).
- Prints "Overall: SMART ok across 1 disk(s)" or similar.
- Prints cache path.

- [ ] **Step 3: Open the TUI and confirm the footer**

```bash
ssh -t db-server pgforge
```

Expected: footer shows `SMART ok  Disk N% used  v0.3.x`. SMART zone dim.

- [ ] **Step 4: Artificially flip the cache to Critical and confirm both TUI + CLI react**

In a second SSH session, edit the cache to force Critical:

```bash
ssh db-server "python3 -c '
import json, sys
p = \"/home/pawel/.local/state/pgforge/disk-smart.json\"
h = json.load(open(p))
h[\"status\"] = \"critical\"
h[\"worst_device\"] = \"/dev/nvme0n1\"
h[\"worst_reasons\"] = [\"Reallocated_Sector_Ct=99\"]
for d in h[\"drives\"]: d[\"status\"] = \"critical\"
json.dump(h, open(p, \"w\"))
'"
```

Expected within 60s: TUI footer SMART zone goes red and reads `SMART fail`. In another shell on db-server: `pgforge ls` prints a red `⚠ SMART CRITICAL on /dev/nvme0n1: Reallocated_Sector_Ct=99. Replace drive before Postgres data corruption.` line on stderr above the normal output.

- [ ] **Step 5: Restore real state by re-running the check**

```bash
ssh -t db-server "pgforge smart check --write-cache"
```

Expected: footer reverts to `SMART ok` within 60s; CLI banner stops firing.

- [ ] **Step 6: Smoke-test uninstall**

```bash
ssh -t db-server "pgforge smart uninstall"
```

Expected: prints "Removed ..." line. Re-opening the TUI shows `SMART ?` dim. `pgforge smart status` reports `NoCache` reason.

If all six steps pass, proceed to Task 17 (merge + tag). If any fails, file the bug here and iterate.

---

## Task 17: Merge feat/disk-smart → main + tag + cleanup

**Files:** none (git operations)

- [ ] **Step 1: Final full test + clippy gate on the branch**

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

Expected: green + clean.

- [ ] **Step 2: Switch to main, fast-forward merge, push**

```bash
git checkout main
git merge --ff-only feat/disk-smart
git push origin main
```

(Use `--no-ff` if the branch grew non-trivial; ask the user which they prefer for this branch's history.)

- [ ] **Step 3: Tag the release**

Determine the next semver. Capacity feature was v0.3.0 — SMART is a new operator-facing feature, so v0.4.0. Adjust if there have been hotfix patches in between.

```bash
git tag v0.4.0
git push origin v0.4.0
```

- [ ] **Step 4: Delete the feature branch**

```bash
git branch -d feat/disk-smart
git push origin --delete feat/disk-smart
```

- [ ] **Step 5: Update auto-memory**

In `/Users/pawel/.claude/projects/-Users-pawel-workspace-rust-packages-pg-forge-cli/memory/project_disk_health_feature.md`, mark SMART as SHIPPED and remove the "in progress on feat/disk-smart" status. Update the `MEMORY.md` index entry to match.

```bash
git status   # confirm clean working tree
```

Done.

---

## Self-review checklist

After completing all tasks above, before the manual TUI checkpoint:

- [ ] Every spec section (`### Disk health module`, `### Background poller`, `### TUI footer`, `### CLI banner`, `### Subcommand surface`, `### Sudoers fragment template`, `### Systemd-user units`, `### README addition`, `### Testing`) maps to at least one task.
- [ ] No "TBD", "TODO", or "implement later" in any task body.
- [ ] Every type, function, or method named in a later task was defined in an earlier task (or in the spec — types in spec §"Types" / §"Installed-state record" are defined in T1 / T2).
- [ ] Every commit message includes the Co-Authored-By trailer.
- [ ] Behaviour-change tasks (T9, T10, T11, T13, T14) call out user-visible changes in the commit body.
- [ ] E2E test is gated and does NOT auto-run in interactive sessions.
- [ ] Manual TUI checkpoint (T16) is between the last code task and the merge task — operator gate held as the user requested.
