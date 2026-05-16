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
credentials. Create `~/Library/Application Support/dev.pgforge.pgforge/config.toml`
(macOS) or `~/.local/share/pgforge/config.toml` (Linux):

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
`…/dev.pgforge.pgforge/instances/<name>/state.toml`.

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
- `s` snapshot · `c` clone · `R` rotate · `u` upgrade · `r` restore · `t` set snapshot hour
- `Enter` — copy the connection string (with password) to the clipboard
- `e` — full snapshot list · `?` — error detail · `q` — quit

A red `⚠BACKUP` badge marks any instance whose scheduled backups are failing
(last attempt newer than last success).

Clipboard on a headless server uses OSC52 (terminal escape sequence) — works
through SSH if your local terminal supports it (iTerm2, kitty, WezTerm,
Alacritty, tmux ≥ 3.3 do). If your terminal doesn't, copy is a no-op and a
flash warning shows.

## How it works

Each instance is a Docker container running `postgres:<version>` with
`pgbackrest` baked into the image. pgforge generates a hardened
`postgresql.conf` and `pg_hba.conf` per instance, bind-mounts them, and pushes
WAL asynchronously to S3 with `archive_timeout = 60s`. Instance state
(passwords, port, preset, schedule, snapshot history) lives in `state.toml`
files under the pgforge data directory; writes are atomic (temp-file + fsync +
rename, with a parent-directory fsync) and serialized with an advisory lock so
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
