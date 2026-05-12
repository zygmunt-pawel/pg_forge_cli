# pgforge

RDS-Single-AZ-equivalent provisioner for hardened PostgreSQL on a single host.

## Status

**Plans 1-5 — implemented.** Foundation + create, snapshot/restore PITR, clone
via pg_basebackup, security hardening, upgrade/rotate/ls/status, and the
ratatui TUI dashboard. 150 unit tests green.

## Install

Two things on the target Mac: the `pgforge` binary and a Docker engine.

### 1. OrbStack (Docker engine)

[OrbStack](https://orbstack.dev) is recommended — significantly faster and
lighter than Docker Desktop on macOS, free for personal use.

```bash
# via Homebrew
brew install --cask orbstack

# launch it (first run prompts for a one-time privileged helper install)
open -a OrbStack
```

Verify it's up:
```bash
docker ps
# CONTAINER ID   IMAGE     COMMAND   CREATED   STATUS   PORTS   NAMES
```

Make it start automatically on every login so your pgforge instances come
back up after a reboot:
```bash
osascript -e 'tell application "System Events" to make login item at end with properties {path:"/Applications/OrbStack.app", hidden:true}'
```
(GUI equivalent: OrbStack → Settings → System → toggle **Open at login**.)

If you already have Docker Desktop running instead, that works too —
pgforge talks to whatever Docker socket is exposed via `DOCKER_HOST`.

### 2. pgforge binary

Universal macOS binary (works on Apple Silicon + Intel):
```bash
curl -L https://github.com/zygmunt-pawel/pg_forge_cli/releases/latest/download/pgforge \
  -o /usr/local/bin/pgforge
chmod +x /usr/local/bin/pgforge
xattr -d com.apple.quarantine /usr/local/bin/pgforge 2>/dev/null || true
pgforge --version
```

## Building from source

Only needed if you're developing on pgforge itself.

1. Install Rust 1.80+ and a working Docker engine.
2. Build:
   ```bash
   cargo build --release
   ```

## Quick start

1. Install pgforge (see above) and start your Docker engine.
2. Configure S3 credentials. Create `~/.config/pgforge/config.toml` (Linux) or
   `~/Library/Application Support/pgforge/config.toml` (macOS):
   ```toml
   port_range_start = 5433
   port_range_end = 5500

   [s3]
   bucket = "your-pgforge-bucket"
   region = "eu-central-1"
   endpoint = "s3.eu-central-1.amazonaws.com"
   access_key = "AKIA…"
   secret_key = "…"
   ```
3. Spawn an instance. You pick two passwords here — they don't exist anywhere
   yet; pgforge creates the postgres roles with whatever you supply, and the
   same strings are then what you use to connect:

   - `PGFORGE_APP_PASSWORD` — password for the **application user** (default
     name `leads`). This is your "database password" — what `psql`, your app,
     ORMs, etc. use to connect.
   - `PGFORGE_PGBACKREST_PASSWORD` — password for the internal `pgbackrest`
     replication role (used for backups and `pgforge clone`). Not used by
     your application code. Skip this one if you're using `--no-backup`.

   ```bash
   PGFORGE_APP_PASSWORD=$(openssl rand -base64 24) \
   PGFORGE_PGBACKREST_PASSWORD=$(openssl rand -base64 24) \
   pgforge create \
       --name billing \
       --preset tiny \
       --version 18
   ```

   For local dev / testing without S3, pass `--no-backup` — no pgbackrest
   role is created so the second password isn't needed:
   ```bash
   PGFORGE_APP_PASSWORD=changeme pgforge create \
       --name dev --preset tiny --version 18 --no-backup
   ```
   No-backup instances run hardened postgres but don't push WAL anywhere
   and refuse `snapshot` / `clone` / `restore`.

   Both passwords are saved in plaintext to
   `~/Library/Application Support/dev.pgforge.pgforge/instances/<name>/state.toml`
   (mode 0600). The TUI's `[Enter]` shortcut reads them from there to
   build a ready-to-paste connection URI.

4. Connect, substituting the password you set above:
   ```bash
   psql "postgresql://leads:<your-app-password>@127.0.0.1:<port>/billing"
   ```
   (The port is printed at the end of `create` and saved alongside the
   passwords in `state.toml`. The TUI shows it next to each instance.)

## TUI mode

`pgforge` with no subcommand launches an interactive dashboard
(ratatui). Keybinds:

- `↑`/`↓` or `j`/`k` — navigate instance list
- `s` snapshot · `c` clone · `R` rotate · `u` upgrade · `r` restore
- `Enter` — copy connection string (with password) to clipboard
- `e` — full snapshot list · `?` — error detail · `q` quit

## Day-2 operations

```bash
pgforge ls                                # list managed instances
pgforge status --name billing             # cpu / mem / connections / sizes
pgforge snapshot --name billing           # on-demand full backup to S3
pgforge snapshots --name billing          # backup list + PITR window
pgforge restore --source billing --as billing-recovery
pgforge clone --source billing --as billing-staging
pgforge reconfigure --name billing        # regenerate pg_hba + pg_ctl reload
pgforge rotate --name billing             # recreate container, keep data volume
pgforge upgrade --name billing --to 19    # pg_upgrade with auto pre-snapshot
```

## Snapshots and restore

Take an on-demand full backup of a running instance:

```bash
pgforge snapshot --name billing --label "before-migration"
# Snapshot taken: 20260511-141259F (label=Some("before-migration"), taken_at=2026-05-11T14:12:59Z)
```

List snapshots:

```bash
pgforge snapshots --name billing
```

Restore as a new instance alongside the source (does not touch source):

```bash
# Restore the latest backup
pgforge restore --source billing --as billing-recovery

# Or PITR to a specific moment
pgforge restore --source billing --as billing-recovery \
    --target-time "2026-05-10T14:23:00Z"
```

The restored instance gets its own port, volume, and state file. The source
keeps running untouched. Both are visible via `docker ps`. Connect to the
restored instance with the connection string printed at the end.

Backups live in your S3 bucket under `pgforge/<instance>/`. `pgforge restore`
reads from the source instance's repo path, even when starting a new
instance under a different name — so you can keep both around or kill the
recovery instance once you've copied what you need.

## Cloning

Make a working copy of a running instance for staging / migration testing.
Uses streaming replication (`pg_basebackup`) under the hood, not S3.

```bash
pgforge clone --source billing --as billing-staging
# Clone ready:
#   postgresql://leads:***@127.0.0.1:5435/billing
```

The clone is independent: own port, own volume, own state file, own backup
repo path, own pgbackrest stanza. The source keeps running untouched.

### Migration: existing instances created before Plan 3.5

Plan 3.5 introduced a dedicated `pgreplica` role (non-SUPERUSER) used for
TCP replication — `pgbackrest` (SUPERUSER) is no longer exposed over the
docker bridge. Instances created before Plan 3.5 only have the
`pgbackrest` role and cannot serve as clone sources until you add
`pgreplica` manually:

```bash
docker exec -u postgres pgforge_<instance> psql -c \
    "CREATE ROLE pgreplica WITH LOGIN REPLICATION PASSWORD '<same-pgbackrest-password>';"
pgforge reconfigure --name <instance>   # regenerates pg_hba.conf and reloads
```

For fresh instances created with Plan 3.5+ this is automatic.

## Architecture

Each instance is a Docker container running `postgres:<version>` with
`pgbackrest` baked into the image, hardened defaults applied via a generated
`postgresql.conf`, and WAL pushed asynchronously to S3 with a 60-second
`archive_timeout` (so worst-case data loss on a host crash is ~60s).

See `docs/plans/2026-05-11-foundation-and-create.md` for the implementation
plan that built this scaffold, and the upcoming `2026-XX-XX-*.md` plans for
snapshot / restore / clone / upgrade / TUI.

## Migration from pre-Plan-4 instances

Plan 4 fixed a Plan-1-era bug where `/etc/postgresql/postgresql.conf` and
`/etc/postgresql/pg_hba.conf` were bind-mounted but ignored — postgres was
running on all-default config (archive_mode=off, default tuning, no
hardened pg_hba). To pick up the fix on existing instances without losing
data:

```bash
pgforge rotate --name billing
```

`rotate` stops + removes the container, regenerates configs, recreates
the container on the SAME data volume with current cmd flags
(`-c config_file=… -c hba_file=…`). Plus it ensures the post-Plan-3.5
`pgreplica` role exists for clone-source instances.

## Caveats

- **macOS host**: Docker Desktop and OrbStack run containers in a Linux VM.
  fsync semantics through that VM are weaker than bare-metal Linux. pgforge
  uses `wal_sync_method = fdatasync` (the only Linux-valid choice — postgres
  runs inside a Linux container regardless of host OS), so the F_FULLFSYNC
  path doesn't apply. True RDS-grade durability is not achievable on macOS
  — use a UPS for Mac mini deployments and rely on the 60-second S3 backup
  window as your real durability guarantee.
- **No HA**: pgforge is intentionally single-host, no replication, no
  failover. Same model as RDS Single-AZ.
- **Restored instances and pgbackrest stanza**: `pgforge restore`
  generates `pgbackrest.conf` with the SOURCE instance's repo path so the
  restore can read source backups. After PITR-promotion the restored
  cluster gets a new system identifier — pgbackrest will then reject
  `archive-push` to the source's stanza, and `pgforge snapshot
  <restored>` is not supported. Treat restored instances as read-only
  forensic copies for now. Promoting a restored instance to a fully
  backed-up primary is a Plan 4 item.
