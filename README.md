# pgforge

RDS-Single-AZ-equivalent provisioner for hardened PostgreSQL on a single host.

`pgforge` turns one Mac mini (or any Unix host with Docker) into a small managed
Postgres service: each database is an isolated, hardened container with WAL
archiving to S3, point-in-time recovery, scheduled backups, in-place major
upgrades, and a terminal dashboard — driven entirely from the CLI, no GUI
session required.

## What it does

- **Provision** isolated, hardened Postgres instances — one Docker container +
  named volume + generated `postgresql.conf` / `pg_hba.conf` per database.
- **Back up** continuously — WAL is pushed asynchronously to S3 via pgbackrest
  (`archive_timeout = 60s`, so worst-case loss on a host crash is ~60s) plus
  on-demand and scheduled full/diff snapshots.
- **Restore** — point-in-time recovery, or restore-to-latest, as a *new* instance
  alongside the original.
- **Clone** a running instance via `pg_basebackup` for staging / migration tests.
- **Dump** a live instance to a portable `pg_dump` file you can copy to your
  laptop and restore locally.
- **Upgrade** major versions in place with `pg_upgrade` (auto pre-upgrade
  snapshot for rollback).
- **Operate** — list, live metrics, resize tuning preset, rotate the container
  onto current config, regenerate `pg_hba`, schedule daily snapshots, destroy.
- **Watch** — an interactive ratatui dashboard (`pgforge` with no subcommand).

Single-host by design: no HA, no replication, no failover — the same model as
RDS Single-AZ.

## Install

pgforge needs Docker and the `pgforge` binary on the host.

### 1. Docker

Install Docker for your distro — see the [official docs](https://docs.docker.com/engine/install/).
Add your user to the `docker` group so the socket at `/var/run/docker.sock` is reachable
without `sudo`:

```bash
sudo usermod -aG docker $USER
# Log out and back in (or `newgrp docker`) for the group change to apply.
docker ps   # should succeed without sudo
```

If `docker ps` says `permission denied while trying to connect to the Docker daemon
socket`, the group change hasn't taken effect yet — start a new shell.

### 2. pgforge binary

x86_64 Linux binary:

```bash
mkdir -p ~/.local/bin
curl -L https://github.com/zygmunt-pawel/pg_forge_cli/releases/latest/download/pgforge-linux-x86_64 \
  -o ~/.local/bin/pgforge
chmod +x ~/.local/bin/pgforge

# add ~/.local/bin to PATH if not already there
grep -q '/.local/bin' ~/.profile 2>/dev/null || echo 'export PATH=$HOME/.local/bin:$PATH' >> ~/.profile
source ~/.profile

pgforge --version
```

For other architectures, build from source (see *Building from source* at the bottom).

## Configuration

pgforge reads one global config file with the host port range and S3
credentials. Create `~/.config/pgforge/config.toml` (XDG config dir):

```toml
port_range_start = 5433
port_range_end   = 5500

[s3]
bucket     = "your-pgforge-bucket"
region     = "eu-central-1"
endpoint   = "s3.eu-central-1.amazonaws.com"   # or an R2 / MinIO endpoint
access_key = "AKIA…"
secret_key = "…"
```

The `[s3]` section is required for backup-enabled instances (the default).
For local-only instances created with `--no-backup`, it can be omitted.

## Quick start

```bash
# 1. Create an instance. You choose two passwords here — they don't exist
#    anywhere yet; pgforge creates the postgres roles with whatever you supply.
PGFORGE_APP_PASSWORD=$(openssl rand -base64 24) \
PGFORGE_PGBACKREST_PASSWORD=$(openssl rand -base64 24) \
pgforge create --name billing --preset tiny --version 18

# 2. Connect (port is printed at the end of `create` and shown in `pgforge ls`)
psql "postgresql://leads:<your-app-password>@127.0.0.1:<port>/billing"
```

- `PGFORGE_APP_PASSWORD` — the **application user** password (default user
  name `leads`). This is your "database password" — what `psql`, your app,
  and ORMs use to connect.
- `PGFORGE_PGBACKREST_PASSWORD` — the internal `pgbackrest` replication role
  password (used for backups and `pgforge clone`). Not used by application
  code. Not needed with `--no-backup`.

Both passwords are stored (plaintext, mode 0600) in
`~/.local/share/pgforge/instances/<name>/state.toml` (XDG data dir).

For local dev / testing without S3, pass `--no-backup` — a plain hardened
Postgres with no WAL archiving; `snapshot` / `clone` / `restore` are refused
on such instances:

```bash
PGFORGE_APP_PASSWORD=changeme pgforge create --name dev --preset tiny --version 18 --no-backup
```

## Commands

Run `pgforge <command> --help` for the full flag list of any command.

### Provisioning

| Command | What it does |
|---|---|
| `pgforge create --name X --preset tiny --version 18` | Create a hardened instance. Presets: `tiny` / `small` / `medium` / `large`. Flags: `--app-user`, `--no-backup`, `--retain-days N`, `--snapshot-hour H` / `--no-snapshot-hour`. |
| `pgforge destroy --name X [--delete-backups] [--yes]` | Permanently delete an instance: stop + remove container, drop the data volume, remove `state.toml`. `--delete-backups` also wipes the S3 stanza (PITR becomes unrecoverable). `--yes` skips the confirmation prompt. |

### Inspect

| Command | What it does |
|---|---|
| `pgforge ls` | List all managed instances — version, preset, port, backups, running, and a `FAILING` flag if scheduled backups are broken. |
| `pgforge status --name X` | Live metrics for one instance: CPU, memory, connection counts, DB + PGDATA size, uptime, and backup health (`Backups: ✓ ok` / `✗ FAILING`, last snapshot time). |

### Backup & recovery

| Command | What it does |
|---|---|
| `pgforge snapshot --name X [--label "..."]` | On-demand full/diff backup to S3 via pgbackrest. |
| `pgforge snapshot --due` | Snapshot every backup-enabled instance whose scheduled hour has passed today and that hasn't been snapshotted yet — the command the scheduler runs. |
| `pgforge snapshots --name X` | List recorded snapshots + the effective PITR window. |
| `pgforge restore --source X --as Y [--target-time RFC3339]` | Restore as a **new** instance alongside the source. Without `--target-time`, restores to the latest archived WAL. The restored instance is inert (no archiving, not scheduled) — see *Caveats*. |
| `pgforge dump --name X [--out PATH] [--force] [--keep N] [--timeout SECS]` | `pg_dump -Fc` a live instance to a portable `.dump` file on the host (default `~/pgforge-dumps/<name>-<timestamp>.dump`). Prints the absolute path on stdout — copy it to your laptop and `pg_restore` it locally. `--keep N` prunes older dumps for that instance. |
| `pgforge clone --source X --as Y` | Make an independent working copy of a running instance via `pg_basebackup` (streaming replication, not S3). Own port, volume, state, and pgbackrest stanza. |

### Scheduled backups

| Command | What it does |
|---|---|
| `pgforge cron --name X --hour H` | Set the daily auto-snapshot hour (0–23, local time) for an instance. `--off` disables it (manual only). |
| `pgforge schedule install` | Install the systemd user timer that runs `pgforge snapshot --due` every 5 minutes — it picks up each instance's `cron` hour. `uninstall` / `status` manage it. |

A typical setup: `pgforge create … --snapshot-hour 3`, then once
`pgforge schedule install` — every instance is then backed up daily at its
configured hour.

On a headless server, run once after `schedule install`:

```bash
sudo loginctl enable-linger $USER
```

so the systemd user timer fires even when no user session is active. `pgforge
schedule install` will print a loud warning if linger isn't enabled.

### Maintenance

| Command | What it does |
|---|---|
| `pgforge rotate --name X` | Recreate the container from current pgforge config, keeping the data volume (~10s downtime). Use to apply new hardening to existing instances. |
| `pgforge reconfigure --name X` | Regenerate `pg_hba.conf` and `pg_ctl reload` — no restart. |
| `pgforge resize --name X --preset small` | Change the tuning preset (RAM limit, `max_connections`, `shared_buffers`, …). Rebuilds `postgresql.conf` and recreates the container; data volume preserved. |
| `pgforge upgrade --name X --to 19` | In-place major-version upgrade via `pg_upgrade`. Takes an automatic pre-upgrade snapshot (backup-enabled instances) so you can roll back with `pgforge restore`. |
| `pgforge self-update [--force]` | Replace the running `pgforge` binary with the latest GitHub release (atomic rename). |

## TUI mode

`pgforge` with no subcommand launches the interactive dashboard (ratatui):

- `↑`/`↓` or `j`/`k` — navigate the instance list
- `Enter` — copy the connection string (with password) to the clipboard
- `n` — create a new instance
- `a` — open the **Actions** menu (snapshot / clone / rotate / preset / time / restore / destroy / upgrade / snapshots history) for the selected instance
- `?` — help (or error detail when an op has failed)
- `q` — quit

Disk usage of the host (worst across Docker volume, pgforge state, and
pgforge dumps filesystems) is shown in the bottom bar as `Disk N% used` —
yellow at ≥ 80%, red at ≥ 90%. `Disk ?` means pgforge could not
measure (e.g. Docker daemon unreachable).

A red `⚠BACKUP` badge marks any instance whose scheduled backups are failing
(last attempt newer than last success).

Clipboard on a headless server uses OSC52 (terminal escape sequence) — works
through SSH if your local terminal supports it (iTerm2, kitty, WezTerm,
Alacritty, tmux ≥ 3.3 do). If your terminal doesn't, copy is a no-op and a
flash warning shows.

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

## How it works

Each instance is a Docker container running `postgres:<version>` with
`pgbackrest` baked into the image. pgforge generates a hardened
`postgresql.conf` and `pg_hba.conf` per instance, bind-mounts them, and pushes
WAL asynchronously to S3 with `archive_timeout = 60s`. Instance state
(passwords, port, preset, schedule, snapshot history) lives in `state.toml`
files under `~/.local/share/pgforge/instances/`; writes are atomic (temp-file
+ fsync + rename, with a parent-directory fsync) and serialized with an
advisory lock so
the scheduler, the TUI, and interactive commands can't clobber each other.

## Caveats

- **Restored instances are inert by design.** `pgforge restore` reads the
  source's S3 repo, then boots the new instance with `archive_mode = off` and
  backups/scheduling disabled — so a restored cluster (which runs on a new
  timeline) can never push WAL into the *source's* stanza and corrupt its
  backup chain. A restored instance is fully queryable and read-write; treat it
  as a recovery / forensic copy. `pgforge snapshot` on it is refused. Promoting
  a restored instance to a fully backup-enabled primary is not yet supported.
- **No HA.** Single-host by design — no replication, no failover.

## Upgrading existing instances

After updating the `pgforge` binary, existing instances keep running on their
old generated config until recreated. To apply current hardening + config
fixes without losing data:

```bash
pgforge rotate --name X
```

`rotate` also ensures the `pgreplica` role exists (a non-SUPERUSER role used
for clone-source replication; instances created by older pgforge versions may
lack it). Fresh instances get it automatically.

## Building from source

Only needed if you're developing on pgforge itself.

```bash
# Rust 1.88+ and a working Docker engine
cargo build --release
cargo test                       # unit + integration suite
PGFORGE_E2E=1 cargo test          # also runs the gated end-to-end tests
                                  # (needs a real Docker engine + S3 config)
```

Design specs and implementation plans live under `docs/superpowers/`.

### Cross-compiling for a Linux server from a Mac dev box

pgforge is Linux-only at runtime but most contributors hack on macOS. The
project builds for x86_64 Linux servers via [`cross`](https://github.com/cross-rs/cross),
which runs the linker inside a Docker container so you don't need a local
Linux toolchain.

```bash
# One-time: install cross (uses Docker under the hood; Docker Desktop or
# Colima must be running).
cargo install cross --locked

# Build a release binary targeting Linux x86_64 (the common server arch).
# For ARM Linux servers (rare) use aarch64-unknown-linux-gnu instead.
cross build --release --target x86_64-unknown-linux-gnu

# Deploy to the server. ~/.local/bin/pgforge is the conventional path
# (matches `pgforge schedule install` and `pgforge smart install`, both
# of which bake this path into the systemd-user service ExecStart).
scp target/x86_64-unknown-linux-gnu/release/pgforge db-server:.local/bin/pgforge
ssh db-server "pgforge --version"   # verify the deploy
```

If you have an existing `pgforge schedule install` or `pgforge smart install`
on the server, the timers and sudoers fragment keep working across a binary
swap — no need to re-install them. Only re-run install commands when adding
a new disk (SMART) or changing snapshot cron schedules.
