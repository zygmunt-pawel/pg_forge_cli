# Disk Health + TUI Actions Modal — Design Spec (v2)

**Date:** 2026-05-17
**Status:** Brainstormed + agent-reviewed, awaiting plan
**Reviewer findings integrated:** 2026-05-17 (review at `/tmp/disk-health-spec-review-1778970961.md`)

## Goal

Give the operator an early warning when the host disk is filling up, and free
TUI footer space by folding per-instance actions behind a single key.

## Motivation

pgforge runs unattended for weeks at a time on a Mac mini or db-server. A full
disk silently breaks Postgres writes, pgbackrest pushes, and snapshots — the
operator usually finds out from a failing app, not from pgforge. We need to
surface disk pressure before it bites.

The natural place to show this is the TUI footer, but the footer is already
saturated with ten per-instance shortcut keys. Compacting those behind a
single `[a]ctions` entry cleans up the bar and frees space for the new
status. The reorg is the only way to fit the new signal without making the
bar wrap or shrink.

## Scope

**In scope:**
- Disk usage (`statvfs`) monitoring of the filesystems holding Docker volumes,
  pgforge state, and pgforge dumps.
- Three-tier thresholds (Ok/Warn/Critical) plus an `Unknown` state for
  "could not measure" so absence of data is visible, not silent.
- Footer indicator in the TUI, fed by a background poller (never a sync
  statvfs from the render loop).
- One-line banner on stderr before every interactive CLI command when state
  is bad.
- TUI footer reorganization: per-instance actions hidden behind a modal opened
  by `[a]`. The previous shortcuts (`s`, `c`, `R`, `p`, `t`, `r`, `d`, `u`,
  `e`) are removed from the top-level event router — they only work inside
  the Actions modal.
- One-time onboarding flash when a removed shortcut is pressed at top level.
- New `[?]` Help modal listing every keybind (closes the "invisible key"
  hole the reorg widens).
- Audit-and-close the existing top-level Char-key set (enumerated below) so
  no key remains that does something without being shown anywhere.

**Out of scope (explicit, queued separately):**
- SMART / drive failure detection.
- Per-instance disk usage breakdown.
- User-configurable thresholds in `config.toml`.
- Push notifications.
- Disk usage of S3/R2 repo.

## Design

### Disk health module

A new module `src/disk/health.rs` exposes:

```rust
pub enum DiskStatus { Ok, Warn, Critical, Unknown }

pub struct MountUsage {
    pub mount_label: String,   // human-friendly label, e.g. "docker", "dumps"
    pub mount_path: PathBuf,   // canonical path of the source dir (may be home-relative)
    pub used_pct: u8,          // 0..=100, rounded UP
    pub free_bytes: u64,
    pub total_bytes: u64,
}

pub struct DiskHealth {
    pub status: DiskStatus,    // worst across mounts, or Unknown if no measurement succeeded
    pub worst_pct: u8,         // worst used_pct (0 if Unknown)
    pub worst_label: String,   // label of worst mount (empty if Unknown)
    pub worst_mount: PathBuf,  // path of worst mount (empty if Unknown)
}

#[async_trait::async_trait]
pub trait DockerRootDirSource {
    async fn docker_root_dir(&self) -> anyhow::Result<Option<String>>;
}

pub async fn check_disk_health<D: DockerRootDirSource>(docker: &D) -> DiskHealth;
```

`DockerRootDirSource` is implemented on the existing `BollardEngine` by
calling `Docker::info().await?.docker_root_dir` (bollard 0.21 exposes
`SystemInfo::docker_root_dir`). We never shell out to `docker info` — same
process, same socket, typed API.

The function:

1. Collects three candidate paths with labels:
   - `("docker", docker.docker_root_dir().await.unwrap_or(None).unwrap_or_else(|| "/var/lib/docker".into()))`
   - `("state", InstanceState::default_state_root())`
   - `("dumps", crate::commands::dump::default_dump_dir()?)` — make
     `default_dump_dir` `pub(crate)`. If the dumps dir does not yet exist,
     walk up to the first ancestor that does (never statvfs a non-existent
     path).
2. For each path:
   a. `path.metadata().map(|m| m.dev())` (via `std::os::unix::fs::MetadataExt`)
      → `u64` device ID for dedup.
   b. `nix::sys::statvfs::statvfs(&path)` → `Statvfs` struct.
3. Dedupe by `dev` only — `statvfs.f_fsid` is unreliable on Linux glibc.
4. For surviving mounts: compute `used_pct = ((total - free) * 100).div_ceil(total)`
   clamped to `0..=100`, so any non-zero use rounds UP (Critical reported
   as early as possible, never late).
   - `total = f_blocks * f_frsize`, `free = f_bavail * f_frsize` (NOT
     `f_bfree`, which counts reserved blocks the user can't write).
5. Status per mount: `< 80 → Ok`, `80..=89 → Warn`, `>= 90 → Critical`.
6. Aggregate: if no mount measured successfully → `status: Unknown`. Else
   `status = max(per-mount status)`, `worst_pct/label/mount` from worst.

`nix::sys::statvfs` (safe wrapper) is chosen over raw `libc::statvfs` to
avoid an `unsafe` block + `CString` plumbing; binary-size delta is
negligible (the crate is already in our transitive deps). Add `nix = {
version = "0.29", default-features = false, features = ["fs"] }` to
`[dependencies]`.

Errors are NOT propagated out of `check_disk_health`. Per-mount errors drop
that mount (`tracing::warn`); all-mounts-fail returns `DiskStatus::Unknown`
with empty worst fields. The function never panics: forbidden inside
`src/disk/` are `unwrap`, `expect`, `panic!`, raw indexing, `as u8` on
values not pre-clamped. Enforced by file-level `#![deny(clippy::unwrap_used,
clippy::expect_used, clippy::indexing_slicing)]`.

### Background poller (TUI only)

`src/tui/refresh.rs` already runs the periodic instance/status pollers via
`tokio::spawn + interval`. Add a sibling task:

```rust
pub fn spawn_disk_health(
    docker: Arc<BollardEngine>,
    tx: UnboundedSender<Event>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut t = tokio::time::interval(Duration::from_secs(15));
        loop {
            t.tick().await;
            let h = tokio::time::timeout(
                Duration::from_secs(2),
                check_disk_health(&*docker),
            ).await.unwrap_or(DiskHealth::unknown());
            let _ = tx.send(Event::DiskHealthRefreshed(h));
        }
    })
}
```

The 2-second per-tick timeout guards against a hung NFS / FUSE mount
freezing the poller (and therefore future ticks). New variant
`Event::DiskHealthRefreshed(DiskHealth)`; `AppState.disk_health: DiskHealth`
updated in `apply_event`. The render path reads `state.disk_health` — pure,
sync, never blocks.

### TUI footer (always-on indicator)

`src/tui/ui/bottom.rs` splits the bottom line into three zones:

```
| content_area (left)       | disk_area | version_area |
| [n]ew [a]ctions [?]help q | Disk 45%  | v0.2.0       |
```

`disk_area` width = `format!(" Disk {pct}% ", pct = …)` length (or
`" Disk ?  "` for `Unknown`). Color:

| Status   | Colour                  |
|----------|-------------------------|
| Ok       | `DIM` (grey)            |
| Warn     | yellow                  |
| Critical | red                     |
| Unknown  | dim grey, text `Disk ?` |

### TUI footer (key hints)

Default footer changes from

```
[n]ew [s]nap [c]lone [R]otate [p]reset [t]ime [r]estore [d]estroy [u]pdate [↵] uri [q]uit
```

to

```
[n]ew [a]ctions [?]help [↵] uri [q]uit
```

Conditional footers (error, in-progress, flash) keep their existing keys.

### Actions modal

New `Modal::ActionsMenu { instance_name: String }`. Triggered by `a` from the
top-level event router when an instance is selected. Centered, ~32 cols ×
13 rows. Reuses `centered_rect` — promote it from `fn` (private) to
`pub(super) fn` in `src/tui/ui/modal.rs`.

Rendered content:

```
┌─ Actions: billing ──────┐
│                         │
│  [s] Snapshot           │
│  [c] Clone              │
│  [R] Rotate             │
│  [p] Preset (resize)    │
│  [t] snapshot Time      │
│  [r] Restore from       │
│  [d] Destroy            │
│  [u] Upgrade            │
│  [e] snapshots History  │
│                         │
│  [esc] Cancel           │
└─────────────────────────┘
```

Event dispatch refactor (required, not optional): each top-level handler in
`handle_key`'s no-modal branch becomes a private method on `AppState`:

```rust
fn open_snapshot_for_selected(&mut self);
fn open_clone_for_selected(&mut self);
fn open_rotate_for_selected(&mut self);
fn open_preset_for_selected(&mut self);
fn open_time_for_selected(&mut self);
fn open_restore_for_selected(&mut self);
fn open_destroy_for_selected(&mut self);
fn open_upgrade_for_selected(&mut self);
fn open_history_for_selected(&mut self);
```

Inside `Modal::ActionsMenu` key handling, the modal first sets `self.modal =
None`, then matches the key and calls the appropriate method. No
re-entrancy, no synthetic key events.

### Help modal (`[?]`)

New `Modal::Help` listing every key in the application — top-level
(`a/n/?/↵/q/j/k/↑/↓`), Actions-modal keys, generic modal keys (`esc`,
`enter`, `y/n` for confirms). Renders centered, two columns
(key + description), ~50 cols × 20 rows. Closes on `esc` or `?`.

Triggered by `?` from the top level. The existing conditional `[?] details
[esc] clear` hint in `bottom.rs:30-33` (when `last_op_error` is set) keeps
its current behaviour — that `?` opens an error-detail modal, not the help
modal. Implementer: route `?` to either based on whether `last_op_error` is
some.

### CLI banner

In `src/cli.rs`, immediately after `Cli::parse()` and before subcommand
dispatch:

```rust
let banner = match maybe_check_disk_for_command(&cli).await {
    Some(line) => Some(line),
    None => None,
};
if let Some(line) = banner {
    let _ = writeln!(std::io::stderr(), "{line}");  // ignore broken-pipe
}
```

The check is gated:

```rust
fn should_emit_banner(cmd: &Command) -> bool {
    use std::io::IsTerminal;
    if !std::io::stderr().is_terminal() { return false; }
    !matches!(cmd,
        Command::Ls
        | Command::Status { .. }
        | Command::Snapshots { .. }
        | Command::Dump { .. }
        | Command::Snapshot { due: true, .. }
        // No-subcommand → TUI dispatch handled separately; TUI corner carries the signal.
    )
}
```

The systemd-timer path (`pgforge snapshot --due`, fires every 5 min) is
explicitly skipped — banner there would spam the journal 288×/day.

`check_disk_health` is wrapped at this call site in a panic-trap:

```rust
let h = match tokio::time::timeout(Duration::from_secs(2),
            check_disk_health(&docker)).await {
    Ok(h) => h,
    Err(_) => DiskHealth::unknown(),
};
```

(`check_disk_health` is panic-free by construction — see `forbidden`
above — but the timeout guards against the docker socket hanging.)

Banner text by status:

| Status   | Banner                                                                                  |
|----------|-----------------------------------------------------------------------------------------|
| Ok       | (no banner)                                                                             |
| Warn     | `⚠ Disk 85% full on {label} ({path}) — Postgres writes / WAL archiving may start failing.` |
| Critical | `⚠ Disk 92% full on {label} ({path}) — Postgres writes / WAL archiving WILL start failing.` |
| Unknown  | (no banner)                                                                             |

`{path}` uses a `~`-collapsed home-relative form when applicable
(small helper in `disk/health.rs`).

Banner colour: yellow for Warn, red for Critical, via ANSI escapes (only
emitted when stderr is a terminal — already gated above).

### Onboarding flash for removed keys

`handle_key` in the no-modal branch adds one new arm:

```rust
KeyCode::Char(c) if matches!(c, 's'|'c'|'R'|'p'|'t'|'r'|'d'|'u'|'e') => {
    self.flash = Some(Flash {
        msg: "Per-instance actions moved to [a]. Press 'a' to open.".into(),
        kind: FlashKind::Info,
        at: self.now,
    });
}
```

Reuses the existing `Flash` infrastructure (`src/tui/app.rs:189-195`). No
new state, no timer logic. The flash fades after the existing 3s timeout.

### Modal cleanup on instance disappearance

`apply_event(Event::InstancesListed(rows))` already updates the list. Add:
if `self.modal` is one of `ActionsMenu`, `CloneAs`, `UpgradeTo`,
`RestoreAs`, `ResizeTo`, `ScheduleEdit` (any variant carrying an
`instance_name`), and that name is no longer in `rows` → `self.modal =
None`. Silent close, no flash (the list change is itself visible).

### Audit of existing top-level Char keys

Enumerated from `rg "KeyCode::Char\('" src/tui/app.rs` (lines 208-345). Each
must be either displayed in the footer or open a modal that shows itself.

| Key   | Today           | After     | Notes |
|-------|-----------------|-----------|-------|
| `j`   | nav down        | unchanged | hint covered by `↑/↓` in description |
| `k`   | nav up          | unchanged | "" |
| `q`   | quit            | unchanged | in footer |
| `s`   | snapshot        | flash hint→ Actions modal | |
| `c`   | clone           | flash hint→ Actions modal | |
| `t`   | snapshot time   | flash hint→ Actions modal | |
| `p`   | preset (resize) | flash hint→ Actions modal | |
| `u`   | **self-update** | **removed** at top-level; in Actions modal `[u]` = Upgrade (pg_upgrade); self-update via `pgforge self-update` CLI only. See "Behaviour changes" below. |
| `r`   | restore         | flash hint→ Actions modal | |
| `R`   | rotate          | flash hint→ Actions modal | |
| `d`   | destroy         | flash hint→ Actions modal | |
| `D`   | destroy+delete-backups | **investigated**: this is a distinct destructive action. Move into a checkbox inside the existing destroy-confirm modal (which already exists). Top-level `D` removed (covered by `[d]` → confirm modal with checkbox). |
| `?`   | error detail (conditional) OR help (new) | unchanged behaviour for error detail; opens Help modal otherwise |
| `e`   | snapshots history | flash hint→ Actions modal `[e]` |
| `n`   | new instance    | unchanged; in footer |

### Behaviour changes beyond reorg

1. **Top-level `u` no longer triggers self-update from the TUI.** Use
   `pgforge self-update` from the CLI. Inside Actions modal, `[u]` opens
   `Modal::UpgradeTo` (the pg_upgrade flow). This is a deliberate breaking
   change; the alternative — keeping `u` ambiguous — is worse.
2. **Top-level `D` (capital) is removed** in favour of a "Delete backups
   too" checkbox in the existing destroy-confirm modal.
3. **All moved keys flash a one-time hint** the first three seconds after
   being pressed — soft onboarding.

## File structure

| Path | What |
|---|---|
| `src/disk/mod.rs` (new) | `pub mod health;` + `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]` |
| `src/disk/health.rs` (new) | module above; trait `DockerRootDirSource`; `check_disk_health` |
| `src/lib.rs` | `pub mod disk;` |
| `src/docker/bollard_engine.rs` | impl `DockerRootDirSource` |
| `src/docker/engine.rs` | optionally add trait method (kept off `DockerEngine` to avoid widening it) |
| `src/tui/refresh.rs` | new `spawn_disk_health` poller |
| `src/tui/events.rs` | new variant `Event::DiskHealthRefreshed(DiskHealth)` |
| `src/tui/app.rs` | `disk_health` field; `apply_event` branch; refactor handlers into `open_*_for_selected` methods; flash-on-moved-key arm; modal-cleanup-on-list arm |
| `src/tui/ui/bottom.rs` | three-zone layout; reads `state.disk_health` |
| `src/tui/ui/modal.rs` | `ActionsMenu` variant + render; `Help` variant + render; promote `centered_rect` to `pub(super)` |
| `src/cli.rs` | `should_emit_banner`, pre-dispatch banner, gated by `is_terminal` + skip list |
| `src/commands/dump.rs` | `default_dump_dir` → `pub(crate)` |
| `Cargo.toml` | add `nix = { version = "0.29", default-features = false, features = ["fs"] }` |
| `tests/disk_health_test.rs` (new) | unit tests for threshold mapping, `div_ceil` boundary, Unknown aggregation, dedup |
| `tests/tui_render_helpers.rs` (new) | `TestBackend`-based helpers (first render test in repo) |
| `tests/tui_actions_modal_test.rs` (new) | render Actions modal, render Help modal, handler-extraction unit tests |
| `tests/tui_flash_on_moved_key_test.rs` (new) | `s` at top-level sets flash, doesn't open snapshot modal |

`tests/tui_render_helpers.rs` establishes the `TestBackend` pattern (none
currently exists in the repo): `let backend = TestBackend::new(80, 24); let
mut terminal = Terminal::new(backend)?; terminal.draw(|f| render(f,
&state))?; assert_buffer_contains(terminal.backend().buffer(), "expected
text");`

## Error handling

- `Docker::info()` failure → fall back to `"/var/lib/docker"`.
- `path.metadata()` failure → drop that mount silently (`tracing::warn`).
- `nix::sys::statvfs` failure → same.
- All mounts fail → `DiskStatus::Unknown`.
- Banner write failure (broken pipe) → ignore.
- Poller task panic → `tokio::spawn` returns a `JoinHandle`; the panic is
  contained, but log it on the next render-tick by spawning a wrapper task
  that re-spawns on panic with a 30-s backoff. (Or: panic-free by
  construction — see `forbidden`.)

The whole subsystem is wrapped in a "never break the parent operation"
contract — disk-health is observability, not a precondition.

## Testing

Unit, fast, no Docker:

- `DiskStatus` mapping: 0/79/80/89/90/95/100 used_pct → Ok/Ok/Warn/Warn/Critical/Critical/Critical.
- `div_ceil` boundary: total=10000, free=11 → used_pct=100 (not 99); free=2000 → 80; free=2001 → 80 (because (10000-2001)*100 = 799900; div_ceil(10000) = 80).
- Aggregation: empty → Unknown; single Ok → Ok; mix of Ok+Warn → Warn; mix of Warn+Critical → Critical.
- Dedup: two paths with same `dev` → 1 mount.
- `should_emit_banner`: each command variant → expected bool.
- Banner format: Warn/Critical produce expected strings; Ok/Unknown produce None.

TUI:

- `TestBackend` snapshot of `Modal::ActionsMenu` contains every listed key label.
- `TestBackend` snapshot of `Modal::Help` contains every key in the audit table.
- `apply_event(Event::Key(s))` at top level → `state.flash.is_some()`,
  `state.modal.is_none()` (no snapshot modal opened).
- `apply_event(Event::Key(a))` → opens `Modal::ActionsMenu`.
- `Event::InstancesListed([])` with `Modal::ActionsMenu { "billing" }` open
  → modal closes silently.

End-to-end:

- Not testable in unit tests (requires real statvfs); rely on manual
  verification (tmpfs-fill in a sandbox).

## Risks

1. **Docker socket lag** — `Docker::info()` from inside the timeout-wrapped
   poller is bounded; from the CLI banner it's bounded by the 2-s timeout.
2. **Many concurrent `pgforge` callers** — each independently calls
   `Docker::info()` for the banner. Acceptable: `info` is read-only and
   lighter than `containers/json` (which `pgforge ls` already calls).
3. **Operator over a colour-less terminal** — banner still readable (ASCII
   `⚠` + words; colour gated by `is_terminal()`).
4. **Loss of muscle memory for moved keys** — mitigated by the onboarding
   flash and the Help modal.
5. **`u` semantic change** — top-level self-update gone from TUI. Mitigated
   by the audit-table documentation and `pgforge self-update` CLI still
   working. README will be updated.
6. **`nix` crate added as a direct dep** — small, mature, already in
   transitive tree.

## Acceptance

- Footer shows `Disk N%` (or `Disk ?`) continuously, colour-coded.
- Pressing `a` opens a centred Actions modal with all 9 keybinds.
- Pressing `?` opens a centred Help modal listing every key.
- `s`/`c`/`R`/`p`/`t`/`r`/`d`/`u`/`e`/`D` do nothing destructive at the top
  level — they flash an info hint.
- Running any interactive CLI command (`create`, `destroy`, `rotate`,
  `restore`, `snapshot` without `--due`, `clone`, `upgrade`, `resize`,
  `reconfigure`, `cron`, `schedule`, `self-update`) while a monitored mount
  is at ≥ 80% prints one stderr line above output.
- Running `pgforge snapshot --due` (systemd path) never prints the banner.
- Running `pgforge ls`/`status`/`snapshots`/`dump` never prints the banner
  (preserves machine-parseable output).
- Filling a tmpfs to 95% in a sandbox surfaces the banner and the TUI
  critical colour, and the `Modal::Help` lists everything.
- All existing TUI tests still pass; the new render-into-buffer tests run
  in CI.
