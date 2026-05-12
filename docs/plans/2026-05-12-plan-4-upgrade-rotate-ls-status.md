# Plan 4 — upgrade, rotate, ls, status, --no-backup

Shipped 2026-05-12 across 11 commits (`36f6c23..d177012`). Written
retrospectively — the plan was iterated on in-session rather than
specced upfront.

## Goals

After Plan 3.5 + the critical bind-mount fix, the remaining gaps for a
useful single-host Postgres provisioner were:

1. **Local dev without S3** — every command required real S3 credentials,
   making `cargo test --test create_e2e_test` impossible to run on a
   fresh checkout.
2. **Migration path** — Plan 3.5 fixed the bind-mount bug (postgres was
   reading initdb defaults, not our hardened conf) but pre-fix instances
   stayed broken until they were destroyed and recreated. No safe way
   to apply the fix in place.
3. **In-place upgrade** — Plan 1's roadmap promised it; `pg_upgrade`
   between major versions is the canonical RDS operation users expect.
4. **Visibility** — no `ls` of managed instances, no `status` per
   instance, no PITR-window display. The TUI (Plan 5) needs all three
   as a backend.

## Tasks shipped

### Faza A — Local dev unblocked
- **A-1** `pgforge create --no-backup` skips pgbackrest setup entirely.
  No S3 requirement, no archive_mode, no stanza-create. State.toml records
  `backup_enabled=false`; `snapshot`/`clone`/`restore` refuse on it.
  (`Instance.backup_enabled` is `#[serde(default=true)]` for backward
  compat with pre-Plan-4 state.toml files.)
- **A-2** `tests/create_nobackup_e2e_test.rs` — first E2E test that runs
  end-to-end without infrastructure setup. Caught a Plan-1 latent bug
  along the way: `wal_sync_method=fsync_writethrough` is macOS-native
  but postgres runs inside Linux — fixed to always emit `fdatasync`.

### Faza B — Migration story
- **B-1** `pgforge rotate <name>` stops + removes the container, keeps
  the volume, regenerates configs from current pgforge code, recreates
  the container on the same volume with current cmd_override flags.
  For backup-enabled instances, ensures the post-Plan-3.5 `pgreplica`
  role exists via a DO block (instances created before P3.5 only have
  `pgbackrest`, and initdb hooks only run on empty PGDATA).
- **B-2** E2E test verifies: container ID changes, port still accepts
  TCP, `SHOW config_file` points at bind-mount (proves Plan 3.5 fix
  applies), seeded row survives.

### Faza C — Upgrade in place
- **C-1** `pgforge upgrade --name X --to <ver>`:
  - Take pre-upgrade snapshot (backup-enabled only).
  - Stop + remove old container, keep its volume for rollback.
  - Build `pgforge/upgrade:<from>-to-<to>` image: based on
    postgres:<to>-bookworm, apt-installs postgresql-<from> from pgdg so
    both binaries coexist.
  - Create new volume `pgforge_data_<name>_v<to>`.
  - Run one-shot upgrade container: initdb new + pg_upgrade old→new
    (no `--link` so old vol survives).
  - On exit_code != 0: remove new vol, leave old + snapshot intact,
    surface error.
  - On success: persist `pg_version=to` + `volume_name_override=<new vol>`,
    recreate regular container on upgraded volume.
  - Adds `DockerEngine::wait_for_container_exit` + `Instance.volume_name()`
    helper.
- **C-2** E2E test (gated, takes ~3-5 min on first run because of image
  build) verifies pg_upgrade 17→18 preserves a marker row + bumps
  server_version_num. Per user preference, not auto-run in session.

### Faza D — Visibility / TUI backend
- **D-1** `pgforge ls` — reads state_root/instances/*/state.toml, tags
  each row with docker-running status, prints a short table. Rust API
  returns `Vec<InstanceSummary>` for the TUI to consume.
- **D-2** `pgforge snapshots --name X` now also queries
  `pgbackrest --output=json info` inside the container and prints a
  "PITR window: <from> .. <to>" line. Adds `serde_json` dep; hand-rolled
  epoch→ISO formatter (Howard Hinnant) avoids pulling chrono.
- **D-3** `pgforge status --name X` — one-shot snapshot of cpu / memory
  (via `docker stats`), connections (pg_stat_activity), db size, on-disk
  PGDATA size.

## Skipped / out of scope

- **W3b** (restore writes WAL into source stanza) — documented as known
  limitation in README; structural issue best fixed alongside a
  promote-flow in a future plan.
- **W4** (massive deduplication across create/clone/restore/rotate) —
  pure refactor, deferred. Plan 5 TUI will likely surface more shared
  shape and is a better moment.
- **MIN/MAX supported PG version** — per user direction, no opinionated
  range. Whatever pgdg apt has is fair game.

## Open issues to investigate in Plan 5

- Plan 5 (TUI dashboard, ratatui) is the next destination. The
  Vec<InstanceSummary> / InstanceStatus / PitrWindow shapes are already
  the right shape for tick-driven refresh.
- pg_upgrade rolling forward to PG 19 once pgdg ships those packages —
  upgrade-image dockerfile already handles arbitrary version pairs.
