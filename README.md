# pgforge

RDS-Single-AZ-equivalent provisioner for hardened PostgreSQL on a single host.

## Status

**Plan 1 (foundation + create) — implemented.** Snapshot, restore, clone, upgrade, and TUI come in Plans 2–5.

## Quick start

1. Install Rust 1.80+ and a working Docker engine (OrbStack > Docker Desktop on macOS for performance).
2. Build:
   ```bash
   cargo build --release
   ```
3. Configure S3 credentials. Create `~/.config/pgforge/config.toml` (Linux) or
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
4. Spawn an instance:
   ```bash
   PGFORGE_APP_PASSWORD=changeme \
   PGFORGE_PGBACKREST_PASSWORD=changeme2 \
   ./target/release/pgforge create \
       --name billing \
       --preset tiny \
       --version 18
   ```
5. Connect:
   ```bash
   psql "postgresql://leads:changeme@127.0.0.1:<port>/billing"
   ```
   (The port is printed at the end of `create` and saved in `~/.local/share/pgforge/instances/billing/state.toml`.)

## Architecture

Each instance is a Docker container running `postgres:<version>` with
`pgbackrest` baked into the image, hardened defaults applied via a generated
`postgresql.conf`, and WAL pushed asynchronously to S3 with a 60-second
`archive_timeout` (so worst-case data loss on a host crash is ~60s).

See `docs/plans/2026-05-11-foundation-and-create.md` for the implementation
plan that built this scaffold, and the upcoming `2026-XX-XX-*.md` plans for
snapshot / restore / clone / upgrade / TUI.

## Caveats

- **macOS host**: Docker Desktop and OrbStack run containers in a Linux VM.
  fsync semantics through that VM are weaker than bare-metal Linux. pgforge
  sets `wal_sync_method = fsync_writethrough` to force `F_FULLFSYNC`, but
  true RDS-grade durability is not achievable on macOS — use a UPS for Mac
  mini deployments and rely on the 60-second S3 backup window as your real
  durability guarantee.
- **No HA**: pgforge is intentionally single-host, no replication, no
  failover. Same model as RDS Single-AZ.
