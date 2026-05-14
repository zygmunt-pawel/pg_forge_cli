# `pgforge dump` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `pgforge dump --name <instance>` CLI command that runs `pg_dump -Fc` inside a live instance's container and streams the output to a crash-safe, 0600, portable `.dump` file on the server, printing the path on stdout.

**Architecture:** A new `src/commands/dump.rs` (`run` / `run_with_engine` split, like `clone`/`restore`). A new `DockerEngine::exec_to_file` trait method streams a container process's stdout to a file; it is implemented over a newly-extracted private `drain_exec` helper in `bollard_engine.rs` so `exec`/`exec_as`/`exec_to_file` share one drain loop instead of three copies. Crash-safety reuses `crate::util::fs` (`create_secret_dir`, `fsync_dir`) and an `atomic_write`-style per-pid `.partial` + `rename`. Pure logic (path resolution, `df` parsing, dump-header validation, `--keep` retention) is unit-tested; the streaming path is covered by a gated E2E test.

**Tech Stack:** Rust 2024, tokio, bollard 0.21, clap 4 (derive), jiff, the existing `DockerEngine` trait + `PgForgeError`.

**Spec:** `docs/superpowers/specs/2026-05-14-local-dump-design.md`

---

## File structure

| File | Responsibility |
|---|---|
| `src/docker/engine.rs` (modify) | Add `ExecToFileOutput` struct + `exec_to_file` to the `DockerEngine` trait |
| `src/docker/bollard_engine.rs` (modify) | Extract private `drain_exec` + `StdoutSink`; reimplement `exec`/`exec_as` over it; add `exec_to_file` |
| `src/commands/create.rs` (modify) | Add trivial `exec_to_file` to the test-module `RecordingEngine` |
| `src/commands/destroy.rs` (modify) | Add trivial `exec_to_file` to the test-module `MockDocker` |
| `tests/wait_test.rs` (modify) | Add trivial `exec_to_file` to `RecordingEngine` |
| `src/commands/dump.rs` (create) | `DumpArgs`, pure helpers (path resolution, `df` parse, dump-header check, `--keep`), `PartialGuard`, `run` / `run_with_engine` |
| `src/commands/mod.rs` (modify) | `pub mod dump;` |
| `src/cli.rs` (modify) | `Dump` subcommand + dispatch arm |
| `tests/dump_test.rs` (create) | Unit tests for the pure helpers + guard logic (mock engine) + gated E2E |

`exec_with_stdin` is intentionally **left untouched** — it has stdin-attach logic the other three don't, folding it in is a separate cleanup, and leaving it is not "a 4th copy" of anything new.

---

## Task 1: Engine layer — `exec_to_file` + shared `drain_exec`

**Files:**
- Modify: `src/docker/engine.rs`
- Modify: `src/docker/bollard_engine.rs`
- Modify: `src/commands/create.rs` (test module), `src/commands/destroy.rs` (test module)
- Modify: `tests/wait_test.rs`

This task is a behavior-preserving refactor + an interface addition. There is no unit test for it: the existing `cargo test` suite exercises `DockerEngine` only through mocks, and the real bollard `drain_exec` is exercised only by the gated E2E (Task 8) — which runs `exec` heavily via every create/snapshot. Verification here is: it compiles and the existing 213 tests stay green; the E2E in Task 8 confirms the real streaming.

- [ ] **Step 1: Add `ExecToFileOutput` and the trait method to `src/docker/engine.rs`**

Add after the `ExecOutput` struct:

```rust
#[derive(Debug, Clone)]
pub struct ExecToFileOutput {
    pub exit_code: i64,
    pub stderr: String,
}
```

Add to the `DockerEngine` trait, after `exec_with_stdin`:

```rust
    /// Run `cmd` inside `container`, streaming the process's stdout directly
    /// into a freshly-created file at `dest` (created with O_EXCL so an
    /// existing file is a hard error; mode 0600 on unix — the content may be
    /// production data). stderr is captured to a String. Returns the exit
    /// code; an unknown/missing exit code (container died mid-exec) is an
    /// `Err`, never silently 0.
    async fn exec_to_file(
        &self,
        container: &str,
        cmd: &[&str],
        dest: &std::path::Path,
    ) -> Result<ExecToFileOutput>;
```

- [ ] **Step 2: Extract `StdoutSink` + `drain_exec` in `src/docker/bollard_engine.rs`**

Add a private `enum` and an `impl BollardEngine` helper method (place it just above the `impl DockerEngine for BollardEngine` block):

```rust
/// Where an exec's stdout bytes are routed.
enum StdoutSink<'a> {
    /// UTF-8-lossy into a String — for text commands.
    Buffer(&'a mut String),
    /// Raw bytes into a file — for binary output (pg_dump -Fc).
    File(&'a mut tokio::fs::File),
}

impl BollardEngine {
    /// Shared exec driver: create_exec + start_exec, drain the output stream
    /// (stdout → `sink`, stderr → the returned String), then inspect_exec for
    /// the exit code. `exit_code` is `None` when Docker reports no code
    /// (container died) — callers decide how to treat that.
    async fn drain_exec(
        &self,
        container: &str,
        opts: bollard::exec::CreateExecOptions<String>,
        mut sink: StdoutSink<'_>,
    ) -> Result<(Option<i64>, String)> {
        use bollard::exec::{StartExecOptions, StartExecResults};
        use bollard::container::LogOutput;
        use tokio::io::AsyncWriteExt;

        let create = self
            .docker
            .create_exec(container, opts)
            .await
            .map_err(|e| PgForgeError::Docker(format!("create_exec: {e}")))?;
        let mut stderr = String::new();
        match self
            .docker
            .start_exec(&create.id, Some(StartExecOptions { detach: false, ..Default::default() }))
            .await
            .map_err(|e| PgForgeError::Docker(format!("start_exec: {e}")))?
        {
            StartExecResults::Attached { mut output, .. } => {
                while let Some(chunk) = output.next().await {
                    match chunk {
                        Ok(LogOutput::StdOut { message }) | Ok(LogOutput::Console { message }) => {
                            match &mut sink {
                                StdoutSink::Buffer(s) => {
                                    s.push_str(&String::from_utf8_lossy(&message));
                                }
                                StdoutSink::File(f) => {
                                    f.write_all(&message).await.map_err(|e| {
                                        PgForgeError::Docker(format!("exec_to_file write: {e}"))
                                    })?;
                                }
                            }
                        }
                        Ok(LogOutput::StdErr { message }) => {
                            stderr.push_str(&String::from_utf8_lossy(&message));
                        }
                        Ok(_) => {}
                        Err(e) => {
                            return Err(PgForgeError::Docker(format!("exec stream: {e}")));
                        }
                    }
                }
            }
            StartExecResults::Detached => {}
        }
        let inspect = self
            .docker
            .inspect_exec(&create.id)
            .await
            .map_err(|e| PgForgeError::Docker(format!("inspect_exec: {e}")))?;
        Ok((inspect.exit_code, stderr))
    }
}
```

- [ ] **Step 3: Reimplement `exec` and `exec_as` over `drain_exec`**

Replace the existing `exec` body with:

```rust
    async fn exec(&self, id: &str, cmd: &[&str]) -> Result<crate::docker::engine::ExecOutput> {
        use bollard::exec::CreateExecOptions;
        use crate::docker::engine::ExecOutput;
        let opts = CreateExecOptions {
            cmd: Some(cmd.iter().map(|s| s.to_string()).collect()),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            ..Default::default()
        };
        let mut stdout = String::new();
        let (exit_code, stderr) =
            self.drain_exec(id, opts, StdoutSink::Buffer(&mut stdout)).await?;
        Ok(ExecOutput { stdout, stderr, exit_code: exit_code.unwrap_or(-1) })
    }
```

Replace the existing `exec_as` body with:

```rust
    async fn exec_as(
        &self,
        container: &str,
        user: &str,
        cmd: &[&str],
    ) -> Result<crate::docker::engine::ExecOutput> {
        use bollard::exec::CreateExecOptions;
        use crate::docker::engine::ExecOutput;
        let opts = CreateExecOptions {
            cmd: Some(cmd.iter().map(|s| s.to_string()).collect()),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            user: Some(user.to_string()),
            ..Default::default()
        };
        let mut stdout = String::new();
        let (exit_code, stderr) =
            self.drain_exec(container, opts, StdoutSink::Buffer(&mut stdout)).await?;
        Ok(ExecOutput { stdout, stderr, exit_code: exit_code.unwrap_or(-1) })
    }
```

Leave `exec_with_stdin` exactly as it is.

- [ ] **Step 4: Implement `exec_to_file` in `bollard_engine.rs`**

Add to the `impl DockerEngine for BollardEngine` block, after `exec_with_stdin`:

```rust
    async fn exec_to_file(
        &self,
        container: &str,
        cmd: &[&str],
        dest: &std::path::Path,
    ) -> Result<crate::docker::engine::ExecToFileOutput> {
        use bollard::exec::CreateExecOptions;
        use crate::docker::engine::ExecToFileOutput;
        use tokio::io::AsyncWriteExt;

        // O_EXCL: an existing file at `dest` is a hard error (callers pass a
        // per-pid-unique path). 0600 on unix — the stream may be production data.
        let mut open = tokio::fs::OpenOptions::new();
        open.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            open.mode(0o600);
        }
        let mut file = open.open(dest).await.map_err(|e| PgForgeError::Io {
            path: dest.to_path_buf(),
            source: e,
        })?;

        let opts = CreateExecOptions {
            cmd: Some(cmd.iter().map(|s| s.to_string()).collect()),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            ..Default::default()
        };
        let (exit_code, stderr) = self
            .drain_exec(container, opts, StdoutSink::File(&mut file))
            .await?;
        file.flush().await.map_err(|e| PgForgeError::Io {
            path: dest.to_path_buf(),
            source: e,
        })?;
        file.sync_all().await.map_err(|e| PgForgeError::Io {
            path: dest.to_path_buf(),
            source: e,
        })?;
        let exit_code = exit_code.ok_or_else(|| {
            PgForgeError::Docker(format!(
                "exec_to_file({container}): no exit code — the container likely died mid-exec"
            ))
        })?;
        Ok(ExecToFileOutput { exit_code, stderr })
    }
```

- [ ] **Step 5: Add trivial `exec_to_file` to the three in-test mocks**

In `src/commands/create.rs` test module's `RecordingEngine`, after its `exec_with_stdin`:

```rust
        async fn exec_to_file(
            &self,
            _: &str,
            _: &[&str],
            _: &std::path::Path,
        ) -> crate::error::Result<crate::docker::engine::ExecToFileOutput> {
            self.calls.lock().unwrap().push("exec_to_file");
            Ok(crate::docker::engine::ExecToFileOutput {
                exit_code: 0,
                stderr: String::new(),
            })
        }
```

In `src/commands/destroy.rs` test module's `MockDocker`, after its `exec_with_stdin`:

```rust
        async fn exec_to_file(
            &self,
            _: &str,
            _: &[&str],
            _: &std::path::Path,
        ) -> Result<crate::docker::engine::ExecToFileOutput> {
            Ok(crate::docker::engine::ExecToFileOutput {
                exit_code: 0,
                stderr: String::new(),
            })
        }
```

In `tests/wait_test.rs`'s `RecordingEngine`, after its `exec_with_stdin`:

```rust
    async fn exec_to_file(
        &self,
        _: &str,
        _: &[&str],
        _: &std::path::Path,
    ) -> Result<pgforge::docker::engine::ExecToFileOutput> {
        unimplemented!()
    }
```

- [ ] **Step 6: Verify build + existing tests stay green**

Run: `cargo test --quiet 2>&1 | grep -E "test result|error" | tail -40`
Expected: no `error[` lines; every `test result:` line says `ok`; total still 213 passing (the suite count is unchanged — this task adds no tests).

- [ ] **Step 7: Commit**

```bash
git add src/docker/engine.rs src/docker/bollard_engine.rs src/commands/create.rs src/commands/destroy.rs tests/wait_test.rs
git commit -m "$(cat <<'EOF'
feat(docker): add exec_to_file streaming engine method

Streams a container process's stdout straight to a 0600 O_EXCL file —
needed for binary pg_dump output that ExecOutput's String can't hold.
exec/exec_as/exec_to_file now share one drain_exec helper instead of
three copy-pasted drain loops.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `dump.rs` skeleton + `resolve_dump_path`

**Files:**
- Create: `src/commands/dump.rs`
- Modify: `src/commands/mod.rs`
- Create: `tests/dump_test.rs`

`resolve_dump_path` is pure and the highest-value thing to TDD: it decides where the file lands.

- [ ] **Step 1: Write the failing test in `tests/dump_test.rs`**

```rust
use pgforge::commands::dump::resolve_dump_path;
use std::path::PathBuf;

#[test]
fn resolve_dump_path_default_uses_dump_dir_instance_and_timestamp() {
    let p = resolve_dump_path(
        None,
        "billing",
        &PathBuf::from("/home/pawel/pgforge-dumps"),
        "2026-05-14T09:30:00Z",
    );
    assert_eq!(
        p,
        PathBuf::from("/home/pawel/pgforge-dumps/billing-20260514-093000.dump")
    );
}

#[test]
fn resolve_dump_path_out_override_is_used_verbatim() {
    let p = resolve_dump_path(
        Some(PathBuf::from("/tmp/mine.dump")),
        "billing",
        &PathBuf::from("/home/pawel/pgforge-dumps"),
        "2026-05-14T09:30:00Z",
    );
    assert_eq!(p, PathBuf::from("/tmp/mine.dump"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test dump_test --quiet 2>&1 | tail -10`
Expected: FAIL — `unresolved import \`pgforge::commands::dump\`` (the module does not exist yet).

- [ ] **Step 3: Create `src/commands/dump.rs` with the minimal implementation**

```rust
//! `pgforge dump --name <instance>` — stream a `pg_dump -Fc` of a live
//! instance to a portable, crash-safe, 0600 `.dump` file on the server.

use std::path::{Path, PathBuf};

/// Decide the final dump file path. `out` (if given) is used verbatim — it
/// is always a full file path. Otherwise the file lands in `dump_dir` as
/// `<instance>-<YYYYMMDD-HHMMSS>.dump`, where the timestamp is derived from
/// `now_iso` (a `YYYY-MM-DDTHH:MM:SSZ` string from `crate::time::now_iso`).
pub fn resolve_dump_path(
    out: Option<PathBuf>,
    instance: &str,
    dump_dir: &Path,
    now_iso: &str,
) -> PathBuf {
    if let Some(out) = out {
        return out;
    }
    // "2026-05-14T09:30:00Z" -> "20260514-093000"
    let compact: String = now_iso
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>();
    let stamp = if compact.len() >= 14 {
        format!("{}-{}", &compact[0..8], &compact[8..14])
    } else {
        compact
    };
    dump_dir.join(format!("{instance}-{stamp}.dump"))
}
```

Add to `src/commands/mod.rs`, keeping the list alphabetical (between `destroy` and `ls`):

```rust
pub mod dump;
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --test dump_test --quiet 2>&1 | tail -5`
Expected: PASS — `test result: ok. 2 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/commands/dump.rs src/commands/mod.rs tests/dump_test.rs
git commit -m "$(cat <<'EOF'
feat(dump): add dump.rs skeleton with resolve_dump_path

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Dump-file integrity check (`is_valid_custom_dump`)

**Files:**
- Modify: `src/commands/dump.rs`
- Modify: `tests/dump_test.rs`

A clean `pg_dump` exit can still leave a truncated/empty file. Custom-format dumps begin with the ASCII magic `PGDMP`. This pure check gates the `rename`.

- [ ] **Step 1: Write the failing test**

Append to `tests/dump_test.rs`:

```rust
use pgforge::commands::dump::is_valid_custom_dump;

#[test]
fn valid_custom_dump_recognises_pgdmp_header() {
    assert!(is_valid_custom_dump(b"PGDMP\x01\x0e\x00"));
}

#[test]
fn valid_custom_dump_rejects_empty_and_truncated() {
    assert!(!is_valid_custom_dump(b""));
    assert!(!is_valid_custom_dump(b"PGD"));
    assert!(!is_valid_custom_dump(b"-- plain sql dump\n"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test dump_test --quiet 2>&1 | tail -10`
Expected: FAIL — `cannot find function \`is_valid_custom_dump\``.

- [ ] **Step 3: Write the minimal implementation**

Append to `src/commands/dump.rs`:

```rust
/// True iff `head` (the first bytes of a file) is the start of a pg_dump
/// custom-format archive. A clean `pg_dump` exit with a 0-byte or truncated
/// file is still a failed dump; the `PGDMP` magic is the cheapest reliable
/// "this is a real dump" gate before we rename `.partial` into place.
pub fn is_valid_custom_dump(head: &[u8]) -> bool {
    head.starts_with(b"PGDMP")
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --test dump_test --quiet 2>&1 | tail -5`
Expected: PASS — `test result: ok. 4 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/commands/dump.rs tests/dump_test.rs
git commit -m "$(cat <<'EOF'
feat(dump): add is_valid_custom_dump header check

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `df` free-space parsing (`parse_df_available_kb`)

**Files:**
- Modify: `src/commands/dump.rs`
- Modify: `tests/dump_test.rs`

The dump dir shares the disk with live PG data volumes; a `df` precheck refuses to start a multi-GB dump that would fill the disk. The parser is pure and testable; the actual `df` invocation is wired in Task 6.

- [ ] **Step 1: Write the failing test**

Append to `tests/dump_test.rs`:

```rust
use pgforge::commands::dump::parse_df_available_kb;

#[test]
fn parse_df_reads_available_column_from_posix_output() {
    // `df -P -k <dir>` output: header line, then one data line.
    let out = "Filesystem 1024-blocks      Used Available Capacity Mounted on\n\
               /dev/disk3s1s1 482797652 12222540 458123880       3% /\n";
    assert_eq!(parse_df_available_kb(out), Some(458_123_880));
}

#[test]
fn parse_df_returns_none_on_garbage() {
    assert_eq!(parse_df_available_kb(""), None);
    assert_eq!(parse_df_available_kb("just a header\n"), None);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test dump_test --quiet 2>&1 | tail -10`
Expected: FAIL — `cannot find function \`parse_df_available_kb\``.

- [ ] **Step 3: Write the minimal implementation**

Append to `src/commands/dump.rs`:

```rust
/// Parse the "Available" column (1K blocks) from `df -P -k <dir>` output.
/// POSIX (`-P`) format guarantees one data line, columns:
/// Filesystem, 1024-blocks, Used, Available, Capacity, Mounted-on.
pub fn parse_df_available_kb(df_output: &str) -> Option<u64> {
    let data_line = df_output.lines().nth(1)?;
    data_line.split_whitespace().nth(3)?.parse::<u64>().ok()
}

/// Minimum free space (KiB) required before starting a dump. 5 GiB — the
/// dump dir shares the disk with live PG data volumes.
pub const MIN_FREE_KB: u64 = 5 * 1024 * 1024;
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --test dump_test --quiet 2>&1 | tail -5`
Expected: PASS — `test result: ok. 6 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/commands/dump.rs tests/dump_test.rs
git commit -m "$(cat <<'EOF'
feat(dump): add df free-space parsing for the disk precheck

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: `--keep N` retention (`dumps_to_prune`)

**Files:**
- Modify: `src/commands/dump.rs`
- Modify: `tests/dump_test.rs`

`--keep N` deletes older dumps for an instance after a successful run. The selection logic is pure: given the existing dump filenames for an instance and `N`, return the ones to delete.

- [ ] **Step 1: Write the failing test**

Append to `tests/dump_test.rs`:

```rust
use pgforge::commands::dump::dumps_to_prune;

#[test]
fn dumps_to_prune_keeps_newest_n_by_filename() {
    // Default filenames sort chronologically (timestamp is fixed-width).
    let mut files = vec![
        "billing-20260514-093000.dump".to_string(),
        "billing-20260512-093000.dump".to_string(),
        "billing-20260513-093000.dump".to_string(),
    ];
    let prune = dumps_to_prune(&mut files, 2);
    assert_eq!(prune, vec!["billing-20260512-093000.dump".to_string()]);
}

#[test]
fn dumps_to_prune_keeps_all_when_count_within_limit() {
    let mut files = vec!["billing-20260514-093000.dump".to_string()];
    assert!(dumps_to_prune(&mut files, 2).is_empty());
    let mut none: Vec<String> = vec![];
    assert!(dumps_to_prune(&mut none, 0).is_empty());
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test dump_test --quiet 2>&1 | tail -10`
Expected: FAIL — `cannot find function \`dumps_to_prune\``.

- [ ] **Step 3: Write the minimal implementation**

Append to `src/commands/dump.rs`:

```rust
/// Given an instance's dump filenames and a keep-count `n`, return the
/// filenames to delete — everything except the newest `n`. Default dump
/// filenames embed a fixed-width timestamp, so lexicographic sort is
/// chronological. `n == 0` is treated as "keep all" (no pruning).
pub fn dumps_to_prune(files: &mut [String], n: usize) -> Vec<String> {
    if n == 0 || files.len() <= n {
        return Vec::new();
    }
    files.sort();
    let cutoff = files.len() - n;
    files[..cutoff].to_vec()
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --test dump_test --quiet 2>&1 | tail -5`
Expected: PASS — `test result: ok. 8 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/commands/dump.rs tests/dump_test.rs
git commit -m "$(cat <<'EOF'
feat(dump): add --keep retention selection logic

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `PartialGuard` + `run_with_engine` orchestration

**Files:**
- Modify: `src/commands/dump.rs`
- Modify: `tests/dump_test.rs`

This task wires the pure helpers together: `DumpArgs`, a RAII `PartialGuard` that removes the `.partial` file unless disarmed, and `run_with_engine`. The early guards (instance-not-found, container-not-running, destination-exists-without-`--force`) are unit-tested with a mock engine; the streaming/`df`/`fsync`/retention path is integration-tested in Task 8.

- [ ] **Step 1: Write the failing tests**

Append to `tests/dump_test.rs` (this adds a local mock engine — each `tests/*.rs` is its own crate, so it cannot share `wait_test.rs`'s mock):

```rust
use async_trait::async_trait;
use pgforge::commands::dump::{run_with_engine, DumpArgs};
use pgforge::docker::engine::{
    BuildImageSpec, ContainerInspect, CreateContainerSpec, DockerEngine, ExecOutput,
    ExecToFileOutput,
};
use pgforge::domain::instance::Instance;
use pgforge::domain::preset::Preset;
use pgforge::error::Result as PgResult;
use pgforge::state::instance::InstanceState;
use std::time::Duration;
use tempfile::TempDir;

struct DumpMockEngine {
    running: bool,
}

#[async_trait]
impl DockerEngine for DumpMockEngine {
    async fn container_running(&self, _: &str) -> PgResult<bool> {
        Ok(self.running)
    }
    async fn build_image(&self, _: &BuildImageSpec) -> PgResult<()> { unimplemented!() }
    async fn ensure_network(&self, _: &str) -> PgResult<()> { unimplemented!() }
    async fn create_container(&self, _: &CreateContainerSpec) -> PgResult<String> { unimplemented!() }
    async fn start_container(&self, _: &str) -> PgResult<()> { unimplemented!() }
    async fn container_exists(&self, _: &str) -> PgResult<bool> { unimplemented!() }
    async fn exec(&self, _: &str, _: &[&str]) -> PgResult<ExecOutput> { unimplemented!() }
    async fn exec_as(&self, _: &str, _: &str, _: &[&str]) -> PgResult<ExecOutput> { unimplemented!() }
    async fn exec_with_stdin(&self, _: &str, _: &[&str], _: &str) -> PgResult<ExecOutput> { unimplemented!() }
    async fn exec_to_file(&self, _: &str, _: &[&str], _: &std::path::Path) -> PgResult<ExecToFileOutput> {
        unimplemented!()
    }
    async fn stop_container(&self, _: &str) -> PgResult<()> { unimplemented!() }
    async fn wait_for_container_running(&self, _: &str, _: Duration) -> PgResult<()> { unimplemented!() }
    async fn wait_for_container_exit(&self, _: &str, _: Duration) -> PgResult<i64> { unimplemented!() }
    async fn remove_container(&self, _: &str, _: bool) -> PgResult<()> { unimplemented!() }
    async fn remove_volume(&self, _: &str) -> PgResult<()> { unimplemented!() }
    async fn inspect_container(&self, _: &str) -> PgResult<ContainerInspect> { unimplemented!() }
    async fn logs(&self, _: &str) -> PgResult<String> { unimplemented!() }
}

fn write_instance(state_root: &std::path::Path, name: &str) {
    InstanceState {
        instance: Instance {
            name: name.into(),
            db_name: name.into(),
            app_user: "leads".into(),
            app_password: "pw".into(),
            pgbackrest_password: "rpw".into(),
            preset: Preset::Tiny,
            pg_version: 18,
            host_port: 5433,
            backup_enabled: true,
            volume_name_override: None,
            retain_days: 30,
            snapshot_hour: Some(3),
            last_snapshot_at: None,
            last_snapshot_attempt_at: None,
            full_backup_day: 0,
        },
        created_at: "2026-05-12T10:00:00Z".into(),
    }
    .save_under(state_root)
    .unwrap();
}

fn args(name: &str, root: &std::path::Path, out: Option<std::path::PathBuf>) -> DumpArgs {
    DumpArgs {
        name: name.into(),
        out,
        force: false,
        keep: None,
        timeout_secs: 1800,
        override_state_root: Some(root.to_path_buf()),
    }
}

#[tokio::test]
async fn run_with_engine_errors_when_instance_missing() {
    let tmp = TempDir::new().unwrap();
    let eng = DumpMockEngine { running: true };
    let err = run_with_engine(args("ghost", tmp.path(), None), &eng, tmp.path().to_path_buf())
        .await
        .unwrap_err();
    assert!(matches!(err, pgforge::error::PgForgeError::InstanceNotFound(_)));
}

#[tokio::test]
async fn run_with_engine_errors_when_container_not_running() {
    let tmp = TempDir::new().unwrap();
    write_instance(tmp.path(), "billing");
    let eng = DumpMockEngine { running: false };
    let err = run_with_engine(args("billing", tmp.path(), None), &eng, tmp.path().to_path_buf())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not running"));
}

#[tokio::test]
async fn run_with_engine_errors_when_destination_exists_without_force() {
    let tmp = TempDir::new().unwrap();
    write_instance(tmp.path(), "billing");
    let existing = tmp.path().join("already.dump");
    std::fs::write(&existing, b"old").unwrap();
    let eng = DumpMockEngine { running: true };
    let err = run_with_engine(
        args("billing", tmp.path(), Some(existing.clone())),
        &eng,
        tmp.path().to_path_buf(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("already exists"));
    // The pre-existing file must be untouched.
    assert_eq!(std::fs::read(&existing).unwrap(), b"old");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test dump_test --quiet 2>&1 | tail -12`
Expected: FAIL — `cannot find function \`run_with_engine\`` / `cannot find type \`DumpArgs\``.

- [ ] **Step 3: Write the implementation**

Append to `src/commands/dump.rs`:

```rust
use crate::docker::bollard_engine::BollardEngine;
use crate::docker::engine::DockerEngine;
use crate::domain::instance::Instance;
use crate::error::{PgForgeError, Result};
use crate::state::instance::InstanceState;

#[derive(Debug, Clone)]
pub struct DumpArgs {
    pub name: String,
    /// Full file path. `None` → default under `$HOME/pgforge-dumps/`.
    pub out: Option<PathBuf>,
    /// Overwrite the destination if a file already exists there.
    pub force: bool,
    /// Keep only the newest N dumps for this instance after a successful run.
    pub keep: Option<usize>,
    /// Hard cap on the dump in seconds.
    pub timeout_secs: u64,
    pub override_state_root: Option<PathBuf>,
}

/// RAII guard: removes the `.partial` file on drop unless `disarm()` was
/// called. Covers panics and every early-return error path with one object.
struct PartialGuard {
    path: PathBuf,
    armed: bool,
}

impl PartialGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PartialGuard {
    fn drop(&mut self) {
        if self.armed && self.path.exists() {
            if let Err(e) = std::fs::remove_file(&self.path) {
                tracing::warn!(
                    target: "pgforge::dump",
                    "could not remove partial dump {}: {e}",
                    self.path.display()
                );
            }
        }
    }
}

/// Default dump directory: `$HOME/pgforge-dumps/`. `~` is not shell-expanded
/// by Rust — resolve `$HOME` explicitly.
fn default_dump_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| PgForgeError::Anyhow(anyhow::anyhow!("HOME not set")))?;
    Ok(PathBuf::from(home).join("pgforge-dumps"))
}

pub async fn run(args: DumpArgs) -> Result<PathBuf> {
    let state_root = args
        .override_state_root
        .clone()
        .unwrap_or_else(InstanceState::default_state_root);
    let docker = BollardEngine::connect()?;
    run_with_engine(args, &docker, state_root).await
}

pub async fn run_with_engine<E: DockerEngine>(
    args: DumpArgs,
    docker: &E,
    state_root: PathBuf,
) -> Result<PathBuf> {
    Instance::validate_name(&args.name)?;
    let state = InstanceState::load_under(&state_root, &args.name)?;

    let container = format!("pgforge_{}", state.instance.name);
    if !docker.container_running(&container).await? {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "instance {:?} is not running; start it first \
             (pgforge rotate) — pg_dump needs a live server.",
            args.name
        )));
    }

    // Resolve the dump dir + final path.
    let dump_dir = match &args.out {
        Some(out) => out
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".")),
        None => default_dump_dir()?,
    };
    crate::util::fs::create_secret_dir(&dump_dir)?;
    let final_path = resolve_dump_path(
        args.out.clone(),
        &state.instance.name,
        &dump_dir,
        &crate::time::now_iso(),
    );
    if final_path.exists() && !args.force {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "dump already exists at {} — pass --force to overwrite, \
             or --out a different path.",
            final_path.display()
        )));
    }

    // Free-space precheck.
    let df_out = std::process::Command::new("df")
        .args(["-P", "-k"])
        .arg(&dump_dir)
        .output()
        .map_err(|e| PgForgeError::Anyhow(anyhow::anyhow!("df: {e}")))?;
    if let Some(avail) = parse_df_available_kb(&String::from_utf8_lossy(&df_out.stdout)) {
        if avail < MIN_FREE_KB {
            return Err(PgForgeError::Anyhow(anyhow::anyhow!(
                "only {} MiB free on the dump filesystem — refusing to start \
                 a dump (need at least {} MiB). Free space or pass --out elsewhere.",
                avail / 1024,
                MIN_FREE_KB / 1024
            )));
        }
    }

    // Sweep stale *.partial orphans (>24h) from prior killed runs.
    sweep_stale_partials(&dump_dir);

    // Per-pid-unique partial path, like util::fs::atomic_write.
    let partial = final_path.with_extension(format!("{}.partial", std::process::id()));
    let mut guard = PartialGuard::new(partial.clone());

    tracing::info!(
        target: "pgforge::dump",
        "dumping {:?} -> {} (reads LIVE production data)",
        args.name,
        final_path.display()
    );

    // Stream pg_dump into the .partial file, bounded by the timeout.
    let cmd = [
        "pg_dump",
        "-Fc",
        "--lock-timeout=5000",
        "-U",
        state.instance.app_user.as_str(),
        "-h",
        "/var/run/postgresql",
        state.instance.db_name.as_str(),
    ];
    let exec = tokio::time::timeout(
        std::time::Duration::from_secs(args.timeout_secs),
        docker.exec_to_file(&container, &cmd, &partial),
    )
    .await
    .map_err(|_| {
        PgForgeError::Docker(format!(
            "pg_dump exceeded {}s; the instance may have a lock or a hung process.",
            args.timeout_secs
        ))
    })??;

    if exec.exit_code == 127 {
        return Err(PgForgeError::Docker(format!(
            "pg_dump not found in the instance image — recreate/rotate {:?}.",
            args.name
        )));
    }
    if exec.exit_code != 0 {
        return Err(PgForgeError::Docker(format!(
            "pg_dump failed (exit {}): {}",
            exec.exit_code,
            exec.stderr.trim()
        )));
    }

    // Verify before commit: non-empty + PGDMP custom-format header.
    let mut head = [0u8; 5];
    let valid = {
        use std::io::Read;
        std::fs::File::open(&partial)
            .and_then(|mut f| f.read(&mut head))
            .map(|n| is_valid_custom_dump(&head[..n]))
            .unwrap_or(false)
    };
    if !valid {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "pg_dump exited 0 but produced a truncated dump (no PGDMP header) — {:?}",
            args.name
        )));
    }

    // Commit: rename .partial -> final, fsync the directory.
    std::fs::rename(&partial, &final_path).map_err(|e| PgForgeError::Io {
        path: final_path.clone(),
        source: e,
    })?;
    crate::util::fs::fsync_dir(&dump_dir)?;
    guard.disarm();

    // Retention.
    if let Some(n) = args.keep {
        apply_retention(&dump_dir, &state.instance.name, n);
    }

    let size = std::fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0);
    tracing::info!(
        target: "pgforge::dump",
        "dump complete: {} ({} bytes)",
        final_path.display(),
        size
    );
    eprintln!(
        "dump: {} ({:.1} MiB) — contains production data, delete after transfer.",
        final_path.display(),
        size as f64 / (1024.0 * 1024.0)
    );
    Ok(final_path)
}

/// Remove `*.partial` files in `dir` older than 24h — orphans from prior
/// killed/crashed runs (the RAII guard cannot run after SIGKILL).
fn sweep_stale_partials(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let day = std::time::Duration::from_secs(24 * 3600);
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("partial") {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().map(|e| e > day).unwrap_or(false))
            .unwrap_or(false);
        if stale {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Apply `--keep N` retention for `instance` in `dir`.
fn apply_retention(dir: &Path, instance: &str, n: usize) {
    let prefix = format!("{instance}-");
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut dumps: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.starts_with(&prefix) && name.ends_with(".dump"))
        .collect();
    for name in dumps_to_prune(&mut dumps, n) {
        if let Err(e) = std::fs::remove_file(dir.join(&name)) {
            tracing::warn!(target: "pgforge::dump", "retention: could not remove {name}: {e}");
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --test dump_test --quiet 2>&1 | tail -6`
Expected: PASS — `test result: ok. 11 passed` (8 pure-fn tests + 3 new orchestration tests).

- [ ] **Step 5: Run the full suite to confirm no regressions**

Run: `cargo test --quiet 2>&1 | grep -E "test result: (ok|FAILED)|error\[" | sort | uniq -c`
Expected: only `test result: ok` lines, no `FAILED`, no `error[`.

- [ ] **Step 6: Commit**

```bash
git add src/commands/dump.rs tests/dump_test.rs
git commit -m "$(cat <<'EOF'
feat(dump): add run_with_engine orchestration + PartialGuard

Crash-safe streaming dump: per-pid .partial, RAII cleanup guard, df
precheck, timeout, PGDMP-header verification, fsync_dir on commit,
stale-partial sweep, --keep retention.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: CLI wiring — `Dump` subcommand

**Files:**
- Modify: `src/cli.rs`

- [ ] **Step 1: Add the `Dump` variant to the `Command` enum**

In `src/cli.rs`, add to the `Command` enum (place it after the `Snapshots` variant, near the other instance commands):

```rust
    /// Dump a live instance's database to a portable .dump file on the server.
    Dump {
        /// Instance name.
        #[arg(long)]
        name: String,
        /// Full output file path. Default: $HOME/pgforge-dumps/<name>-<ts>.dump
        #[arg(long)]
        out: Option<std::path::PathBuf>,
        /// Overwrite the destination if it already exists.
        #[arg(long)]
        force: bool,
        /// After a successful dump, keep only the newest N dumps for this instance.
        #[arg(long)]
        keep: Option<usize>,
        /// Hard cap on the dump, in seconds.
        #[arg(long, default_value_t = 1800)]
        timeout: u64,
    },
```

- [ ] **Step 2: Add the dispatch arm**

In `src/cli.rs`, in the `match cli.command` block, add (after the `Snapshots` arm):

```rust
        Some(Command::Dump { name, out, force, keep, timeout }) => {
            let path = crate::commands::dump::run(crate::commands::dump::DumpArgs {
                name,
                out,
                force,
                keep,
                timeout_secs: timeout,
                override_state_root: None,
            })
            .await?;
            // stdout: exactly the path, nothing else — the operator's `make`
            // glue consumes this line for `scp`.
            println!("{}", path.display());
            Ok(())
        }
```

- [ ] **Step 3: Verify the build and the CLI surface**

Run: `cargo build --quiet 2>&1 | tail -5 && cargo run --quiet -- dump --help`
Expected: builds with no errors; `--help` shows `--name`, `--out`, `--force`, `--keep`, `--timeout`.

- [ ] **Step 4: Run the full test suite**

Run: `cargo test --quiet 2>&1 | grep -E "test result: (ok|FAILED)|error\[" | sort | uniq -c`
Expected: only `test result: ok` lines, no `FAILED`.

- [ ] **Step 5: Commit**

```bash
git add src/cli.rs
git commit -m "$(cat <<'EOF'
feat(dump): wire the `pgforge dump` CLI subcommand

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Gated end-to-end test

**Files:**
- Modify: `tests/dump_test.rs`

This is the only test that exercises the real `exec_to_file` streaming, the `df` precheck, the `PGDMP` verification, and `fsync_dir`. It is gated by `PGFORGE_E2E=1` (the project convention — it needs a real Docker engine and a running instance) and is run manually on the macmini, like the other `*_e2e` tests.

- [ ] **Step 1: Write the gated E2E test**

Append to `tests/dump_test.rs`:

```rust
use pgforge::commands::create::{run_with_engine as create_run, CreateArgs};
use pgforge::commands::destroy::{run_with_engine as destroy_run, DestroyArgs};
use pgforge::docker::bollard_engine::BollardEngine;

#[tokio::test]
async fn dump_e2e_produces_a_restorable_file() {
    if std::env::var("PGFORGE_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping: set PGFORGE_E2E=1 to run");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let state_root = tmp.path().to_path_buf();
    let docker = BollardEngine::connect().expect("docker reachable");
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let name = format!("pgfdump_e2e_{suffix}");

    // 1. Create a throwaway --no-backup instance (no S3 needed for a dump test).
    create_run(
        CreateArgs {
            name: name.clone(),
            preset: Preset::Tiny,
            pg_version: 18,
            app_user: "leads".into(),
            app_password: "pw".into(),
            pgbackrest_password: String::new(),
            override_state_root: Some(state_root.clone()),
            no_backup: true,
            retain_days: 30,
            snapshot_hour: None,
        },
        &docker,
        state_root.clone(),
        Default::default(),
        None,
    )
    .await
    .expect("create");

    // 2. Dump it.
    let out = tmp.path().join("e2e.dump");
    let path = run_with_engine(
        DumpArgs {
            name: name.clone(),
            out: Some(out.clone()),
            force: false,
            keep: None,
            timeout_secs: 600,
            override_state_root: Some(state_root.clone()),
        },
        &docker,
        state_root.clone(),
    )
    .await
    .expect("dump");

    // 3. Verify: file exists, non-empty, PGDMP header, 0600, no leftover .partial,
    //    and pg_restore --list can read it.
    assert_eq!(path, out);
    let meta = std::fs::metadata(&out).expect("dump file exists");
    assert!(meta.len() > 0, "dump must be non-empty");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }
    let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().to_string_lossy().contains(".partial"))
        .collect();
    assert!(leftovers.is_empty(), "no .partial should remain: {leftovers:?}");
    let listed = std::process::Command::new("pg_restore")
        .arg("--list")
        .arg(&out)
        .output()
        .expect("pg_restore on PATH");
    assert!(listed.status.success(), "pg_restore --list must accept the dump");

    // 4. Cleanup.
    destroy_run(
        DestroyArgs {
            name: name.clone(),
            delete_backups: false,
            override_state_root: Some(state_root.clone()),
        },
        &docker,
        state_root.clone(),
    )
    .await
    .expect("destroy");
}
```

- [ ] **Step 2: Verify it compiles and skips by default**

Run: `cargo test --test dump_test --quiet 2>&1 | tail -6`
Expected: PASS — all tests pass; the E2E prints `skipping: set PGFORGE_E2E=1 to run` and counts as passed.

- [ ] **Step 3: Commit**

```bash
git add tests/dump_test.rs
git commit -m "$(cat <<'EOF'
test(dump): add gated end-to-end dump test

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 4: Manual E2E on macmini (checkpoint, not automated)**

After deploying the rebuilt binary to macmini (re-sign per `project_macmini_setup` memory), run a real dump against an existing instance and confirm: a `.dump` file appears at the printed path, `pg_restore --list` reads it, the file is 0600, no `.partial` left behind. This validates the real `exec_to_file` streaming + `df` precheck + `fsync_dir` that no in-repo test covers.

---

## Self-review notes

- **Spec coverage:** every spec section maps to a task — engine method/refactor (T1), path resolution + `--out` (T2), truncated-dump guard (T3), `df` precheck (T4), `--keep` retention (T5), `run_with_engine` orchestration incl. validate_name / container check / destination-exists / `.partial` guard / timeout / exit-127 / tracing / fsync (T6), CLI + stdout contract (T7), E2E + `pg_restore --list` validation (T8).
- **`exec_to_file` opens the file** (not the caller) — this resolves the spec's slight ambiguity between "Step 4 opens the `.partial`" and the `exec_to_file(dest: &Path)` signature; the 0600 + O_EXCL invariant lives in one place, and `dump.rs`'s `PartialGuard` only needs the path for cleanup.
- **Known limitations carried from the spec** (documented, not implemented): no cross-command operation lock (don't dump mid-upgrade/restore); cross-instance I/O contention; `exec_with_stdin` not folded into `drain_exec`.
- **Not unit-tested, by design:** the real `drain_exec`/`exec_to_file` streaming, the `df` invocation, and `fsync_dir` — all covered by the Task 8 gated E2E + the manual macmini checkpoint, consistent with how the project tests every other Docker-touching path.
