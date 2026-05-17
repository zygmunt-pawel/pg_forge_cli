# Disk SMART Health Check — Design Spec

**Date:** 2026-05-17
**Status:** Brainstormed, awaiting agent review → plan
**Parent feature:** Capacity disk-health (shipped v0.3.0, spec `2026-05-17-disk-health-and-tui-actions-modal-design.md`) — SMART was explicitly out-of-scope there, "queued separately." This is that follow-up.

## Goal

Surface impending physical disk failure (predictive failure from SMART) so the operator can replace a dying drive **before** Postgres writes start corrupting. Independent from the existing capacity signal — a 30%-full drive can still be dying, a 95%-full drive can still be healthy. Two distinct signals, two distinct remediations (clean up vs. swap hardware).

## Motivation

pgforge runs unattended for weeks at a time on a single Linux host with no HA. The capacity signal already shipped in v0.3.0 catches "disk is filling up." It does NOT catch reallocated sectors, NVMe wear, available-spare exhaustion, or SMART OVERALL_HEALTH=FAILED. Those are typically the first announcement a drive makes before it stops accepting writes — hours to days of warning, usually wasted because nothing was watching.

The operator's workflow when SMART surfaces a problem is: take a dump, provision a new host, restore. SMART warning at T-24h means a planned cutover; SMART warning at T+0 means lost transactions. Worth the small amount of work to wire up.

## Decisions made up-front (locked)

These came out of brainstorming with the user on 2026-05-17 and are not re-litigated below. Listed here so reviewers can challenge the foundation:

1. **Cache-and-read architecture, not on-demand.** A daily systemd-user timer writes a JSON cache; TUI/CLI read the cache. Sudo escalation happens once per day in the timer, never in the hot path.
2. **sudoers NOPASSWD scoped tight,** not setuid, not setcap. `pgforge smart install` writes `/etc/sudoers.d/pgforge-smart` once with the user authenticating; the fragment enumerates exact device paths.
3. **Parse predictive attributes,** not just `smartctl -H`. OVERALL_HEALTH alone is "drive has already decided it's dying" — too late. We watch reallocated/pending sectors (SATA) and critical_warning / media_errors / available_spare / percentage_used (NVMe).
4. **Autodiscover all physical disks** (`lsblk -d -o NAME,TYPE,TRAN -J`, filter `type=disk AND tran in {sata,sas,nvme}`). Aggregate worst-of, mirroring existing `DiskHealth::aggregate`.
5. **Daily cadence** with manual `pgforge smart check` override. Cache stale > 48h → status `Unknown(Stale)`.
6. **Silent degradation on VPS without SMART passthrough.** `SMART ?` dim in TUI, no CLI banner. Install-time test prints an explicit "no disks expose SMART" warning so the operator knows upfront, not three days later.
7. **TUI footer: second separate zone** left of the existing capacity zone. Words ("SMART ok / warn / fail / ?"), per-status color (dim / yellow / red / dim).

## Scope

**In scope:**
- New `pgforge smart` subcommand tree: `install`, `check`, `status`, `uninstall`.
- New `src/smart/` module: `types.rs`, `check.rs`, `cache.rs`, `install.rs`, `mod.rs`.
- Daily systemd-user `pgforge-smart.timer` + `.service` (linger already configured on db-server, no new infra).
- Sudoers fragment `/etc/sudoers.d/pgforge-smart` written by `pgforge smart install` and validated with `visudo -c -f` before declaring success.
- Cache file at `~/.local/state/pgforge/disk-smart.json` (XDG_STATE_HOME default, override via env).
- Smartctl JSON parsing: SATA/SAS attributes 5/197/198/temperature, NVMe smart-health-information-log critical_warning/media_errors/available_spare/percentage_used/temperature, plus OVERALL_HEALTH PASSED/FAILED.
- TUI footer: extend `bottom.rs` from 3-zone to 4-zone layout; new `format_smart_zone()` that mirrors `format_disk_zone()`.
- Background TUI reader poller in `refresh.rs::spawn_smart_reader` — 60s tick that reads cache file (no smartctl invocation, no Docker call, no privilege) and emits `Event::SmartRefreshed`.
- CLI pre-dispatch banner extension in `cli.rs::dispatch`: read cache; if status is `Critical`, print red stderr line ABOVE the existing capacity banner. Same gating as capacity banner (is_terminal, skip-list for machine-readable commands and `snapshot --due`).
- Banner is `Critical`-only — Warn surfaces in TUI but NOT in CLI banner. Reason: CLI banner is for "drop everything." Spamming Warn (percentage_used=82% on NVMe, normal aging) desensitizes operators.
- README: dedicated "SMART hardware monitoring" subsection — install steps, what's monitored, where the cache lives, how to interpret each status, troubleshooting, and uninstall.
- Test coverage: parsing fixtures (SATA/NVMe/unsupported/malformed), cache round-trip + stale, sudoers fragment format, banner format, TUI render.

**Out of scope (explicit, queued):**
- SMART self-tests (`smartctl -t short/long`). Adds I/O during the test, opens a separate "test failed" signal path.
- Push notifications (email/Slack/desktop). Banner + TUI is enough for the unattended single-host model.
- Per-disk historical trending / wear graphs.
- Block backup/restore/snapshot operations when SMART is Critical. Banner is enough — the operator decides.
- macOS support. pgforge is Linux-only as of v0.2.0.
- User-configurable thresholds in `config.toml`. Defaults baked in; if needed later, add then.
- Re-discovery without explicit `pgforge smart install` re-run. Sudoers has enumerated paths; adding a disk requires re-install.
- Disk addition / removal between timer ticks (next tick picks it up; cache lags by up to 24h).

## File layout

| Path | Status | Responsibility |
|---|---|---|
| `src/smart/mod.rs` | new | `pub mod {types, check, cache, install};` + file-level `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]` mirroring `src/disk/mod.rs` |
| `src/smart/types.rs` | new | `SmartStatus`, `SmartUnknownReason`, `DriveSmart`, `SmartHealth`; serde derives; aggregate helper |
| `src/smart/check.rs` | new | `discover_disks()` (lsblk JSON wrap), `run_smartctl(device)` (subprocess wrap), `parse_smartctl_json(json, transport)` (per-transport dispatch); `check_all()` that wires the three together |
| `src/smart/cache.rs` | new | `default_cache_path()`, `read_cache(path, now, max_age) -> SmartHealth`, `write_cache(path, &health)`, `STALE_AFTER_HOURS` constant |
| `src/smart/installed.rs` | new | `InstalledState { smartctl_path: PathBuf, user: String, devices: Vec<PathBuf>, installed_at: jiff::Timestamp }`; `default_installed_path()` → `$XDG_STATE_HOME/pgforge/smart-installed.json`; `read_installed() -> Option<InstalledState>`, `write_installed(&InstalledState)` |
| `src/smart/install.rs` | new | `install_all(opts)` (writes sudoers via tempfile→`visudo -c -f`→`sudo install -m 0440`, writes timer units, daemon-reload+enable+start, writes `InstalledState`, runs first check), `uninstall_all()` (reverse), `render_sudoers_fragment(user, smartctl_path, devices) -> Result<String, InstallError>`, `render_timer_unit()`, `render_service_unit(smartctl_path)`, `postinstall_summary(&SmartHealth)` |
| `src/commands/smart.rs` | new | dispatch for `pgforge smart {install, check, status, uninstall}`; argument parsing; human-readable output |
| `src/cli.rs` | modify | add `Command::Smart { #[command(subcommand)] action: SmartAction }`; extend `dispatch` to read SMART cache + print Critical banner ABOVE capacity banner (same gating); extend `should_emit_banner_for_command` impact (SmartCheck etc. excluded from banner same as Ls) |
| `src/tui/events.rs` | modify | new `Event::SmartRefreshed(SmartHealth)` |
| `src/tui/app.rs` | modify | `smart_health: Option<SmartHealth>` field; `apply_event` arm for SmartRefreshed |
| `src/tui/refresh.rs` | modify | new `spawn_smart_reader(tx)` poller (60s, reads cache file via `smart::cache::read_cache`, sends `Event::SmartRefreshed`); wire into `spawn_pollers` |
| `src/tui/ui/bottom.rs` | modify | extend Layout::horizontal to 4 zones `[content, smart, disk, version]`; add `format_smart_zone(Option<&SmartHealth>)` mirroring `format_disk_zone` |
| `tests/smart_parsing_test.rs` | new | fixtures in `tests/fixtures/smart/`: `sata_ok.json`, `sata_reallocated.json`, `sata_pending.json`, `sata_offline_uncorrectable.json`, `sata_temp_high.json`, `nvme_ok.json`, `nvme_critical_warning.json`, `nvme_media_errors.json`, `nvme_spare_below_threshold.json`, `nvme_percentage_used_82.json`, `unsupported_device.json`. Each loaded + parsed + asserted against expected `DriveSmart`. |
| `tests/smart_cache_test.rs` | new | round-trip serialize→write→read→deserialize equals original; missing file → `Unknown(NoCache)`; corrupt JSON → `Unknown(ParseError)`; older than 48h → `Unknown(Stale)`; valid + fresh → unchanged |
| `tests/smart_install_test.rs` | new | `render_sudoers_fragment("pawel", &["/dev/nvme0n1", "/dev/sda"])` returns exactly the expected bytes; `render_timer_unit()` and `render_service_unit()` snapshot tests. Does NOT actually call sudo or write files. |
| `tests/cli_smart_banner_test.rs` | new | `format_smart_banner_line(&health)` returns Some only for Critical; returns None for Ok/Warn/Unknown/Stale; format string contains device + reasons |
| `tests/tui_smart_zone_test.rs` | new | `format_smart_zone(None)` → "SMART ?" dim; `format_smart_zone(Some(ok))` → "SMART ok" dim; `format_smart_zone(Some(warn))` → "SMART warn" yellow; `format_smart_zone(Some(critical))` → "SMART fail" red |
| `tests/fixtures/smart/` | new | JSON sample outputs (real `smartctl -H -A -j` runs captured from a test box, sanitized of serial numbers) |
| `README.md` | modify | new top-level subsection "Disk health (SMART)" under existing "Operations" or equivalent — see README section below |
| Deps | none new | `serde_json` already in tree; subprocess via `tokio::process::Command`; no new crates |

## Types

```rust
// src/smart/types.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SmartStatus {
    Ok,
    Warn,
    Critical,
    Unknown,
}

impl SmartStatus {
    /// Severity ordering: Unknown < Ok < Warn < Critical.
    /// Matches DiskStatus::rank in src/disk/health.rs so aggregate logic is
    /// consistent across the two health surfaces.
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
    NotInstalled,        // smartmontools missing on host
    NoSudoers,           // sudoers fragment not present or sudo -n failed
    NoInstalledState,    // smart-installed.json missing (pgforge smart install never ran)
    NoDevicesFound,      // lsblk returned zero matching disks
    DeviceNotSupported,  // smartctl said the device doesn't support SMART
    DeviceMissing,       // smartctl could not open the device (ENOENT — hot-unplugged?)
    Stale,               // cache file checked_at older than max_age (or in the future)
    NoCache,             // cache file does not exist (timer never ran)
    ParseError,          // smartctl JSON or cache JSON failed to parse
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveSmart {
    pub device: String,                          // "/dev/nvme0n1"
    pub model: String,                            // best-effort model string from smartctl
    pub transport: String,                        // "nvme" | "sata" | "sas"
    pub status: SmartStatus,
    pub reasons: Vec<String>,                     // human-readable, e.g. "Reallocated_Sector_Ct=3"
    pub unknown_reason: Option<SmartUnknownReason>, // populated only when status==Unknown
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartHealth {
    pub status: SmartStatus,                      // worst-of across drives; Unknown if all Unknown / empty
    pub worst_device: Option<String>,             // device of the worst drive (None if Unknown overall)
    pub worst_reasons: Vec<String>,               // reasons of the worst drive
    pub unknown_reason: Option<SmartUnknownReason>, // populated only when status==Unknown overall
    pub drives: Vec<DriveSmart>,                  // every probed drive, including Unknown ones
    pub checked_at: jiff::Timestamp,              // when the snapshot was produced (UTC)
}

impl SmartHealth {
    pub fn unknown(reason: SmartUnknownReason) -> Self { /* ... */ }

    /// Worst-of aggregate across drives.
    ///
    /// Empty drives → Unknown(NoDevicesFound).
    /// Otherwise pick the worst by `SmartStatus::rank`; ties broken by first
    /// occurrence (input order from `discover_disks`, which is lsblk's order).
    ///
    /// Aggregate semantics (chosen, NOT to be quietly changed): the rank
    /// table places `Unknown < Ok`, so a mix of one Ok drive and ten Unknown
    /// drives reports overall = Ok. Rationale: a real measurement should
    /// always win over "we don't know." The reasons vector of the worst drive
    /// surfaces in TUI/banner; the full `drives` vector is preserved so
    /// `pgforge smart status` can show every Unknown drive separately. We do
    /// NOT escalate to Warn just because most drives are Unknown — that would
    /// turn a working SATA disk plus an unsupported virtio disk into a
    /// constant Warn, which is the same desensitization problem Warn-in-banner
    /// would cause.
    pub fn aggregate(drives: Vec<DriveSmart>, now: jiff::Timestamp) -> Self { /* ... */ }

    /// Returns true if the snapshot is too old to trust OR if `checked_at` is
    /// in the future (clock skew / NTP step backward / container with frozen
    /// clock). Both cases collapse to "we can't trust this snapshot."
    pub fn is_stale(&self, now: jiff::Timestamp, max_age_hours: u32) -> bool {
        // now < checked_at  → future timestamp, fail-safe stale
        // (now - checked_at) > max_age → genuine staleness
    }
}
```

Module-level lint denies (`unwrap_used`, `expect_used`, `indexing_slicing`) mirror `src/disk/mod.rs`. This subsystem is observability — it must NEVER panic and take down the TUI or block CLI dispatch. Every fallible step degrades to `Unknown(<reason>)`.

### Installed-state record

`src/smart/installed.rs` defines a small companion record that captures what `pgforge smart install` set up. Persisted at `~/.local/state/pgforge/smart-installed.json`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledState {
    pub smartctl_path: PathBuf,       // absolute path, e.g. "/usr/sbin/smartctl"
    pub user: String,                 // who the sudoers entry applies to
    pub devices: Vec<PathBuf>,        // enumerated device paths granted in sudoers
    pub installed_at: jiff::Timestamp,
}
```

`run_smartctl` MUST read `InstalledState.smartctl_path` to know the absolute binary path; if `InstalledState` is missing → `Unknown(NoInstalledState)` (mapped to the same "?" TUI symbol). This solves the `$PATH` mismatch problem: the sudoers fragment grants exactly `/usr/sbin/smartctl ...`, the systemd service `ExecStart` uses the same absolute path, and the on-demand `check` resolves it via this record — all three agree by construction.

## Discovery

`src/smart/check.rs::discover_disks() -> Vec<(device_path, transport, model_hint)>`

```rust
async fn discover_disks() -> Vec<DiscoveredDisk> {
    // Tokio subprocess: `lsblk -d -o NAME,TYPE,TRAN,MODEL -J`
    // Parse JSON via serde_json into LsblkRoot { blockdevices: Vec<LsblkDevice> }
    // Filter: type == "disk" AND tran in {"sata", "sas", "nvme"}
    // Output: DiscoveredDisk { path: "/dev/{name}", transport: tran, model: model.unwrap_or_default() }
}
```

Failure modes:
- lsblk binary missing → log warn, return empty Vec. `aggregate(empty)` → `Unknown(NoDevicesFound)`.
- lsblk JSON parse error → same.
- `type` of a device is anything else (`loop`, `dm`, `md`, `part`) → filtered out (mirrors what we want; RAID member dev is not a physical drive).
- Empty filtered result → `Unknown(NoDevicesFound)`.

## smartctl invocation

`src/smart/check.rs::run_smartctl(smartctl_path, device, sudo_mode) -> Result<Vec<u8>, SmartUnknownReason>`

Where `sudo_mode` is:
- `NonInteractive` — `sudo -n {smartctl_path} -H -A -j {device}`. Used by `--write-cache` (timer service) and by the on-demand TUI/CLI cache-read paths. Never prompts. Missing password → fast fail.
- `Interactive` — `sudo {smartctl_path} -H -A -j {device}`. Used by `pgforge smart check` when run from a TTY without `--write-cache` — convenience for ad-hoc operator use when sudoers happens to not be set up yet.

`smartctl_path` comes from `InstalledState.smartctl_path` (preferred) or as a fallback from `which smartctl` (the `Interactive` path only). The absolute path MUST match the sudoers fragment byte-for-byte or the rule won't apply.

We do NOT pass `-d {type}` — smartctl autodetects on Linux for sata/sas/nvme, and locking the type in the sudoers entry would force per-type enumeration.

Failure mapping (per attempt):
- `sudo: a password is required` (sudo exit 1, NonInteractive) → `Unknown(NoSudoers)`.
- smartctl binary missing (sudo exit 1 with "command not found") → `Unknown(NotInstalled)`.
- smartctl exit code 2 with stderr "Smartctl open device: /dev/X failed: No such file or directory" — device was hot-unplugged between install and check → `Unknown(DeviceMissing)`. Detected by ENOENT in stderr.
- smartctl exit code 2 with stderr "Smartctl open device: /dev/X failed: Unknown USB bridge" / "Device does not support SMART" → `Unknown(DeviceNotSupported)`.
- Any other non-zero exit WITH non-empty JSON stdout → still try to parse (smartctl exit codes are a bitfield; OVERALL_HEALTH=FAILED returns nonzero but JSON is valid). Only treat as Unknown if JSON is empty/unparseable.
- Empty stdout, any exit code → `Unknown(ParseError)`.

Timeout: 5s per device via `tokio::time::timeout`. Hung device → `Unknown(ParseError)`, logged at `warn`.

## Parsing logic

`src/smart/check.rs::parse_smartctl_json(bytes) -> DriveSmart`

Dispatches on smartctl's own `device.protocol` field (`"ATA"`, `"SCSI"`, `"NVMe"`) — NOT on lsblk's `tran`. Reason: SAS-attached SATA drives (common on enterprise hardware) report `tran=sas` from lsblk and `device.protocol=ATA` from smartctl; trusting the smartctl-reported protocol always picks the right parser. lsblk transport is used only for discovery + the sudoers fragment; parsing is driven by what smartctl actually returns.

Dispatch:
- `device.protocol == "ATA"` → `parse_sata` (handles SATA and ATA-over-SAS)
- `device.protocol == "SCSI"` → `parse_sata` (smartctl reports SAS-native drives as SCSI but the relevant attributes are the SAS error counters; for MVP we apply the same SATA rules and accept SAS-native is best-effort — flag as a known limitation)
- `device.protocol == "NVMe"` → `parse_nvme`
- anything else → `Unknown(ParseError)` with reason `"unsupported device.protocol={}"`

Tolerant of unknown fields (serde `#[serde(default)]` where useful). If required fields are missing → `DriveSmart { status: Unknown, unknown_reason: Some(ParseError), reasons: vec!["missing field {name}"], ... }`.

### SATA / SAS

Parsed shape (relevant fields only):

```json
{
  "smart_status": { "passed": true },
  "model_name": "Samsung SSD 870 EVO 500GB",
  "ata_smart_attributes": {
    "table": [
      { "id": 5,   "name": "Reallocated_Sector_Ct",   "raw": { "value": 0   } },
      { "id": 194, "name": "Temperature_Celsius",     "raw": { "value": 35  } },
      { "id": 197, "name": "Current_Pending_Sector",  "raw": { "value": 0   } },
      { "id": 198, "name": "Offline_Uncorrectable",   "raw": { "value": 0   } }
    ]
  }
}
```

Rules (first match wins for status; reasons can accumulate from non-fatal levels):
1. `smart_status.passed == false` → Critical, reason `"OVERALL_HEALTH=FAILED"`.
2. Attribute id 5 (Reallocated_Sector_Ct) raw.value > 0 → Critical, reason `"Reallocated_Sector_Ct={N}"`.
3. Attribute id 197 (Current_Pending_Sector) raw.value > 0 → Critical, reason `"Current_Pending_Sector={N}"`.
4. Attribute id 198 (Offline_Uncorrectable) raw.value > 0 → Critical, reason `"Offline_Uncorrectable={N}"`.
5. Attribute id 194 OR id 190 (Temperature_Celsius / Airflow_Temperature_Cel) raw.value > 60 → Warn, reason `"Temperature={N}°C"`.
6. Otherwise Ok with empty reasons.

### NVMe

Parsed shape (note: `nvme_smart_health_information_log.temperature` is reported in **kelvin** in smartmontools 7.x — we read the top-level `temperature.current` Celsius value that smartctl normalises for us, not the raw kelvin field):

```json
{
  "smart_status": { "passed": true },
  "model_name": "SK hynix BC901 HFS512GEJ9X108N",
  "temperature": { "current": 38 },
  "nvme_smart_health_information_log": {
    "critical_warning":            0,
    "available_spare":             100,
    "available_spare_threshold":   10,
    "percentage_used":             3,
    "media_errors":                0
  }
}
```

`critical_warning` is a NVMe spec bitmap — decode bits per the NVMe specification:
- bit 0: available_spare below threshold
- bit 1: temperature above (or below) operational threshold
- bit 2: NVM subsystem reliability degraded
- bit 3: media is in read-only mode
- bit 4: volatile memory backup device failed
- bit 5: persistent memory region read-only or unreliable

Rules:
1. `smart_status.passed == false` → Critical, reason `"OVERALL_HEALTH=FAILED"`.
2. `critical_warning != 0` → Critical, reasons = decoded bit names (e.g. `"critical_warning: media_read_only,nvm_reliability_degraded"`).
3. `media_errors > 0` → Critical, reason `"media_errors={N}"`.
4. `available_spare < available_spare_threshold` → Critical, reason `"available_spare={spare}% < threshold={thr}%"`.
5. `percentage_used >= 80` → Warn, reason `"percentage_used={N}%"`.
6. `temperature.current > 70` → Warn, reason `"Temperature={N}°C"`. If `temperature.current` is missing, fall back to converting `nvme_smart_health_information_log.temperature` from kelvin (`celsius = kelvin - 273`); if that path also fails → skip the temperature rule for this drive (do NOT trip a false Warn).
7. Otherwise Ok.

Thresholds: `SATA_TEMP_WARN_C = 60`, `NVME_TEMP_WARN_C = 70`, `NVME_WEAR_WARN_PCT = 80` — constants in `src/smart/check.rs`, documented with sources (NVMe spec for spare/percentage; common SSD vendor docs for temperature). NOT configurable in MVP. (Note: `STALE_AFTER_HOURS = 48` lives in `src/smart/cache.rs` instead, since staleness is a cache concern.)

## Cache

`src/smart/cache.rs`

Path resolution: `default_cache_path()` returns `$XDG_STATE_HOME/pgforge/disk-smart.json` (fallback `$HOME/.local/state/pgforge/disk-smart.json`). Parent dir created on `write_cache` (mode 0700).

Read path:
```rust
pub fn read_cache(
    path: &Path,
    now: jiff::Timestamp,
    max_age_hours: u32,
) -> SmartHealth {
    // 1. open file -> NotFound? return SmartHealth::unknown(NoCache)
    // 2. read -> serde_json::from_str::<SmartHealth>
    //    parse error? return SmartHealth::unknown(ParseError)
    // 3. if `now < checked_at` OR `(now - checked_at) > max_age_hours hours`
    //    -> return SmartHealth::unknown(Stale)
    //    (the `now < checked_at` arm handles clock skew / NTP step backward
    //     / container with frozen-in-future clock; both cases collapse to
    //     "we can't trust this snapshot")
    //    Preserve drives Vec only inside the returned drives field? No — keep
    //    type simple, return Unknown(Stale) with empty drives. Operator runs
    //    `pgforge smart status` for the raw cache contents.
    // 4. otherwise return as-is
}
```

This function is sync I/O — a few-KB file read from local disk. We deliberately do NOT use `tokio::fs` here; the read takes microseconds and the brief block is harmless inside the TUI reader poller. Documented at the call site so the next person doesn't reach for `spawn_blocking`.

Write path:
```rust
pub fn write_cache(path: &Path, health: &SmartHealth) -> std::io::Result<()> {
    // tempfile::NamedTempFile::new_in(path.parent()) so rename is same-filesystem,
    // therefore atomic. Tempfile created with mode 0600 via OpenOptions before write.
    // After serde_json::to_writer, persist() the tempfile to `path`.
}
```

Cache is `~/.local/state/pgforge/disk-smart.json`. JSON contents are the serialized `SmartHealth`. `jiff::Timestamp` serializes as RFC3339. Sample:

```json
{
  "status": "ok",
  "worst_device": "/dev/nvme0n1",
  "worst_reasons": [],
  "unknown_reason": null,
  "drives": [
    {
      "device": "/dev/nvme0n1",
      "model": "SK hynix BC901 HFS512GEJ9X108N",
      "transport": "nvme",
      "status": "ok",
      "reasons": [],
      "unknown_reason": null
    }
  ],
  "checked_at": "2026-05-17T04:17:33Z"
}
```

`max_age_hours` constant: `STALE_AFTER_HOURS = 48` (2x the daily cadence — one missed run is tolerated, two is suspicious).

## Subcommand surface

```
pgforge smart install     # set up sudoers + timer + first check
pgforge smart check       # run smartctl now, print human-readable status
pgforge smart check --write-cache   # internal — used by the timer
pgforge smart status      # read cache + print (no smartctl call); also: when last checked
pgforge smart uninstall   # remove sudoers + timer + cache
```

### `pgforge smart install`

Steps (each is its own logical phase, idempotent, prints what it did):
1. **Probe smartmontools.** `which smartctl` → absolute path. Missing → print install command (`sudo apt install smartmontools`) and exit code 1. Don't auto-install (user might be on a non-apt distro).
2. **Discover disks.** Run `discover_disks()`. Zero result → print "no physical disks discovered (lsblk found nothing matching sata/sas/nvme)" and exit code 1 — refuse to install with nothing to monitor.
3. **Render + validate sudoers fragment in a tempfile FIRST.** Render via `render_sudoers_fragment(user, smartctl_path, devices)`; if devices is empty the function returns `Err(InstallError::NoDevices)` (defense in depth with step 2). Write to a host-local tempfile (e.g. `/tmp/pgforge-smart-XXXXXX`). Run `sudo visudo -c -f /tmp/pgforge-smart-XXXXXX`. If validation fails → bail with visudo's error message; nothing has touched `/etc/sudoers.d/` yet, so sudo is unaffected.
4. **Atomically install validated fragment.** Only after step 3 passes: `sudo install -m 0440 -o root -g root /tmp/pgforge-smart-XXXXXX /etc/sudoers.d/pgforge-smart`. `install` is atomic (rename under the hood) AND sets mode/owner in one operation — no window where the file is wrong-mode. Refuse to overwrite an existing file whose contents differ UNLESS `--force` (idempotent re-run with identical content is fine).
5. **Write `InstalledState` record.** Serialize `{ smartctl_path, user, devices, installed_at }` to `~/.local/state/pgforge/smart-installed.json` (atomic write via `write_installed`). This is what `run_smartctl` reads at runtime to know which absolute path to invoke.
6. **Write systemd-user units.** `~/.config/systemd/user/pgforge-smart.{service,timer}`. The service `ExecStart` bakes in the absolute `pgforge` binary path (resolved via `std::env::current_exe`, with `%h/.local/bin/pgforge` as fallback only if `current_exe` returns nothing useful). Idempotent overwrite — these are pgforge-generated, no manual edits expected.
7. **Reload + enable + start timer.** `systemctl --user daemon-reload && systemctl --user enable --now pgforge-smart.timer`.
8. **Run first check now** to populate cache and verify wiring end-to-end. `pgforge smart check --write-cache`.
9. **Print summary.** Lines:
   - One line per discovered disk: `/dev/nvme0n1 (SK hynix BC901): SMART ok` (color via status).
   - Overall: `SMART: ok across 1 disk(s). Cache: ~/.local/state/pgforge/disk-smart.json. Next check: tomorrow ~03:00 (jittered).`
   - If all disks reported `Unknown(DeviceNotSupported)`:
     ```
     ⚠ Install completed, but no disk exposes SMART data (typical on VPS
       without passthrough). Status will be 'SMART ?' indefinitely. Capacity
       monitoring (Disk N% used) continues to work. To remove: pgforge smart uninstall.
     ```

`--force` flag: overwrites existing sudoers file without checking equality. Documented as "use when re-running after adding a new disk."

Exit codes:
- 0: install succeeded (incl. the "no SMART exposed" path — install IS done, the warning is informational).
- 1: prerequisites missing (smartctl not installed; no disks discovered; user declined sudo password).
- 2: sudoers validation failed (visudo -c returned non-zero); fragment already removed.

### `pgforge smart check [--write-cache]`

Runs `discover_disks` → per-disk `run_smartctl` + parse → `SmartHealth::aggregate`. Always prints human-readable status to stdout.

Sudo interactivity:
- Without `--write-cache`, when stdout is a TTY: calls `run_smartctl` with `sudo_mode = Interactive` (no `-n`). Lets a curious operator run `pgforge smart check` ad-hoc before `pgforge smart install` has set up sudoers; sudo prompts for password.
- With `--write-cache` (the timer's invocation path): always `sudo_mode = NonInteractive` (`sudo -n`). Timer can't prompt. Missing sudoers → `Unknown(NoSudoers)` written to cache.
- Without `--write-cache`, when stdout is not a TTY (e.g. piped, scripted): `sudo_mode = NonInteractive` as well, to avoid hanging waiting for input.

`--write-cache` is marked `#[arg(hide = true)]` in the clap definition — it's an internal flag for the timer, not part of the operator-facing CLI surface.

Output:

```
SMART check (2026-05-17T08:42:11Z):
  /dev/nvme0n1 (SK hynix BC901):  ok
  Overall: ok
```

Critical example:

```
SMART check (2026-05-17T08:42:11Z):
  /dev/sda (Samsung 870 EVO 500GB):  FAIL — Reallocated_Sector_Ct=3, Current_Pending_Sector=1
  /dev/nvme0n1 (SK hynix BC901):     ok
  Overall: FAIL (worst: /dev/sda)
```

`--write-cache` additionally writes to `default_cache_path()`. Used by the systemd timer service.

Exit code: 0 always (this is observability, not a precondition). Stderr gets logging at warn level for sub-failures.

### `pgforge smart status`

Reads the cache (no smartctl invocation) and prints. Useful for "what does pgforge currently think." Also surfaces stale-cache and missing-cache states with explicit messaging:

```
SMART status (cache: ~/.local/state/pgforge/disk-smart.json):
  Last checked: 2026-05-17T03:08:22Z (5h 34m ago)
  /dev/nvme0n1: ok
  Overall: ok
```

Stale variant:

```
SMART status (cache: ~/.local/state/pgforge/disk-smart.json):
  Last checked: 2026-05-15T03:08:22Z (53h 34m ago) — STALE
  Status: Unknown(Stale). Run `pgforge smart check --write-cache` or check timer:
    systemctl --user status pgforge-smart.timer
```

Missing-cache variant:

```
SMART status: no cache file found at ~/.local/state/pgforge/disk-smart.json
  Run `pgforge smart install` first, or `pgforge smart check` for an ad-hoc check.
```

Exit code: 0 always.

### `pgforge smart uninstall`

Reverse of install:
1. `systemctl --user disable --now pgforge-smart.timer` (best effort; not-found is OK).
2. Remove `~/.config/systemd/user/pgforge-smart.{service,timer}`.
3. `systemctl --user daemon-reload`.
4. `sudo rm /etc/sudoers.d/pgforge-smart` (user authenticates).
5. Remove cache file.
6. Print summary of what was removed.

Idempotent — running on a clean host prints "nothing to remove" and exits 0.

## Sudoers fragment template

```
# pgforge SMART disk health checks
#
# Installed by `pgforge smart install` on 2026-05-17T08:42:11Z.
# Allows the pgforge-smart.timer (systemd-user) to read SMART data from
# the disks discovered at install time. Each line is one exact device path
# (no wildcards) so adding a new disk requires `pgforge smart install --force`.
#
# Remove with: pgforge smart uninstall

pawel ALL=(root) NOPASSWD: /usr/sbin/smartctl -H -A -j /dev/nvme0n1
pawel ALL=(root) NOPASSWD: /usr/sbin/smartctl -H -A -j /dev/sda
```

Notes for the implementer / reviewer:
- The user field is `whoami` of the install-time invoker, NOT a literal "pawel."
- We deliberately enumerate exact device paths instead of `/dev/nvme*n*` wildcards. Sudoers wildcard semantics around `/` are subtle; an exact list is auditable and unambiguous.
- The smartctl path comes from `which smartctl` at install time, persisted in `InstalledState.smartctl_path`, and used identically in (a) the sudoers fragment, (b) the systemd service `ExecStart`, and (c) the runtime `run_smartctl` invocation. Same byte-for-byte string in all three — that's the contract that makes the sudoers rule match.
- File mode: 0440 (sudoers default), enforced by `sudo install -m 0440 -o root -g root` (NOT via `sudo tee`, which doesn't set mode atomically).
- One rule per line (rather than a comma-separated `Cmnd_Spec_List`) is a deliberate readability choice — git diffs of the rendered fragment are line-by-device.
- `render_sudoers_fragment(user, smartctl_path, &devices)` returns `Result<String, InstallError>` and returns `Err(InstallError::NoDevices)` for empty `devices`. The function never emits a fragment with zero `Cmnd_Spec` lines (which would parse cleanly via `visudo -c` but grant nothing — a silent install failure mode we want to refuse loudly).

## Systemd-user units

```ini
# ~/.config/systemd/user/pgforge-smart.timer
[Unit]
Description=pgforge daily SMART disk health check

[Timer]
OnCalendar=daily
RandomizedDelaySec=1h
Persistent=true
Unit=pgforge-smart.service

[Install]
WantedBy=timers.target
```

```ini
# ~/.config/systemd/user/pgforge-smart.service
[Unit]
Description=pgforge SMART disk health check (writes cache)

[Service]
Type=oneshot
ExecStart=/home/pawel/.local/bin/pgforge smart check --write-cache
```

`ExecStart` uses an **absolute** path resolved at install time via `std::env::current_exe()`. Falls back to `%h/.local/bin/pgforge` only if `current_exe` returns nothing useful (the same default location as `pgforge schedule install`). Persisted into the rendered unit file — no path-resolution at service-start time, no surprises when the pgforge binary is moved.

`Persistent=true` ensures a missed daily run fires on boot (laptop closed for the night, etc.). **Caveat**: `Persistent=true` only back-fills missed runs after the timer has fired at least once previously — the very first install does NOT trigger an immediate back-fill, which is why step 8 of `install` runs `pgforge smart check --write-cache` explicitly to populate the cache on day zero.

`RandomizedDelaySec=1h` so a fleet of pgforge boxes doesn't hit shared infra at the same instant — not a concern for single-host, kept for hygiene.

## TUI integration

### Reader poller

`src/tui/refresh.rs::spawn_smart_reader(tx, cache_path)`:

```rust
const SMART_READ_PERIOD: Duration = Duration::from_secs(60);

pub fn spawn_smart_reader(
    tx: UnboundedSender<Event>,
    cache_path: PathBuf,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Eager first read so the footer doesn't show "?" for a full minute.
        let h = smart::cache::read_cache(
            &cache_path, jiff::Timestamp::now(), smart::cache::STALE_AFTER_HOURS,
        );
        let _ = tx.send(Event::SmartRefreshed(h));

        let mut iv = tokio::time::interval(SMART_READ_PERIOD);
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Consume the immediate first tick (interval fires immediately on first
        // tick(); we already did the eager read above).
        iv.tick().await;
        loop {
            iv.tick().await;
            let h = smart::cache::read_cache(
                &cache_path, jiff::Timestamp::now(), smart::cache::STALE_AFTER_HOURS,
            );
            let _ = tx.send(Event::SmartRefreshed(h));
        }
    })
}
```

This pattern diverges from `spawn_disk_health` (refresh.rs:148), which has no eager pre-read — relying on the immediate first tick of its 15s interval to populate the TUI. We add an explicit eager read here because the SMART poller's interval is 60s, and showing `SMART ?` for a full minute on TUI startup (when there's a valid cache sitting on disk) would be unnecessary.

`read_cache` is sync I/O — a few-KB file read from local disk, microseconds. We deliberately do NOT use `tokio::fs` or wrap in `spawn_blocking`; the brief block is harmless and the simpler call site is worth more. Documented in `cache.rs` so the next person doesn't reach for an async wrapper.

Reader does NO smartctl invocation, NO sudo, NO Docker call. Pure file read.

### `AppState` field

```rust
pub struct AppState {
    // existing fields...
    pub disk_health: Option<DiskHealth>,   // already present
    pub smart_health: Option<SmartHealth>, // NEW
}
```

`apply_event(Event::SmartRefreshed(h))` writes `state.smart_health = Some(h)`.

### Footer layout

`src/tui/ui/bottom.rs::render` extends to 4-zone layout:

```rust
let version = format!(" v{} ", env!("CARGO_PKG_VERSION"));
let disk    = format_disk_zone(state.disk_health.as_ref());
let smart   = format_smart_zone(state.smart_health.as_ref());
let [content_area, smart_area, disk_area, version_area] = Layout::horizontal([
    Constraint::Min(0),
    Constraint::Length(smart.label.chars().count() as u16),
    Constraint::Length(disk.label.chars().count() as u16),
    Constraint::Length(version.chars().count() as u16),
]).areas(area);
```

`format_smart_zone`:

```rust
struct SmartZone { label: String, style: Style }

fn format_smart_zone(h: Option<&SmartHealth>) -> SmartZone {
    let Some(h) = h else {
        return SmartZone {
            label: " SMART ? ".into(),
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
    SmartZone { label, style }
}
```

Net footer (default state, all OK):

```
[n]ew [a]ctions [?]help [↵] uri [q]uit          SMART ok  Disk 45% used  v0.3.1
```

No new keybinds — SMART is read-only display. Consistent with the `feedback_tui_no_implicit_keys` rule: nothing in the footer says you can interact with the SMART zone, and indeed you can't.

### Help modal mention

`src/tui/ui/modal.rs::Modal::Help` already exists. Add one line under a "Footer indicators" section (or create one if not present):

```
SMART ok/warn/fail/?    Daily SMART disk health (set up with `pgforge smart install`)
Disk N% used / ?        Capacity of disks holding Docker, state, dumps
```

This is purely text in the existing Help modal; no new modal, no new key.

## CLI banner

`src/cli.rs::dispatch` already does a pre-dispatch capacity banner. Extend it:

```rust
// New: pre-dispatch SMART banner (Critical only). Runs BEFORE the existing
// capacity banner so SMART (more urgent — hardware) appears above Capacity
// (less urgent — fixable by cleanup).
if let Some(cmd) = &cli.command
    && should_emit_banner_for_command(cmd)
{
    let cache_path = crate::smart::cache::default_cache_path();
    let sh = crate::smart::cache::read_cache(
        &cache_path,
        jiff::Timestamp::now(),
        crate::smart::cache::STALE_AFTER_HOURS,
    );
    if let Some(line) = format_smart_banner_line(&sh) {
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), "{line}");
    }
}

// Existing capacity banner block stays below, unchanged.
```

`format_smart_banner_line`:

```rust
pub fn format_smart_banner_line(h: &SmartHealth) -> Option<String> {
    if h.status != SmartStatus::Critical { return None; }
    let device = h.worst_device.as_deref().unwrap_or("?");
    let reasons = h.worst_reasons.join(", ");
    Some(format!(
        "\u{26A0} SMART CRITICAL on {device}: {reasons}. Replace drive before Postgres data corruption."
    ))
}
```

Use the `\u{26A0}` escape (matches existing cli.rs:255 capacity banner — no mixed-encoding lines in source). Color: red ANSI when stderr is a terminal (already gated by `should_emit_banner_for_command`).

Skip-list extension: append `| Command::Smart { .. }` to the existing match arm in `should_emit_banner_for_command(cmd)` (cli.rs:232–239). DO NOT create a parallel function — keeping one gating function is the whole point of the design. After the edit:

```rust
!matches!(cmd,
    Command::Ls
        | Command::Status { .. }
        | Command::Snapshots { .. }
        | Command::Dump { .. }
        | Command::Snapshot { due: true, .. }
        | Command::Smart { .. }   // NEW
)
```

This skips the SMART banner for `pgforge smart status` (its own output is the SMART status — banner would be redundant), `pgforge smart check` (same), `pgforge smart install` (the install summary already covers it), and `pgforge smart uninstall`.

When both banners fire (SMART Critical AND capacity Critical), stderr gets two lines, SMART first (red), capacity second (yellow or red).

## README addition

The existing README already has a `### Disk pressure` subsection inside `## How it works` (around line 216). That subsection currently covers only the capacity signal. **Replace** it with a top-level `## Disk health` section (sibling to existing `## TUI mode`, `## Caveats`, etc., positioned after `## TUI mode` and before `## How it works`). The new section covers BOTH capacity (recap of what already shipped) and SMART (new content). Full text below — this is the explicit user-requested README documentation.

```markdown
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
```

## Testing

### Unit (fast, no privilege, no devices)

- `tests/smart_parsing_test.rs`:
  - Load each fixture in `tests/fixtures/smart/`, call `parse_smartctl_json`, assert resulting `DriveSmart`.
  - SATA fixtures: `sata_ok.json`, `sata_reallocated_3.json`, `sata_pending_1.json`, `sata_offline_uncorrectable_1.json`, `sata_temp_65.json`, `sata_temp_55.json`, `sas_attached_sata_ok.json` (verifies device.protocol=ATA dispatch when lsblk reports tran=sas).
  - NVMe fixtures: `nvme_ok.json`, `nvme_critical_warning_spare.json`, `nvme_critical_warning_temp.json`, `nvme_media_errors_5.json`, `nvme_spare_below_threshold.json`, `nvme_percentage_used_82.json`, `nvme_temp_75_celsius.json` (top-level `temperature.current` path), `nvme_temp_only_kelvin.json` (fallback kelvin→celsius conversion when `temperature.current` is missing).
  - Malformed: empty bytes → `Unknown(ParseError)`; missing required fields → `Unknown(ParseError)`.
  - Unsupported device output → `Unknown(DeviceNotSupported)`.
  - Missing device (`open device failed: No such file or directory`) → `Unknown(DeviceMissing)`.
  - Unknown `device.protocol` (e.g. `"SAT"`) → `Unknown(ParseError)`.
- `tests/smart_cache_test.rs`:
  - Round-trip: build `SmartHealth`, `write_cache` to tempdir, `read_cache` → equal.
  - Missing file → `Unknown(NoCache)`.
  - Corrupt JSON → `Unknown(ParseError)`.
  - `checked_at` older than 48h → `Unknown(Stale)`.
  - `checked_at` exactly 48h → `Unknown(Stale)` (boundary).
  - `checked_at` 47h59m → not stale, returns as-is.
  - `checked_at` in the future (10 minutes ahead of `now`) → `Unknown(Stale)` (clock-skew fail-safe).
- `tests/smart_install_test.rs`:
  - `render_sudoers_fragment("pawel", "/usr/sbin/smartctl", &["/dev/nvme0n1", "/dev/sda"])` → byte-exact match against expected literal.
  - `render_sudoers_fragment("pawel", "/usr/sbin/smartctl", &[])` → `Err(InstallError::NoDevices)`.
  - `render_timer_unit()` and `render_service_unit("/usr/sbin/smartctl", "/home/pawel/.local/bin/pgforge")` → byte-exact (snapshot). ExecStart contains the absolute pgforge path.
  - `InstalledState` round-trip (build → write → read → equal).
  - No actual sudo, no actual write to /etc — just rendering and tempdir tests.
- `tests/cli_smart_banner_test.rs`:
  - `format_smart_banner_line` returns `Some` only for Critical.
  - Format contains device path + at least one reason.
  - Returns `None` for Ok, Warn, Unknown.
- `tests/tui_smart_zone_test.rs`:
  - `format_smart_zone(None)` → " SMART ? " dim.
  - `format_smart_zone(Some(SmartHealth { status: Ok, .. }))` → " SMART ok " dim.
  - Warn → yellow, Critical → red, Unknown → dim.
  - Uses `TestBackend` pattern established in `tests/tui_render_helpers.rs` (created during the capacity feature).

### Integration / E2E (gated by `PGFORGE_E2E=1`, user runs manually)

- Real `pgforge smart install` on db-server: assert sudoers file exists, visudo -c passes, timer is enabled, cache file is populated within 5s.
- Real `pgforge smart check` on db-server: assert exit code 0, output contains the discovered device path.
- Real `pgforge smart uninstall`: assert sudoers gone, timer disabled, cache gone.

These tests use `cargo test --test smart_e2e_test -- --ignored` to keep them out of the default suite per `feedback_long_tasks`.

### Manual TUI checkpoint (final gate before merge)

The user runs `pgforge` on db-server after `pgforge smart install` and confirms:
- Footer shows `SMART ok  Disk N% used  v0.3.x`.
- Footer SMART zone is dim when ok.
- After artificially editing the cache file to set status=Critical: footer flips to red `SMART fail` within 60s, and `pgforge ls` (in another shell) prints the red banner.

## Risks

1. **Malformed sudoers fragment locks the user out of sudo.** Mitigated by rendering and validating the fragment in a **tempfile** first (`sudo visudo -c -f /tmp/pgforge-smart-XXXXXX`), and only on validation success atomically installing via `sudo install -m 0440 -o root -g root` to `/etc/sudoers.d/pgforge-smart`. This means `/etc/sudoers.d/` is never touched by a malformed file — if validation fails, the live sudoers tree is unchanged and the user's existing sudo access is unaffected.
2. **smartctl JSON schema varies between smartmontools versions.** Mitigated by tolerant parsing (missing fields → Unknown not crash) and version-stable fields (attribute IDs 5/197/198 are fixed in the ATA spec; NVMe smart-health-information-log field names are NVMe spec).
3. **`lsblk` output format varies across util-linux versions.** Less risky than smartctl — the `-J` output for `NAME,TYPE,TRAN` has been stable since util-linux 2.27 (2015). Some interim versions (≤ 2.38) had known quoting bugs with `-J` for unusual model strings (util-linux issue tracker); we mitigate by tolerant parsing with serde `#[serde(default)]` and by skipping any device whose JSON entry fails to deserialize rather than aborting the whole discovery.
4. **`smartctl -d auto` autodetection might pick wrong type for exotic transports.** Unlikely on the sata/sas/nvme filter; if it happens, that disk reports `Unknown(DeviceNotSupported)` — graceful.
5. **Container/VM environments (Docker container with `/dev` mounts, KVM virtio without passthrough)** will mostly fail SMART. This is the expected `Unknown(DeviceNotSupported)` path and is documented at install time.
6. **Cache file privilege.** Written mode 0600 by the timer's service. Readable by the user only — fine for our threat model (no secrets in it).
7. **systemd-user timer requires linger.** Already enabled on db-server (per `project_macmini_setup.md` and current install). Documented in README troubleshooting.
8. **Adding a new disk** doesn't auto-update the sudoers fragment. Documented; `pgforge smart install --force` is the explicit re-run.

## Acceptance

- `pgforge smart install` on db-server completes 0, writes sudoers + timer, runs first check.
- `pgforge smart status` shows the latest cache contents.
- `pgforge smart check` produces a human-readable status without affecting the cache.
- `pgforge smart check --write-cache` writes the cache atomically.
- TUI footer shows `SMART ok` (dim) → `SMART warn` (yellow) → `SMART fail` (red) → `SMART ?` (dim) as the underlying cache changes.
- `pgforge ls` (in CLI) when SMART Critical prints a red one-line stderr banner above its output.
- `pgforge ls`, `pgforge status`, `pgforge dump`, `pgforge snapshot --due` never print the SMART banner.
- `pgforge smart uninstall` removes everything `install` added.
- All new unit tests pass; clippy clean with the existing `-D warnings`.
- README contains the documented setup + status table + troubleshooting.
- Manual TUI eyeball passes on db-server.
