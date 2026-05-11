# pgforge Plan 3: Clone via pg_basebackup

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `pgforge clone <source> --as <new>` — creates a NEW instance that is a sibling copy of `<source>` at "now". Uses `pg_basebackup` (streaming replication protocol) instead of pgbackrest, so no S3 round-trip is needed. Source must be running.

Plus a small new command `pgforge reconfigure <name>` to update existing instances' `pg_hba.conf` (needed because Plan 1/2 didn't allow `host replication`, which the clone container needs).

Plus 2 quality follow-ups from Plan 2's opus review that this plan would otherwise have to re-litigate: extract the duplicated `wait_for_pg_ready` helper, and add a rollback helper used by create/restore/clone on mid-flight failures.

**Architecture:**
- Clone is implemented identically to Plan 2's restore in structure: a custom `clone-entrypoint.sh` is bind-mounted into the new container, runs `pg_basebackup -h pgforge_<source>` against the running source on first start, then exec's into the official postgres entrypoint.
- pg_basebackup authenticates as the `pgbackrest` role (which has REPLICATION SUPERUSER from Plan 1 init SQL). The clone container reads the password from a generated `.pgpass` file bind-mounted in.
- Source instance must allow `host replication pgbackrest samenet scram-sha-256`. New instances get this from the updated generator; existing ones need `pgforge reconfigure` to pick it up.
- `pgforge reconfigure` re-renders `pg_hba.conf` against current state and runs `pg_ctl reload` inside the container — no restart needed since pg_hba reload is online.

**Tech stack additions:** None. Everything builds on existing deps (bollard, tokio, etc.).

---

## Plan roadmap (this plan = #3 of 5)

1. ✅ Foundation + create — Plan 1.
2. ✅ Snapshot + restore PITR — Plan 2.
3. **Clone (pg_basebackup)** — this plan.
4. Upgrade in place (pg_upgrade with auto pre-snapshot) — needs Plan 2's snapshot.
5. TUI dashboard (ratatui).

---

## File structure (delta on top of Plans 1+2)

```
pg_forge_cli/
├── src/
│   ├── cli.rs                          # add Clone + Reconfigure subcommands
│   ├── commands/
│   │   ├── mod.rs                      # add clone, reconfigure
│   │   ├── clone.rs                    # NEW: pgforge clone orchestration
│   │   ├── reconfigure.rs              # NEW: pgforge reconfigure command
│   │   ├── create.rs                   # MODIFY: use docker::wait + cleanup_on_failure
│   │   └── restore.rs                  # MODIFY: use docker::wait + cleanup_on_failure
│   ├── docker/
│   │   ├── mod.rs                      # add clone_entrypoint, wait, cleanup
│   │   ├── clone_entrypoint.rs         # NEW: generate clone-entrypoint.sh
│   │   ├── wait.rs                     # NEW: extracted wait_for_pg_ready (short + long)
│   │   ├── cleanup.rs                  # NEW: cleanup_container_and_volume helper
│   │   ├── engine.rs                   # MODIFY: add remove_container + remove_volume methods
│   │   └── bollard_engine.rs           # MODIFY: implement new methods
│   ├── postgres/
│   │   └── hba.rs                      # MODIFY: add host replication rule
│   └── pgbackrest/
│       └── pgpass.rs                   # NEW: generate .pgpass for pgbackrest role
└── tests/
    ├── pg_hba_test.rs                  # MODIFY: assert new replication rule
    ├── pgpass_test.rs                  # NEW
    ├── clone_entrypoint_test.rs        # NEW
    └── clone_e2e_test.rs               # NEW gated by PGFORGE_E2E=1
```

---

## Task 1: Extract `wait_for_pg_ready` to `docker/wait.rs`

**Files:**
- Create: `src/docker/wait.rs`
- Modify: `src/docker/mod.rs`
- Modify: `src/commands/create.rs` (delete local helper, use shared)
- Modify: `src/commands/restore.rs` (delete local helper, use shared)

- [ ] **Step 1: Create `src/docker/wait.rs`**

```rust
use crate::docker::engine::DockerEngine;
use crate::error::{PgForgeError, Result};
use std::time::Duration;

/// Poll `pg_isready -h /var/run/postgresql` inside the container until exit
/// code 0 or `seconds` elapse. Used by create (30s) and clone (60s).
pub async fn wait_for_pg_ready<E: DockerEngine>(
    docker: &E,
    id: &str,
    seconds: u64,
) -> Result<()> {
    for _ in 0..seconds {
        let out = docker
            .exec(id, &["pg_isready", "-h", "/var/run/postgresql"])
            .await?;
        if out.exit_code == 0 {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(PgForgeError::Docker(format!(
        "container {id}: postgres did not accept connections within {seconds}s"
    )))
}
```

- [ ] **Step 2: Add to `src/docker/mod.rs`**

```rust
pub mod bollard_engine;
pub mod engine;
pub mod image;
pub mod restore_entrypoint;
pub mod wait;
```

- [ ] **Step 3: Replace helper in `src/commands/create.rs`**

Delete the local `wait_for_pg_ready` function at the bottom of the file. Replace the call site:
```rust
// was: wait_for_pg_ready(docker, &id).await?;
crate::docker::wait::wait_for_pg_ready(docker, &id, 30).await?;
```

- [ ] **Step 4: Replace helper in `src/commands/restore.rs`**

Delete the local `wait_for_pg_ready_long` function. Replace the call site:
```rust
// was: wait_for_pg_ready_long(docker, &id).await?;
crate::docker::wait::wait_for_pg_ready(docker, &id, 600).await?;
```

- [ ] **Step 5: Build + test**

```bash
cargo build
cargo test
```
Expect: all tests pass, no regressions.

- [ ] **Step 6: Commit**

```bash
git add .
git commit -m "refactor(docker): extract wait_for_pg_ready helper, drop duplicates"
```

---

## Task 2: Add `remove_container` + `remove_volume` to DockerEngine

**Files:**
- Modify: `src/docker/engine.rs`
- Modify: `src/docker/bollard_engine.rs`
- Modify: `src/commands/create.rs` (extend RecordingEngine mock with new methods)

- [ ] **Step 1: Edit `src/docker/engine.rs` — add to `DockerEngine` trait**

```rust
/// Remove a container (force=true → kill if running). Used for rollback.
async fn remove_container(&self, id: &str, force: bool) -> Result<()>;

/// Remove a named volume. Used for rollback. No-op if missing.
async fn remove_volume(&self, name: &str) -> Result<()>;
```

- [ ] **Step 2: Edit `src/docker/bollard_engine.rs` — add real impls**

```rust
async fn remove_container(&self, id: &str, force: bool) -> Result<()> {
    use bollard::query_parameters::RemoveContainerOptionsBuilder;
    let opts = RemoveContainerOptionsBuilder::default()
        .force(force)
        .v(true) // also remove anonymous volumes attached
        .build();
    self.docker
        .remove_container(id, Some(opts))
        .await
        .map_err(|e| PgForgeError::Docker(format!("remove_container({id}): {e}")))
}

async fn remove_volume(&self, name: &str) -> Result<()> {
    use bollard::query_parameters::RemoveVolumeOptionsBuilder;
    let opts = RemoveVolumeOptionsBuilder::default().force(true).build();
    match self.docker.remove_volume(name, Some(opts)).await {
        Ok(_) => Ok(()),
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => Ok(()), // already gone
        Err(e) => Err(PgForgeError::Docker(format!(
            "remove_volume({name}): {e}"
        ))),
    }
}
```

If bollard 0.21 API has moved these (e.g. `RemoveContainerOptions` in a different module), use the compiler errors to find the right path. The 404 idempotency for volumes may need a different error-match shape — check what `Error` variants bollard 0.21 actually exposes.

- [ ] **Step 3: Extend `RecordingEngine` mock in `src/commands/create.rs` `#[cfg(test)] mod tests`**

```rust
async fn remove_container(&self, _: &str, _: bool) -> crate::error::Result<()> {
    self.calls.lock().unwrap().push("remove_container");
    Ok(())
}
async fn remove_volume(&self, _: &str) -> crate::error::Result<()> {
    self.calls.lock().unwrap().push("remove_volume");
    Ok(())
}
```

- [ ] **Step 4: Build + test**

```bash
cargo build
cargo test
```
Expect: clean, no regressions.

- [ ] **Step 5: Commit**

```bash
git add .
git commit -m "feat(docker): add remove_container + remove_volume for rollback"
```

---

## Task 3: `cleanup_on_failure` helper

**Files:**
- Create: `src/docker/cleanup.rs`
- Modify: `src/docker/mod.rs`
- Modify: `src/commands/create.rs` (wrap post-create steps)
- Modify: `src/commands/restore.rs` (wrap post-create steps)

**Design:** A small RAII-style cleanup. Wrap the "create container + start + wait + post-init" sequence in a `CleanupGuard` that on Drop (if not disarmed) calls `remove_container` + `remove_volume`. The orchestrator disarms it after `state.save_under` succeeds.

Async Drop is awkward in Rust, so use a manual cleanup function instead of Drop — call it explicitly on the error paths.

- [ ] **Step 1: Implement `src/docker/cleanup.rs`**

```rust
use crate::docker::engine::DockerEngine;
use crate::error::Result;

/// Best-effort cleanup of a half-created instance. Used after a mid-flight
/// failure (between create_container and state.save_under). Swallows
/// errors from individual steps — the caller already has a primary error
/// they want to surface.
pub async fn cleanup_partial<E: DockerEngine>(
    docker: &E,
    container_name: &str,
    volume_name: &str,
) {
    let _ = docker.remove_container(container_name, true).await;
    let _ = docker.remove_volume(volume_name).await;
}
```

- [ ] **Step 2: Wire into `src/docker/mod.rs`**

```rust
pub mod bollard_engine;
pub mod cleanup;
pub mod engine;
pub mod image;
pub mod restore_entrypoint;
pub mod wait;
```

- [ ] **Step 3: Use in `src/commands/create.rs`**

After `let id = docker.create_container(&spec).await?;`, wrap the remaining steps (start, wait_for_container_running, wait_for_pg_ready, stanza-create, state.save_under) in a closure-like pattern that calls cleanup on any error. Simplest: refactor to an `async fn post_create_steps` that returns Result, and the outer code does:

```rust
let id = docker.create_container(&spec).await?;
let container_name = spec.container_name.clone();
let volume_name = spec.volumes[0].volume_name.clone();
match post_create_steps(docker, &id, /* state + args */).await {
    Ok(state) => Ok(state),
    Err(e) => {
        crate::docker::cleanup::cleanup_partial(docker, &container_name, &volume_name).await;
        Err(e)
    }
}
```

Where `post_create_steps` does: start_container → wait_for_container_running → wait_for_pg_ready → stanza-create → build InstanceState → state.save_under → return state.

Same pattern applied in `restore.rs` for the start → wait → state-save sequence.

- [ ] **Step 4: Build + test**

```bash
cargo build
cargo test
```

- [ ] **Step 5: Commit**

```bash
git add .
git commit -m "feat(commands): rollback orphan container + volume on mid-flight failure"
```

---

## Task 4: Update `pg_hba.conf` to allow host replication (TDD)

**Files:**
- Modify: `src/postgres/hba.rs`
- Modify: `tests/pg_hba_test.rs`

- [ ] **Step 1: Add a failing assertion to `tests/pg_hba_test.rs`**

After the existing tests, append:

```rust
#[test]
fn hba_grants_host_replication_to_pgbackrest_over_samenet() {
    // Clone uses pg_basebackup from a sibling container — host replication
    // over the docker bridge is required.
    let hba = generate_pg_hba("billing", "leads");
    assert!(
        hba.contains("host    replication     pgbackrest      samenet                 scram-sha-256"),
        "must allow host replication from samenet, got:\n{hba}"
    );
}
```

- [ ] **Step 2: Run — expect failure**

```bash
cargo test --test pg_hba_test
```

- [ ] **Step 3: Edit `src/postgres/hba.rs`**

Find the existing pg_hba template body. Between the "local replication / local all pgbackrest trust" block and the "host {db} {user} samenet scram" line, add:

```
# pgBackRest / pg_basebackup can also connect over the docker bridge from
# sibling containers (used by `pgforge clone`).
host    replication     pgbackrest      samenet                 scram-sha-256
```

Make sure column alignment matches the surrounding rows so the existing tests still pass.

- [ ] **Step 4: Run pg_hba test — expect pass**

```bash
cargo test --test pg_hba_test
```

- [ ] **Step 5: Run all tests**

```bash
cargo test
```

- [ ] **Step 6: Commit**

```bash
git add .
git commit -m "feat(postgres): allow host replication from samenet (enables clone)"
```

---

## Task 5: `pgforge reconfigure` command

**Files:**
- Create: `src/commands/reconfigure.rs`
- Modify: `src/commands/mod.rs`
- Modify: `src/cli.rs`

**Purpose:** Re-render `pg_hba.conf` for an existing instance, write it to the bind-mounted file on host, run `pg_ctl reload` inside the container to make PG pick up the change. No container restart needed.

- [ ] **Step 1: Implement `src/commands/reconfigure.rs`**

```rust
use crate::docker::bollard_engine::BollardEngine;
use crate::docker::engine::DockerEngine;
use crate::error::{PgForgeError, Result};
use crate::postgres::hba::generate_pg_hba;
use crate::state::instance::InstanceState;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ReconfigureArgs {
    pub instance: String,
    pub override_state_root: Option<PathBuf>,
}

pub async fn run(args: ReconfigureArgs) -> Result<()> {
    let state_root = args
        .override_state_root
        .clone()
        .unwrap_or_else(InstanceState::default_state_root);
    let state = InstanceState::load_under(&state_root, &args.instance)?;
    let docker = BollardEngine::connect()?;
    run_with_engine(args.instance.clone(), state, &docker, state_root).await
}

pub async fn run_with_engine<E: DockerEngine>(
    instance: String,
    state: InstanceState,
    docker: &E,
    state_root: PathBuf,
) -> Result<()> {
    // 1. Regenerate pg_hba.conf on host.
    let conf_dir = state_root
        .join("instances")
        .join(&instance)
        .join("conf");
    let pg_hba_path = conf_dir.join("pg_hba.conf");
    let new_hba = generate_pg_hba(&state.instance.db_name, &state.instance.app_user);
    std::fs::write(&pg_hba_path, new_hba).map_err(|e| PgForgeError::Io {
        path: pg_hba_path.clone(),
        source: e,
    })?;

    // 2. Reload PG inside container (no restart).
    let container = format!("pgforge_{}", instance);
    let out = docker
        .exec(
            &container,
            &[
                "su", "-", "postgres", "-c",
                "pg_ctl reload -D /var/lib/postgresql/data/pgdata",
            ],
        )
        .await?;
    if out.exit_code != 0 {
        return Err(PgForgeError::Docker(format!(
            "pg_ctl reload failed (exit {}): {}",
            out.exit_code, out.stderr
        )));
    }
    Ok(())
}
```

- [ ] **Step 2: Update `src/commands/mod.rs` (alphabetical)**

```rust
pub mod create;
pub mod reconfigure;
pub mod restore;
pub mod snapshot;
pub mod snapshots;
```

- [ ] **Step 3: Wire CLI in `src/cli.rs`**

Add variant:

```rust
/// Regenerate pg_hba.conf for an instance and reload PG (no restart).
Reconfigure {
    #[arg(long)]
    name: String,
},
```

Match arm:

```rust
Some(Command::Reconfigure { name }) => {
    crate::commands::reconfigure::run(crate::commands::reconfigure::ReconfigureArgs {
        instance: name.clone(),
        override_state_root: None,
    })
    .await?;
    println!("Reconfigured {name}.");
    Ok(())
}
```

- [ ] **Step 4: Build + verify help**

```bash
cargo build
cargo run -- reconfigure --help
```

- [ ] **Step 5: Run all tests**

```bash
cargo test
```

- [ ] **Step 6: Commit**

```bash
git add .
git commit -m "feat(commands): pgforge reconfigure — regenerate pg_hba + pg_ctl reload"
```

---

## Task 6: `.pgpass` generator for pgbackrest credentials (TDD)

**Files:**
- Create: `src/pgbackrest/pgpass.rs`
- Modify: `src/pgbackrest/mod.rs`
- Create: `tests/pgpass_test.rs`

**Why:** pg_basebackup (run inside the clone container) needs to authenticate as `pgbackrest` over scram-sha-256. Without a `.pgpass` file or `PGPASSWORD` env var, it will prompt and fail. We generate a `.pgpass` and bind-mount it.

- [ ] **Step 1: Failing test — `tests/pgpass_test.rs`**

```rust
use pgforge::pgbackrest::pgpass::generate_pgpass;

#[test]
fn pgpass_contains_pgbackrest_role_with_password() {
    let s = generate_pgpass("hunter2");
    assert!(s.contains(":pgbackrest:hunter2"));
}

#[test]
fn pgpass_is_wildcard_host_port_db() {
    let s = generate_pgpass("hunter2");
    // pgpass format: hostname:port:database:username:password
    // We use *:*:* for hostname/port/database (matches any).
    assert!(s.starts_with("*:*:*:pgbackrest:"));
}

#[test]
fn pgpass_ends_with_newline() {
    let s = generate_pgpass("hunter2");
    assert!(s.ends_with('\n'), "pgpass must end with newline, got: {s:?}");
}

#[test]
fn pgpass_escapes_colons_and_backslashes() {
    // pgpass uses : as field separator and \ as escape character.
    // A password containing : must be escaped as \:
    let s = generate_pgpass(r"pa:ss\word");
    assert!(s.contains(r"pa\:ss\\word"), "expected escaped, got: {s}");
}
```

- [ ] **Step 2: Run — expect module missing**

```bash
cargo test --test pgpass_test
```

- [ ] **Step 3: Implement `src/pgbackrest/pgpass.rs`**

```rust
/// Render a `.pgpass` file allowing the `pgbackrest` role to authenticate
/// without prompting. Format per Postgres docs:
///   hostname:port:database:username:password
/// `\` and `:` in the password field must be escaped with `\`.
pub fn generate_pgpass(pgbackrest_password: &str) -> String {
    let escaped: String = pgbackrest_password
        .chars()
        .flat_map(|c| match c {
            '\\' => vec!['\\', '\\'],
            ':' => vec!['\\', ':'],
            other => vec![other],
        })
        .collect();
    format!("*:*:*:pgbackrest:{escaped}\n")
}
```

- [ ] **Step 4: Wire into `src/pgbackrest/mod.rs`**

```rust
pub mod conf;
pub mod parse;
pub mod pgpass;
```

- [ ] **Step 5: Run — expect 4 pass**

```bash
cargo test --test pgpass_test
```

- [ ] **Step 6: Commit**

```bash
git add .
git commit -m "feat(pgbackrest): generate .pgpass for pgbackrest role"
```

---

## Task 7: Clone entrypoint generator (TDD)

**Files:**
- Create: `src/docker/clone_entrypoint.rs`
- Modify: `src/docker/mod.rs`
- Create: `tests/clone_entrypoint_test.rs`

The script runs as the postgres image's entrypoint, checks whether PGDATA is empty, and if so runs `pg_basebackup` from the source over the docker network. Then `exec`s the official postgres entrypoint.

- [ ] **Step 1: Failing test — `tests/clone_entrypoint_test.rs`**

```rust
use pgforge::docker::clone_entrypoint::generate_clone_entrypoint;

#[test]
fn entrypoint_runs_pg_basebackup_from_source() {
    let script = generate_clone_entrypoint("pgforge_billing");
    assert!(script.contains("pg_basebackup"));
    assert!(script.contains("-h pgforge_billing"));
    assert!(script.contains("-U pgbackrest"));
    assert!(script.contains("-D /var/lib/postgresql/data/pgdata"));
    assert!(script.contains("-X stream"));
}

#[test]
fn entrypoint_skips_basebackup_if_pgdata_populated() {
    let script = generate_clone_entrypoint("pgforge_billing");
    assert!(
        script.contains("PG_VERSION"),
        "expected a 'is PGDATA empty?' check, got:\n{script}"
    );
}

#[test]
fn entrypoint_sets_pgpassfile_to_mounted_path() {
    let script = generate_clone_entrypoint("pgforge_billing");
    // .pgpass is bind-mounted; PGPASSFILE env tells pg_basebackup to use it.
    assert!(script.contains("PGPASSFILE=/var/lib/postgresql/.pgpass"));
}

#[test]
fn entrypoint_execs_official_postgres_entrypoint_at_end() {
    let script = generate_clone_entrypoint("pgforge_billing");
    assert!(script.contains("exec docker-entrypoint.sh postgres"));
}

#[test]
fn entrypoint_starts_with_shebang() {
    let script = generate_clone_entrypoint("pgforge_billing");
    assert!(script.starts_with("#!/"));
}
```

- [ ] **Step 2: Run — expect module missing**

```bash
cargo test --test clone_entrypoint_test
```

- [ ] **Step 3: Implement `src/docker/clone_entrypoint.rs`**

```rust
/// Render the entrypoint script bind-mounted into a clone container. If
/// PGDATA is empty, pg_basebackup from the source container over the
/// docker bridge (host `source_container`, port 5432, role pgbackrest).
/// Then chain into the official postgres entrypoint.
pub fn generate_clone_entrypoint(source_container: &str) -> String {
    format!(
        r#"#!/bin/sh
# Generated by pgforge — do not edit by hand.
set -eu

PGDATA="/var/lib/postgresql/data/pgdata"
export PGPASSFILE=/var/lib/postgresql/.pgpass

# Clone only if the data directory is empty / has no PG cluster yet.
if [ ! -f "$PGDATA/PG_VERSION" ]; then
    mkdir -p "$PGDATA"
    chown -R postgres:postgres "$PGDATA"
    su - postgres -c 'PGPASSFILE=/var/lib/postgresql/.pgpass pg_basebackup -h {source} -p 5432 -U pgbackrest -D /var/lib/postgresql/data/pgdata -X stream -P --wal-method=stream --no-password'
fi

exec docker-entrypoint.sh postgres
"#,
        source = source_container
    )
}
```

- [ ] **Step 4: Update `src/docker/mod.rs`**

```rust
pub mod bollard_engine;
pub mod cleanup;
pub mod clone_entrypoint;
pub mod engine;
pub mod image;
pub mod restore_entrypoint;
pub mod wait;
```

- [ ] **Step 5: Run — expect 5 pass**

```bash
cargo test --test clone_entrypoint_test
```

- [ ] **Step 6: Commit**

```bash
git add .
git commit -m "feat(docker): clone-entrypoint script that runs pg_basebackup then chains to postgres"
```

---

## Task 8: `pgforge clone` orchestration

**Files:**
- Create: `src/commands/clone.rs`
- Modify: `src/commands/mod.rs`
- Modify: `src/cli.rs`

**Note:** `clone` is a Rust keyword in some contexts (`Clone` trait), but as a module name `clone` is fine. As a CLI subcommand `Clone { ... }` and a function `commands::clone::run` work without renaming.

- [ ] **Step 1: Implement `src/commands/clone.rs`**

```rust
use crate::config::global::GlobalConfig;
use crate::docker::bollard_engine::BollardEngine;
use crate::docker::clone_entrypoint::generate_clone_entrypoint;
use crate::docker::cleanup::cleanup_partial;
use crate::docker::engine::{
    BindMount, BuildImageSpec, CreateContainerSpec, DockerEngine, NamedVolume,
};
use crate::docker::image::dockerfile;
use crate::docker::wait::wait_for_pg_ready;
use crate::domain::instance::Instance;
use crate::domain::platform::current_platform;
use crate::error::{PgForgeError, Result};
use crate::pgbackrest::conf::generate_pgbackrest_conf;
use crate::pgbackrest::pgpass::generate_pgpass;
use crate::ports::{TcpProbe, allocate_port};
use crate::postgres::conf::generate_postgresql_conf;
use crate::postgres::hba::generate_pg_hba;
use crate::state::instance::InstanceState;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CloneArgs {
    pub source: String,
    pub as_name: String,
    pub override_state_root: Option<PathBuf>,
}

pub async fn run(args: CloneArgs) -> Result<InstanceState> {
    Instance::validate_name(&args.as_name)?;
    let state_root = args
        .override_state_root
        .clone()
        .unwrap_or_else(InstanceState::default_state_root);
    let global = GlobalConfig::load()?;
    let s3 = global
        .s3
        .as_ref()
        .ok_or_else(|| PgForgeError::Anyhow(anyhow::anyhow!(
            "S3 settings missing in global config"
        )))?
        .clone();
    let source = InstanceState::load_under(&state_root, &args.source)?;
    if InstanceState::exists_under(&state_root, &args.as_name) {
        return Err(PgForgeError::InstanceExists(args.as_name.clone()));
    }
    let docker = BollardEngine::connect()?;
    run_with_engine(args, &docker, state_root, global, s3, source).await
}

pub async fn run_with_engine<E: DockerEngine>(
    args: CloneArgs,
    docker: &E,
    state_root: PathBuf,
    global: GlobalConfig,
    s3: crate::pgbackrest::conf::S3Settings,
    source: InstanceState,
) -> Result<InstanceState> {
    // Source must be running so pg_basebackup can connect to it.
    let source_container = format!("pgforge_{}", args.source);
    if !docker.container_exists(&source_container).await? {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "source container {source_container:?} is not running. Start it and retry."
        )));
    }

    let plat = current_platform();
    let tuning = source.instance.preset.tuning();

    // Port allocation.
    let taken: HashSet<u16> = InstanceState::list_under(&state_root)?
        .into_iter()
        .filter_map(|n| InstanceState::load_under(&state_root, &n).ok())
        .map(|s| s.instance.host_port)
        .collect();
    let host_port = allocate_port(
        global.port_range_start,
        global.port_range_end,
        &taken,
        &TcpProbe,
    )?;

    // Generate per-instance config dir.
    let root = state_root
        .join("instances")
        .join(&args.as_name)
        .join("conf");
    std::fs::create_dir_all(&root).map_err(|e| PgForgeError::Io {
        path: root.clone(),
        source: e,
    })?;
    let postgresql_conf = root.join("postgresql.conf");
    let pg_hba = root.join("pg_hba.conf");
    let pgbackrest_conf = root.join("pgbackrest.conf");
    let entrypoint = root.join("clone-entrypoint.sh");
    let pgpass = root.join("pgpass");

    // pg_hba uses the SOURCE's db_name (the cloned cluster will have it).
    std::fs::write(&postgresql_conf, generate_postgresql_conf(source.instance.preset, plat))
        .map_err(|e| PgForgeError::Io { path: postgresql_conf.clone(), source: e })?;
    std::fs::write(&pg_hba, generate_pg_hba(&source.instance.db_name, &source.instance.app_user))
        .map_err(|e| PgForgeError::Io { path: pg_hba.clone(), source: e })?;
    // pgbackrest.conf: this clone will have its OWN backup repo path going
    // forward (so post-clone snapshots/restores are tracked separately).
    std::fs::write(&pgbackrest_conf, generate_pgbackrest_conf(&args.as_name, &s3))
        .map_err(|e| PgForgeError::Io { path: pgbackrest_conf.clone(), source: e })?;
    std::fs::write(&entrypoint, generate_clone_entrypoint(&source_container))
        .map_err(|e| PgForgeError::Io { path: entrypoint.clone(), source: e })?;
    std::fs::write(&pgpass, generate_pgpass(&source.instance.pgbackrest_password))
        .map_err(|e| PgForgeError::Io { path: pgpass.clone(), source: e })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // entrypoint.sh — executable
        let mut perms = std::fs::metadata(&entrypoint)
            .map_err(|e| PgForgeError::Io { path: entrypoint.clone(), source: e })?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&entrypoint, perms)
            .map_err(|e| PgForgeError::Io { path: entrypoint.clone(), source: e })?;
        // .pgpass — must be 0600 or postgres will refuse it
        let mut pp = std::fs::metadata(&pgpass)
            .map_err(|e| PgForgeError::Io { path: pgpass.clone(), source: e })?
            .permissions();
        pp.set_mode(0o600);
        std::fs::set_permissions(&pgpass, pp)
            .map_err(|e| PgForgeError::Io { path: pgpass.clone(), source: e })?;
    }

    docker
        .build_image(&BuildImageSpec {
            tag: format!("pgforge/postgres:{}", source.instance.pg_version),
            dockerfile: dockerfile(source.instance.pg_version),
        })
        .await?;
    docker.ensure_network("pgforge_net").await?;

    let mut env = HashMap::new();
    env.insert("POSTGRES_USER".into(), source.instance.app_user.clone());
    env.insert("POSTGRES_PASSWORD".into(), source.instance.app_password.clone());
    env.insert("POSTGRES_DB".into(), source.instance.db_name.clone());
    env.insert("PGDATA".into(), "/var/lib/postgresql/data/pgdata".into());

    let binds = vec![
        BindMount {
            host_path: postgresql_conf.clone(),
            container_path: "/etc/postgresql/postgresql.conf".into(),
            read_only: true,
        },
        BindMount {
            host_path: pg_hba.clone(),
            container_path: "/etc/postgresql/pg_hba.conf".into(),
            read_only: true,
        },
        BindMount {
            host_path: pgbackrest_conf.clone(),
            container_path: "/etc/pgbackrest/pgbackrest.conf".into(),
            read_only: true,
        },
        BindMount {
            host_path: entrypoint.clone(),
            container_path: "/usr/local/bin/pgforge-clone-entrypoint.sh".into(),
            read_only: true,
        },
        BindMount {
            host_path: pgpass.clone(),
            container_path: "/var/lib/postgresql/.pgpass".into(),
            read_only: true,
        },
    ];
    let volumes = vec![NamedVolume {
        volume_name: format!("pgforge_data_{}", args.as_name),
        container_path: "/var/lib/postgresql/data".into(),
    }];

    let container_name = format!("pgforge_{}", args.as_name);
    let volume_name = format!("pgforge_data_{}", args.as_name);
    let spec = CreateContainerSpec {
        container_name: container_name.clone(),
        image: format!("pgforge/postgres:{}", source.instance.pg_version),
        env,
        binds,
        volumes,
        host_port,
        container_port: 5432,
        memory_mb: tuning.ram_mb,
        network: "pgforge_net".into(),
        shm_size_mb: 256,
        command_override: Some(vec!["/usr/local/bin/pgforge-clone-entrypoint.sh".into()]),
    };
    let id = docker.create_container(&spec).await?;

    // From here on, any failure should clean up the half-created container + volume.
    let result = post_create(
        docker,
        &id,
        args,
        source,
        host_port,
        state_root,
    )
    .await;

    match result {
        Ok(state) => Ok(state),
        Err(e) => {
            cleanup_partial(docker, &container_name, &volume_name).await;
            Err(e)
        }
    }
}

async fn post_create<E: DockerEngine>(
    docker: &E,
    id: &str,
    args: CloneArgs,
    source: InstanceState,
    host_port: u16,
    state_root: PathBuf,
) -> Result<InstanceState> {
    docker.start_container(id).await?;
    docker
        .wait_for_container_running(id, std::time::Duration::from_secs(30))
        .await?;
    // pg_basebackup of a small DB takes seconds; large ones minutes. Allow 10 min.
    wait_for_pg_ready(docker, id, 600).await?;

    let state = InstanceState {
        instance: Instance {
            name: args.as_name.clone(),
            db_name: source.instance.db_name,
            app_user: source.instance.app_user,
            app_password: source.instance.app_password,
            pgbackrest_password: source.instance.pgbackrest_password,
            preset: source.instance.preset,
            pg_version: source.instance.pg_version,
            host_port,
        },
        created_at: crate::time::now_iso(),
    };
    state.save_under(&state_root)?;
    Ok(state)
}
```

- [ ] **Step 2: Update `src/commands/mod.rs`**

```rust
pub mod clone;
pub mod create;
pub mod reconfigure;
pub mod restore;
pub mod snapshot;
pub mod snapshots;
```

- [ ] **Step 3: Wire CLI in `src/cli.rs`**

Add variant:

```rust
/// Clone a running instance as a NEW sibling via pg_basebackup.
Clone {
    #[arg(long)]
    source: String,
    #[arg(long = "as")]
    as_: String,
},
```

Match arm:

```rust
Some(Command::Clone { source, as_ }) => {
    let state = crate::commands::clone::run(crate::commands::clone::CloneArgs {
        source,
        as_name: as_,
        override_state_root: None,
    })
    .await?;
    let i = &state.instance;
    println!(
        "Clone ready:\n  postgresql://{}:***@127.0.0.1:{}/{}",
        i.app_user, i.host_port, i.db_name
    );
    Ok(())
}
```

- [ ] **Step 4: Build + verify help**

```bash
cargo build
cargo run -- clone --help
```

- [ ] **Step 5: Run tests**

```bash
cargo test
```

- [ ] **Step 6: Commit**

```bash
git add .
git commit -m "feat(commands): pgforge clone — pg_basebackup-driven instance copy"
```

---

## Task 9: End-to-end clone test (gated)

**Files:**
- Create: `tests/clone_e2e_test.rs`

- [ ] **Step 1: Implement `tests/clone_e2e_test.rs`**

```rust
//! E2E: create source, clone it, verify clone reachable. Gated by PGFORGE_E2E=1.

use pgforge::commands::clone::{CloneArgs, run_with_engine as clone_run};
use pgforge::commands::create::{CreateArgs, run_with_engine as create_run};
use pgforge::commands::reconfigure::{ReconfigureArgs, run_with_engine as reconfigure_run};
use pgforge::config::global::GlobalConfig;
use pgforge::docker::bollard_engine::BollardEngine;
use pgforge::domain::preset::Preset;
use pgforge::pgbackrest::conf::S3Settings;
use std::time::Duration;
use tempfile::TempDir;

fn fake_s3() -> S3Settings {
    S3Settings {
        bucket: std::env::var("PGFORGE_E2E_BUCKET").unwrap_or_else(|_| "pgforge-e2e".into()),
        region: std::env::var("PGFORGE_E2E_REGION").unwrap_or_else(|_| "eu-central-1".into()),
        endpoint: std::env::var("PGFORGE_E2E_ENDPOINT")
            .unwrap_or_else(|_| "s3.eu-central-1.amazonaws.com".into()),
        access_key: std::env::var("PGFORGE_E2E_S3_KEY").unwrap_or_else(|_| "AKIAFAKE".into()),
        secret_key: std::env::var("PGFORGE_E2E_S3_SECRET").unwrap_or_else(|_| "secret".into()),
    }
}

#[tokio::test]
async fn clone_running_instance_is_reachable() {
    if std::env::var("PGFORGE_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping: set PGFORGE_E2E=1 to run");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let state_root = tmp.path().to_path_buf();
    let docker = BollardEngine::connect().expect("docker engine reachable");
    let s3 = fake_s3();
    let global = GlobalConfig { s3: Some(s3.clone()), ..Default::default() };

    let suffix = uniq_suffix();
    let src_name = format!("pgforge_e2e_clonesrc_{suffix}");
    let clone_name = format!("pgforge_e2e_clonedst_{suffix}");

    let src_state = create_run(
        CreateArgs {
            name: src_name.clone(),
            preset: Preset::Tiny,
            pg_version: 18,
            app_user: "leads".into(),
            app_password: "pw".into(),
            pgbackrest_password: "rpw".into(),
            override_state_root: Some(state_root.clone()),
        },
        &docker,
        state_root.clone(),
        global.clone(),
        s3.clone(),
    )
    .await
    .expect("create source");
    poll_tcp_ready(src_state.instance.host_port, 30).await;

    // Source was just created with the new pg_hba (Task 4) — it already
    // allows host replication. No reconfigure needed in this test.

    let cloned = clone_run(
        CloneArgs {
            source: src_name.clone(),
            as_name: clone_name.clone(),
            override_state_root: Some(state_root.clone()),
        },
        &docker,
        state_root.clone(),
        global,
        s3,
        src_state,
    )
    .await;

    cleanup(&src_name);
    cleanup(&clone_name);

    let cloned = cloned.expect("clone should succeed");
    assert!(cloned.instance.host_port >= 5433);
    poll_tcp_ready(cloned.instance.host_port, 600).await;
}

fn uniq_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos().to_string()
}

fn cleanup(name: &str) {
    let _ = std::process::Command::new("docker")
        .args(["rm", "-f", &format!("pgforge_{name}")])
        .output();
    let _ = std::process::Command::new("docker")
        .args(["volume", "rm", "-f", &format!("pgforge_data_{name}")])
        .output();
}

async fn poll_tcp_ready(port: u16, seconds: u64) {
    use tokio::net::TcpStream;
    for _ in 0..seconds {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("port {port} did not accept TCP within {seconds}s");
}
```

(The `reconfigure_run` import is added in case the test wants to reconfigure mid-flight; it's not used in the happy-path test but is available for future expansion.)

- [ ] **Step 2: Build**

```bash
cargo test --no-run
```

- [ ] **Step 3: Run without PGFORGE_E2E**

```bash
cargo test
```

- [ ] **Step 4: Commit**

```bash
git add .
git commit -m "test: end-to-end clone via pg_basebackup (gated)"
```

---

## Task 10: README clone section

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Append "Cloning" section to `README.md`**

After the existing "Snapshots and restore" section, before "Architecture", insert:

```markdown
## Cloning

Make a working copy of a running instance for staging / migration testing.
Uses streaming replication (`pg_basebackup`) under the hood, not S3.

\`\`\`bash
pgforge clone --source billing --as billing-staging
# Clone ready:
#   postgresql://leads:***@127.0.0.1:5435/billing
\`\`\`

The clone is independent: own port, own volume, own state file, own backup
repo path. The source keeps running untouched.

If you have instances created before pgforge 0.3 (Plan 3) that need
`host replication` in their pg_hba.conf, run once per instance:

\`\`\`bash
pgforge reconfigure --name billing
\`\`\`

This regenerates pg_hba.conf and runs `pg_ctl reload` inside the
container — no restart needed.
```

(Replace the escaped backticks with real triple-backticks when writing the file.)

Also update the Status section to mark Plan 3 implemented:

```markdown
## Status

**Plan 1 (foundation + create) — implemented.**  
**Plan 2 (snapshot + restore PITR) — implemented.**  
**Plan 3 (clone via pg_basebackup) — implemented.**  
Upgrade and TUI come in Plans 4-5.
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: clone + reconfigure quickstart in README"
```

---

## Self-review checklist

- [x] **Spec coverage:** `pgforge clone` end-to-end (orchestration, entrypoint, pgpass, pg_hba allowing host replication). `pgforge reconfigure` for existing instances. Cleanup-on-failure helper wired into create + restore + clone. wait_for_pg_ready deduplicated.
- [x] **No placeholders:** Every step has inline code or exact commands. No "TBD".
- [x] **Type consistency:**
  - `CloneArgs { source, as_name, override_state_root }` defined Task 8, used Task 8/9.
  - `ReconfigureArgs { instance, override_state_root }` defined Task 5, used Task 5.
  - `DockerEngine` new methods (remove_container, remove_volume) declared Task 2, used in Task 3 cleanup.
  - `cleanup_partial(docker, container_name, volume_name)` defined Task 3, called Task 8.
  - `wait_for_pg_ready(docker, id, seconds)` extracted Task 1, called from create.rs / restore.rs / clone.rs.
- [x] **TDD where it matters:** pure functions (`generate_pgpass`, `generate_clone_entrypoint`, pg_hba new row) are TDD'd. Orchestration is verified by the gated E2E.

## Known follow-ups for Plan 4+

- The Plan 2 snapshot-list-of-Option ergonomic and the no-cron-in-container item remain. Plan 4 (upgrade) is a natural home for cron because upgrade also wants weekly fulls as a recovery hatch.
- The Plan 2 "snapshot label parse failure" fallback via `pgbackrest info` is still pending. Add when Plan 4 needs `pgbackrest info` for its own pre-upgrade audit.
- `pgforge reconfigure` only regenerates pg_hba currently. If Plan 4 changes other PG config (e.g. shared_buffers), extend reconfigure to also re-render postgresql.conf and SIGHUP (or restart) as appropriate.
- The clone entrypoint uses `--no-password` — if pg_basebackup ever needs a password prompt fallback for some weird CI scenario, switch to providing PGPASSWORD env directly instead of relying on .pgpass.
