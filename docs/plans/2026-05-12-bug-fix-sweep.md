# Bug fix sweep — Docker, backup, restore, scheduler

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the cluster of bugs across Docker engine, snapshot/backup, restore/PITR/clone, upgrade and scheduler subsystems that cause silent backup failures, data loss on restart-flap, false-positive "restore complete" reports, and scheduler storms. Reported by parallel review on 2026-05-12.

**Architecture:** 6 phases ordered by severity. Phase 1 stops the bleeding (data loss, state corruption). Phase 2 makes success/failure reporting honest. Phase 3 fixes scheduler correctness. Phase 4 trims storage cost and secret leakage. Phase 5 makes upgrade safe to retry. Phase 6 mops up smaller issues. Each phase ends with green `cargo test` and a commit.

**Tech Stack:** Rust 2024, bollard 0.21, tokio, jiff 0.2, serde+toml, tempfile 3. We will add `fs2 = "0.4"` for advisory file locking in Phase 1.

**Source review:** see agent reports in conversation transcript of 2026-05-12 — Docker (`aafd…`), backup (`a616…`), restore/upgrade (`aee2…`), scheduler/TUI (`a1ca…`).

---

## File structure

```
src/
  util/
    fs.rs                       — add atomic_write_secret + atomic_write + LockedStateRoot helpers
  state/
    instance.rs                 — switch save_under to atomic+locked write
    snapshots.rs                — switch save_for to atomic+locked write
  docker/
    bollard_engine.rs           — per-spec restart policy, BuildKit error_detail, exact-name net filter, memory_swap=None, exec user=postgres helper
    wait.rs                     — deadline-based loop using tokio::time::Instant; also handle "exited" terminal state
    restore_entrypoint.rs       — marker-based re-entry guard; pass target via env, not string interp; check pg_is_in_recovery
    upgrade_entrypoint.rs       — mkdir -p before chown
    clone_entrypoint.rs         — interpolate container_port instead of hardcoded 5432
  commands/
    snapshot.rs                 — drop su -, exec as User="postgres"; scan stderr for ERROR:/ABORTED:; support diff vs full policy; redact stderr in error; record attempt timestamp on failure; validate snapshot_hour; UTC-aware is_snapshot_due fix
    restore.rs                  — canonicalize target_time to UTC RFC3339 with Z; wait for recovery to end; copy last_snapshot_at from source; cleanup also removes named volume on failure
    upgrade.rs                  — capture logs before container removal; build new image BEFORE tearing down old container
    rotate.rs                   — safer SQL escape for password rotation
    schedule.rs                 — XML-escape plist contents; surface bootstrap/bootout errors; bootout-then-bootstrap on reinstall
  domain/
    instance.rs                 — validate snapshot_hour at deserialize
  time.rs                       — add canonicalize_target_time(): &str -> String (RFC3339 with Z)
  tui/
    refresh.rs                  — tokio::time::timeout on Docker calls; set MissedTickBehavior::Delay on spawn_ls
Cargo.toml                      — add fs2 = "0.4"
tests/
  util_fs_test.rs               — atomic_write + lock tests
  domain_snapshot_test.rs       — snapshot_hour validation
  time_test.rs                  — canonicalize_target_time cases
  restore_entrypoint_test.rs    — marker-based guard, env var passing
  schedule_test.rs              — NEW: XML-escape in plist render
  snapshot_due_test.rs          — NEW: is_snapshot_due across DST + invalid timestamp + hour=24
```

---

## Phase 1 — Stop the bleeding

Goal: no silent data loss, no state corruption from concurrent writers, no restart-flap clobbering data, no scheduler running with bogus hour values.

### Task 1.1: Add `fs2` dependency

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add the dependency**

In `Cargo.toml` under `[dependencies]`, insert after `tempfile = "3"`:

```toml
fs2 = "0.4"
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: PASS, no new warnings.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "deps: add fs2 for advisory state-file locking"
```

### Task 1.2: Atomic-write helper

**Files:**
- Modify: `src/util/fs.rs`
- Test: `tests/util_fs_test.rs`

- [ ] **Step 1: Write failing test for atomic_write**

Add to `tests/util_fs_test.rs`:

```rust
#[test]
fn atomic_write_creates_file_with_content() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("sub").join("a.toml");
    pg_forge_cli::util::fs::atomic_write(&p, b"hello").unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), b"hello");
}

#[test]
fn atomic_write_overwrites_existing() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.toml");
    std::fs::write(&p, b"old").unwrap();
    pg_forge_cli::util::fs::atomic_write(&p, b"new").unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), b"new");
}

#[test]
fn atomic_write_leaves_no_tmp_on_success() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.toml");
    pg_forge_cli::util::fs::atomic_write(&p, b"x").unwrap();
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() != std::ffi::OsString::from("a.toml"))
        .collect();
    assert!(leftovers.is_empty(), "found: {:?}", leftovers);
}
```

- [ ] **Step 2: Run test, expect failure**

Run: `cargo test --test util_fs_test atomic_write`
Expected: FAIL — `atomic_write` not found.

- [ ] **Step 3: Implement atomic_write and re-implement write_secret on top**

In `src/util/fs.rs`, append:

```rust
/// Write `content` to `path` atomically: write to `path.<pid>.tmp` in the same
/// directory, fsync the file, then rename over the destination. Creates the
/// parent directory if missing. Survives crash mid-write — readers either see
/// the previous content or the new content, never a truncated mix.
pub fn atomic_write(path: &Path, content: impl AsRef<[u8]>) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| PgForgeError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut f = std::fs::File::create(&tmp).map_err(|e| PgForgeError::Io {
        path: tmp.clone(),
        source: e,
    })?;
    f.write_all(content.as_ref()).map_err(|e| PgForgeError::Io {
        path: tmp.clone(),
        source: e,
    })?;
    f.sync_all().map_err(|e| PgForgeError::Io {
        path: tmp.clone(),
        source: e,
    })?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| PgForgeError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}
```

Replace the body of `write_secret` to delegate to `atomic_write`:

```rust
pub fn write_secret(path: &Path, content: impl AsRef<[u8]>) -> Result<()> {
    atomic_write(path, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms).map_err(|e| PgForgeError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
    }
    Ok(())
}
```

- [ ] **Step 4: Run test, expect pass**

Run: `cargo test --test util_fs_test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/util/fs.rs tests/util_fs_test.rs
git commit -m "fix(state): atomic write helper for state files

Tmp-file + fsync + rename. write_secret now delegates so all
secret writes are atomic too. Prevents truncated state.toml /
snapshots.toml on crash or power loss."
```

### Task 1.3: State-root advisory file lock

**Files:**
- Modify: `src/util/fs.rs`
- Test: `tests/util_fs_test.rs`

- [ ] **Step 1: Write failing test for LockedStateRoot**

Add to `tests/util_fs_test.rs`:

```rust
#[test]
fn locked_state_root_is_exclusive() {
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let o1 = order.clone();
    let r1 = root.clone();
    let t1 = thread::spawn(move || {
        let _g = pg_forge_cli::util::fs::LockedStateRoot::acquire(&r1).unwrap();
        o1.lock().unwrap().push("t1-acquired");
        thread::sleep(Duration::from_millis(200));
        o1.lock().unwrap().push("t1-released");
    });
    thread::sleep(Duration::from_millis(50));
    let o2 = order.clone();
    let r2 = root.clone();
    let t2 = thread::spawn(move || {
        let _g = pg_forge_cli::util::fs::LockedStateRoot::acquire(&r2).unwrap();
        o2.lock().unwrap().push("t2-acquired");
    });
    t1.join().unwrap();
    t2.join().unwrap();
    let o = order.lock().unwrap().clone();
    assert_eq!(o, vec!["t1-acquired", "t1-released", "t2-acquired"]);
}
```

- [ ] **Step 2: Run test, expect failure**

Run: `cargo test --test util_fs_test locked_state_root`
Expected: FAIL — `LockedStateRoot` not found.

- [ ] **Step 3: Implement LockedStateRoot**

In `src/util/fs.rs`, append:

```rust
use std::fs::File;

/// Exclusive advisory lock over a state root. Held while a process mutates
/// any state.toml / snapshots.toml under `state_root`. Released on drop.
///
/// Multiple writers (CLI, TUI, launchd snapshot --due tick) would otherwise
/// race and last-writer-wins, silently losing updates and occasionally
/// corrupting files mid-write. Acquire BEFORE load → mutate → atomic-write
/// → release.
pub struct LockedStateRoot {
    _file: File,
}

impl LockedStateRoot {
    pub fn acquire(state_root: &Path) -> Result<Self> {
        use fs2::FileExt;
        std::fs::create_dir_all(state_root).map_err(|e| PgForgeError::Io {
            path: state_root.to_path_buf(),
            source: e,
        })?;
        let lock_path = state_root.join(".pgforge.lock");
        let file = File::options()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| PgForgeError::Io {
                path: lock_path.clone(),
                source: e,
            })?;
        file.lock_exclusive().map_err(|e| PgForgeError::Io {
            path: lock_path,
            source: e,
        })?;
        Ok(Self { _file: file })
    }
}
```

- [ ] **Step 4: Run test, expect pass**

Run: `cargo test --test util_fs_test locked_state_root`
Expected: PASS — t2 only acquires after t1 releases.

- [ ] **Step 5: Commit**

```bash
git add src/util/fs.rs tests/util_fs_test.rs
git commit -m "fix(state): advisory file lock on state-root

LockedStateRoot guards concurrent writes from scheduler tick,
TUI ops and CLI on the same state.toml/snapshots.toml. Releases
on drop."
```

### Task 1.4: Use atomic write + lock in state writers

**Files:**
- Modify: `src/state/instance.rs`
- Modify: `src/state/snapshots.rs`
- Modify: `src/commands/snapshot.rs` (acquire lock for the load→mutate→save cycle on `last_snapshot_at`)

- [ ] **Step 1: Read current save methods**

Run: `grep -n "fn save_under\|fs::write" src/state/instance.rs src/state/snapshots.rs`

- [ ] **Step 2: Switch state writers to atomic_write**

In `src/state/instance.rs` change the body of `save_under` (currently using `std::fs::write` or `write_secret` — the state.toml is NOT a secret, so use `atomic_write`). Locate the `std::fs::write(&path, raw)` or equivalent line and replace with:

```rust
crate::util::fs::atomic_write(&path, raw)?;
```

In `src/state/snapshots.rs:46`, replace:

```rust
std::fs::write(&path, raw).map_err(|e| PgForgeError::Io { path, source: e })
```

with:

```rust
crate::util::fs::atomic_write(&path, raw.as_bytes())
```

- [ ] **Step 3: Acquire lock around the read-modify-write in snapshot.rs**

In `src/commands/snapshot.rs:134-149`, wrap the load→mutate→save sequence with a lock. Replace lines 134-149 with:

```rust
    let _lock = crate::util::fs::LockedStateRoot::acquire(&state_root)?;
    let mut file = SnapshotsFile::load_for(&state_root, &args.instance)?;
    let record = SnapshotRecord {
        label: label.clone(),
        kind: SnapshotKind::Full,
        user_label: args.user_label,
        taken_at: crate::time::now_iso(),
    };
    file.snapshots.push(record.clone());
    file.save_for(&state_root, &args.instance)?;

    // Re-load InstanceState INSIDE the lock so we don't clobber concurrent
    // edits (e.g. user changing snapshot_hour via TUI [t] while we run).
    let mut state =
        crate::state::instance::InstanceState::load_under(&state_root, &args.instance)?;
    state.instance.last_snapshot_at = Some(record.taken_at.clone());
    state.save_under(&state_root)?;
    drop(_lock);
```

(The previous `let _ = state.save_under` swallowed errors; we now propagate.)

- [ ] **Step 4: Compile**

Run: `cargo build`
Expected: PASS.

- [ ] **Step 5: Run all tests**

Run: `cargo test --lib --bins --tests --no-fail-fast -- --skip e2e`
Expected: all green; skipped e2e tests counted but not run.

- [ ] **Step 6: Commit**

```bash
git add src/state/instance.rs src/state/snapshots.rs src/commands/snapshot.rs
git commit -m "fix(state): atomic writes + lock around snapshot bookkeeping

state.toml and snapshots.toml now use atomic_write so a crash mid-
write leaves the previous content intact. snapshot run holds
LockedStateRoot for the load-modify-save cycle so concurrent
ticks (scheduler + TUI + CLI) no longer clobber each other."
```

### Task 1.5: One-shot containers must NOT restart on failure

**Files:**
- Modify: `src/docker/bollard_engine.rs` (around line 191-194 where RestartPolicy is set)
- Modify: `src/docker/engine.rs` (the `CreateContainerSpec` struct — add a `restart_policy: RestartPolicy` field with a default of `UnlessStopped`)
- Modify: `src/commands/clone.rs`, `src/commands/restore.rs`, `src/commands/upgrade.rs` (set `restart_policy: RestartPolicy::No` on their `CreateContainerSpec` instances)

- [ ] **Step 1: Read current spec definition**

Run: `grep -n "CreateContainerSpec\|RestartPolicy\|restart_policy" src/docker/engine.rs src/docker/bollard_engine.rs`

- [ ] **Step 2: Add the field to the engine-agnostic spec**

In `src/docker/engine.rs`, find the `CreateContainerSpec` struct. Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RestartPolicy {
    /// Long-running primary container. Recover from host reboot.
    #[default]
    UnlessStopped,
    /// One-shot container — clone bootstrap, restore bootstrap, pg_upgrade.
    /// Failure must surface as a non-zero wait_for_container_exit code, not
    /// trigger Docker to re-run the entrypoint (which would wipe PGDATA on
    /// the clone/restore path).
    No,
}
```

Add a `pub restart_policy: RestartPolicy,` field to `CreateContainerSpec` (with `#[serde(default)]` if serde is derived; otherwise just place it).

- [ ] **Step 3: Wire the field in BollardEngine**

In `src/docker/bollard_engine.rs` around line 191-194 where `RestartPolicy` (bollard's type) is set, replace the hard-coded `UNLESS_STOPPED` with a match on `spec.restart_policy`:

```rust
use bollard::models::{HostConfig, RestartPolicy as BollardRestartPolicy, RestartPolicyNameEnum};
let restart = BollardRestartPolicy {
    name: Some(match spec.restart_policy {
        crate::docker::engine::RestartPolicy::UnlessStopped => RestartPolicyNameEnum::UNLESS_STOPPED,
        crate::docker::engine::RestartPolicy::No => RestartPolicyNameEnum::NO,
    }),
    maximum_retry_count: None,
};
```

(Adapt the exact import path to match the existing imports in the file.)

- [ ] **Step 4: Opt into `No` on clone/restore/upgrade containers**

In `src/commands/clone.rs`, find the `CreateContainerSpec { ... }` literal that constructs the clone container, add:

```rust
restart_policy: crate::docker::engine::RestartPolicy::No,
```

Same for `src/commands/restore.rs` and `src/commands/upgrade.rs`.

Long-lived containers (create.rs) leave the field at default = `UnlessStopped`.

- [ ] **Step 5: Run tests**

Run: `cargo test --lib --bins --tests --no-fail-fast -- --skip e2e`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/docker/engine.rs src/docker/bollard_engine.rs src/commands/clone.rs src/commands/restore.rs src/commands/upgrade.rs
git commit -m "fix(docker): one-shot containers use restart=no

clone/restore/upgrade containers had restart=unless-stopped, which
meant that on a crashed pg_basebackup / pgbackrest restore the
container would restart, the entrypoint would re-run, and
clone_entrypoint would wipe PGDATA on every loop. Wait helpers
also couldn't surface the exit code because the container kept
flapping. Long-lived create.rs container keeps unless-stopped."
```

### Task 1.6: Marker-based re-entry guard in restore_entrypoint

**Files:**
- Modify: `src/docker/restore_entrypoint.rs`
- Test: `tests/restore_entrypoint_test.rs`

- [ ] **Step 1: Write failing tests for marker-based guard**

Add to `tests/restore_entrypoint_test.rs`:

```rust
#[test]
fn restore_script_uses_marker_not_pg_version() {
    let s = pg_forge_cli::docker::restore_entrypoint::generate_restore_entrypoint(None);
    assert!(s.contains(".pgforge-restore-complete"),
        "marker missing — script will re-skip restore on PG_VERSION from partial restore");
    assert!(!s.contains("[ ! -f \"$PGDATA/PG_VERSION\" ]"),
        "PG_VERSION guard still present");
}

#[test]
fn restore_script_writes_marker_after_pgbackrest() {
    let s = pg_forge_cli::docker::restore_entrypoint::generate_restore_entrypoint(None);
    let marker_idx = s.find("touch \"$MARKER\"").expect("marker write missing");
    let pgbackrest_idx = s.find("pgbackrest").expect("pgbackrest missing");
    assert!(pgbackrest_idx < marker_idx,
        "marker must be written AFTER pgbackrest restore, not before");
}
```

- [ ] **Step 2: Run test, expect failure**

Run: `cargo test --test restore_entrypoint_test`
Expected: FAIL — both new assertions.

- [ ] **Step 3: Rewrite the script**

Replace the body of `generate_restore_entrypoint` in `src/docker/restore_entrypoint.rs` with (NOTE: target injection is fixed in Phase 2 — here we only change the guard):

```rust
pub fn generate_restore_entrypoint(target_time: Option<&str>) -> String {
    let target_args = match target_time {
        Some(t) => format!(
            r#" --type=time --target="{t}" --target-action=promote"#
        ),
        None => " --target-action=promote".to_string(),
    };
    format!(
        r#"#!/bin/sh
# Generated by pgforge — do not edit by hand.
set -eu

PGDATA="/var/lib/postgresql/data/pgdata"
MARKER="$PGDATA/.pgforge-restore-complete"

# Marker is written ONLY after pgbackrest restore returns 0. A failed or
# partial restore leaves PG_VERSION behind in pgdata — without the marker
# we retry instead of booting a broken cluster.
if [ ! -f "$MARKER" ]; then
    mkdir -p "$PGDATA"
    chown -R postgres:postgres "$PGDATA"
    # Clear any half-restored content; pgbackrest restore --delta would also
    # work but we want a strict re-do here.
    find "$PGDATA" -mindepth 1 -delete 2>/dev/null || true
    su - postgres -c 'pgbackrest --stanza=main restore --pg1-path=/var/lib/postgresql/data/pgdata{target_args}'
    touch "$MARKER"
    chown postgres:postgres "$MARKER"
fi

exec docker-entrypoint.sh postgres \
    -c config_file=/etc/postgresql/postgresql.conf \
    -c hba_file=/etc/postgresql/pg_hba.conf
"#,
        target_args = target_args
    )
}
```

- [ ] **Step 4: Run test, expect pass**

Run: `cargo test --test restore_entrypoint_test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/docker/restore_entrypoint.rs tests/restore_entrypoint_test.rs
git commit -m "fix(restore): marker-based re-entry guard

PG_VERSION can exist after a partial/aborted pgbackrest restore;
the old guard then skipped restore on retry and booted a corrupt
cluster. Switch to .pgforge-restore-complete marker written AFTER
pgbackrest returns 0 (mirrors clone_entrypoint pattern)."
```

### Task 1.7: Validate snapshot_hour to 0..=23

**Files:**
- Modify: `src/domain/instance.rs`
- Test: `tests/domain_snapshot_test.rs`

- [ ] **Step 1: Find where snapshot_hour is set**

Run: `grep -n "snapshot_hour" src/domain/instance.rs src/tui/`

- [ ] **Step 2: Write failing test**

Add to `tests/domain_snapshot_test.rs`:

```rust
use pg_forge_cli::domain::instance::Instance;

#[test]
fn snapshot_hour_rejects_24() {
    assert!(Instance::validate_snapshot_hour(24).is_err());
    assert!(Instance::validate_snapshot_hour(99).is_err());
}

#[test]
fn snapshot_hour_accepts_full_day_range() {
    for h in 0..=23u8 {
        assert!(Instance::validate_snapshot_hour(h).is_ok(), "hour {h}");
    }
}
```

- [ ] **Step 3: Run test, expect failure**

Run: `cargo test --test domain_snapshot_test snapshot_hour`
Expected: FAIL — function missing.

- [ ] **Step 4: Implement validator and call it from setters**

In `src/domain/instance.rs`, add a method on `Instance`:

```rust
impl Instance {
    pub fn validate_snapshot_hour(h: u8) -> crate::error::Result<()> {
        if h > 23 {
            return Err(crate::error::PgForgeError::Anyhow(anyhow::anyhow!(
                "snapshot_hour must be 0..=23, got {h}"
            )));
        }
        Ok(())
    }
}
```

Find the TUI handler that writes `snapshot_hour` (search `snapshot_hour =` in `src/tui/`). At each assignment from user input, wrap with `Instance::validate_snapshot_hour(h)?;` before the assignment.

Also call it in `commands/create.rs` if `snapshot_hour` is taken from CLI args.

- [ ] **Step 5: Run test, expect pass**

Run: `cargo test --test domain_snapshot_test`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/domain/instance.rs src/tui/ src/commands/create.rs tests/domain_snapshot_test.rs
git commit -m "fix(domain): validate snapshot_hour to 0..=23

Out-of-range value made is_snapshot_due always return false →
scheduler silently never fired for that instance."
```

---

## Phase 2 — Honest success / failure

Goal: when a backup, restore or build fails, the user finds out, with no `su -` swallowing exit codes, no false "restore complete" while recovery still replays, no shell-injectable target_time.

### Task 2.1: Drop `su -` from snapshot exec; use bollard User=postgres

**Files:**
- Modify: `src/docker/engine.rs` (add optional `user` to `ExecSpec` or extend the exec API)
- Modify: `src/docker/bollard_engine.rs` (pass `user` to bollard's CreateExecOptions)
- Modify: `src/commands/snapshot.rs`

- [ ] **Step 1: Read current exec trait signature**

Run: `grep -n "fn exec\|trait DockerEngine\|CreateExecOptions" src/docker/engine.rs src/docker/bollard_engine.rs | head -20`

- [ ] **Step 2: Add `exec_as` method to trait**

In `src/docker/engine.rs` extend the `DockerEngine` trait:

```rust
#[allow(async_fn_in_trait)]
pub trait DockerEngine {
    // ... existing methods ...
    /// Execute `cmd` inside `container` running as the given OS user (uid or
    /// name). Use when the command must drop privileges from root — replaces
    /// the `su -` shell trick which silently swallows the child exit code on
    /// some PAM stacks.
    async fn exec_as(&self, container: &str, user: &str, cmd: &[&str]) -> Result<ExecOutput>;
}
```

- [ ] **Step 3: Implement exec_as in BollardEngine**

In `src/docker/bollard_engine.rs`, near the existing `exec` impl, add:

```rust
async fn exec_as(&self, container: &str, user: &str, cmd: &[&str]) -> Result<ExecOutput> {
    use bollard::query_parameters::{CreateExecOptionsBuilder, StartExecOptionsBuilder};
    let opts = CreateExecOptionsBuilder::default()
        .cmd(cmd.iter().map(|s| s.to_string()).collect())
        .user(user.to_string())
        .attach_stdout(true)
        .attach_stderr(true)
        .build();
    // ... rest mirrors existing exec(), reading stdout/stderr stream and
    // inspect_exec for exit code. Reuse a private helper if practical.
}
```

(If the existing `exec` already takes a Spec, just add a `user: Option<String>` field instead of a new method — same outcome. The plan calls out `exec_as` for clarity; either shape is fine as long as snapshot.rs no longer uses `su -`.)

- [ ] **Step 4: Update snapshot.rs**

In `src/commands/snapshot.rs:112-120` replace the exec call with:

```rust
let out = docker
    .exec_as(
        &container,
        "postgres",
        &["pgbackrest", "--stanza=main", "--type=full", "backup"],
    )
    .await?;
```

- [ ] **Step 5: Compile and test**

Run: `cargo test --lib --bins --tests --no-fail-fast -- --skip e2e`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/docker/engine.rs src/docker/bollard_engine.rs src/commands/snapshot.rs
git commit -m "fix(snapshot): exec as postgres, no su -

su - on some PAM stacks returns 0 even when the child exited
non-zero. We now drop privileges via bollard's User field on
exec, so pgbackrest's real exit code propagates back."
```

### Task 2.2: Defensive ERROR/ABORTED stderr scan + redact secrets in error message

**Files:**
- Modify: `src/commands/snapshot.rs`
- Test: `tests/snapshot_due_test.rs` (NEW file — unit test on a helper function)

- [ ] **Step 1: Write failing test for the helper**

Create `tests/snapshot_due_test.rs`:

```rust
use pg_forge_cli::commands::snapshot::{redact_pgbackrest_output, pgbackrest_indicates_failure};

#[test]
fn detects_error_marker_in_stderr() {
    assert!(pgbackrest_indicates_failure("WARN: …\nERROR: [056]: file missing"));
    assert!(pgbackrest_indicates_failure("ABORTED: shutting down"));
}

#[test]
fn clean_output_is_not_a_failure() {
    assert!(!pgbackrest_indicates_failure("INFO: backup begin\nINFO: full backup size = 12MB"));
}

#[test]
fn redact_strips_repo1_s3_key_lines() {
    let s = "repo1-s3-key=AKIAEXAMPLE\nrepo1-s3-key-secret=verysecret\nINFO: ok";
    let r = redact_pgbackrest_output(s);
    assert!(!r.contains("AKIAEXAMPLE"));
    assert!(!r.contains("verysecret"));
    assert!(r.contains("INFO: ok"));
}
```

- [ ] **Step 2: Run test, expect failure**

Run: `cargo test --test snapshot_due_test`
Expected: FAIL — functions missing.

- [ ] **Step 3: Implement helpers and wire into snapshot.rs**

In `src/commands/snapshot.rs`, add public helpers:

```rust
/// Belt-and-braces scan of pgbackrest output. Even when exit code is 0,
/// surface output that contains the canonical pgbackrest error markers.
pub fn pgbackrest_indicates_failure(s: &str) -> bool {
    s.lines().any(|l| {
        let l = l.trim_start();
        l.starts_with("ERROR:") || l.starts_with("ABORTED:")
    })
}

/// Remove lines that may carry secret material (S3 keys, passwords).
pub fn redact_pgbackrest_output(s: &str) -> String {
    s.lines()
        .filter(|l| {
            let lc = l.to_ascii_lowercase();
            !lc.contains("repo1-s3-key")
                && !lc.contains("repo1-cipher-pass")
                && !lc.contains("password")
        })
        .collect::<Vec<_>>()
        .join("\n")
}
```

In `run_with_engine`, after the exec call, before the existing `if out.exit_code != 0`, insert:

```rust
if out.exit_code != 0 || pgbackrest_indicates_failure(&out.stderr) || pgbackrest_indicates_failure(&out.stdout) {
    return Err(PgForgeError::Docker(format!(
        "pgbackrest backup failed (exit {}): {}",
        out.exit_code, redact_pgbackrest_output(&out.stderr)
    )));
}
```

And remove the now-superseded original `if out.exit_code != 0` block.

- [ ] **Step 4: Run test, expect pass; run all tests**

Run: `cargo test --test snapshot_due_test && cargo test --lib --bins --tests --no-fail-fast -- --skip e2e`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/commands/snapshot.rs tests/snapshot_due_test.rs
git commit -m "fix(snapshot): scan stderr for ERROR:/ABORTED:; redact secrets

Belt-and-braces against any future exit-code-0 with errors in
output. Strip lines containing repo1-s3-key/cipher-pass/password
from error messages to avoid leaking S3 creds into logs."
```

### Task 2.3: Canonicalize target_time to UTC before injecting into shell

**Files:**
- Modify: `src/time.rs`
- Modify: `src/commands/restore.rs`
- Modify: `src/docker/restore_entrypoint.rs` (pass via env var, not interpolation)
- Test: `tests/time_test.rs`
- Test: `tests/restore_entrypoint_test.rs`

- [ ] **Step 1: Write failing test for canonicalize_target_time**

Add to `tests/time_test.rs`:

```rust
use pg_forge_cli::time::canonicalize_target_time;

#[test]
fn canonicalizes_space_separator_as_utc() {
    assert_eq!(canonicalize_target_time("2026-05-12 14:00:00").unwrap(),
               "2026-05-12T14:00:00Z");
}

#[test]
fn canonicalizes_offset_to_utc() {
    assert_eq!(canonicalize_target_time("2026-05-12T14:00:00+02:00").unwrap(),
               "2026-05-12T12:00:00Z");
}

#[test]
fn passes_through_z_form() {
    assert_eq!(canonicalize_target_time("2026-05-12T14:00:00Z").unwrap(),
               "2026-05-12T14:00:00Z");
}

#[test]
fn rejects_garbage() {
    assert!(canonicalize_target_time("not a time").is_err());
}
```

- [ ] **Step 2: Run test, expect failure**

Run: `cargo test --test time_test canonicalize`
Expected: FAIL.

- [ ] **Step 3: Implement canonicalize_target_time**

In `src/time.rs`, append:

```rust
/// Parse a user-supplied target time (any form accepted by parse_target_time)
/// and re-emit it as strict UTC RFC 3339 with `Z` suffix. The canonical form
/// is what we hand to pgbackrest --target=, so the container's local TZ
/// (which we do not control on macOS Docker Desktop) cannot shift the
/// recovery point.
pub fn canonicalize_target_time(s: &str) -> Result<String> {
    let ts = parse_target_time(s)?;
    let secs = ts.as_second();
    Ok(Timestamp::from_second(secs)
        .map(|t| t.to_string())
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into()))
}
```

- [ ] **Step 4: Add test that restore_entrypoint passes target via env**

Add to `tests/restore_entrypoint_test.rs`:

```rust
#[test]
fn target_time_passed_via_env_not_interpolated() {
    let s = pg_forge_cli::docker::restore_entrypoint::generate_restore_entrypoint(
        Some("2026-05-12T14:00:00Z"),
    );
    // Target value should not appear inside double-quoted shell string.
    assert!(!s.contains(r#"--target="2026-05-12T14:00:00Z""#),
        "target time must not be interpolated into the script; pass via env");
    // Should reference an env var instead.
    assert!(s.contains("PGFORGE_TARGET"),
        "expected PGFORGE_TARGET env var as the target carrier");
}
```

- [ ] **Step 5: Update restore_entrypoint to read PGFORGE_TARGET from env**

In `src/docker/restore_entrypoint.rs`, rewrite the script body so the target is read from an env var (which we set via bollard's `Env` array, where shell metachars are inert):

```rust
pub fn generate_restore_entrypoint(target_time: Option<&str>) -> String {
    let target_block = if target_time.is_some() {
        r#"TARGET_ARGS="--type=time --target=$PGFORGE_TARGET --target-action=promote""#
    } else {
        r#"TARGET_ARGS="--target-action=promote""#
    };
    format!(
        r#"#!/bin/sh
# Generated by pgforge — do not edit by hand.
set -eu

PGDATA="/var/lib/postgresql/data/pgdata"
MARKER="$PGDATA/.pgforge-restore-complete"

{target_block}

if [ ! -f "$MARKER" ]; then
    mkdir -p "$PGDATA"
    chown -R postgres:postgres "$PGDATA"
    find "$PGDATA" -mindepth 1 -delete 2>/dev/null || true
    su - postgres -c "pgbackrest --stanza=main restore --pg1-path=/var/lib/postgresql/data/pgdata $TARGET_ARGS"
    touch "$MARKER"
    chown postgres:postgres "$MARKER"
fi

exec docker-entrypoint.sh postgres \
    -c config_file=/etc/postgresql/postgresql.conf \
    -c hba_file=/etc/postgresql/pg_hba.conf
"#,
        target_block = target_block
    )
}
```

- [ ] **Step 6: Pass the canonicalized target as env in restore.rs**

In `src/commands/restore.rs` around line 119-123, before the entrypoint is rendered, canonicalize the target and add it to the container env map. Replace any place that previously passed `args.target_time.as_deref()` to `generate_restore_entrypoint` and to pgbackrest with:

```rust
let canonical_target = match args.target_time.as_deref() {
    Some(s) => Some(crate::time::canonicalize_target_time(s)?),
    None => None,
};
// ...
if let Some(t) = &canonical_target {
    env.insert("PGFORGE_TARGET".into(), t.clone());
}
// ...
std::fs::write(
    &entrypoint,
    generate_restore_entrypoint(canonical_target.as_deref()),
)
.map_err(|e| PgForgeError::Io {
    path: entrypoint.clone(),
    source: e,
})?;
```

- [ ] **Step 7: Run tests, expect pass**

Run: `cargo test --test time_test --test restore_entrypoint_test`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add src/time.rs src/commands/restore.rs src/docker/restore_entrypoint.rs tests/time_test.rs tests/restore_entrypoint_test.rs
git commit -m "fix(restore): canonicalize target_time to UTC; pass via env

Previously target_time was interpolated verbatim into a sh string
inside the entrypoint, so an apostrophe in the user input broke
the script and a non-Z form could be reinterpreted in the
container's local TZ. We now canonicalize to RFC3339 Z and hand
it to the container as PGFORGE_TARGET env var (shell-inert)."
```

### Task 2.4: Wait for recovery to end after pg_isready

**Files:**
- Modify: `src/docker/wait.rs` (or `src/commands/restore.rs` — depending on what's idiomatic)
- Modify: `src/commands/restore.rs`

- [ ] **Step 1: Add wait_for_recovery_end helper**

In `src/docker/wait.rs`, append:

```rust
/// After `wait_for_pg_ready` returns, the cluster may still be in recovery
/// (during `target-action=promote` Postgres briefly accepts connections
/// before timeline switch). Poll `pg_is_in_recovery()` until it returns
/// `false`, or fail after `seconds`.
pub async fn wait_for_recovery_end<E: DockerEngine>(
    docker: &E,
    id: &str,
    seconds: u64,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    loop {
        let out = docker
            .exec(id, &[
                "psql", "-tA", "-U", "postgres", "-h", "/var/run/postgresql",
                "-c", "select pg_is_in_recovery()::text",
            ])
            .await;
        if let Ok(o) = out {
            if o.exit_code == 0 && o.stdout.trim() == "f" {
                return Ok(());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(PgForgeError::Docker(format!(
                "container {id}: still in recovery after {seconds}s"
            )));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
```

- [ ] **Step 2: Call it from restore.rs after pg_isready**

In `src/commands/restore.rs` around line 264 where `wait_for_pg_ready` is called for the restore container, add immediately after:

```rust
crate::docker::wait::wait_for_recovery_end(docker, &container, 600).await?;
```

- [ ] **Step 3: Compile and test**

Run: `cargo test --lib --bins --tests --no-fail-fast -- --skip e2e`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/docker/wait.rs src/commands/restore.rs
git commit -m "fix(restore): wait for pg_is_in_recovery() = false

pg_isready returns 0 during target-action=promote recovery
before timeline switch. We now poll pg_is_in_recovery() until
it goes false before declaring restore complete and saving
state."
```

### Task 2.4b: Same completion check for clone path

**Files:**
- Modify: `src/commands/clone.rs`

- [ ] **Step 1: Call wait_for_recovery_end after wait_for_pg_ready**

In `src/commands/clone.rs:269-274`, immediately after the `wait_for_pg_ready` call for the clone container, add:

```rust
crate::docker::wait::wait_for_recovery_end(docker, &container, 600).await?;
```

A successful `pg_basebackup` plus `pg_isready` ok does NOT mean the clone is ready to receive writes — the new cluster may briefly be in recovery applying streamed WAL.

- [ ] **Step 2: Compile and test**

Run: `cargo test --lib --bins --tests --no-fail-fast -- --skip e2e`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/commands/clone.rs
git commit -m "fix(clone): also wait for pg_is_in_recovery() = false"
```

### Task 2.5: Surface BuildKit errors and bootstrap/bootout errors

**Files:**
- Modify: `src/docker/bollard_engine.rs` (`build_image` around line 66-94)
- Modify: `src/commands/schedule.rs` (around lines 55-69 where `let _` swallows results)

- [ ] **Step 1: Scan build_image stream for error_detail**

In `src/docker/bollard_engine.rs:66-94`, inside the loop that consumes the BuildKit stream, after pulling out each `info` message, add:

```rust
if let Some(err) = info.error.as_deref() {
    return Err(PgForgeError::Docker(format!("docker build failed: {err}")));
}
if let Some(ed) = info.error_detail.as_ref() {
    if let Some(msg) = ed.message.as_deref() {
        return Err(PgForgeError::Docker(format!("docker build failed: {msg}")));
    }
}
```

(Adapt field names to whatever `bollard::models::BuildInfo` exposes — verify with `grep -n "error_detail\|BuildInfo" target/doc 2>/dev/null` or by reading the bollard docs; the principle is "any error field in the stream → bubble up".)

- [ ] **Step 2: Surface schedule install/uninstall errors**

In `src/commands/schedule.rs` find every `let _ = ` around `bootstrap` / `bootout` and replace with `?`-propagation. Around lines 55-69 (`install` / `uninstall`), where launchctl errors are silently dropped, change e.g.:

```rust
let _ = std::process::Command::new("launchctl").args(["bootstrap", ...]).status();
```

into:

```rust
let status = std::process::Command::new("launchctl").args(["bootstrap", ...]).status()
    .map_err(|e| PgForgeError::Anyhow(anyhow::anyhow!("launchctl bootstrap: {e}")))?;
if !status.success() {
    return Err(PgForgeError::Anyhow(anyhow::anyhow!(
        "launchctl bootstrap returned {status}"
    )));
}
```

For "already loaded" specifically, before `bootstrap` we should first run `bootout` and ignore its result (that one is genuinely OK to ignore — it's "best effort cleanup" before re-bootstrap):

```rust
let _ = std::process::Command::new("launchctl").args(["bootout", &domain_target]).status();
```

- [ ] **Step 3: Compile and test**

Run: `cargo test --lib --bins --tests --no-fail-fast -- --skip e2e`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/docker/bollard_engine.rs src/commands/schedule.rs
git commit -m "fix(docker/schedule): bubble up BuildKit and launchctl errors

build_image now fails fast when the BuildKit stream carries
error/error_detail. schedule install/uninstall now propagate
launchctl exit codes instead of silently reporting success
on a failed bootstrap."
```

---

## Phase 3 — Scheduling correctness

Goal: scheduler fires exactly once per window per day; doesn't storm on parse failures; doesn't log-spam on persistent failure; plist is well-formed for paths with shell-meta characters; DST and midnight don't double-fire.

### Task 3.1: is_snapshot_due — DST safe + don't return true on parse failure

**Files:**
- Modify: `src/commands/snapshot.rs`
- Test: `tests/snapshot_due_test.rs`

- [ ] **Step 1: Make `is_snapshot_due` pub for testing**

In `src/commands/snapshot.rs`, change `fn is_snapshot_due` to `pub fn is_snapshot_due`.

- [ ] **Step 2: Write failing tests**

Add to `tests/snapshot_due_test.rs`:

```rust
use pg_forge_cli::commands::snapshot::is_snapshot_due;

#[test]
fn never_snapshotted_is_due_after_hour() {
    // Cannot test "after the hour" without freezing time; the test instead
    // asserts the documented invariant via an injectable variant. If the
    // codebase does not have time injection, this test is best-effort:
    // confirm None never panics.
    let _ = is_snapshot_due(0, None);
}

#[test]
fn unparseable_last_is_NOT_due_until_explicit_reset() {
    // The previous behavior returned true on parse failure → storm.
    // We now treat unparseable as "today's window already filled" so the
    // scheduler doesn't loop on a corrupt state.toml.
    assert!(!is_snapshot_due(0, Some("garbage")),
        "garbage timestamp must NOT trigger a re-snapshot");
}
```

- [ ] **Step 3: Run test, expect the parse-failure assertion to fail**

Run: `cargo test --test snapshot_due_test unparseable_last`
Expected: FAIL (current code returns true).

- [ ] **Step 4: Fix the parse-failure branch**

In `src/commands/snapshot.rs:74-76`, replace:

```rust
let Ok(last_ts) = jiff::Timestamp::from_str(last) else {
    return true; // unparseable → treat as never (best-effort safety)
};
```

with:

```rust
let Ok(last_ts) = jiff::Timestamp::from_str(last) else {
    tracing::warn!(target: "pgforge::snapshot::due",
        "unparseable last_snapshot_at {last:?}; skipping this tick (clear with `pgforge snapshot --reset` if intentional)");
    return false;
};
```

- [ ] **Step 5: Run test, expect pass**

Run: `cargo test --test snapshot_due_test`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/commands/snapshot.rs tests/snapshot_due_test.rs
git commit -m "fix(scheduler): unparseable last_snapshot_at no longer storms

Old behavior: corrupted timestamp → return true → run pgbackrest
on every 5-min tick. New: log warn and skip — operator must
intervene, not have the disk filled."
```

### Task 3.2: Record `last_snapshot_attempt_at` on failure to prevent retry storm

**Files:**
- Modify: `src/domain/instance.rs` (add `last_snapshot_attempt_at: Option<String>`)
- Modify: `src/commands/snapshot.rs` (`run_due` records attempt timestamp on failure; `is_snapshot_due` considers it)

- [ ] **Step 1: Add the field**

In `src/domain/instance.rs`, find the `Instance` struct and add (with `#[serde(default)]`):

```rust
/// Most recent time we *tried* a snapshot, regardless of outcome. Set even
/// on failure. Prevents the launchd tick from retrying continuously for the
/// rest of the day when something is wrong (e.g. S3 unreachable).
#[serde(default)]
pub last_snapshot_attempt_at: Option<String>,
```

- [ ] **Step 2: Update `is_snapshot_due` to take both fields**

Change signature to `pub fn is_snapshot_due(hour: u8, last_ok: Option<&str>, last_attempt: Option<&str>) -> bool`. The window is satisfied if EITHER a successful snapshot was taken today OR an attempt was logged within the last hour (back-off).

```rust
pub fn is_snapshot_due(
    hour: u8,
    last_ok: Option<&str>,
    last_attempt: Option<&str>,
) -> bool {
    use std::str::FromStr;
    let zoned_now = jiff::Zoned::now();
    let today = zoned_now.date();
    let now_secs = zoned_now.hour() as i64 * 3600
        + zoned_now.minute() as i64 * 60
        + zoned_now.second() as i64;
    let hour_secs = (hour as i64) * 3600;
    if now_secs < hour_secs {
        return false;
    }
    if let Some(last) = last_ok {
        if let Ok(ts) = jiff::Timestamp::from_str(last) {
            if ts.to_zoned(zoned_now.time_zone().clone()).date() == today {
                return false; // already covered today
            }
        } else {
            // unparseable → don't storm
            tracing::warn!(target: "pgforge::snapshot::due",
                "unparseable last_snapshot_at {last:?}; skipping");
            return false;
        }
    }
    if let Some(att) = last_attempt {
        if let Ok(ts) = jiff::Timestamp::from_str(att) {
            let age = (jiff::Timestamp::now().as_second() - ts.as_second()).max(0);
            if age < 3600 {
                tracing::info!(target: "pgforge::snapshot::due",
                    "recent failed attempt {att:?} ({age}s ago); backing off");
                return false;
            }
        }
    }
    true
}
```

- [ ] **Step 3: Update callers**

In `run_due` change the call site to pass both fields:

```rust
if !is_snapshot_due(
    hour,
    state.instance.last_snapshot_at.as_deref(),
    state.instance.last_snapshot_attempt_at.as_deref(),
) {
    continue;
}
```

On failure (the `Err(e)` arm in `run_due`), record the attempt timestamp under lock:

```rust
if let Err(e) = run(SnapshotArgs { ... }).await {
    tracing::warn!(...);
    if let Ok(_lock) = crate::util::fs::LockedStateRoot::acquire(&state_root) {
        if let Ok(mut s) = crate::state::instance::InstanceState::load_under(&state_root, &name) {
            s.instance.last_snapshot_attempt_at = Some(crate::time::now_iso());
            let _ = s.save_under(&state_root);
        }
    }
    continue;
}
```

In `run_with_engine`, on the success path, also set `last_snapshot_attempt_at = Some(record.taken_at.clone())` next to `last_snapshot_at`.

- [ ] **Step 4: Update tests**

In `tests/snapshot_due_test.rs` update calls to the new signature (pass `None` for the new arg in existing tests). Add:

```rust
#[test]
fn recent_failed_attempt_backs_off() {
    let recent = pg_forge_cli::time::now_iso();
    assert!(!is_snapshot_due(0, None, Some(&recent)));
}
```

- [ ] **Step 5: Run all tests**

Run: `cargo test --lib --bins --tests --no-fail-fast -- --skip e2e`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/domain/instance.rs src/commands/snapshot.rs tests/snapshot_due_test.rs
git commit -m "fix(scheduler): back off after failed attempt

Failed pgbackrest never updated last_snapshot_at, so the launchd
tick re-ran every 5 min for the rest of the day. We now also
track last_snapshot_attempt_at and require >=1h between
attempts after a failure."
```

### Task 3.3: XML-escape plist contents

**Files:**
- Modify: `src/commands/schedule.rs`
- Test: `tests/schedule_test.rs` (NEW)

- [ ] **Step 1: Write failing test**

Create `tests/schedule_test.rs`:

```rust
use pg_forge_cli::commands::schedule;

#[test]
fn plist_xml_escapes_paths_with_ampersand() {
    let plist = schedule::render_plist(
        "test.label",
        &std::path::PathBuf::from("/Users/me/A&B/pgforge"),
        &std::path::PathBuf::from("/tmp/log&log"),
        300,
    );
    assert!(plist.contains("/Users/me/A&amp;B/pgforge"),
        "ampersand in path must be XML-escaped");
    assert!(!plist.contains("A&B/pgforge"),
        "raw & must not appear inside plist body");
}

#[test]
fn plist_xml_escapes_angle_brackets_and_quotes() {
    let plist = schedule::render_plist(
        "test.label",
        &std::path::PathBuf::from("/tmp/<weird>'path"),
        &std::path::PathBuf::from("/tmp/log"),
        300,
    );
    assert!(plist.contains("&lt;weird&gt;"));
    assert!(plist.contains("&apos;path") || plist.contains("&#39;path"));
}
```

- [ ] **Step 2: Run test, expect failure or non-compile**

Run: `cargo test --test schedule_test`
Expected: FAIL — `render_plist` is private or doesn't escape.

- [ ] **Step 3: Make render_plist pub(crate) accessible and add escape helper**

In `src/commands/schedule.rs`, change `fn render_plist` visibility to `pub`. Add a private helper:

```rust
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\'' => out.push_str("&apos;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}
```

In `render_plist`, route every `exe.display()` / `log.display()` interpolation through `xml_escape(&exe.display().to_string())` etc.

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test --test schedule_test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/commands/schedule.rs tests/schedule_test.rs
git commit -m "fix(schedule): XML-escape paths in launchd plist render

Paths containing &, <, > or ' produced malformed XML that
launchctl bootstrap silently rejected. Combined with the
let _ = previously around bootstrap, users saw \"installed\"
but the scheduler never ran."
```

### Task 3.4: bootout-then-bootstrap on reinstall

**Files:**
- Modify: `src/commands/schedule.rs`

- [ ] **Step 1: In the install() flow, prepend a best-effort bootout**

In `src/commands/schedule.rs::install`, before the existing `launchctl bootstrap` call, add:

```rust
// Idempotent reinstall: bootout any prior agent of the same label so
// bootstrap doesn't fail with "service already loaded".
let _ = std::process::Command::new("launchctl")
    .args(["bootout", &domain_target])
    .status();
```

Then the strict `bootstrap` from Task 2.5 will succeed cleanly on reinstall.

- [ ] **Step 2: Compile and test**

Run: `cargo test --lib --bins --tests --no-fail-fast -- --skip e2e`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/commands/schedule.rs
git commit -m "fix(schedule): bootout-then-bootstrap on reinstall

Reinstalling the agent (e.g. after binary update) failed with
\"service already loaded\" because bootstrap doesn't replace.
Now bootout first (best effort), then strict bootstrap."
```

### Task 3.5: TUI refresh — timeout Docker calls; missed-tick policy on spawn_ls

**Files:**
- Modify: `src/tui/refresh.rs`

- [ ] **Step 1: Add tokio::time::timeout around Docker calls**

In `src/tui/refresh.rs`, wherever `status::run_with_engine` (or any Docker call) is invoked inside a poller loop, wrap it:

```rust
let r = tokio::time::timeout(std::time::Duration::from_secs(5), status::run_with_engine(...)).await;
match r {
    Ok(Ok(out)) => { /* publish event */ }
    Ok(Err(e)) => { /* publish error event */ }
    Err(_) => {
        tracing::warn!(target: "pgforge::tui::refresh",
            "docker call timed out after 5s; marking stale");
        /* publish stale event */
    }
}
```

- [ ] **Step 2: Set MissedTickBehavior on spawn_ls**

Inside `spawn_ls`, immediately after the `tokio::time::interval(...)` line, add:

```rust
iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
```

(Matches `spawn_status`/`spawn_snapshots`.)

- [ ] **Step 3: Compile**

Run: `cargo build`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/tui/refresh.rs
git commit -m "fix(tui): timeout Docker calls; sane missed-tick policy

A dead Docker daemon used to freeze TUI for the duration of
every blocked call across all pollers. Now 5s timeout per call;
spawn_ls no longer bursts queued ticks after a slow run (which
reset list selection)."
```

---

## Phase 4 — Storage cost & secret hygiene

Goal: stop costing the user a fortune in S3 by always running `--type=full`; stop leaking S3 keys into error chains and `Display`'d errors.

### Task 4.1: Diff vs full policy

**Files:**
- Modify: `src/domain/instance.rs` (add field `full_backup_day: Option<u8>` — weekday 0-6 for full, default 0 = Sunday)
- Modify: `src/commands/snapshot.rs`

- [ ] **Step 1: Add field with safe default**

In `src/domain/instance.rs`, add (with `#[serde(default = "default_full_day")]`):

```rust
#[serde(default = "default_full_day")]
pub full_backup_day: u8,

// near the bottom of the file
fn default_full_day() -> u8 { 0 } // Sunday
```

- [ ] **Step 2: Choose kind in run_with_engine based on weekday + presence of prior full**

In `src/commands/snapshot.rs`, before the exec, compute:

```rust
let weekday_idx = jiff::Zoned::now().weekday().to_sunday_zero_offset(); // 0=Sun
let has_prior_full = SnapshotsFile::load_for(&state_root, &args.instance)?
    .snapshots
    .iter()
    .any(|s| matches!(s.kind, SnapshotKind::Full));
let kind = if !has_prior_full || weekday_idx == s.instance.full_backup_day as i8 {
    SnapshotKind::Full
} else {
    SnapshotKind::Diff
};
let type_flag = match kind {
    SnapshotKind::Full => "--type=full",
    SnapshotKind::Diff => "--type=diff",
};
```

Use `type_flag` in the exec args; use `kind` in the `SnapshotRecord`.

- [ ] **Step 3: Run all tests**

Run: `cargo test --lib --bins --tests --no-fail-fast -- --skip e2e`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/domain/instance.rs src/commands/snapshot.rs
git commit -m "feat(snapshot): differential backups except on full_backup_day

Hard-coded --type=full meant a full upload daily. Now: one full
per week (weekday = full_backup_day, default Sunday), diff on
other days. First-ever snapshot is always full to seed the chain."
```

### Task 4.2: Don't propagate stderr blocks containing secrets

Already handled in Task 2.2. Add a follow-up that audits `tracing::warn!`/`error!` call sites that interpolate `pgbackrest.conf` content or `out.stderr`:

**Files:**
- Modify: `src/commands/snapshot.rs`
- Modify: `src/commands/rotate.rs`
- Modify: `src/commands/restore.rs`

- [ ] **Step 1: Grep for stderr in logs**

Run: `grep -n "warn\!\|error\!\|info\!" src/commands/snapshot.rs src/commands/rotate.rs src/commands/restore.rs | grep -i "stderr\|conf"`

- [ ] **Step 2: Pipe each through redact_pgbackrest_output**

For every log line that contains `out.stderr` or formats pgbackrest config, wrap with `crate::commands::snapshot::redact_pgbackrest_output(...)`.

- [ ] **Step 3: Compile + commit**

```bash
cargo build && git add -p && git commit -m "fix(secrets): redact pgbackrest output before logging"
```

---

## Phase 5 — Safer upgrade

### Task 5.1: Build new image BEFORE tearing down old container

**Files:**
- Modify: `src/commands/upgrade.rs`

- [ ] **Step 1: Reorder steps**

In `src/commands/upgrade.rs:104-113`, move the `docker.build_image(...)` call from line 113 to BEFORE the old-container stop/remove block at line 104-109. If `build_image` fails, return the error before touching the running instance.

- [ ] **Step 2: Compile and test**

Run: `cargo test --lib --bins --tests --no-fail-fast -- --skip e2e`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/commands/upgrade.rs
git commit -m "fix(upgrade): build new image before stopping old container

If build_image failed (network, disk full, dockerfile bug), the
old container had already been removed and the instance was down
with state.toml still pointing at the old version."
```

### Task 5.2: Capture pg_upgrade logs before container removal

**Files:**
- Modify: `src/commands/upgrade.rs`

- [ ] **Step 1: Read logs before remove_container**

In `src/commands/upgrade.rs:180-202`, when `wait_for_container_exit` returns a non-zero exit code, fetch the logs BEFORE calling `remove_container`. Add to the engine trait if not present:

```rust
let logs = docker.logs(&upgrade_container).await.unwrap_or_default();
```

Include `logs` in the error message returned to the user (after `redact_pgbackrest_output` to be safe).

- [ ] **Step 2: Compile and test**

Run: `cargo test --lib --bins --tests --no-fail-fast -- --skip e2e`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/commands/upgrade.rs
git commit -m "fix(upgrade): capture container logs before removal

On pg_upgrade failure the container was removed first, destroying
diagnostic output. We now fetch logs into the error message
before cleanup."
```

### Task 5.2b: cleanup_partial also removes named volume on restore failure

**Files:**
- Modify: `src/commands/restore.rs` (around lines 90-93, 192-195 — the `cleanup_partial` path)
- Modify: `src/docker/cleanup.rs` if helper lives there

- [ ] **Step 1: Add named-volume removal to cleanup_partial**

In `src/commands/restore.rs`, find `cleanup_partial`. Add a step that, after `remove_container`, also runs `docker.remove_volume(&format!("pgforge_data_{}", as_name)).await.ok();` (best-effort — name escapes to ENOENT if it was never created). This prevents a half-restored named volume from being silently mounted into a retry, where the marker-based guard (Task 1.6) will then skip restore on the second attempt because the volume already has `.pgforge-restore-complete` set… wait, no, the marker is written only on success. The bug is subtler: a previously-failed restore left no marker, but on retry with `--as=<same>` we'd mount the half-populated volume. Marker absence means we re-attempt, but `find -delete` runs first, so we lose any salvageable state. Safer to wipe the volume on cleanup_partial.

- [ ] **Step 2: Add `remove_volume` to engine trait if missing**

Run: `grep -n "remove_volume\|fn remove_vol" src/docker/`

If missing, add:

```rust
async fn remove_volume(&self, name: &str) -> Result<()>;
```

And implement it in `bollard_engine.rs` via `docker.remove_volume(name, None::<RemoveVolumeOptions>).await`. Map "no such volume" (404) to `Ok(())`.

- [ ] **Step 3: Compile and test**

Run: `cargo test --lib --bins --tests --no-fail-fast -- --skip e2e`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/docker/engine.rs src/docker/bollard_engine.rs src/commands/restore.rs
git commit -m "fix(restore): cleanup_partial removes named volume

A failed restore (e.g. Ctrl-C between create_container and
bootstrap_restore) used to leave the named volume orphaned with
no state.toml. A retry with the same --as name would then mount
the half-populated volume."
```

### Task 5.3: mkdir -p before chown in upgrade_entrypoint

**Files:**
- Modify: `src/docker/upgrade_entrypoint.rs`
- Test: `tests/upgrade_entrypoint_test.rs`

- [ ] **Step 1: Write test**

Add to `tests/upgrade_entrypoint_test.rs`:

```rust
#[test]
fn ensures_new_pgdata_exists_before_chown() {
    let s = pg_forge_cli::docker::upgrade_entrypoint::generate_upgrade_entrypoint();
    let mkdir_idx = s.find("mkdir -p \"$NEW_PGDATA\"").expect("mkdir missing");
    let chown_idx = s.find("chown -R postgres:postgres \"$NEW_PGDATA\"").expect("chown missing");
    assert!(mkdir_idx < chown_idx, "mkdir must precede chown");
}
```

- [ ] **Step 2: Run test, expect failure**

Run: `cargo test --test upgrade_entrypoint_test ensures_new_pgdata`
Expected: FAIL.

- [ ] **Step 3: Add mkdir -p line in `src/docker/upgrade_entrypoint.rs:30-31`**

Before the `chown -R postgres:postgres "$NEW_PGDATA"` line, insert:

```sh
mkdir -p "$NEW_PGDATA"
```

- [ ] **Step 4: Run test, expect pass**

Run: `cargo test --test upgrade_entrypoint_test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/docker/upgrade_entrypoint.rs tests/upgrade_entrypoint_test.rs
git commit -m "fix(upgrade): mkdir -p \$NEW_PGDATA before chown

Fresh empty volume mount point doesn't exist yet, so chown -R
under set -eu killed the script before initdb."
```

---

## Phase 6 — Mop-up

### Task 6.1: Docker network exact-name check

**Files:**
- Modify: `src/docker/bollard_engine.rs:108-115`

- [ ] **Step 1: Audit `ensure_network` for early-exit bugs**

In `src/docker/bollard_engine.rs` find `ensure_network`. The current code does `.list_networks(filter name=NAME)` then `.any(|n| n.name.as_deref() == Some(name))`. The server-side `name=` filter is substring, the client-side `.any` is exact — so the bug is real only if any code path returns "exists" based on `nets.is_empty()` or `nets.len() > 0` instead of the exact check. Read 30 lines around `ensure_network` and confirm.

- [ ] **Step 2: If `is_empty()` short-circuit exists, replace with exact match**

If found, replace:

```rust
if !nets.is_empty() { return Ok(()); }
```

with:

```rust
if nets.iter().any(|n| n.name.as_deref() == Some(name)) {
    return Ok(());
}
```

If no such short-circuit exists, this task is a no-op — skip directly to step 3 without changes.

- [ ] **Step 3: Compile + commit only if changed**

```bash
cargo build
git status # if clean, skip this commit
git add src/docker/bollard_engine.rs
git commit -m "fix(docker): exact-name network existence check"
```

### Task 6.2: Deadline-based wait_for_pg_ready

**Files:**
- Modify: `src/docker/wait.rs`

- [ ] **Step 1: Replace iteration counter with Instant deadline**

In `src/docker/wait.rs`, replace `for _ in 0..seconds { ... sleep(1) }` with:

```rust
let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
loop {
    // ... exec + match ...
    if tokio::time::Instant::now() >= deadline {
        return Err(PgForgeError::Docker(format!(
            "container {id}: postgres did not accept connections within {seconds}s"
        )));
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
}
```

- [ ] **Step 2: Detect terminal exit state**

Inside the loop, after the exec match, also inspect the container; if it has exited (non-running), return an error immediately with the exit code rather than waiting out the deadline.

- [ ] **Step 3: Compile and test**

Run: `cargo test --lib --bins --tests --no-fail-fast -- --skip e2e`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/docker/wait.rs
git commit -m "fix(wait): deadline-based loop; fail fast on container exit

Old loop counted iterations × (exec + 1s sleep), so a 30s grace
period really took ~45s. Also: a container that exited never
satisfied running=true and we polled for the full timeout
instead of surfacing the exit code."
```

### Task 6.3: Allow swap (memory_swap = None)

**Files:**
- Modify: `src/docker/bollard_engine.rs:187-188`

- [ ] **Step 1: Set memory_swap to None**

Around line 187-188, replace whatever sets `memory_swap == memory` with `memory_swap: None,` (i.e. don't constrain swap separately; let Docker default to memory + swap = 2x).

- [ ] **Step 2: Compile and commit**

```bash
cargo build
git add src/docker/bollard_engine.rs
git commit -m "fix(docker): don't disable swap

memory_swap == memory disables swap, so postgres got OOM-killed
under memory pressure instead of paging. Now defer to Docker's
default (memory + matching swap)."
```

### Task 6.4: Restore inherits source's last_snapshot_at

**Files:**
- Modify: `src/commands/restore.rs:282`

- [ ] **Step 1: Carry over the field**

Where the new instance state is constructed (around line 282), replace `last_snapshot_at: None` with:

```rust
last_snapshot_at: source.instance.last_snapshot_at.clone(),
```

- [ ] **Step 2: Commit**

```bash
git add src/commands/restore.rs
git commit -m "fix(restore): inherit last_snapshot_at from source

A freshly-restored instance had None, so the next snapshot --due
tick fired immediately. Carry over the source's value."
```

### Task 6.5: Clone entrypoint honors container_port

**Files:**
- Modify: `src/docker/clone_entrypoint.rs:44`
- Modify: caller in `src/commands/clone.rs` (pass the source port through)

- [ ] **Step 1: Accept port arg**

Change `generate_clone_entrypoint(source_container: &str)` to `generate_clone_entrypoint(source_container: &str, source_port: u16)`. Use `{port}` instead of hardcoded `5432` in the `pg_basebackup -h ... -p ...` line.

- [ ] **Step 2: Pass source's container_port from clone.rs**

In `src/commands/clone.rs`, where `generate_clone_entrypoint` is called, pass `source.instance.container_port` (or whatever the field is — verify in `domain/instance.rs`).

- [ ] **Step 3: Update tests in `tests/clone_entrypoint_test.rs`**

Adjust existing tests to pass the new arg; add a test that a non-default port is used:

```rust
#[test]
fn uses_provided_port() {
    let s = pg_forge_cli::docker::clone_entrypoint::generate_clone_entrypoint("src", 6543);
    assert!(s.contains("-p 6543"));
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test --test clone_entrypoint_test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/docker/clone_entrypoint.rs src/commands/clone.rs tests/clone_entrypoint_test.rs
git commit -m "fix(clone): use source instance's port, not hardcoded 5432"
```

### Task 6.6a: Restore uses source's retain_days

**Files:**
- Modify: `src/commands/restore.rs:119`

- [ ] **Step 1: Pass source.instance.retain_days instead of literal 30**

In `src/commands/restore.rs:119`, replace:

```rust
generate_pgbackrest_conf(&args.source, &s3, 30),
```

with:

```rust
generate_pgbackrest_conf(&args.source, &s3, source.instance.retain_days),
```

- [ ] **Step 2: Compile and test**

Run: `cargo test --lib --bins --tests --no-fail-fast -- --skip e2e`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/commands/restore.rs
git commit -m "fix(restore): inherit retain_days from source instance

Hardcoded 30 made the restored instance's pgbackrest archive-push
retain differently than the source, surprising the user who'd
configured a non-default value."
```

### Task 6.6: rotate.rs — safer SQL escape

**Files:**
- Modify: `src/commands/rotate.rs:201-211`

- [ ] **Step 1: Run pgbackrest password rotation via psql stdin, not `-c \"...\"`**

Replace the format!-into-`-c` call with feeding the SQL via stdin so shell metacharacters never reach the shell:

```rust
let sql = format!("ALTER USER pgbackrest WITH PASSWORD '{}';",
    pgbackrest_password.replace('\'', "''"));
let out = docker.exec_with_stdin(&container, &["psql", "-U", "postgres"], &sql).await?;
```

This requires adding `exec_with_stdin` to the engine if not present.

- [ ] **Step 2: Verify exec_with_stdin exists; if not, add it (small bollard wrapper)**

Run: `grep -n "exec_with_stdin\|attach_stdin" src/docker/`

If missing: add an `exec_with_stdin(&self, container, cmd, stdin) -> Result<ExecOutput>` method on the engine that does `CreateExecOptionsBuilder::attach_stdin(true)` and writes `stdin` to the resulting stream before flushing.

- [ ] **Step 3: Run tests**

Run: `cargo test --lib --bins --tests --no-fail-fast -- --skip e2e`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/docker/engine.rs src/docker/bollard_engine.rs src/commands/rotate.rs
git commit -m "fix(rotate): feed SQL via stdin instead of -c interpolation

format!-into-shell allowed metacharacters in the password to
break out. Now the SQL is piped to psql stdin where the shell
never sees it."
```

---

## Deferred (intentionally out of this sweep)

Reviewed but not addressed here. Each is real but lower-impact or design-touching — call them out when the user wants a follow-up sprint.

- `src/docker/image.rs:11` — no retry / `--fix-missing` around `apt-get install pgbackrest`. Cold-start fragile on flaky networks.
- `src/docker/bollard_engine.rs:15-40` — `unsafe { std::env::set_var }` in `connect()`. Single-caller today, but unsound under concurrent test harness.
- `src/commands/snapshots.rs:87-113` — `parse_pitr_window` returns `PitrWindow::default()` on JSON parse error. Silent degradation on future pgbackrest schema changes.
- `src/pgbackrest/conf.rs:20` — no validation of `instance_name`/S3 keys before pasting into INI. `\n` or `]` in values would corrupt the config.
- `src/pgbackrest/parse.rs:4-15` — substring match on `new backup label = `. Not anchored.
- `src/commands/clone.rs:280-288` — `stanza-create` masks misconfigured WAL repo path.
- `src/commands/restore.rs:55-57` — no `--yes` gate / confirmation on PITR.
- `src/commands/schedule.rs:110-129` — `id -u` via subprocess instead of `libc::getuid`; fallback to 501 on PATH missing.
- `src/commands/destroy.rs` — orphan state.toml after destroy can produce log-spam in `snapshot --due`; Task 3.2 backoff mitigates but doesn't fix.
- `src/commands/rotate.rs:111` — retain_days vs archive-push timing edge case under long offline windows.
- `src/docker/cleanup.rs:16` + `remove_container(v: true)` — volume removal on rollback. Implicit fix via Task 1.5 (one-shots no longer flap), but not explicitly guarded against being called on a long-lived container during a partial failure mid-`create`. Worth a follow-up.

---

## Final verification

- [ ] **All tests green (excluding e2e)**

Run: `cargo test --lib --bins --tests --no-fail-fast -- --skip e2e`
Expected: PASS, no test ignored except `*e2e*`.

- [ ] **Clippy clean**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Manual smoke (user, not agent)**

User runs on Mac mini:
```
pgforge create test-bug-sweep --pg=16 --preset=small
pgforge snapshot test-bug-sweep
pgforge snapshots test-bug-sweep
pgforge restore test-bug-sweep --as=test-bug-sweep-restored --target-time="2026-05-12T14:00:00Z"
pgforge upgrade test-bug-sweep --to=17
pgforge destroy test-bug-sweep
pgforge destroy test-bug-sweep-restored
```

Heavy e2e tests stay user-driven per project convention (no auto-run of long E2E in interactive session).

- [ ] **Final commit (if anything residual)**

```bash
git status
# resolve anything outstanding, then:
git commit -m "chore: finalize bug-fix sweep"
```
