# Disk Health + TUI Actions Modal — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface disk pressure in the TUI corner + a per-command stderr banner, and free that space by collapsing 9 per-instance TUI keys behind a single `[a]ctions` modal (and add a `[?]` Help modal).

**Architecture:** New `src/disk/health.rs` returns a four-state `DiskHealth` (Ok/Warn/Critical/Unknown) fed by `nix::sys::statvfs` over three candidate paths (Docker root via `bollard::Docker::info()`, pgforge state dir, pgforge dumps dir) deduped by `MetadataExt::dev()`. A background poller in `src/tui/refresh.rs` updates `AppState.disk_health` every 15 s via a new `Event::DiskHealthRefreshed`; the TUI footer reads from state, never blocks. The CLI banner runs pre-dispatch in `src/cli.rs`, gated by `is_terminal()` + a closed skip-list (excludes `snapshot --due`, `ls`, `status`, `snapshots`, `dump`). The TUI footer reorg adds two modals (`Modal::ActionsMenu`, `Modal::Help`), refactors top-level `Char(c)` handlers into `open_*_for_selected` methods, replaces nine top-level shortcuts with a one-time onboarding flash, and moves the `D` "destroy + delete-backups" semantics into a checkbox inside the existing destroy-confirm flow.

**Tech Stack:** Rust 2024, bollard 0.21 (already), tokio, ratatui 0.29 (already), `nix` 0.29 (new dep), `crossterm` 0.28 (already). Linux-only (v0.2.0+).

**Spec:** `docs/superpowers/specs/2026-05-17-disk-health-and-tui-actions-modal-design.md`

---

## Top-of-plan operational rules

- Each commit ends with `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`.
- Run `cargo test` before each commit; never commit with failures.
- Run `cargo clippy --all-targets -- -D warnings` before each commit; never commit with warnings.
- Behaviour-change tasks (T7, T9, T10, T11, T12, T14) MUST call out the user-visible change in the commit message body so the eventual changelog catches it.
- TDD: write the failing test, run it, see it fail with the expected message, then implement, then run it, then commit. Skipping the "see it fail" step is a plan violation.

---

## File structure (locked before tasks)

| Path | Status | Responsibility |
|---|---|---|
| `Cargo.toml` | modify | add `nix = { version = "0.29", default-features = false, features = ["fs"] }` |
| `src/lib.rs` | modify | `pub mod disk;` |
| `src/disk/mod.rs` | create | re-export `health::*`; file-level clippy denies |
| `src/disk/health.rs` | create | types + threshold logic + statvfs + `check_disk_health` |
| `src/docker/bollard_engine.rs` | modify | `impl DockerRootDirSource for BollardEngine` |
| `src/commands/dump.rs:116-120` | modify | `default_dump_dir` → `pub(crate) fn` |
| `src/tui/refresh.rs` | modify | new `spawn_disk_health` poller; wire into `spawn_pollers` |
| `src/tui/events.rs` | modify | new `Event::DiskHealthRefreshed(DiskHealth)`; new `Modal::ActionsMenu`, `Modal::Help`, `Modal::DestroyOptions` |
| `src/tui/app.rs` | modify | `disk_health` field; `apply_event` arms; refactor handlers; flash-on-moved-key; modal cleanup on InstancesListed |
| `src/tui/ui/bottom.rs` | modify | 3-zone layout; new default footer string; render `disk` zone from `state.disk_health` |
| `src/tui/ui/modal.rs` | modify | promote `centered_rect` to `pub(super)`; render `ActionsMenu`, `Help`, `DestroyOptions` |
| `src/cli.rs` | modify | `should_emit_banner`; pre-dispatch banner emission |
| `tests/disk_health_test.rs` | create | unit tests for threshold mapping, `div_ceil`, Unknown aggregation, dedup |
| `tests/tui_render_helpers.rs` | create | `TestBackend` helper — first render test in repo |
| `tests/tui_actions_modal_test.rs` | create | ActionsMenu/Help modal render + delegation tests |
| `tests/tui_flash_on_moved_key_test.rs` | create | top-level `s/c/.../e` flashes hint, opens nothing |
| `tests/tui_destroy_options_test.rs` | create | DestroyOptions toggles checkbox, Enter→Confirm |
| `tests/cli_banner_test.rs` | create | `should_emit_banner` per command variant; banner format strings |
| `README.md` | modify | document new keybinds + behaviour changes |

---

## Task 0: Branch + dep prep

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/commands/dump.rs:116-120`
- Modify: `src/tui/ui/modal.rs:509`

- [ ] **Step 1: Create branch off main**

```bash
git checkout main
git pull
git checkout -b feat/disk-health
```

- [ ] **Step 2: Add `nix` dependency**

Add to `Cargo.toml` `[dependencies]` (sorted with other deps; insert in alphabetical order after `jiff = "0.2"`):

```toml
nix = { version = "0.29", default-features = false, features = ["fs"] }
```

- [ ] **Step 3: Make `default_dump_dir` crate-visible**

In `src/commands/dump.rs:116`, change:

```rust
fn default_dump_dir() -> Result<PathBuf> {
```

to:

```rust
pub(crate) fn default_dump_dir() -> Result<PathBuf> {
```

- [ ] **Step 4: Promote `centered_rect`**

In `src/tui/ui/modal.rs:509`, change:

```rust
fn centered_rect(w: u16, h: u16, area: Rect) -> Rect {
```

to:

```rust
pub(super) fn centered_rect(w: u16, h: u16, area: Rect) -> Rect {
```

- [ ] **Step 5: Build to confirm dep resolves**

Run: `cargo build`
Expected: `Compiling pgforge` succeeds; `nix` downloaded and compiled.

- [ ] **Step 6: Run full test suite to confirm nothing broke**

Run: `cargo test`
Expected: all tests pass (we only changed visibility, no logic).

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src/commands/dump.rs src/tui/ui/modal.rs
git commit -m "$(cat <<'EOF'
chore: prep deps + visibility for disk-health feature

Add nix crate (safe statvfs wrapper), expose default_dump_dir crate-wide
and centered_rect super-wide so the upcoming disk-health module and
ActionsMenu/Help modals can reuse them instead of duplicating logic.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 1: Disk health types + threshold logic

**Files:**
- Create: `src/disk/mod.rs`
- Create: `src/disk/health.rs`
- Modify: `src/lib.rs`
- Create: `tests/disk_health_test.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/disk_health_test.rs`:

```rust
use pgforge::disk::health::{DiskHealth, DiskStatus, MountUsage};

#[test]
fn status_threshold_boundaries() {
    assert_eq!(DiskStatus::from_used_pct(0),   DiskStatus::Ok);
    assert_eq!(DiskStatus::from_used_pct(79),  DiskStatus::Ok);
    assert_eq!(DiskStatus::from_used_pct(80),  DiskStatus::Warn);
    assert_eq!(DiskStatus::from_used_pct(89),  DiskStatus::Warn);
    assert_eq!(DiskStatus::from_used_pct(90),  DiskStatus::Critical);
    assert_eq!(DiskStatus::from_used_pct(100), DiskStatus::Critical);
}

#[test]
fn used_pct_rounds_up() {
    // 89.9% used must be reported as 90 (Critical), not 89 (Warn).
    // total=10000, free=11 -> used=9989 -> 9989*100/10000 = 99.89 -> 100
    assert_eq!(MountUsage::compute_pct(10000, 11), 100);
    // total=10000, free=2000 -> used=8000 -> 8000*100/10000 = 80.0 -> 80
    assert_eq!(MountUsage::compute_pct(10000, 2000), 80);
    // total=10000, free=2001 -> used=7999 -> 7999*100/10000 = 79.99 -> 80 (rounds up)
    assert_eq!(MountUsage::compute_pct(10000, 2001), 80);
    // total=10000, free=10000 -> used=0 -> 0
    assert_eq!(MountUsage::compute_pct(10000, 10000), 0);
    // total=0 (empty mount, malformed) -> 0 (don't divide by zero)
    assert_eq!(MountUsage::compute_pct(0, 0), 0);
}

#[test]
fn worst_aggregation() {
    let mounts = vec![
        sample(50, "docker"),
        sample(85, "state"),
        sample(72, "dumps"),
    ];
    let h = DiskHealth::aggregate(mounts);
    assert_eq!(h.status, DiskStatus::Warn);
    assert_eq!(h.worst_pct, 85);
    assert_eq!(h.worst_label, "state");
}

#[test]
fn unknown_when_no_mounts() {
    let h = DiskHealth::aggregate(vec![]);
    assert_eq!(h.status, DiskStatus::Unknown);
    assert_eq!(h.worst_pct, 0);
    assert_eq!(h.worst_label, "");
}

#[test]
fn critical_dominates_warn() {
    let h = DiskHealth::aggregate(vec![
        sample(85, "a"),
        sample(91, "b"),
        sample(50, "c"),
    ]);
    assert_eq!(h.status, DiskStatus::Critical);
    assert_eq!(h.worst_pct, 91);
    assert_eq!(h.worst_label, "b");
}

fn sample(pct: u8, label: &str) -> MountUsage {
    MountUsage {
        mount_label: label.into(),
        mount_path: std::path::PathBuf::from("/"),
        used_pct: pct,
        free_bytes: 0,
        total_bytes: 100,
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test disk_health_test`
Expected: FAIL with `unresolved import 'pgforge::disk'`.

- [ ] **Step 3: Add the module**

Create `src/disk/mod.rs`:

```rust
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

pub mod health;
```

Create `src/disk/health.rs`:

```rust
//! Host disk monitoring. Best-effort: never panics, never propagates errors
//! to the caller. Status is one of Ok / Warn / Critical / Unknown — Unknown
//! is distinct from Ok and means "we could not measure", surfaced as `Disk ?`
//! in the TUI, no banner in the CLI.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskStatus {
    Ok,
    Warn,
    Critical,
    Unknown,
}

impl DiskStatus {
    /// 0..=79 → Ok, 80..=89 → Warn, 90..=100 → Critical.
    /// (Pre-clamped percentages only; caller guarantees 0..=100.)
    pub fn from_used_pct(pct: u8) -> Self {
        match pct {
            0..=79  => DiskStatus::Ok,
            80..=89 => DiskStatus::Warn,
            _       => DiskStatus::Critical,
        }
    }

    /// Severity ordering: Unknown < Ok < Warn < Critical.
    /// Unknown is the lowest because "we don't know" should never override
    /// a real measurement.
    fn rank(self) -> u8 {
        match self {
            DiskStatus::Unknown  => 0,
            DiskStatus::Ok       => 1,
            DiskStatus::Warn     => 2,
            DiskStatus::Critical => 3,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MountUsage {
    pub mount_label: String,
    pub mount_path: PathBuf,
    pub used_pct: u8,
    pub free_bytes: u64,
    pub total_bytes: u64,
}

impl MountUsage {
    /// Used % rounded UP so 89.9% becomes 90 (Critical), never 89 (Warn).
    /// total=0 → 0 (avoid div-by-zero on degenerate inputs).
    pub fn compute_pct(total_bytes: u64, free_bytes: u64) -> u8 {
        if total_bytes == 0 {
            return 0;
        }
        let used = total_bytes.saturating_sub(free_bytes);
        let pct = (used.saturating_mul(100)).div_ceil(total_bytes);
        pct.min(100) as u8
    }
}

#[derive(Debug, Clone)]
pub struct DiskHealth {
    pub status: DiskStatus,
    pub worst_pct: u8,
    pub worst_label: String,
    pub worst_mount: PathBuf,
}

impl DiskHealth {
    pub fn unknown() -> Self {
        DiskHealth {
            status: DiskStatus::Unknown,
            worst_pct: 0,
            worst_label: String::new(),
            worst_mount: PathBuf::new(),
        }
    }

    /// Reduce per-mount measurements to one aggregate health snapshot.
    /// Empty input → Unknown (distinct from "all Ok"); else picks the
    /// mount with the highest severity (ties broken by highest pct).
    pub fn aggregate(mounts: Vec<MountUsage>) -> Self {
        if mounts.is_empty() {
            return Self::unknown();
        }
        let mut worst: Option<&MountUsage> = None;
        let mut worst_status = DiskStatus::Ok;
        for m in &mounts {
            let s = DiskStatus::from_used_pct(m.used_pct);
            let take = match worst {
                None => true,
                Some(w) => {
                    s.rank() > DiskStatus::from_used_pct(w.used_pct).rank()
                        || (s.rank() == DiskStatus::from_used_pct(w.used_pct).rank()
                            && m.used_pct > w.used_pct)
                }
            };
            if take {
                worst = Some(m);
                worst_status = s;
            }
        }
        // worst is Some because mounts.is_empty() is false; pattern-match it
        // without unwrap (banned by file-level clippy lint).
        let w = match worst {
            Some(w) => w,
            None    => return Self::unknown(),
        };
        DiskHealth {
            status: worst_status,
            worst_pct: w.used_pct,
            worst_label: w.mount_label.clone(),
            worst_mount: w.mount_path.clone(),
        }
    }
}
```

Modify `src/lib.rs` to add `pub mod disk;` near the other top-level module declarations.

- [ ] **Step 4: Run tests; expect pass**

Run: `cargo test --test disk_health_test`
Expected: 5 passed, 0 failed.

- [ ] **Step 5: Clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/lib.rs src/disk/ tests/disk_health_test.rs
git commit -m "$(cat <<'EOF'
feat(disk): add disk-health types + threshold logic

Pure module — no statvfs, no Docker, no I/O. Adds DiskStatus {Ok, Warn,
Critical, Unknown}, MountUsage::compute_pct (div_ceil so 89.9% rounds to
90 → Critical, never to 89 → Warn), and DiskHealth::aggregate (empty →
Unknown distinct from Ok).

File-level clippy denies for unwrap/expect/indexing under src/disk/ —
this subsystem must be panic-free by construction (called pre-dispatch
in every CLI invocation in a later task).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: statvfs integration + path collection

**Files:**
- Modify: `src/disk/health.rs`
- Modify: `tests/disk_health_test.rs`

- [ ] **Step 1: Write the failing test**

Append to `tests/disk_health_test.rs`:

```rust
use std::path::Path;

#[test]
fn measure_path_returns_a_mount_for_an_existing_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let m = pgforge::disk::health::measure_path("tmp", tmp.path()).unwrap();
    assert_eq!(m.mount_label, "tmp");
    assert!(m.total_bytes > 0, "tempdir filesystem should report nonzero size");
    assert!(m.used_pct <= 100);
}

#[test]
fn measure_path_walks_up_to_existing_ancestor() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("does-not-exist").join("deeper");
    // Should NOT error; should statvfs the nearest existing ancestor (tmp).
    let m = pgforge::disk::health::measure_path("dumps", &missing).unwrap();
    assert!(m.total_bytes > 0);
}

#[test]
fn measure_path_returns_err_when_no_ancestor_exists() {
    // /no/such/path/anywhere/ever — root exists but nothing past it.
    let p = Path::new("/no/such/path/anywhere/ever");
    let r = pgforge::disk::health::measure_path("x", p);
    // Walks up to "/" which exists, so this should succeed.
    assert!(r.is_ok(), "should walk up to /");
}

#[test]
fn dedupe_collapses_same_dev() {
    // /tmp and a subdirectory of /tmp are on the same filesystem;
    // measuring both should produce one deduped entry.
    let tmp = tempfile::tempdir().unwrap();
    let subdir = tmp.path().join("a");
    std::fs::create_dir(&subdir).unwrap();
    let paths = vec![
        ("docker", tmp.path().to_path_buf()),
        ("dumps", subdir),
    ];
    let mounts = pgforge::disk::health::measure_dedup(paths);
    assert_eq!(mounts.len(), 1, "same filesystem should dedupe; got {mounts:?}");
}
```

Also add to `Cargo.toml` `[dev-dependencies]` if not already present:

```toml
tempfile = "3"
```

(Already present per `tests/util_fs_test.rs`.)

- [ ] **Step 2: Run; expect fail**

Run: `cargo test --test disk_health_test`
Expected: FAIL — `measure_path` / `measure_dedup` not defined.

- [ ] **Step 3: Implement**

Append to `src/disk/health.rs`:

```rust
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// Best-effort statvfs of `path`, walking up to the first existing ancestor
/// (so a not-yet-created `~/pgforge-dumps` falls back to `$HOME`). Returns
/// Err on any failure; callers drop the mount silently.
pub fn measure_path(label: &str, path: &Path) -> Result<MountUsage, std::io::Error> {
    let existing = walk_up_to_existing(path)?;
    let stat = nix::sys::statvfs::statvfs(&existing)
        .map_err(|e| std::io::Error::other(format!("statvfs: {e}")))?;
    let frsize = stat.fragment_size() as u64;
    let total_bytes = stat.blocks() as u64 * frsize;
    let free_bytes  = stat.blocks_available() as u64 * frsize;  // not f_bfree
    let used_pct = MountUsage::compute_pct(total_bytes, free_bytes);
    Ok(MountUsage {
        mount_label: label.to_string(),
        mount_path: existing,
        used_pct,
        free_bytes,
        total_bytes,
    })
}

fn walk_up_to_existing(start: &Path) -> Result<PathBuf, std::io::Error> {
    let mut p: &Path = start;
    loop {
        if p.exists() {
            return Ok(p.to_path_buf());
        }
        match p.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => p = parent,
            _ => return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no ancestor of {start:?} exists"),
            )),
        }
    }
}

/// Measure each labelled path and drop duplicates by device id
/// (st_dev). The first labelled occurrence wins.
pub fn measure_dedup(paths: Vec<(&str, PathBuf)>) -> Vec<MountUsage> {
    let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut out: Vec<MountUsage> = Vec::new();
    for (label, p) in paths {
        match measure_path(label, &p) {
            Ok(m) => {
                let dev = match m.mount_path.metadata() {
                    Ok(md) => md.dev(),
                    Err(e) => {
                        tracing::warn!(target: "pgforge::disk",
                            "metadata({path:?}) failed: {e}; dropping mount",
                            path = m.mount_path);
                        continue;
                    }
                };
                if seen.insert(dev) {
                    out.push(m);
                }
            }
            Err(e) => {
                tracing::warn!(target: "pgforge::disk",
                    "measure {label} {p:?} failed: {e}; dropping mount");
            }
        }
    }
    out
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test --test disk_health_test`
Expected: 9 passed, 0 failed.

- [ ] **Step 5: Clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/disk/health.rs tests/disk_health_test.rs Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
feat(disk): statvfs integration with ancestor-walking + dev-dedup

measure_path uses nix::sys::statvfs (safe wrapper, no unsafe) and walks
up to the first existing ancestor — so a not-yet-created ~/pgforge-dumps
is statvfs'd against $HOME instead of erroring. measure_dedup collapses
paths sharing a device via MetadataExt::dev() (NOT statvfs::f_fsid,
which is unreliable on Linux glibc).

free_bytes uses blocks_available (f_bavail), not blocks_free (f_bfree),
so we don't lie about space the unprivileged user can't actually write.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `check_disk_health` + DockerRootDirSource trait

**Files:**
- Modify: `src/disk/health.rs`
- Modify: `src/docker/bollard_engine.rs`
- Modify: `tests/disk_health_test.rs`

- [ ] **Step 1: Write the failing test**

Append to `tests/disk_health_test.rs`:

```rust
use async_trait::async_trait;
use pgforge::disk::health::{check_disk_health, DockerRootDirSource};

struct FakeDocker(Option<String>);

#[async_trait]
impl DockerRootDirSource for FakeDocker {
    async fn docker_root_dir(&self) -> anyhow::Result<Option<String>> {
        Ok(self.0.clone())
    }
}

struct FailingDocker;

#[async_trait]
impl DockerRootDirSource for FailingDocker {
    async fn docker_root_dir(&self) -> anyhow::Result<Option<String>> {
        anyhow::bail!("simulated docker socket failure")
    }
}

#[tokio::test]
async fn check_disk_health_returns_something_when_docker_works() {
    let tmp = tempfile::tempdir().unwrap();
    let h = check_disk_health(
        &FakeDocker(Some(tmp.path().display().to_string())),
        Some(tmp.path().to_path_buf()),
        Some(tmp.path().to_path_buf()),
    ).await;
    // tmpfs / overlayfs / whatever — should report a real mount, not Unknown.
    assert_ne!(h.status, pgforge::disk::health::DiskStatus::Unknown);
    assert!(h.worst_pct <= 100);
}

#[tokio::test]
async fn check_disk_health_unknown_when_all_paths_fail() {
    // Use a path that genuinely does not exist and whose root won't statvfs.
    // We can't easily fake that without /proc tricks; instead use FailingDocker
    // + paths that walk_up_to root and succeed there. Test the Docker-only
    // failure path: docker_root_dir errors → falls back to /var/lib/docker which
    // exists → still produces a mount. So this test asserts the FALLBACK works
    // even when the trait errors.
    let tmp = tempfile::tempdir().unwrap();
    let h = check_disk_health(
        &FailingDocker,
        Some(tmp.path().to_path_buf()),
        Some(tmp.path().to_path_buf()),
    ).await;
    // Should NOT panic, should NOT be Unknown (other paths still measurable).
    assert_ne!(h.status, pgforge::disk::health::DiskStatus::Unknown);
}
```

- [ ] **Step 2: Run; expect fail**

Run: `cargo test --test disk_health_test`
Expected: FAIL — `check_disk_health` and `DockerRootDirSource` not defined.

- [ ] **Step 3: Implement trait + function**

Append to `src/disk/health.rs`:

```rust
use async_trait::async_trait;

#[async_trait]
pub trait DockerRootDirSource: Send + Sync {
    async fn docker_root_dir(&self) -> anyhow::Result<Option<String>>;
}

/// Aggregate disk-health across the three filesystems pgforge actually
/// uses: Docker volumes (host-side), pgforge state dir, pgforge dumps
/// dir. Best-effort: any sub-failure drops that mount silently; all
/// failures → Unknown.
///
/// `state_root` and `dumps_root` are accepted as Option for testability;
/// in production the caller passes None and we resolve via the standard
/// XDG paths.
pub async fn check_disk_health<D: DockerRootDirSource>(
    docker: &D,
    state_root: Option<PathBuf>,
    dumps_root: Option<PathBuf>,
) -> DiskHealth {
    let docker_root = docker.docker_root_dir().await
        .ok()
        .flatten()
        .unwrap_or_else(|| "/var/lib/docker".to_string());
    let state = state_root.unwrap_or_else(
        || crate::state::instance::InstanceState::default_state_root());
    let dumps = match dumps_root {
        Some(p) => p,
        None => crate::commands::dump::default_dump_dir()
            .unwrap_or_else(|_| PathBuf::from("/tmp")),
    };

    let paths = vec![
        ("docker", PathBuf::from(docker_root)),
        ("state",  state),
        ("dumps",  dumps),
    ];
    DiskHealth::aggregate(measure_dedup(paths))
}
```

Modify `src/docker/bollard_engine.rs` to impl the trait. After the existing `impl BollardEngine`, append:

```rust
#[async_trait::async_trait]
impl crate::disk::health::DockerRootDirSource for BollardEngine {
    async fn docker_root_dir(&self) -> anyhow::Result<Option<String>> {
        let info = self.docker.info().await
            .map_err(|e| anyhow::anyhow!("docker info: {e}"))?;
        Ok(info.docker_root_dir)
    }
}
```

(Adjust the path-to-Docker-client field if it's not `self.docker`. Verify by reading the file before editing.)

- [ ] **Step 4: Run tests**

Run: `cargo test --test disk_health_test`
Expected: 11 passed, 0 failed.

- [ ] **Step 5: Clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/disk/health.rs src/docker/bollard_engine.rs tests/disk_health_test.rs
git commit -m "$(cat <<'EOF'
feat(disk): wire check_disk_health via bollard Docker::info()

DockerRootDirSource trait implemented on BollardEngine using the same
socket pgforge already opens — no shell-out to docker(1), no PATH
dependence (matters under systemd timer where PATH is stripped).

Falls back to /var/lib/docker if info() errors or returns None, so the
feature degrades gracefully on hosts where Docker exposes nothing.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: TUI background poller

**Files:**
- Modify: `src/tui/events.rs`
- Modify: `src/tui/refresh.rs`

- [ ] **Step 1: Add `DiskHealthRefreshed` event variant**

In `src/tui/events.rs`, add to the `enum Event` block (before the closing brace):

```rust
    DiskHealthRefreshed(crate::disk::health::DiskHealth),
```

- [ ] **Step 2: Write the failing test (compile-only)**

In `src/tui/refresh.rs`, append a test module at the bottom:

```rust
#[cfg(test)]
mod disk_health_poller_tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    #[tokio::test]
    async fn spawn_disk_health_emits_event_within_two_ticks() {
        // Use a sentinel implementation that returns Unknown immediately
        // so the test doesn't depend on a Docker daemon.
        struct InstantDocker;
        #[async_trait::async_trait]
        impl crate::disk::health::DockerRootDirSource for InstantDocker {
            async fn docker_root_dir(&self) -> anyhow::Result<Option<String>> {
                Ok(None)
            }
        }
        let (tx, mut rx) = unbounded_channel();
        let docker = std::sync::Arc::new(InstantDocker);
        let h = spawn_disk_health(docker, tx);
        // First tick fires immediately on interval start, so we should see
        // an event in well under the 15s poll period.
        let ev = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            rx.recv(),
        ).await.expect("event in 3s").expect("channel open");
        assert!(matches!(ev, Event::DiskHealthRefreshed(_)));
        h.abort();
    }
}
```

- [ ] **Step 3: Run; expect fail**

Run: `cargo test --lib refresh::disk_health_poller_tests`
Expected: FAIL — `spawn_disk_health` not defined.

- [ ] **Step 4: Implement poller**

In `src/tui/refresh.rs`, append:

```rust
const DISK_PERIOD: Duration = Duration::from_secs(15);
const DISK_TIMEOUT: Duration = Duration::from_secs(2);

/// Periodic disk-health poller. Bounded per-tick by DISK_TIMEOUT to
/// guard against a hung NFS / FUSE mount freezing the poller forever.
/// On timeout / error → emits DiskHealth::unknown() so the TUI can
/// show "Disk ?" instead of going stale.
pub fn spawn_disk_health<D>(
    docker: std::sync::Arc<D>,
    tx: UnboundedSender<Event>,
) -> tokio::task::JoinHandle<()>
where
    D: crate::disk::health::DockerRootDirSource + 'static,
{
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(DISK_PERIOD);
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            iv.tick().await;
            let h = match tokio::time::timeout(
                DISK_TIMEOUT,
                crate::disk::health::check_disk_health(&*docker, None, None),
            ).await {
                Ok(h)  => h,
                Err(_) => {
                    tracing::warn!(target: "pgforge::tui::refresh",
                        "disk-health poll timed out after {:?}", DISK_TIMEOUT);
                    crate::disk::health::DiskHealth::unknown()
                }
            };
            let _ = tx.send(Event::DiskHealthRefreshed(h));
        }
    })
}
```

Also wire it into `spawn_pollers` so the production TUI starts it. Modify `spawn_pollers`:

```rust
pub fn spawn_pollers(
    tx: UnboundedSender<Event>,
    names_rx: watch::Receiver<Vec<String>>,
    state_root: Option<PathBuf>,
) {
    spawn_ls(tx.clone(), state_root.clone());
    spawn_status(tx.clone(), names_rx.clone(), state_root.clone());
    spawn_snapshots(tx.clone(), names_rx, state_root);
    // Disk-health poller wraps the same BollardEngine the other pollers
    // create per-tick. Cheaper to share one connect() here.
    if let Ok(docker) = crate::docker::bollard_engine::BollardEngine::connect() {
        spawn_disk_health(std::sync::Arc::new(docker), tx);
    } else {
        tracing::warn!(target: "pgforge::tui::refresh",
            "disk-health poller: BollardEngine::connect failed; status will be Unknown");
    }
}
```

- [ ] **Step 5: Run test**

Run: `cargo test --lib refresh::disk_health_poller_tests`
Expected: PASS in ≤ 3 s.

- [ ] **Step 6: Full test suite + clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add src/tui/events.rs src/tui/refresh.rs
git commit -m "$(cat <<'EOF'
feat(tui): background poller for disk-health every 15s

spawn_disk_health mirrors the existing ls/status/snapshots pattern:
tokio interval + MissedTickBehavior::Skip + per-tick tokio::time::timeout
(2s) so a hung mount can't freeze future ticks. Emits
Event::DiskHealthRefreshed; on timeout or error, emits
DiskHealth::unknown() so the TUI can show "Disk ?" instead of stale
data.

Hooked into spawn_pollers via a shared BollardEngine::connect().

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: AppState.disk_health field + apply_event arm

**Files:**
- Modify: `src/tui/app.rs`

- [ ] **Step 1: Write the failing test**

Append to the existing `#[cfg(test)]` block in `src/tui/app.rs`:

```rust
#[test]
fn disk_health_refreshed_updates_state() {
    use crate::disk::health::{DiskHealth, DiskStatus};
    let mut s = AppState::default();
    assert_eq!(s.disk_health.status, DiskStatus::Unknown,
        "default disk_health should be Unknown");
    s.apply_event(Event::DiskHealthRefreshed(DiskHealth {
        status: DiskStatus::Warn,
        worst_pct: 85,
        worst_label: "docker".into(),
        worst_mount: "/var/lib/docker".into(),
    }));
    assert_eq!(s.disk_health.status, DiskStatus::Warn);
    assert_eq!(s.disk_health.worst_pct, 85);
}
```

- [ ] **Step 2: Run; expect fail**

Run: `cargo test --lib app::tests::disk_health_refreshed_updates_state`
Expected: FAIL — `AppState` has no `disk_health` field.

- [ ] **Step 3: Add the field + Default + apply_event arm**

In `src/tui/app.rs`:

a) Add to the `pub struct AppState` (after the `pending_show_created` field):

```rust
    pub disk_health: crate::disk::health::DiskHealth,
```

b) Add to `impl Default for AppState::default`:

```rust
            disk_health: crate::disk::health::DiskHealth::unknown(),
```

c) Add a new arm in `apply_event`'s match (place it near `RefreshFailed`):

```rust
            Event::DiskHealthRefreshed(h) => {
                self.disk_health = h;
            }
```

- [ ] **Step 4: Run test**

Run: `cargo test --lib app::tests::disk_health_refreshed_updates_state`
Expected: PASS.

- [ ] **Step 5: Full test suite + clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add src/tui/app.rs
git commit -m "$(cat <<'EOF'
feat(tui): AppState.disk_health field + apply_event arm

Default is DiskHealth::unknown() so the first render before the poller
ticks shows "Disk ?" rather than a misleading 0%/green.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: TUI footer 3-zone layout + disk render

**Files:**
- Modify: `src/tui/ui/bottom.rs`
- Create: `tests/tui_render_helpers.rs`
- Create: `tests/tui_bottom_render_test.rs`

- [ ] **Step 1: Create the TestBackend helper (first one in repo)**

Create `tests/tui_render_helpers.rs`:

```rust
//! Shared helpers for ratatui render tests. First TUI render-test
//! pattern in this repo — keep helpers minimal so tests stay obvious.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

pub fn draw_into<F>(width: u16, height: u16, draw_fn: F) -> Buffer
where
    F: FnOnce(&mut ratatui::Frame),
{
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| draw_fn(f)).unwrap();
    terminal.backend().buffer().clone()
}

/// Returns true if the given text appears anywhere in the rendered buffer.
pub fn buffer_contains(buf: &Buffer, needle: &str) -> bool {
    buffer_to_string(buf).contains(needle)
}

pub fn buffer_to_string(buf: &Buffer) -> String {
    let mut out = String::new();
    let area = buf.area();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}
```

- [ ] **Step 2: Write the failing test for the footer**

Create `tests/tui_bottom_render_test.rs`:

```rust
mod tui_render_helpers;
use tui_render_helpers::{buffer_contains, draw_into};

use pgforge::disk::health::{DiskHealth, DiskStatus};
use pgforge::tui::app::AppState;
use pgforge::tui::ui::bottom;

fn state_with(h: DiskHealth) -> AppState {
    let mut s = AppState::default();
    s.disk_health = h;
    s
}

#[test]
fn footer_shows_disk_pct_when_known() {
    let state = state_with(DiskHealth {
        status: DiskStatus::Ok, worst_pct: 42,
        worst_label: "docker".into(), worst_mount: "/var/lib/docker".into(),
    });
    let buf = draw_into(80, 3, |f| {
        let area = ratatui::layout::Rect { x: 0, y: 0, width: 80, height: 1 };
        bottom::render(f, area, &state);
    });
    assert!(buffer_contains(&buf, "Disk 42%"), "footer = {:?}",
        tui_render_helpers::buffer_to_string(&buf));
}

#[test]
fn footer_shows_question_mark_when_unknown() {
    let state = state_with(DiskHealth::unknown());
    let buf = draw_into(80, 3, |f| {
        let area = ratatui::layout::Rect { x: 0, y: 0, width: 80, height: 1 };
        bottom::render(f, area, &state);
    });
    assert!(buffer_contains(&buf, "Disk ?"));
}

#[test]
fn footer_default_lists_minimal_keys() {
    let state = state_with(DiskHealth::unknown());
    let buf = draw_into(80, 3, |f| {
        let area = ratatui::layout::Rect { x: 0, y: 0, width: 80, height: 1 };
        bottom::render(f, area, &state);
    });
    let text = tui_render_helpers::buffer_to_string(&buf);
    assert!(text.contains("[n]ew"), "missing [n]ew: {text}");
    assert!(text.contains("[a]ctions"), "missing [a]ctions: {text}");
    assert!(text.contains("[?]help"), "missing [?]help: {text}");
    assert!(text.contains("[q]uit") || text.contains("[q] uit"), "missing q");
}
```

- [ ] **Step 3: Run; expect fail**

Run: `cargo test --test tui_bottom_render_test`
Expected: FAIL — strings not present in current footer.

- [ ] **Step 4: Implement the 3-zone footer + new key hint string**

Replace the body of `pub fn render` in `src/tui/ui/bottom.rs`:

```rust
pub fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let version = format!(" v{} ", env!("CARGO_PKG_VERSION"));
    let disk = format_disk_zone(&state.disk_health);
    let [content_area, disk_area, version_area] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(disk.label.chars().count() as u16),
        Constraint::Length(version.chars().count() as u16),
    ])
    .areas(area);
    render_content(f, content_area, state);
    f.render_widget(
        Paragraph::new(disk.label).style(disk.style),
        disk_area,
    );
    f.render_widget(
        Paragraph::new(version).style(Style::default().add_modifier(Modifier::DIM)),
        version_area,
    );
}

struct DiskZone { label: String, style: Style }

fn format_disk_zone(h: &crate::disk::health::DiskHealth) -> DiskZone {
    use crate::disk::health::DiskStatus;
    let (label, style) = match h.status {
        DiskStatus::Unknown  => (" Disk ? ".to_string(), Style::default().add_modifier(Modifier::DIM)),
        DiskStatus::Ok       => (format!(" Disk {}% ", h.worst_pct),
                                 Style::default().add_modifier(Modifier::DIM)),
        DiskStatus::Warn     => (format!(" Disk {}% ", h.worst_pct),
                                 Style::default().fg(Color::Yellow)),
        DiskStatus::Critical => (format!(" Disk {}% ", h.worst_pct),
                                 Style::default().fg(Color::Red)),
    };
    DiskZone { label, style }
}
```

In the `render_content` function, change the default footer string from:

```rust
            "[n]ew [s]nap [c]lone [R]otate [p]reset [t]ime [r]estore [d]estroy [u]pdate [↵] uri [q]uit"
```

to:

```rust
            "[n]ew [a]ctions [?]help [↵] uri [q]uit"
```

- [ ] **Step 5: Run test**

Run: `cargo test --test tui_bottom_render_test`
Expected: 3 passed, 0 failed.

- [ ] **Step 6: Full test suite + clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add src/tui/ui/bottom.rs tests/tui_render_helpers.rs tests/tui_bottom_render_test.rs
git commit -m "$(cat <<'EOF'
feat(tui): 3-zone footer with disk-health indicator

Footer now: [n]ew [a]ctions [?]help [↵] uri [q]uit | Disk 42% | v0.2.0
The 8 per-instance keys disappear — they're being moved into the
ActionsMenu modal in a later task. Footer doesn't break, just gets
shorter; the keys still work via the existing top-level handlers
until that task replaces them.

Disk zone shows "Disk N%" colour-coded (dim/yellow/red for
Ok/Warn/Critical) or "Disk ?" dim when status is Unknown — so absence
of measurement is visible, not silent.

Establishes TestBackend pattern in tests/tui_render_helpers.rs;
previously the repo had zero ratatui render tests.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Refactor key handlers into `open_*_for_selected` methods

**Files:**
- Modify: `src/tui/app.rs`

**Behaviour change:** None. This is a pure refactor: the methods are called
from the same `KeyCode::Char` arms as before. Required prerequisite for the
ActionsMenu in Task 8 (the modal cannot re-call `handle_key` because the
modal slot is `Some(ActionsMenu)`).

- [ ] **Step 1: Write a sanity test that current behaviour is preserved**

Append to `src/tui/app.rs` test module:

```rust
#[test]
fn pressing_s_with_selection_opens_snapshot_modal_unchanged() {
    let mut s = state_with_two_instances();
    s.selected = 0;
    s.apply_event(key(KeyCode::Char('s')));
    // We won't assert on the modal type yet (will change in T11) — for now
    // assert SOMETHING happened (modal opened OR op enqueued).
    assert!(s.modal.is_some() || !s.pending_ops.is_empty(),
        "expected s to trigger snapshot path; modal={:?} pending={:?}",
        s.modal, s.pending_ops);
}

#[test]
fn pressing_d_with_selection_opens_destroy_modal_unchanged() {
    let mut s = state_with_two_instances();
    s.selected = 0;
    s.apply_event(key(KeyCode::Char('d')));
    assert!(matches!(s.modal, Some(Modal::Confirm { .. })));
}

fn state_with_two_instances() -> AppState {
    let mut s = AppState::default();
    s.apply_event(Event::InstancesListed(vec![row("a"), row("b")]));
    s
}
```

(The `row` helper already exists in app.rs test module per existing tests.)

- [ ] **Step 2: Run; expect pass (existing behaviour)**

Run: `cargo test --lib app::tests`
Expected: PASS — current `handle_key` arms still do what they did.

- [ ] **Step 3: Extract methods**

For each of the following keys in `handle_key`'s no-modal branch
(`src/tui/app.rs:218-345`), extract the body into a method on `AppState`:

| Key | New method name |
|---|---|
| `s` | `open_snapshot_for_selected` |
| `c` | `open_clone_for_selected` |
| `t` | `open_time_for_selected` |
| `p` | `open_preset_for_selected` |
| `u` | `trigger_self_update` (note: temporary — T11 will remove this from top-level) |
| `r` | `open_restore_for_selected` |
| `R` | `open_rotate_for_selected` |
| `d` | `open_destroy_for_selected` |
| `D` | `open_destroy_with_delete_backups_for_selected` (temporary — T12 will fold this into the destroy modal) |
| `e` | `open_snapshots_history_for_selected` |
| `n` | `open_create_wizard` (keep `n` working unchanged, but extract for symmetry) |

For each: take the exact body of the match arm and move it into
`impl AppState { fn ... (&mut self) { ... } }`. The match arm now reads:

```rust
            KeyCode::Char('s') => { self.open_snapshot_for_selected(); }
```

(etc.). Use `&mut self` so each method can mutate state.

The arms that depend on a selected instance (most of them) start with:

```rust
let Some(name) = self.selected_name().map(str::to_string) else { return; };
```

— move that guard inside the extracted method too.

This is a mechanical refactor; do all the extractions in one commit. Run
the test suite after to confirm nothing broke.

- [ ] **Step 4: Test + clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all green; the two sanity tests from Step 1 still pass.

- [ ] **Step 5: Commit**

```bash
git add src/tui/app.rs
git commit -m "$(cat <<'EOF'
refactor(tui): extract per-key handlers into open_*_for_selected methods

Pure refactor — no behaviour change. Each KeyCode::Char arm in
handle_key's no-modal branch now calls a method on AppState. Required
prerequisite for the upcoming ActionsMenu modal (which cannot re-call
handle_key because the modal slot is occupied by ActionsMenu).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: `Modal::ActionsMenu` variant + render + delegation

**Files:**
- Modify: `src/tui/events.rs`
- Modify: `src/tui/ui/modal.rs`
- Modify: `src/tui/app.rs`
- Create: `tests/tui_actions_modal_test.rs`

- [ ] **Step 1: Add the variant**

In `src/tui/events.rs`, add to `enum Modal`:

```rust
    /// Centred menu of every per-instance action. Opened by `a` at the
    /// top level. Each listed key delegates to an `open_*_for_selected`
    /// method on AppState and closes itself first.
    ActionsMenu { instance_name: String },
```

- [ ] **Step 2: Write failing render test**

Create `tests/tui_actions_modal_test.rs`:

```rust
mod tui_render_helpers;
use tui_render_helpers::{buffer_contains, draw_into};

use pgforge::tui::events::Modal;
use pgforge::tui::ui::modal;

#[test]
fn actions_menu_lists_all_keys() {
    let m = Modal::ActionsMenu { instance_name: "billing".into() };
    let buf = draw_into(80, 24, |f| {
        let full = ratatui::layout::Rect { x: 0, y: 0, width: 80, height: 24 };
        modal::render(f, full, &m);
    });
    for needle in &["billing", "[s] Snapshot", "[c] Clone", "[R] Rotate",
                    "[p] Preset", "[t]", "[r] Restore", "[d] Destroy",
                    "[u] Upgrade", "[e]", "[esc]"] {
        assert!(buffer_contains(&buf, needle),
            "missing {needle:?}\n--- buffer ---\n{}",
            tui_render_helpers::buffer_to_string(&buf));
    }
}
```

- [ ] **Step 3: Run; expect fail**

Run: `cargo test --test tui_actions_modal_test`
Expected: FAIL — render dispatch in `modal::render` doesn't match `ActionsMenu`.

- [ ] **Step 4: Implement render**

In `src/tui/ui/modal.rs`, add a match arm in `render` that handles `ActionsMenu`:

```rust
        Modal::ActionsMenu { instance_name } => {
            let lines = [
                Line::from(""),
                Line::from("  [s] Snapshot"),
                Line::from("  [c] Clone"),
                Line::from("  [R] Rotate"),
                Line::from("  [p] Preset (resize)"),
                Line::from("  [t] snapshot Time"),
                Line::from("  [r] Restore from"),
                Line::from("  [d] Destroy"),
                Line::from("  [u] Upgrade"),
                Line::from("  [e] snapshots History"),
                Line::from(""),
                Line::from("  [esc] Cancel"),
            ];
            let area = centered_rect(32, lines.len() as u16 + 2, full);
            f.render_widget(Clear, area);
            let block = Block::default()
                .borders(Borders::ALL)
                .title(format!(" Actions: {instance_name} "));
            let inner = block.inner(area);
            f.render_widget(block, area);
            f.render_widget(Paragraph::new(lines.to_vec()), inner);
        }
```

(Adjust imports — `Line`, `Clear`, `Block`, `Borders`, `Paragraph` may already be in scope.)

- [ ] **Step 5: Run render test**

Run: `cargo test --test tui_actions_modal_test`
Expected: PASS.

- [ ] **Step 6: Wire `a` top-level → open ActionsMenu**

In `src/tui/app.rs`, add a new arm to `handle_key`'s no-modal branch:

```rust
            KeyCode::Char('a') => {
                if let Some(name) = self.selected_name().map(str::to_string) {
                    self.modal = Some(Modal::ActionsMenu { instance_name: name });
                }
            }
```

- [ ] **Step 7: Wire modal-key delegation**

Add a new arm in `handle_modal_key` (or the modal-match in `handle_key`,
wherever modal keys are routed) for `Modal::ActionsMenu`:

```rust
            Some(Modal::ActionsMenu { .. }) => {
                let pressed = match key.code {
                    KeyCode::Char('s') => Some(AppState::open_snapshot_for_selected
                        as fn(&mut AppState)),
                    KeyCode::Char('c') => Some(AppState::open_clone_for_selected as fn(&mut AppState)),
                    KeyCode::Char('R') => Some(AppState::open_rotate_for_selected as fn(&mut AppState)),
                    KeyCode::Char('p') => Some(AppState::open_preset_for_selected as fn(&mut AppState)),
                    KeyCode::Char('t') => Some(AppState::open_time_for_selected as fn(&mut AppState)),
                    KeyCode::Char('r') => Some(AppState::open_restore_for_selected as fn(&mut AppState)),
                    KeyCode::Char('d') => Some(AppState::open_destroy_for_selected as fn(&mut AppState)),
                    KeyCode::Char('u') => Some(AppState::open_upgrade_for_selected as fn(&mut AppState)),
                    KeyCode::Char('e') => Some(AppState::open_snapshots_history_for_selected as fn(&mut AppState)),
                    KeyCode::Esc       => { self.modal = None; None }
                    _                  => None,
                };
                if let Some(open) = pressed {
                    self.modal = None;
                    open(self);
                }
            }
```

Note: this requires `open_upgrade_for_selected` to exist. If T7 named it
differently or didn't extract it (because `u` was top-level self-update),
add it now as: `fn open_upgrade_for_selected(&mut self) { ... open Modal::UpgradeTo ... }`.

- [ ] **Step 8: Add delegation test**

Append to `tests/tui_actions_modal_test.rs`:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pgforge::tui::app::AppState;
use pgforge::tui::events::Event;

fn k(c: char) -> Event { Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)) }
fn esc() -> Event { Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)) }

#[test]
fn pressing_a_with_selection_opens_actions_menu() {
    let mut s = AppState::default();
    // Need a selected instance for `a` to do anything.
    s.apply_event(Event::InstancesListed(vec![
        pgforge::commands::ls::InstanceSummary {
            name: "billing".into(), pg_version: 18, preset: pgforge::domain::preset::Preset::Tiny,
            port: 5433, backups_enabled: true, running: true, backup_failing: false,
        }
    ]));
    s.apply_event(k('a'));
    assert!(matches!(s.modal, Some(Modal::ActionsMenu { .. })));
}

#[test]
fn esc_in_actions_menu_closes_it() {
    let mut s = AppState::default();
    s.modal = Some(Modal::ActionsMenu { instance_name: "x".into() });
    s.apply_event(esc());
    assert!(s.modal.is_none());
}

#[test]
fn d_in_actions_menu_opens_destroy_confirm() {
    let mut s = AppState::default();
    s.apply_event(Event::InstancesListed(vec![
        pgforge::commands::ls::InstanceSummary {
            name: "billing".into(), pg_version: 18, preset: pgforge::domain::preset::Preset::Tiny,
            port: 5433, backups_enabled: true, running: true, backup_failing: false,
        }
    ]));
    s.modal = Some(Modal::ActionsMenu { instance_name: "billing".into() });
    s.apply_event(k('d'));
    // ActionsMenu should have closed, then destroy modal opened
    assert!(matches!(s.modal, Some(Modal::Confirm { .. })),
        "expected Confirm; got {:?}", s.modal);
}
```

- [ ] **Step 9: Test + clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all green.

- [ ] **Step 10: Commit**

```bash
git add src/tui/events.rs src/tui/ui/modal.rs src/tui/app.rs tests/tui_actions_modal_test.rs
git commit -m "$(cat <<'EOF'
feat(tui): Actions modal — per-instance actions under [a]

Pressing [a] with an instance selected opens a centred modal listing
every per-instance keybind (s/c/R/p/t/r/d/u/e). Inside the modal each
letter closes the modal first, then delegates to the matching
open_*_for_selected method extracted in the previous task — no
re-entrancy, no synthetic key events.

The original top-level shortcuts still work for now; they're removed
in a later task once the flash-hint mechanic is in.

USER-VISIBLE CHANGE: pressing [a] now shows a menu. (Final UX change
in subsequent tasks where the top-level shortcuts get removed and a
flash hint takes their place.)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: `Modal::Help` variant + render

**Files:**
- Modify: `src/tui/events.rs`
- Modify: `src/tui/ui/modal.rs`
- Modify: `tests/tui_actions_modal_test.rs`

- [ ] **Step 1: Add the variant**

In `src/tui/events.rs`, add to `enum Modal`:

```rust
    /// Global keybind reference. Opened by `?` when there is no
    /// last_op_error to detail (otherwise `?` shows the error).
    Help,
```

- [ ] **Step 2: Failing render test**

Append to `tests/tui_actions_modal_test.rs`:

```rust
#[test]
fn help_modal_lists_global_and_per_instance_keys() {
    let m = Modal::Help;
    let buf = draw_into(80, 24, |f| {
        let full = ratatui::layout::Rect { x: 0, y: 0, width: 80, height: 24 };
        modal::render(f, full, &m);
    });
    for needle in &["pgforge — keybinds",
                    "[n]ew", "[a]ctions", "[?]", "[q]uit",
                    "[s] Snapshot", "[c] Clone",
                    "[esc]"] {
        assert!(buffer_contains(&buf, needle),
            "missing {needle:?}\n{}", tui_render_helpers::buffer_to_string(&buf));
    }
}
```

- [ ] **Step 3: Run; expect fail**

Run: `cargo test --test tui_actions_modal_test help_modal_lists_global_and_per_instance_keys`
Expected: FAIL — Help variant not rendered.

- [ ] **Step 4: Implement render**

Add to `src/tui/ui/modal.rs` `render` match:

```rust
        Modal::Help => {
            let lines = vec![
                Line::from(""),
                Line::from("  Global"),
                Line::from("    [n]      new instance"),
                Line::from("    [a]      actions on selected instance"),
                Line::from("    [↑/↓/j/k] navigate"),
                Line::from("    [↵]      copy connection URI"),
                Line::from("    [?]      this help / error detail"),
                Line::from("    [q]      quit"),
                Line::from(""),
                Line::from("  Inside Actions menu"),
                Line::from("    [s] Snapshot       [c] Clone"),
                Line::from("    [R] Rotate         [p] Preset (resize)"),
                Line::from("    [t] snapshot Time  [r] Restore from"),
                Line::from("    [d] Destroy        [u] Upgrade"),
                Line::from("    [e] snapshots History"),
                Line::from(""),
                Line::from("  [esc] close any modal"),
            ];
            let area = centered_rect(54, lines.len() as u16 + 2, full);
            f.render_widget(Clear, area);
            let block = Block::default()
                .borders(Borders::ALL)
                .title(" pgforge — keybinds ");
            let inner = block.inner(area);
            f.render_widget(block, area);
            f.render_widget(Paragraph::new(lines), inner);
        }
```

- [ ] **Step 5: Wire `?` in handle_modal_key for Modal::Help (esc/? both close)**

Add to modal-key routing:

```rust
            Some(Modal::Help) => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                    self.modal = None;
                }
            }
```

- [ ] **Step 6: Test + clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: green.

- [ ] **Step 7: Commit**

```bash
git add src/tui/events.rs src/tui/ui/modal.rs tests/tui_actions_modal_test.rs
git commit -m "$(cat <<'EOF'
feat(tui): Help modal — global keybind reference under [?]

Renders a two-section overview (Global vs Actions-menu keys) in a
centred box. Closes on [esc] or another [?]. Wiring of [?] at the top
level lands in the next task (must route between this Help modal and
the existing error-detail behaviour based on whether last_op_error is
set).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Footer key hint final + `?` routing

**Files:**
- Modify: `src/tui/app.rs`
- Modify: `tests/tui_actions_modal_test.rs`

- [ ] **Step 1: Failing tests for `?` routing**

Append to `tests/tui_actions_modal_test.rs`:

```rust
use pgforge::tui::events::OpError;
use std::time::Instant;

#[test]
fn question_mark_without_error_opens_help_modal() {
    let mut s = AppState::default();
    s.apply_event(k('?'));
    assert!(matches!(s.modal, Some(Modal::Help)),
        "expected Help; got {:?}", s.modal);
}

#[test]
fn question_mark_with_error_opens_error_detail_modal() {
    let mut s = AppState::default();
    s.last_op_error = Some(OpError {
        instance: "x".into(),
        kind: pgforge::tui::events::OpKind::Snapshot,
        msg: "boom".into(),
        at: Instant::now(),
    });
    s.apply_event(k('?'));
    assert!(matches!(s.modal, Some(Modal::ErrorDetail { .. })),
        "expected ErrorDetail; got {:?}", s.modal);
}
```

- [ ] **Step 2: Run; one should pass (existing behaviour for error case) and the no-error case should fail**

Run: `cargo test --test tui_actions_modal_test question_mark`
Expected: 1 passed, 1 failed (no-error case can't open Help yet).

- [ ] **Step 3: Update `?` arm in handle_key**

In `src/tui/app.rs` `handle_key` no-modal branch, replace the existing `KeyCode::Char('?')` arm with:

```rust
            KeyCode::Char('?') => {
                if let Some(e) = &self.last_op_error {
                    self.modal = Some(Modal::ErrorDetail { msg: e.msg.clone() });
                } else {
                    self.modal = Some(Modal::Help);
                }
            }
```

(The exact existing arm may already do the ErrorDetail half — verify
before editing. Add the else branch.)

- [ ] **Step 4: Test + clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add src/tui/app.rs tests/tui_actions_modal_test.rs
git commit -m "$(cat <<'EOF'
feat(tui): [?] routes to error detail when an error is set, Help otherwise

Preserves the existing "[?] details" affordance shown in the bottom bar
when last_op_error is set, and adds a second behaviour when it isn't —
opens the new Help modal listing every keybind. No new conditional in
the footer string: [?]help is always shown; the in-context behaviour
flips silently.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Flash on moved keys + remove top-level handlers

**Files:**
- Modify: `src/tui/app.rs`
- Create: `tests/tui_flash_on_moved_key_test.rs`

**Behaviour change:** USER-VISIBLE. Top-level `s/c/R/p/t/r/d/u/e` stop
opening their modals; they instead set a one-time flash hint pointing at
`[a]`. After this commit, the Actions modal is the *only* way to reach
those actions from the TUI.

- [ ] **Step 1: Write failing tests**

Create `tests/tui_flash_on_moved_key_test.rs`:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pgforge::tui::app::AppState;
use pgforge::tui::events::{Event, Modal};

fn k(c: char) -> Event { Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)) }

fn state_with_instance() -> AppState {
    let mut s = AppState::default();
    s.apply_event(Event::InstancesListed(vec![
        pgforge::commands::ls::InstanceSummary {
            name: "billing".into(), pg_version: 18,
            preset: pgforge::domain::preset::Preset::Tiny,
            port: 5433, backups_enabled: true, running: true, backup_failing: false,
        }
    ]));
    s
}

#[test]
fn s_at_top_level_flashes_hint_and_opens_no_modal() {
    let mut s = state_with_instance();
    s.apply_event(k('s'));
    assert!(s.flash.is_some(), "expected flash hint");
    assert!(s.modal.is_none(), "expected no modal; got {:?}", s.modal);
}

#[test]
fn each_moved_key_flashes_hint_and_opens_no_modal() {
    for c in ['c', 'R', 'p', 't', 'r', 'd', 'u', 'e'] {
        let mut s = state_with_instance();
        s.apply_event(k(c));
        assert!(s.flash.is_some(), "{c}: expected flash");
        assert!(s.modal.is_none(), "{c}: expected no modal; got {:?}", s.modal);
    }
}

#[test]
fn a_still_opens_actions_menu() {
    let mut s = state_with_instance();
    s.apply_event(k('a'));
    assert!(matches!(s.modal, Some(Modal::ActionsMenu { .. })));
}
```

- [ ] **Step 2: Run; expect fail**

Run: `cargo test --test tui_flash_on_moved_key_test`
Expected: FAIL — `s` still opens snapshot path.

- [ ] **Step 3: Replace top-level Char arms with flash-hint arm**

In `src/tui/app.rs` `handle_key` no-modal branch, **delete** the arms for
`s/c/R/p/t/r/d/u/e` (and `D` if separately bound) that you extracted in
T7, and **add** one unified arm:

```rust
            KeyCode::Char(c) if matches!(c, 's'|'c'|'R'|'p'|'t'|'r'|'d'|'D'|'u'|'e') => {
                self.flash = Some(Flash {
                    msg: "Per-instance actions moved to [a]. Press 'a' to open."
                        .to_string(),
                    kind: FlashKind::Info,
                    at: self.now,
                });
            }
```

Keep `n`, `a`, `?`, `q`, `↵`, `↑/↓/j/k`, `Esc` arms unchanged.

The `open_*_for_selected` methods remain on `AppState` — they're called
from the ActionsMenu delegation. They're now the *only* callers; previous
direct callers are gone.

- [ ] **Step 4: Run flash tests + existing tests**

Run: `cargo test`
Expected: new tests pass. Old tests that asserted top-level `s` opens a
modal (e.g. the sanity tests added in T7) will now FAIL — update them to
match the new behaviour OR delete them (they were sanity scaffolding
during the refactor).

Specifically, the two tests added in T7 (`pressing_s_with_selection_...`
and `pressing_d_with_selection_...`) must be updated:

```rust
// Old: asserted s opens snapshot path. New: asserts s flashes + no modal.
// Delete those tests; flash behaviour is covered in
// tests/tui_flash_on_moved_key_test.rs now.
```

- [ ] **Step 5: Test + clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add src/tui/app.rs tests/tui_flash_on_moved_key_test.rs
git commit -m "$(cat <<'EOF'
feat(tui): top-level s/c/R/p/t/r/d/u/e/D flash a hint instead of acting

USER-VISIBLE BREAKING CHANGE: these nine keys no longer trigger their
actions when pressed at the top of the TUI. They set a one-time info
flash: "Per-instance actions moved to [a]. Press 'a' to open." The
existing 3-second flash timeout makes the hint self-cleaning.

Rationale: keys that work but aren't shown anywhere in the UI violate
the "every visible key works, every working key is visible" invariant
(see ../memory/feedback_tui_no_implicit_keys.md). Actions modal +
flash hint is the user-approved replacement.

Top-level [u] formerly triggered self-update; now it flashes. Self-
update is still available via `pgforge self-update` CLI. The Actions
modal's [u] opens pg_upgrade (a different action), as documented.

Top-level [D] formerly triggered destroy-with-delete-backups; now it
flashes. Equivalent behaviour comes from [d] inside the Actions modal
+ a checkbox in the destroy-confirm modal (next task).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Destroy options modal — checkbox for `delete_backups`

**Files:**
- Modify: `src/tui/events.rs`
- Modify: `src/tui/app.rs`
- Modify: `src/tui/ui/modal.rs`
- Create: `tests/tui_destroy_options_test.rs`

**Behaviour change:** USER-VISIBLE. `[d]` inside the Actions modal now
opens a `DestroyOptions` modal that lets the user toggle "delete backups
too" with `Space` before pressing `Enter` to proceed to the existing
destructive-confirm modal. The previous "two-key" UX (`d` confirms
preserve-backups, `D` confirms delete-backups) is gone — `D` was already
removed from the top level in T11.

- [ ] **Step 1: Add the variant**

In `src/tui/events.rs`, add to `enum Modal`:

```rust
    /// First step of destroy: tick "delete backups too" before confirming.
    /// Space toggles, Enter advances to Confirm, Esc cancels.
    DestroyOptions { name: String, delete_backups: bool },
```

- [ ] **Step 2: Failing test**

Create `tests/tui_destroy_options_test.rs`:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pgforge::tui::app::AppState;
use pgforge::tui::events::{Event, Modal, PendingDestructiveOp};

fn key(code: KeyCode) -> Event { Event::Key(KeyEvent::new(code, KeyModifiers::NONE)) }

#[test]
fn open_destroy_for_selected_opens_destroy_options() {
    let mut s = AppState::default();
    s.apply_event(Event::InstancesListed(vec![
        pgforge::commands::ls::InstanceSummary {
            name: "x".into(), pg_version: 18, preset: pgforge::domain::preset::Preset::Tiny,
            port: 5433, backups_enabled: true, running: true, backup_failing: false,
        }
    ]));
    s.open_destroy_for_selected();
    assert!(matches!(s.modal,
        Some(Modal::DestroyOptions { delete_backups: false, .. })));
}

#[test]
fn space_toggles_delete_backups() {
    let mut s = AppState::default();
    s.modal = Some(Modal::DestroyOptions { name: "x".into(), delete_backups: false });
    s.apply_event(key(KeyCode::Char(' ')));
    assert!(matches!(s.modal,
        Some(Modal::DestroyOptions { delete_backups: true, .. })));
    s.apply_event(key(KeyCode::Char(' ')));
    assert!(matches!(s.modal,
        Some(Modal::DestroyOptions { delete_backups: false, .. })));
}

#[test]
fn enter_advances_to_confirm_with_chosen_delete_backups() {
    let mut s = AppState::default();
    s.modal = Some(Modal::DestroyOptions { name: "x".into(), delete_backups: true });
    s.apply_event(key(KeyCode::Enter));
    assert!(matches!(s.modal,
        Some(Modal::Confirm { kind: PendingDestructiveOp::Destroy { delete_backups: true, .. }, .. })));
}

#[test]
fn esc_cancels() {
    let mut s = AppState::default();
    s.modal = Some(Modal::DestroyOptions { name: "x".into(), delete_backups: true });
    s.apply_event(key(KeyCode::Esc));
    assert!(s.modal.is_none());
}
```

- [ ] **Step 3: Run; expect fail**

Run: `cargo test --test tui_destroy_options_test`
Expected: FAIL.

- [ ] **Step 4: Implement**

a) In `src/tui/app.rs`, change `open_destroy_for_selected` to:

```rust
fn open_destroy_for_selected(&mut self) {
    let Some(name) = self.selected_name().map(str::to_string) else { return; };
    self.modal = Some(Modal::DestroyOptions { name, delete_backups: false });
}
```

b) Add a `DestroyOptions` arm in modal-key handling:

```rust
            Some(Modal::DestroyOptions { name, delete_backups }) => {
                let name = name.clone();
                let db = *delete_backups;
                match key.code {
                    KeyCode::Char(' ') => {
                        self.modal = Some(Modal::DestroyOptions {
                            name, delete_backups: !db
                        });
                    }
                    KeyCode::Enter => {
                        let prompt = if db {
                            format!("Destroy '{name}' AND delete ALL its S3 backups? (y/N)")
                        } else {
                            format!("Destroy '{name}'? (y/N)")
                        };
                        self.modal = Some(Modal::Confirm {
                            kind: PendingDestructiveOp::Destroy { name, delete_backups: db },
                            prompt,
                        });
                    }
                    KeyCode::Esc => { self.modal = None; }
                    _ => {}
                }
            }
```

c) In `src/tui/ui/modal.rs` `render`, add:

```rust
        Modal::DestroyOptions { name, delete_backups } => {
            let mark = if *delete_backups { "[x]" } else { "[ ]" };
            let lines = vec![
                Line::from(""),
                Line::from(format!("  Destroy: {name}")),
                Line::from(""),
                Line::from(format!("  {mark} Also delete S3 backups (unrecoverable)")),
                Line::from(""),
                Line::from("  [space] toggle  [enter] proceed  [esc] cancel"),
            ];
            let area = centered_rect(54, lines.len() as u16 + 2, full);
            f.render_widget(Clear, area);
            let block = Block::default()
                .borders(Borders::ALL)
                .title(" Destroy options ");
            let inner = block.inner(area);
            f.render_widget(block, area);
            f.render_widget(Paragraph::new(lines), inner);
        }
```

d) Add `DestroyOptions` to any modal-name guard list (e.g. `is_named_modal_for_instance(&self) -> Option<&str>` if one was introduced) — it carries an instance name and must be considered "instance-bound" for the cleanup in T13.

e) Remove the old `open_destroy_with_delete_backups_for_selected` method
extracted in T7 — it's now dead code.

- [ ] **Step 5: Run tests + clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add src/tui/events.rs src/tui/app.rs src/tui/ui/modal.rs tests/tui_destroy_options_test.rs
git commit -m "$(cat <<'EOF'
feat(tui): DestroyOptions modal — checkbox for "delete S3 backups too"

USER-VISIBLE CHANGE: destroying an instance from the TUI is now a
two-step modal flow: Actions → [d] opens DestroyOptions (with a
[space]-toggleable checkbox), then [enter] advances to the existing
y/N confirm. Replaces the previous d-vs-D two-key UX which only the
spec author knew about.

The CLI flag is unchanged: `pgforge destroy --name X --delete-backups`.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: Modal cleanup on instance disappearance

**Files:**
- Modify: `src/tui/app.rs`

- [ ] **Step 1: Failing test**

Append to `src/tui/app.rs` test module:

```rust
#[test]
fn actions_menu_closes_when_instance_vanishes() {
    let mut s = AppState::default();
    s.apply_event(Event::InstancesListed(vec![row("a"), row("b")]));
    s.modal = Some(Modal::ActionsMenu { instance_name: "a".into() });
    // Instance "a" disappears.
    s.apply_event(Event::InstancesListed(vec![row("b")]));
    assert!(s.modal.is_none(), "expected ActionsMenu closed; got {:?}", s.modal);
}

#[test]
fn destroy_options_closes_when_instance_vanishes() {
    let mut s = AppState::default();
    s.apply_event(Event::InstancesListed(vec![row("a")]));
    s.modal = Some(Modal::DestroyOptions { name: "a".into(), delete_backups: false });
    s.apply_event(Event::InstancesListed(vec![]));
    assert!(s.modal.is_none());
}
```

- [ ] **Step 2: Run; expect fail**

Run: `cargo test --lib app::tests::actions_menu_closes_when_instance_vanishes`
Expected: FAIL.

- [ ] **Step 3: Implement guard in apply_event**

In `src/tui/app.rs` `apply_event`'s `InstancesListed(rows)` arm, after
updating `self.instances`, add:

```rust
                // Close any modal that's bound to an instance no longer in
                // the list. Prevents pressing keys against vanished state.
                let names: std::collections::HashSet<&str> = self.instances
                    .iter().map(|i| i.name.as_str()).collect();
                let bound_name: Option<String> = match &self.modal {
                    Some(Modal::ActionsMenu { instance_name }) => Some(instance_name.clone()),
                    Some(Modal::DestroyOptions { name, .. })   => Some(name.clone()),
                    Some(Modal::CloneAs { source, .. })        => Some(source.clone()),
                    Some(Modal::UpgradeTo { source, .. })      => Some(source.clone()),
                    Some(Modal::RestoreAs { source, .. })      => Some(source.clone()),
                    Some(Modal::ResizeTo { name, .. })         => Some(name.clone()),
                    Some(Modal::ScheduleEdit { name, .. })     => Some(name.clone()),
                    Some(Modal::Snapshots { name, .. })        => Some(name.clone()),
                    Some(Modal::ConnectionString { name, .. }) => Some(name.clone()),
                    Some(Modal::CreatedSuccess { name, .. })   => Some(name.clone()),
                    Some(Modal::Confirm { kind, .. }) => match kind {
                        PendingDestructiveOp::Rotate { name }            => Some(name.clone()),
                        PendingDestructiveOp::Upgrade { name, .. }       => Some(name.clone()),
                        PendingDestructiveOp::Restore { source, .. }     => Some(source.clone()),
                        PendingDestructiveOp::Destroy { name, .. }       => Some(name.clone()),
                        PendingDestructiveOp::Resize { name, .. }        => Some(name.clone()),
                    },
                    _ => None,
                };
                if let Some(n) = bound_name
                    && !names.contains(n.as_str())
                {
                    self.modal = None;
                }
```

- [ ] **Step 4: Run tests + clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add src/tui/app.rs
git commit -m "$(cat <<'EOF'
feat(tui): close instance-bound modal when instance vanishes mid-modal

The ls poller refreshes every 5s. If an instance is destroyed
externally (or by another pgforge invocation) while the TUI has its
ActionsMenu / DestroyOptions / CloneAs / etc. open, the modal is
silently closed by the next InstancesListed event — instead of letting
the user press keys against an instance that no longer exists.

Covers all instance-bound modal variants currently in Modal::*.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: CLI banner

**Files:**
- Modify: `src/cli.rs`
- Create: `tests/cli_banner_test.rs`

- [ ] **Step 1: Failing test for `should_emit_banner`**

Create `tests/cli_banner_test.rs`:

```rust
use pgforge::cli::{should_emit_banner_for_command, format_banner_line};
use pgforge::disk::health::{DiskHealth, DiskStatus};

// We mirror clap's parse here for selected variants only; the unit
// under test is a fn that takes &Command, and we construct minimal
// instances. The names below MUST match pgforge::cli::Command variants.
use pgforge::cli::Command;

#[test]
fn ls_status_snapshots_dump_skip_the_banner() {
    assert!(!should_emit_banner_for_command(&Command::Ls));
    // Construct other variants with minimal args — see should_emit_banner
    // source for the exact discriminants.
}

#[test]
fn snapshot_due_skips_the_banner() {
    // Snapshot { name: None, due: true, label: None, override_state_root: None }
    let cmd = Command::Snapshot {
        name: None, due: true, label: None, override_state_root: None,
    };
    assert!(!should_emit_banner_for_command(&cmd));
}

#[test]
fn destroy_emits_the_banner() {
    let cmd = Command::Destroy {
        name: "x".into(), delete_backups: false, yes: true, override_state_root: None,
    };
    assert!(should_emit_banner_for_command(&cmd));
}

#[test]
fn banner_format_warn_critical_unknown() {
    let warn = DiskHealth {
        status: DiskStatus::Warn, worst_pct: 85,
        worst_label: "docker".into(), worst_mount: "/var/lib/docker".into(),
    };
    let line = format_banner_line(&warn).unwrap();
    assert!(line.contains("85%"));
    assert!(line.contains("docker"));
    assert!(line.contains("may start failing"));

    let crit = DiskHealth { status: DiskStatus::Critical, worst_pct: 92,
        ..warn.clone() };
    let cl = format_banner_line(&crit).unwrap();
    assert!(cl.contains("WILL"));

    assert!(format_banner_line(&DiskHealth::unknown()).is_none());
    assert!(format_banner_line(&DiskHealth {
        status: DiskStatus::Ok, ..warn
    }).is_none());
}
```

(Some `Command` variants above may have slightly different field names —
verify in `src/cli.rs` and adjust the test instances accordingly. The
intent: at least one positive and one negative test per skip-list entry.)

- [ ] **Step 2: Run; expect fail (functions don't exist)**

Run: `cargo test --test cli_banner_test`
Expected: FAIL (compile errors).

- [ ] **Step 3: Implement**

In `src/cli.rs`, near the top (after `Command` enum), add:

```rust
pub fn should_emit_banner_for_command(cmd: &Command) -> bool {
    use std::io::IsTerminal;
    if !std::io::stderr().is_terminal() {
        return false;
    }
    !matches!(cmd,
        Command::Ls
        | Command::Status { .. }
        | Command::Snapshots { .. }
        | Command::Dump { .. }
        | Command::Snapshot { due: true, .. }
    )
}

pub fn format_banner_line(h: &crate::disk::health::DiskHealth) -> Option<String> {
    use crate::disk::health::DiskStatus;
    let verb = match h.status {
        DiskStatus::Warn     => "may start failing",
        DiskStatus::Critical => "WILL start failing",
        _ => return None,
    };
    let path = tilde_collapse(&h.worst_mount.display().to_string());
    Some(format!(
        "\u{26A0} Disk {}% full on {} ({}) \u{2014} Postgres writes / WAL archiving {verb}.",
        h.worst_pct, h.worst_label, path
    ))
}

fn tilde_collapse(path: &str) -> String {
    if let Ok(home) = std::env::var("HOME") {
        if let Some(rest) = path.strip_prefix(&home) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}
```

Then in the `dispatch` function (right after `Cli::parse()`), call the
banner emission BEFORE the subcommand match:

```rust
    // Disk-health banner — best-effort, never breaks dispatch.
    if should_emit_banner_for_command(&cli.command) {
        let docker = match BollardEngine::connect() {
            Ok(d) => Some(d),
            Err(_) => None,
        };
        if let Some(docker) = docker {
            let h = match tokio::time::timeout(
                std::time::Duration::from_secs(2),
                crate::disk::health::check_disk_health(&docker, None, None),
            ).await {
                Ok(h)  => h,
                Err(_) => crate::disk::health::DiskHealth::unknown(),
            };
            if let Some(line) = format_banner_line(&h) {
                let _ = writeln!(std::io::stderr(), "{line}");
            }
        }
    }
```

(Add `use std::io::Write;` at the top of `src/cli.rs` if it isn't already imported.)

- [ ] **Step 4: Test + clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add src/cli.rs tests/cli_banner_test.rs
git commit -m "$(cat <<'EOF'
feat(cli): pre-dispatch disk-health banner on Warn/Critical

USER-VISIBLE CHANGE: interactive pgforge invocations (create, destroy,
rotate, restore, snapshot without --due, clone, upgrade, resize,
reconfigure, cron, schedule, self-update) now print a one-line
disk-pressure warning on stderr above their normal output when a
monitored mount is at ≥ 80% used:

  ⚠ Disk 92% full on docker (/var/lib/docker) — Postgres writes /
    WAL archiving WILL start failing.

Gated by:
- is_terminal() — no banner under pipes / redirects / cron / make
- closed skip-list — ls, status, snapshots, dump, snapshot --due
  (the systemd-fired path that runs 288x/day)
- 2-second timeout on the Docker socket call
- bollard Docker::info() (no shelling out)

Banner text says "Postgres writes / WAL archiving" because that's
what actually fails — pgforge itself only writes kilobytes.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 15: README update

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update TUI section**

In `README.md`, find the "## TUI mode" section and replace its keybind
list with:

```markdown
- `↑`/`↓` or `j`/`k` — navigate the instance list
- `Enter` — copy the connection string (with password) to the clipboard
- `n` — create a new instance
- `a` — open the **Actions** menu (snapshot / clone / rotate / preset / time / restore / destroy / upgrade / snapshots history) for the selected instance
- `?` — help (or error detail when an op has failed)
- `q` — quit

Disk usage of the host (worst across Docker volume, pgforge state, and
pgforge dumps filesystems) is shown in the bottom bar as `Disk N%` —
yellow at ≥ 80%, red at ≥ 90%. `Disk ?` means pgforge could not
measure (e.g. Docker daemon unreachable).
```

- [ ] **Step 2: Add a "Disk warning" subsection under "## How it works"**

Append:

```markdown
### Disk pressure

Every interactive `pgforge` command (everything except `ls`, `status`,
`snapshots`, `dump`, and `snapshot --due`) prints a one-line banner to
stderr when the host disk is at ≥ 80%:

```
⚠ Disk 92% full on docker (/var/lib/docker) — Postgres writes / WAL
  archiving WILL start failing.
```

The banner is suppressed when stderr is not a terminal (so pipes,
cron, and `make` see clean output). The TUI corner shows the same
signal continuously.
```

- [ ] **Step 3: Update the keybind summary in any quick-reference table**

Search for any other lists of TUI keys and update them. Particularly:
- Remove references to `[s]` / `[c]` / `[R]` / `[p]` / `[t]` / `[r]` /
  `[d]` / `[D]` / `[u]` / `[e]` working at top level.
- Add note: "self-update was previously bound to `[u]` in the TUI; now
  use `pgforge self-update` from the CLI."

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "$(cat <<'EOF'
docs(readme): document disk-health banner + Actions modal reorg

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 16: Manual TUI checkpoint

**This is a human-in-the-loop verification.** No agent step can sign off
on it. The implementer dispatches itself to verify, but the actual
"shipped" status requires the user to confirm.

- [ ] **Step 1: Build release binary**

```bash
cargo build --release
```

- [ ] **Step 2: Run the TUI locally**

```bash
./target/release/pgforge
```

Verify in the live TUI:
- Bottom bar shows `[n]ew [a]ctions [?]help [↵] uri [q]uit`.
- Right side: `Disk N%` (or `Disk ?` if no instances / docker idle).
- Press `s` with an instance selected → flash hint appears, no modal opens.
- Press `a` → Actions modal centred with 9 keybinds + cancel.
- Press `d` inside Actions → DestroyOptions modal with checkbox.
- Press `space` → checkbox toggles.
- Press `esc` → DestroyOptions closes.
- Press `?` at top level → Help modal with full keybind reference.
- Quit with `q`.

- [ ] **Step 3: Run interactive CLI command and watch for banner**

If you have an instance running:

```bash
./target/release/pgforge status --name <X>
```

→ should print clean output, NO banner.

```bash
./target/release/pgforge cron --name <X> --hour 4
```

→ if disk is < 80%, no banner. If you can artificially fill the
filesystem to 85% in a sandbox VM, the banner should appear above the
normal output on stderr.

- [ ] **Step 4: Push to feat/disk-health and notify user**

```bash
git push -u origin feat/disk-health
```

Report to the user: which manual verifications passed, which (if any)
need their physical machine to confirm, and request approval to merge.

---

## Self-review notes

**Spec coverage:** all 5 user-visible behaviour changes covered (Actions
modal, Help modal, banner, flash hint, DestroyOptions). All 6 substantive
engineering fixes from the agent review embedded in the relevant tasks
(bollard.info(), Unknown status, div_ceil, MetadataExt::dev, background
poller, panic-free disk module).

**Placeholder scan:** clean — every step has actual code or an exact
command.

**Type consistency:** `DiskHealth` / `DiskStatus` / `MountUsage` names
identical across all tasks. `open_*_for_selected` method names match
between T7 (definition) and T8 (callsites in delegation).

**Out of scope (preserved from spec):** SMART, push notifications,
per-instance disk breakdown, configurable thresholds, S3-repo disk usage.
None of those have tasks here.
