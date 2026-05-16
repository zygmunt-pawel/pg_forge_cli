# Disk Health + TUI Actions Modal — Design Spec

**Date:** 2026-05-17
**Status:** Brainstormed, awaiting plan
**Predecessor of:** none (queued before restore-drill and SMART check)

## Goal

Give the operator an early warning when the host disk is filling up, and free
TUI footer space by folding per-instance actions behind a single key.

## Motivation

pgforge runs unattended for weeks at a time on a Mac mini or db-server. A full
disk silently breaks Postgres writes, pgbackrest pushes, and snapshots — and
the operator usually finds out from a failing app, not from pgforge. We need
"wczesnie sygnalizowac" — surface disk pressure before it bites.

The natural place to show this is the TUI footer, but the footer is already
saturated with ten per-instance shortcut keys. Compacting those behind a
single `[a]ctions` entry both cleans up the bar and frees space for the new
status. The reorg is not a refactor for refactor's sake — it is the only way
to fit the new signal without making the bar wrap or shrink.

## Scope

**In scope:**
- Disk usage (`statvfs`) monitoring of the filesystems holding Docker volumes,
  pgforge state, and pgforge dumps.
- Two-tier thresholds (warn, critical) with fixed values.
- Footer indicator in the TUI (always on).
- One-line banner before every CLI command's normal output when state is bad.
- TUI footer reorganization: per-instance actions hidden behind a modal opened
  by `[a]`. The previous shortcuts (`s`, `c`, `R`, `p`, `t`, `r`, `d`, `u`)
  are removed from the top-level event router — they only work inside the
  Actions modal.
- Audit hidden keys: any working key that is not visible in the footer or an
  open modal is either surfaced or removed.

**Out of scope (explicit, queued separately):**
- SMART / drive failure detection — separate spec, requires `smartmontools`,
  sudo, device autodiscovery, graceful unavailability on VPSes.
- Per-instance disk usage breakdown (how big is the `pgforge_billing` volume).
- User-configurable thresholds in `config.toml` (YAGNI; revisit if a real
  workload needs different cutoffs).
- Push notifications (ntfy.sh, Slack, mail) — explicitly rejected for this
  cycle: keep pgforge a single self-contained CLI with no network egress
  beyond S3.
- Disk usage of S3/R2 repo (different problem; pgbackrest manages retention).

## Design

### Disk health module

A small module `src/disk/health.rs` (new) exposes:

```rust
pub enum DiskStatus { Ok, Warn, Critical }

pub struct MountUsage {
    pub mount_point: PathBuf,  // canonicalized mount-point path
    pub used_pct: u8,          // 0..=100, rounded
    pub free_bytes: u64,
    pub total_bytes: u64,
}

pub struct DiskHealth {
    pub status: DiskStatus,     // worst across mounts
    pub worst_pct: u8,          // worst used_pct, for compact display
    pub worst_mount: PathBuf,   // which mount caused the worst status
    pub mounts: Vec<MountUsage>,
}

pub fn check_disk_health() -> DiskHealth;
```

The function:

1. Collects three candidate paths:
   - Docker root dir, obtained via `docker info --format '{{.DockerRootDir}}'`
     (default `/var/lib/docker`). If `docker info` fails, fall back to
     `/var/lib/docker` and let the statvfs error surface naturally.
   - `~/pgforge-dumps` (the dumps directory, even if it does not exist yet —
     use its would-be parent in that case).
   - `InstanceState::default_state_root()` (`~/.local/share/pgforge`).
2. For each path: `statvfs` to learn the underlying filesystem; dedupe by
   `(f_fsid, st_dev)` so two paths on the same FS are counted once.
3. Each surviving mount → compute used_pct = `100 * (total - free) / total`.
4. Map: `< 80% → Ok`, `80..90 → Warn`, `≥ 90 → Critical`.
5. Aggregate: `status = max(per-mount status)`, `worst_pct = max(per-mount pct)`.

`statvfs` is microsecond-cheap, so we never cache — every call is fresh.

Errors at this layer (FS missing, permission denied) return a
`DiskHealth { status: Ok, worst_pct: 0, mounts: vec![] }` and log at
`tracing::warn` level. The feature is best-effort and must not block any
operation.

### TUI footer (always-on indicator)

`src/tui/ui/bottom.rs` already splits the bottom line into `content_area` and
`version_area`. We add a third zone:

```
| content_area (left)         | disk_area | version_area |
| [n]ew [a]ctions [↵] uri q   | Disk 45%  | v0.2.0       |
```

`disk_area` width = exact length of the formatted string (e.g. `" Disk 45% "`).
Color: dim/grey for Ok, yellow for Warn, red for Critical. When `mounts` is
empty (statvfs failed), show nothing — degrade silently.

### TUI footer (key hints)

Default footer string changes from

```
[n]ew [s]nap [c]lone [R]otate [p]reset [t]ime [r]estore [d]estroy [u]pdate [↵] uri [q]uit
```

to

```
[n]ew [a]ctions [↵] uri [q]uit
```

Conditional footers (error, in-progress, flash) are unchanged — they already
surface their own keys (`[?] details [esc] clear`, etc.).

### Actions modal

New `Modal::ActionsMenu { instance_name: String }`. Triggered by `a` from the
top-level event router only when an instance is selected. The modal renders
centered, ~32 cols × 13 rows:

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
│  [e] snapshots history  │
│                         │
│  [esc] Cancel           │
└─────────────────────────┘
```

Event handling inside the modal: each listed key delegates to the existing
handler that previously fired from the top-level router (open snap modal,
open clone modal, etc.). After delegation the ActionsMenu closes — the
follow-up modal opens on top of the (now closed) menu. `Esc` closes without
firing anything.

The top-level event router no longer responds to `s/c/R/p/t/r/d/u/e` at all.
This is the substantive behaviour change: keys that were silent shortcuts
become invisible (do nothing) until the user opens the modal. The footer is
now the truth.

### CLI banner

In `src/cli.rs`, immediately after argument parsing and before dispatching
to a subcommand, call `check_disk_health()`. If `status != Ok`, emit one line
to **stderr** (so it doesn't pollute pipeable stdout):

```
⚠ Disk 92% full on /var/lib/docker — pgforge writes may start failing.
```

(Yellow at Warn, red at Critical.) Skip the check when the subcommand is one
that should be silent and machine-readable (`--help`, `--version`, and the
no-subcommand case which launches the TUI — the TUI shows the same signal
anyway). Skip when the subcommand is the bare `pgforge` (TUI launches) —
banner would be lost behind the TUI and the corner already shows it.

The banner is informational only — it never aborts the command. A user
deliberately running `pgforge destroy` to free space must be allowed to.

### Hidden-keys audit

While reorganizing the footer, sweep the event router for any handler that
fires on a key not currently displayed:

- `e` (snapshots history detail) — currently undisplayed. Move to Actions
  modal (already in mockup above) and remove the top-level binding.
- `?` (error detail) — currently shown conditionally in `bottom.rs:30` when
  `last_op_error` is set. That counts as visible: keep behavior, keep
  binding.
- Any others the implementer finds: surface in the footer/modal or delete.

## File structure

| Path | What |
|---|---|
| `src/disk/mod.rs` (new) | `pub mod health;` |
| `src/disk/health.rs` (new) | the module above |
| `src/lib.rs` | add `pub mod disk;` |
| `src/tui/ui/bottom.rs` | split into three zones; new default footer string |
| `src/tui/ui/modal.rs` | add `ActionsMenu` variant + render |
| `src/tui/events.rs` | wire `a` → ActionsMenu; drop top-level `s/c/R/p/t/r/d/u/e` |
| `src/cli.rs` | banner pre-dispatch |
| `tests/disk_health_test.rs` (new) | threshold tests + aggregation |
| `tests/tui_actions_modal_test.rs` (new) | ActionsMenu open/close/delegate |

`statvfs` access goes through `std::os::unix::fs::MetadataExt` and `libc`
direct, no new crate. We already depend on `libc` transitively via tokio.

## Error handling

- `docker info` failure → fall back to `/var/lib/docker` as Docker root.
- `statvfs` failure on a path → drop that mount silently (warn-log), continue.
- All mounts fail → `status: Ok, mounts: vec![]` (degrade to silence).
- Banner write failure (broken stderr pipe) → ignore.

The whole subsystem is wrapped in a "never break the parent operation"
contract — disk-health is observability, not a precondition.

## Testing

Unit, fast, no Docker:

- `DiskStatus` mapping: 0/79/80/89/90/95/100 → Ok/Ok/Warn/Warn/Critical/Critical/Critical.
- `DiskHealth::aggregate` correctness on multi-mount inputs.
- Banner formatting (worst mount + worst pct + suffix per severity).

Integration:

- TUI snapshot test (existing pattern): ActionsMenu renders with all 9 keys.
- TUI event test: `a` opens ActionsMenu; `s` inside delegates to snapshot
  modal; `s` outside does nothing.

End-to-end disk-statvfs is not unit-tested (would require root or mounted
tmpfs); rely on manual verification.

## Risks / open questions

1. **Docker volumes on a separate disk** — autodiscovery via `docker info`
   handles this; if it fails we fall back to `/var/lib/docker` which might
   be misleading on a relocated Docker root. Acceptable for v1.
2. **Operator runs `pgforge` over SSH from a terminal that doesn't render
   colour** — banner still readable (the `⚠` ASCII char survives without
   colour, the words explain the rest).
3. **Banner could be noisy if user runs many quick commands** — mitigated
   by Warn at 80% and Critical at 90%, both signals that warrant repeating.
4. **Loss of muscle memory** — only relevant user (the spec author) accepted
   this trade-off explicitly when locking the design.

## Acceptance

- Footer shows `Disk N%` continuously, colour-coded.
- Pressing `a` opens a centred Actions modal listing every per-instance key.
- `s`/`c`/`R`/`p`/`t`/`r`/`d`/`u`/`e` do nothing at the top level.
- Running any CLI command while a monitored mount is at ≥ 80% prints one
  stderr line at the top of output.
- Filling a tmpfs to 95% in a test environment surfaces both the banner and
  the TUI critical colour.
- All existing TUI tests still pass.
