# `pgforge dump` — local portable database dump

**Date:** 2026-05-14
**Status:** design + plan-review integrated, pending final spec review

## Purpose

Give the operator a one-command way to produce a **fresh, portable logical backup**
of a managed instance's database as a file on the server. They copy that file to
their laptop and `pg_restore` it into any local Postgres — to quickly run
production data locally for development/debugging.

This is a **dev-convenience** feature, not disaster recovery (pgbackrest + the
restore path already cover DR). "Freshest copy" is an explicit requirement, so the
dump is taken from the **live instance**, not from an R2 backup.

pgforge's whole job is: produce the file, print its path on stdout. The operator
wires their own `make` target (ssh to the server → `pgforge dump` → `scp` the file
back) — the transfer is out of scope, and the command must stay **non-interactive**
so it works from `make`.

## CLI surface

```
pgforge dump --name <instance> [--out <file-path>] [--force] [--keep <N>] [--timeout <secs>]
```

- `--name <instance>` — required. Matches the `--name` convention of `create` /
  `destroy` / `snapshot`.
- `--out <file-path>` — optional, **always a full file path** (no dir-vs-file
  detection). The parent dir is created if missing. Omitted → default
  `$HOME/pgforge-dumps/<instance>-<now_iso timestamp>.dump`.
- `--force` — optional. Overwrite the destination if a file already exists there.
  Without it, an existing destination is a hard error.
- `--keep <N>` — optional. After a successful dump, delete older `*.dump` files for
  this instance in the dump directory, keeping the newest `N`. Omitted → keep all
  (but a total-size line is printed to stderr — see below).
- `--timeout <secs>` — optional, default `1800` (30 min). Hard cap on the dump.

**stdout contract:** exactly **one line — the absolute path of the finished file**,
nothing else, so `scp $(ssh server pgforge dump --name x)` works. Size, warnings,
the production-data reminder, and all `tracing` output go to **stderr**. Exit `0`
on success, non-zero on every failure path.

No TUI button in v1 (CLI is the priority). It can be added later as a thin wrapper
over `run_with_engine`.

## Behaviour / flow

`src/commands/dump.rs` exposes the `run` / `run_with_engine` split used by
`clone` / `restore` / `destroy`, so the `cli.rs` arm is a pure pass-through and the
testable logic lives in `run_with_engine`.

`DumpArgs { name: String, out: Option<PathBuf>, force: bool, keep: Option<usize>,
timeout_secs: u64, override_state_root: Option<PathBuf> }`.

1. **Validate + load.** `Instance::validate_name(&args.name)?` as the literal first
   line (the name flows into the container name and the default filename).
   State-root resolution mirrors `snapshot`/`status`:
   `args.override_state_root.unwrap_or_else(InstanceState::default_state_root)` then
   `InstanceState::load_under` (`InstanceNotFound` if absent).
2. **Container check.** Connect to Docker; container name is
   `format!("pgforge_{}", state.instance.name)` (the existing convention — a shared
   `container_name()` helper would be a good cleanup but is deferred, out of scope
   here). If `!container_running` → error: `instance "<name>" is not running; start
   it first — pg_dump needs a live server`.
   *Known limitation, documented in `--help`:* `dump` only checks "container
   running"; it does not coordinate with `upgrade`/`restore`/`clone`, which can
   leave the container transiently running mid-rewrite. Dumping an instance during
   one of those operations is unsupported and may yield an inconsistent file.
3. **Resolve paths + precheck.**
   - Resolve the dump directory (`--out`'s parent, or `$HOME/pgforge-dumps/` —
     `~` is **not** shell-expanded; resolve via `$HOME` explicitly). Create it with
     `crate::util::fs::create_secret_dir` (mode 0700 — the dir holds production
     data).
   - Resolve the final path. If it exists and `--force` is not set → hard error
     before any expensive work.
   - **Free-space precheck:** `df` the target filesystem; if free space is below a
     floor (5 GiB) → refuse with a clear error. The dump dir shares the disk with
     live PG data volumes; a full disk takes down every instance on the host.
   - Sweep stale `*.partial` files (>24 h old) in the dump directory — orphans from
     prior killed/crashed runs (analogous to `docker/cleanup.rs::cleanup_partial`).
   - Compute the temp path `<final>.<pid>.partial` (per-pid unique, like
     `atomic_write`'s `{pid}.tmp`).
4. **Stream the dump.** Create the `.partial` file with
   `OpenOptions::new().write(true).create_new(true).mode(0o600)` — `create_new`
   (O_EXCL) fails loud on any collision; 0600 means the production data is never
   world-readable, and the later `rename` preserves the mode onto the final file.
   Register a RAII drop-guard holding the `.partial` path; it removes the file on
   any panic / early return and is disarmed only after a successful `rename`.
   - `tracing::info!(target: "pgforge::dump", ...)` a **start line** to stderr
     before streaming (a `pg_dump` runs for minutes silently otherwise), including
     a "reads live production data" warning.
   - Run, inside the container, via `exec_to_file` (see below):
     `pg_dump -Fc -U <app_user> -h /var/run/postgresql <db_name>`
     — `-Fc` custom format (compressed, `pg_restore`-able anywhere). (No
     `--lock-timeout`: that is a server GUC, not a pg_dump CLI flag — the
     overall `tokio::time::timeout` bounds a dump stuck behind a lock.)
     Connects over the container's local socket as `<app_user>`, which
     `generate_pg_hba` already trusts (`local all <app_user> trust`) — **no
     password on argv/env**,
     so nothing leaks via `docker inspect` or `/proc`.
   - Wrap the whole stream-consumption in `tokio::time::timeout(args.timeout_secs)`.
     On timeout → guard removes `.partial`, error: `pg_dump exceeded <N>s; the
     instance may have a lock or hung process`.
5. **Verify before commit.** On a non-zero exit code, or `exit_code: None`
   (container died mid-dump), or any stream error → guard cleans up, return a
   distinct error (see error table). On exit 0: `flush` + `sync_all` the file,
   then assert it is non-empty **and** begins with the `-Fc` magic header `PGDMP`
   — a clean exit with a truncated/empty file is still a failure.
6. **Commit.** `rename` `<final>.<pid>.partial` → `<final>`, then
   `crate::util::fs::fsync_dir(parent)` (already `pub`) so the rename survives
   power loss — matching `atomic_write`'s durability. Disarm the RAII guard.
   Apply `--keep N` retention if set.
7. **Report.** Print the absolute final path to **stdout** (one line, nothing
   else). To **stderr**: `tracing::info!` success with path + size + elapsed; a
   human size line; the current `pgforge-dumps/` total size (always — a cheap
   nudge against unbounded growth); and a one-line "contains production data —
   delete after transfer" reminder.

## New Docker engine method

`DockerEngine::exec` returns `ExecOutput { stdout: String, .. }` — unusable here:
`-Fc` output is binary and can be gigabytes. Add:

```rust
/// Run `cmd` in `container`, streaming process stdout directly into `dest`.
/// stderr is captured to a String. Returns the exit code, or an explicit
/// error — exit_code is NEVER coerced to 0 on an unknown/None result.
async fn exec_to_file(
    &self, container: &str, cmd: &[&str], dest: &std::path::Path,
) -> Result<ExecToFileOutput>;   // { exit_code: i64, stderr: String }
```

**Reuse, do not copy-paste.** `bollard_engine.rs` already has the identical
`create_exec → start_exec → drain loop → inspect_exec` block in `exec`, `exec_as`,
and `exec_with_stdin` (~50 lines, ~90% identical — already flagged in the codebase
review). Extract a private helper parameterized by a stdout sink
(`enum StdoutSink<'a> { Buffer(&'a mut String), File(&'a mut tokio::fs::File) }`)
and make all four methods thin wrappers over it. `exec_to_file` is then the
`File` variant.

Error mapping (specified, not collapsed):
- `create_exec` / `start_exec` failure → `PgForgeError::Docker`
- a stream item that is `Err(_)` (mid-stream transport error) →
  `PgForgeError::Docker("exec_to_file stream: {e}")`
- a file-write error → `PgForgeError::Io { path: dest, source }`
- `inspect_exec` must be called **only after the stream is fully drained**
  (calling it early yields `exit_code: None`); its failure → `PgForgeError::Docker`
- `inspect_exec` returning `exit_code: None` → explicit `PgForgeError::Docker`,
  never `0`.

Every existing in-test mock that implements `DockerEngine` (the test modules in
`create.rs` and `destroy.rs`, and `wait_test.rs`'s `RecordingEngine`) gets a
trivial `exec_to_file` impl. (A shared `#[cfg(test)]` stub engine would avoid the
N-mock edit churn but is a separate cleanup — out of scope here; the spec just
acknowledges the 3-mock edit.)

## Error handling

`.partial` cleanup is **best-effort and runs on every error path**, routed through
the single RAII guard: if `remove_file` itself fails, `tracing::warn!` it and still
return the *original* error — never mask the real diagnostic, never panic.

| Situation | Behaviour |
|---|---|
| instance doesn't exist | `InstanceNotFound` |
| container not running | clear error: start it first |
| destination exists, no `--force` | hard error before any work |
| free space below 5 GiB floor | refuse before streaming |
| `pg_dump` exit 127 | distinct error: `pg_dump not found in the instance image — recreate/rotate the instance` |
| `pg_dump` other non-zero exit | `pg_dump failed (exit {code}): {stderr.trim()}` (mirrors `snapshot.rs`) |
| Docker stream error mid-dump | `.partial` removed; `PgForgeError::Docker` |
| container dies mid-dump (`exit_code: None`) | `.partial` removed; `instance "<name>" stopped during dump` |
| timeout exceeded | `.partial` removed; `pg_dump exceeded <N>s` |
| exit 0 but empty / no `PGDMP` header | `.partial` removed; `pg_dump produced a truncated dump` |
| disk full / IO error while streaming | `PgForgeError::Io { path }`; `.partial` removed |
| dump dir / `--out` parent not creatable | `PgForgeError::Io` carrying the *directory* path |

`pg_dump` stderr is safe to surface verbatim — unlike pgbackrest it carries no
secrets (no S3 keys, no passwords). Note this in code so a future edit doesn't add
redaction it doesn't need, nor a password it must not.

## Concurrency

`dump` is a **pure reader** of `state.toml` (immutable fields only) — it takes no
`LockedStateRoot` and writes no pgforge state; it relies on `atomic_write`'s
rename semantics for a consistent read (`InstanceState::load_under` reads in a
single pass — verify during implementation). Two concurrent dumps of the same
instance cannot collide: the `.partial` path is per-pid-unique and opened
`create_new`, and the default final filename is timestamped; an explicit identical
`--out` from two runs is caught by the destination-exists check.

## Testing

- **Unit (`tests/dump_test.rs`)** — path resolution as a pure function: default
  path shape + timestamp format; `--out` passthrough. Plus: `Instance::validate_name`
  rejection; destination-exists refusal without `--force`; container-not-running
  guard via a mock engine. Assert the dump dir is created 0700 and (where unit-
  testable) the `.partial` is 0600.
- **E2E (gated, `PGFORGE_E2E=1`)** — `pgforge dump` against a real running
  instance: file exists at the printed path, is non-empty, starts with `PGDMP`,
  no leftover `.partial`, and `pg_restore --list <file>` exits 0 (proves validity).
  Mode of the final file is 0600.
- The streaming `exec_to_file` and the `df` precheck are covered by the E2E.

## Files touched

- `src/commands/dump.rs` — new (`run` / `run_with_engine`, path resolution, RAII
  `.partial` guard, `df` precheck, `--keep` retention)
- `src/commands/mod.rs` — `pub mod dump;`
- `src/cli.rs` — `Dump` subcommand + thin dispatch arm (stdout = path only)
- `src/docker/engine.rs` — `exec_to_file` on the trait + `ExecToFileOutput`
- `src/docker/bollard_engine.rs` — extract the shared drain-loop helper with a
  `StdoutSink` enum; reimplement `exec`/`exec_as`/`exec_with_stdin` over it; add
  `exec_to_file`
- existing in-test `DockerEngine` mocks (`create.rs`, `destroy.rs`, `wait_test.rs`)
  — trivial `exec_to_file`
- `tests/dump_test.rs` — new

## Out of scope

- TUI button (later; thin wrapper over `run_with_engine`).
- Dumping from an R2 backup instead of the live instance (conflicts with the
  "freshest copy" requirement; revisit only if live-DB load becomes a real problem).
- Transferring the file off the server — the operator's own `make`/`scp` glue.
- Plain-SQL (`-Fp`) output — `-Fc` is strictly more capable.
- A durable dump registry (`dumps.toml`) — a dump is a transient artifact; the
  `tracing` line + the printed path are the proportionate trace.
- A cross-command per-instance operation lock so `dump` can refuse during
  `upgrade`/`restore`/`clone` — worth doing, but it touches those commands and
  belongs with the broader locking work, not this feature. Documented as a known
  limitation instead.
- `nice`/`ionice` throttling of `pg_dump` vs. other instances on the host —
  `ionice` doesn't exist on macOS; documented as a known limitation.
- Interactive confirmation above a size threshold — rejected: the command must
  stay non-interactive for the `make`-target use case. The start-line warning to
  stderr is the chosen mitigation.
