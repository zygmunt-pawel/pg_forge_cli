# pgforge Plan 1: Foundation + `create` command

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a working `pgforge create --preset=<size> --version=<pg> --name=<n>` command that spins up a single hardened PostgreSQL container on the local Docker engine, registers it in pgforge state, and prints a connection string. This is the walking-skeleton end-to-end slice — every other operation (snapshot, restore, clone, upgrade, TUI) builds on top of these abstractions.

**Architecture:** Single-binary Rust CLI. Pure-functional core (preset tuning, config-file generation, port allocator) wrapped by an `async` shell that talks to the Docker Engine via `bollard`. State lives in two places on disk: global config (`~/.config/pgforge/config.toml`, S3 creds + defaults) and per-instance metadata (`~/.local/share/pgforge/instances/<name>/state.toml`). Each PG instance is a custom Docker image (`pgforge/postgres:<version>`) that bakes `pgbackrest` on top of the official `postgres:<version>` image, so `archive_command` and scheduled backups can run inside the same container. Configs (`postgresql.conf`, `pg_hba.conf`, `pgbackrest.conf`) are generated on every `create` from typed Rust values and bind-mounted into the container.

**Tech Stack:**
- Rust 2024 edition, single binary crate
- `clap` 4.x for CLI parsing
- `serde` + `toml` for config / state files
- `bollard` for Docker Engine API (async via `tokio`)
- `directories` for cross-platform XDG paths
- `anyhow` (app errors) + `thiserror` (library errors)
- `tracing` + `tracing-subscriber` for structured logging
- `tempfile` + `pretty_assertions` for tests

---

## Plan roadmap (this plan = #1 of 5)

1. **Foundation + create** — this plan. Walking skeleton. Output: working `pgforge create`.
2. Snapshot + Restore PITR — needs Plan 1 done.
3. Clone (`pg_basebackup`) — needs Plan 1 done.
4. Upgrade in place (`pg_upgrade`) — needs Plan 2 done (uses pre-upgrade snapshot).
5. TUI dashboard (`ratatui`) — needs all CRUD ops from Plans 1-4.

---

## File structure

After this plan completes, the repository looks like:

```
pg_forge_cli/
├── Cargo.toml                     # crate manifest, deps pinned
├── Cargo.lock
├── README.md                      # quickstart
├── .gitignore
├── docs/
│   └── plans/
│       └── 2026-05-11-foundation-and-create.md   # this file
├── src/
│   ├── main.rs                    # tokio entrypoint, tracing init, CLI dispatch
│   ├── lib.rs                     # public re-exports for integration tests
│   ├── error.rs                   # PgForgeError, Result alias
│   ├── cli.rs                     # clap Command + Subcommand definitions
│   ├── commands/
│   │   ├── mod.rs                 # pub use create::*;
│   │   └── create.rs              # `pgforge create` orchestration
│   ├── config/
│   │   ├── mod.rs
│   │   └── global.rs              # GlobalConfig load/save (~/.config/pgforge/config.toml)
│   ├── state/
│   │   ├── mod.rs
│   │   └── instance.rs            # InstanceState load/save per instance
│   ├── domain/
│   │   ├── mod.rs
│   │   ├── instance.rs            # Instance struct (name, version, port, preset, paths)
│   │   ├── preset.rs              # Preset enum + tuning logic
│   │   └── platform.rs            # Platform detection (macOS / Linux)
│   ├── ports.rs                   # next_free_port, with TcpListener probe
│   ├── postgres/
│   │   ├── mod.rs
│   │   ├── conf.rs                # generate_postgresql_conf(...)
│   │   └── hba.rs                 # generate_pg_hba(...)
│   ├── pgbackrest/
│   │   ├── mod.rs
│   │   └── conf.rs                # generate_pgbackrest_conf(...)
│   └── docker/
│       ├── mod.rs
│       ├── engine.rs              # DockerEngine trait
│       ├── bollard_engine.rs      # Bollard impl
│       └── image.rs               # build pgforge/postgres:<version> image (Dockerfile inline)
└── tests/
    ├── common/
    │   └── mod.rs                 # test helpers (tempdir fixture, etc.)
    ├── postgres_conf_test.rs
    ├── pg_hba_test.rs
    ├── pgbackrest_conf_test.rs
    ├── preset_test.rs
    ├── platform_test.rs
    ├── ports_test.rs
    ├── global_config_test.rs
    ├── instance_state_test.rs
    └── create_e2e_test.rs         # gated by PGFORGE_E2E=1, requires Docker
```

**Design rules followed:**
- One file = one responsibility. `conf.rs` only generates `postgresql.conf`, `hba.rs` only `pg_hba.conf`, etc.
- Pure functions stay pure (no IO). All IO is in `commands/`, `docker/`, `state/`, `config/`.
- Docker is behind a trait (`DockerEngine`) so unit tests can mock without daemon.
- Files that change together live together (e.g. `domain/preset.rs` + the conf generators that read it).

---

## Task 1: Cargo project initialization

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `src/lib.rs`
- Create: `.gitignore`
- Create: `README.md` (one-line stub, expanded in Task 16)

- [ ] **Step 1: Initialize the Cargo crate**

Run:
```bash
cd /Users/pawel/workspace/rust_packages/pg_forge_cli
cargo init --name pgforge --bin --edition 2024
```

Expected: creates `Cargo.toml`, `src/main.rs`, `.gitignore`. If `Cargo.toml` already exists, the command will refuse — that's fine, just open it and edit.

- [ ] **Step 2: Replace `Cargo.toml` with the full manifest**

Write the entire content below to `Cargo.toml`. Versions are minor-pinned to spring 2026 stable; if `cargo build` complains about a version, run `cargo search <crate>` and bump to the next minor.

```toml
[package]
name = "pgforge"
version = "0.1.0"
edition = "2024"
description = "RDS-Single-AZ-equivalent provisioner for hardened PostgreSQL on a single host"
license = "MIT OR Apache-2.0"
publish = false

[lib]
name = "pgforge"
path = "src/lib.rs"

[[bin]]
name = "pgforge"
path = "src/main.rs"

[dependencies]
anyhow = "1.0"
thiserror = "2.0"
clap = { version = "4.5", features = ["derive"] }
serde = { version = "1.0", features = ["derive"] }
toml = "0.8"
tokio = { version = "1.40", features = ["macros", "rt-multi-thread", "fs", "process", "net", "time"] }
bollard = "0.18"
directories = "5.0"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
futures-util = "0.3"
async-trait = "0.1"
indoc = "2"
regex = "1"

[dev-dependencies]
tempfile = "3"
pretty_assertions = "1"
```

- [ ] **Step 3: Replace `src/main.rs` with a minimal entrypoint**

Write to `src/main.rs`:

```rust
use anyhow::Result;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_target(false)
        .init();

    tracing::info!("pgforge starting");
    Ok(())
}
```

- [ ] **Step 4: Create `src/lib.rs`**

Write to `src/lib.rs`:

```rust
//! pgforge — hardened PostgreSQL provisioner for a single host.
//!
//! This crate exposes its modules publicly so integration tests in `tests/`
//! can reach into the same code paths the binary uses.
```

(Modules are added as later tasks introduce them; nothing else here yet.)

- [ ] **Step 5: Write `.gitignore`**

Write to `.gitignore`:

```gitignore
/target
Cargo.lock.bak
*.swp
.DS_Store
```

(Note: do NOT ignore `Cargo.lock` — this is a binary crate, the lockfile must be committed.)

- [ ] **Step 6: Write a one-line stub `README.md`**

Write to `README.md`:

```markdown
# pgforge

RDS-Single-AZ-equivalent provisioner for hardened PostgreSQL on a single host. WIP.
```

- [ ] **Step 7: Verify the project builds**

Run:
```bash
cargo build
```

Expected: builds clean. Warnings about unused crates are fine — most deps aren't referenced yet. If you see compile errors, the most likely culprit is a `bollard` major-version bump that changed the API; the only thing referenced so far is the dep itself in `Cargo.toml`, so if `cargo build` fails on `bollard`, drop to `bollard = "0.17"` and continue.

- [ ] **Step 8: Run the binary to verify tracing wiring**

Run:
```bash
cargo run
```

Expected: prints something like `INFO pgforge starting`.

- [ ] **Step 9: Commit**

```bash
git init   # if not already a git repo
git add .
git commit -m "feat: initialize pgforge cargo project with deps and tracing"
```

---

## Task 2: Error type

**Files:**
- Create: `src/error.rs`
- Modify: `src/lib.rs` (add `pub mod error;`)
- Modify: `src/main.rs` (use `pgforge::error::Result`)

- [ ] **Step 1: Create `src/error.rs`**

Write to `src/error.rs`:

```rust
use std::path::PathBuf;
use thiserror::Error;

pub type Result<T, E = PgForgeError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum PgForgeError {
    #[error("instance {0:?} already exists")]
    InstanceExists(String),

    #[error("instance {0:?} not found")]
    InstanceNotFound(String),

    #[error("invalid instance name {0:?}: must match [a-z][a-z0-9_-]{{0,62}}")]
    InvalidInstanceName(String),

    #[error("no free TCP port in range {start}..{end}")]
    NoFreePort { start: u16, end: u16 },

    #[error("config file at {path:?} is malformed: {source}")]
    ConfigMalformed { path: PathBuf, source: toml::de::Error },

    #[error("docker engine error: {0}")]
    Docker(String),

    #[error("io error at {path:?}: {source}")]
    Io { path: PathBuf, source: std::io::Error },

    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
}
```

- [ ] **Step 2: Wire it in `src/lib.rs`**

Replace `src/lib.rs` with:

```rust
//! pgforge — hardened PostgreSQL provisioner for a single host.

pub mod error;
```

- [ ] **Step 3: Use it from `src/main.rs`**

Replace `src/main.rs` with:

```rust
use pgforge::error::Result;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_target(false)
        .init();

    tracing::info!("pgforge starting");
    Ok(())
}
```

- [ ] **Step 4: Build to verify**

Run:
```bash
cargo build
```
Expected: clean build (warnings about unused `PgForgeError` variants are expected — they get used in later tasks).

- [ ] **Step 5: Commit**

```bash
git add .
git commit -m "feat: add PgForgeError and Result alias"
```

---

## Task 3: Platform detection (TDD)

**Why now:** `postgresql.conf` generation depends on the platform (`wal_sync_method` differs on macOS). Easiest pure function to drive out first.

**Files:**
- Create: `src/domain/mod.rs`
- Create: `src/domain/platform.rs`
- Create: `tests/platform_test.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/platform_test.rs`:

```rust
use pgforge::domain::platform::{Platform, current_platform};

#[test]
fn current_platform_matches_compile_target() {
    let p = current_platform();
    if cfg!(target_os = "macos") {
        assert_eq!(p, Platform::MacOs);
    } else if cfg!(target_os = "linux") {
        assert_eq!(p, Platform::Linux);
    } else {
        // unsupported targets fall back to Linux for now
        assert_eq!(p, Platform::Linux);
    }
}

#[test]
fn platform_short_name_is_stable() {
    assert_eq!(Platform::MacOs.short_name(), "macos");
    assert_eq!(Platform::Linux.short_name(), "linux");
}
```

- [ ] **Step 2: Run the test — expect it to fail**

Run:
```bash
cargo test --test platform_test
```
Expected: compile error — `pgforge::domain::platform` does not exist.

- [ ] **Step 3: Create `src/domain/mod.rs`**

Write to `src/domain/mod.rs`:

```rust
pub mod platform;
```

- [ ] **Step 4: Create `src/domain/platform.rs`**

Write to `src/domain/platform.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
}

impl Platform {
    pub fn short_name(self) -> &'static str {
        match self {
            Platform::MacOs => "macos",
            Platform::Linux => "linux",
        }
    }
}

pub fn current_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::MacOs
    } else {
        Platform::Linux
    }
}
```

- [ ] **Step 5: Wire `domain` into `src/lib.rs`**

Replace `src/lib.rs` with:

```rust
//! pgforge — hardened PostgreSQL provisioner for a single host.

pub mod domain;
pub mod error;
```

- [ ] **Step 6: Run the test — expect it to pass**

Run:
```bash
cargo test --test platform_test
```
Expected: 2 passed.

- [ ] **Step 7: Commit**

```bash
git add .
git commit -m "feat(domain): add Platform enum and current_platform()"
```

---

## Task 4: Preset enum + tuning logic (TDD)

**Files:**
- Create: `src/domain/preset.rs`
- Modify: `src/domain/mod.rs`
- Create: `tests/preset_test.rs`

**Background — what tuning is computed:** Every preset deterministically derives PostgreSQL memory params from a single RAM budget so they stay aligned with the Docker container memory limit. The rule of thumb (PG docs) is `shared_buffers ≈ 25% RAM`, `effective_cache_size ≈ 75% RAM`, `work_mem ≈ RAM / max_connections / 4`. We round to clean MB so generated configs stay readable.

| Preset  | RAM    | max_connections | shared_buffers | effective_cache_size | work_mem | max_wal_size |
|---------|--------|-----------------|----------------|----------------------|----------|--------------|
| Tiny    | 1 GB   | 50              | 256 MB         | 768 MB               | 5 MB     | 1 GB         |
| Small   | 2 GB   | 100             | 512 MB         | 1536 MB              | 5 MB     | 2 GB         |
| Medium  | 4 GB   | 200             | 1024 MB        | 3072 MB              | 5 MB     | 4 GB         |
| Large   | 8 GB   | 400             | 2048 MB        | 6144 MB              | 5 MB     | 8 GB         |

`work_mem = 5 MB` is conservative across the board; tune-per-query later. Docker memory limit = RAM column exactly.

- [ ] **Step 1: Write the failing test**

Create `tests/preset_test.rs`:

```rust
use pgforge::domain::preset::{Preset, Tuning};

#[test]
fn tiny_preset_tuning() {
    let t = Preset::Tiny.tuning();
    assert_eq!(t.ram_mb, 1024);
    assert_eq!(t.max_connections, 50);
    assert_eq!(t.shared_buffers_mb, 256);
    assert_eq!(t.effective_cache_size_mb, 768);
    assert_eq!(t.work_mem_mb, 5);
    assert_eq!(t.max_wal_size_mb, 1024);
}

#[test]
fn medium_preset_tuning() {
    let t = Preset::Medium.tuning();
    assert_eq!(t.ram_mb, 4096);
    assert_eq!(t.max_connections, 200);
    assert_eq!(t.shared_buffers_mb, 1024);
    assert_eq!(t.effective_cache_size_mb, 3072);
}

#[test]
fn preset_parses_from_lowercase_str() {
    use std::str::FromStr;
    assert_eq!(Preset::from_str("tiny").unwrap(), Preset::Tiny);
    assert_eq!(Preset::from_str("small").unwrap(), Preset::Small);
    assert_eq!(Preset::from_str("medium").unwrap(), Preset::Medium);
    assert_eq!(Preset::from_str("large").unwrap(), Preset::Large);
    assert!(Preset::from_str("huge").is_err());
}

#[test]
fn tuning_struct_is_serializable() {
    let t = Preset::Small.tuning();
    let s = toml::to_string(&t).unwrap();
    let parsed: Tuning = toml::from_str(&s).unwrap();
    assert_eq!(t, parsed);
}
```

- [ ] **Step 2: Run — expect failure**

Run:
```bash
cargo test --test preset_test
```
Expected: `pgforge::domain::preset` does not exist.

- [ ] **Step 3: Implement `src/domain/preset.rs`**

Write to `src/domain/preset.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    Tiny,
    Small,
    Medium,
    Large,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tuning {
    pub ram_mb: u32,
    pub max_connections: u32,
    pub shared_buffers_mb: u32,
    pub effective_cache_size_mb: u32,
    pub work_mem_mb: u32,
    pub max_wal_size_mb: u32,
}

impl Preset {
    pub fn tuning(self) -> Tuning {
        match self {
            Preset::Tiny => Tuning {
                ram_mb: 1024,
                max_connections: 50,
                shared_buffers_mb: 256,
                effective_cache_size_mb: 768,
                work_mem_mb: 5,
                max_wal_size_mb: 1024,
            },
            Preset::Small => Tuning {
                ram_mb: 2048,
                max_connections: 100,
                shared_buffers_mb: 512,
                effective_cache_size_mb: 1536,
                work_mem_mb: 5,
                max_wal_size_mb: 2048,
            },
            Preset::Medium => Tuning {
                ram_mb: 4096,
                max_connections: 200,
                shared_buffers_mb: 1024,
                effective_cache_size_mb: 3072,
                work_mem_mb: 5,
                max_wal_size_mb: 4096,
            },
            Preset::Large => Tuning {
                ram_mb: 8192,
                max_connections: 400,
                shared_buffers_mb: 2048,
                effective_cache_size_mb: 6144,
                work_mem_mb: 5,
                max_wal_size_mb: 8192,
            },
        }
    }
}

impl FromStr for Preset {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "tiny" => Ok(Preset::Tiny),
            "small" => Ok(Preset::Small),
            "medium" => Ok(Preset::Medium),
            "large" => Ok(Preset::Large),
            other => Err(format!("unknown preset: {other:?}")),
        }
    }
}
```

- [ ] **Step 4: Wire into `src/domain/mod.rs`**

Replace `src/domain/mod.rs` with:

```rust
pub mod platform;
pub mod preset;
```

- [ ] **Step 5: Run — expect pass**

Run:
```bash
cargo test --test preset_test
```
Expected: 4 passed.

- [ ] **Step 6: Commit**

```bash
git add .
git commit -m "feat(domain): add Preset enum with deterministic Tuning"
```

---

## Task 5: postgresql.conf generation (TDD)

**Files:**
- Create: `src/postgres/mod.rs`
- Create: `src/postgres/conf.rs`
- Create: `tests/postgres_conf_test.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/postgres_conf_test.rs`:

```rust
use pgforge::domain::platform::Platform;
use pgforge::domain::preset::Preset;
use pgforge::postgres::conf::generate_postgresql_conf;

#[test]
fn tiny_macos_conf_contains_full_fsync_writethrough() {
    let conf = generate_postgresql_conf(Preset::Tiny, Platform::MacOs);
    assert!(conf.contains("wal_sync_method = fsync_writethrough"),
            "expected fsync_writethrough on macOS, got:\n{conf}");
}

#[test]
fn linux_conf_uses_fdatasync() {
    let conf = generate_postgresql_conf(Preset::Tiny, Platform::Linux);
    assert!(conf.contains("wal_sync_method = fdatasync"));
}

#[test]
fn conf_always_contains_durability_settings() {
    for preset in [Preset::Tiny, Preset::Small, Preset::Medium, Preset::Large] {
        for plat in [Platform::MacOs, Platform::Linux] {
            let conf = generate_postgresql_conf(preset, plat);
            for must in [
                "fsync = on",
                "synchronous_commit = on",
                "full_page_writes = on",
                "wal_level = replica",
                "archive_mode = on",
                "archive_timeout = 60",
                "ssl = off",
                "password_encryption = scram-sha-256",
            ] {
                assert!(conf.contains(must), "preset={preset:?} plat={plat:?} missing {must:?}");
            }
        }
    }
}

#[test]
fn medium_conf_uses_medium_tuning() {
    let conf = generate_postgresql_conf(Preset::Medium, Platform::Linux);
    assert!(conf.contains("max_connections = 200"));
    assert!(conf.contains("shared_buffers = 1024MB"));
    assert!(conf.contains("effective_cache_size = 3072MB"));
    assert!(conf.contains("max_wal_size = 4096MB"));
}

#[test]
fn conf_uses_pgbackrest_archive_command() {
    let conf = generate_postgresql_conf(Preset::Tiny, Platform::Linux);
    assert!(conf.contains("archive_command = 'pgbackrest --stanza=main archive-push %p'"));
}
```

- [ ] **Step 2: Run — expect compile failure**

Run:
```bash
cargo test --test postgres_conf_test
```
Expected: `pgforge::postgres::conf` does not exist.

- [ ] **Step 3: Create `src/postgres/mod.rs`**

Write to `src/postgres/mod.rs`:

```rust
pub mod conf;
```

- [ ] **Step 4: Implement `src/postgres/conf.rs`**

Write to `src/postgres/conf.rs`:

```rust
use crate::domain::platform::Platform;
use crate::domain::preset::Preset;

/// Render a complete `postgresql.conf` for the given preset on the given host
/// platform. Pure function — no IO, deterministic output.
pub fn generate_postgresql_conf(preset: Preset, platform: Platform) -> String {
    let t = preset.tuning();
    let wal_sync_method = match platform {
        Platform::MacOs => "fsync_writethrough",
        Platform::Linux => "fdatasync",
    };

    format!(
        r#"# Generated by pgforge — do not edit by hand. Regenerate via `pgforge create` or `pgforge reconfigure`.
#
# Preset: {preset:?}
# Platform: {plat:?}
# RAM budget: {ram} MB
#

# ----- Connections ----------------------------------------------------------
listen_addresses = '*'
port = 5432
max_connections = {max_conn}
superuser_reserved_connections = 3

# ----- Memory ---------------------------------------------------------------
shared_buffers = {sb}MB
effective_cache_size = {ec}MB
work_mem = {wm}MB
maintenance_work_mem = 64MB

# ----- WAL / Durability -----------------------------------------------------
fsync = on
synchronous_commit = on
full_page_writes = on
wal_compression = on
wal_sync_method = {wsm}
wal_level = replica
max_wal_size = {mws}MB
min_wal_size = 256MB
checkpoint_timeout = 15min
checkpoint_completion_target = 0.9

# ----- Archiving (pgBackRest, async push to S3) -----------------------------
archive_mode = on
archive_command = 'pgbackrest --stanza=main archive-push %p'
archive_timeout = 60

# ----- Security -------------------------------------------------------------
ssl = off
password_encryption = scram-sha-256

# ----- Logging --------------------------------------------------------------
log_destination = 'stderr'
logging_collector = on
log_directory = '/var/log/postgresql'
log_filename = 'postgresql-%Y-%m-%d.log'
log_rotation_age = 1d
log_rotation_size = 100MB
log_min_duration_statement = 1000
log_checkpoints = on
log_connections = off
log_disconnections = off
log_lock_waits = on
log_temp_files = 0
log_line_prefix = '%t [%p]: db=%d,user=%u,app=%a,client=%h '

# ----- Autovacuum -----------------------------------------------------------
autovacuum = on
autovacuum_max_workers = 3
autovacuum_naptime = 30s
autovacuum_vacuum_scale_factor = 0.05
autovacuum_analyze_scale_factor = 0.02
"#,
        preset = preset,
        plat = platform,
        ram = t.ram_mb,
        max_conn = t.max_connections,
        sb = t.shared_buffers_mb,
        ec = t.effective_cache_size_mb,
        wm = t.work_mem_mb,
        wsm = wal_sync_method,
        mws = t.max_wal_size_mb,
    )
}
```

- [ ] **Step 5: Wire `postgres` into `src/lib.rs`**

Replace `src/lib.rs` with:

```rust
//! pgforge — hardened PostgreSQL provisioner for a single host.

pub mod domain;
pub mod error;
pub mod postgres;
```

- [ ] **Step 6: Run — expect pass**

Run:
```bash
cargo test --test postgres_conf_test
```
Expected: 5 passed.

- [ ] **Step 7: Commit**

```bash
git add .
git commit -m "feat(postgres): generate hardened postgresql.conf per preset and platform"
```

---

## Task 6: pg_hba.conf generation (TDD)

**Files:**
- Create: `src/postgres/hba.rs`
- Modify: `src/postgres/mod.rs`
- Create: `tests/pg_hba_test.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/pg_hba_test.rs`:

```rust
use pgforge::postgres::hba::generate_pg_hba;

#[test]
fn hba_allows_local_postgres_socket_trust() {
    let hba = generate_pg_hba("billing", "leads");
    assert!(hba.contains("local   all             postgres                                trust"));
}

#[test]
fn hba_uses_scram_for_app_user_over_network() {
    let hba = generate_pg_hba("billing", "leads");
    assert!(hba.contains("billing"), "should reference db name");
    assert!(hba.contains("leads"), "should reference app user");
    assert!(hba.contains("scram-sha-256"));
}

#[test]
fn hba_grants_local_replication_to_pgbackrest() {
    let hba = generate_pg_hba("billing", "leads");
    assert!(hba.contains("local   replication     pgbackrest                              scram-sha-256"));
}

#[test]
fn hba_rejects_default_anything_else() {
    let hba = generate_pg_hba("billing", "leads");
    assert!(hba.contains("host    all             all             all                     reject"),
            "must end with default-reject row");
}
```

- [ ] **Step 2: Run — expect failure**

Run:
```bash
cargo test --test pg_hba_test
```
Expected: module doesn't exist.

- [ ] **Step 3: Implement `src/postgres/hba.rs`**

Write to `src/postgres/hba.rs`:

```rust
/// Render `pg_hba.conf`. The instance has exactly one app database (`db_name`)
/// and one app user (`app_user`), accessible only from inside the docker network.
pub fn generate_pg_hba(db_name: &str, app_user: &str) -> String {
    format!(
        r#"# Generated by pgforge — do not edit by hand.
# TYPE  DATABASE        USER            ADDRESS                 METHOD

# Local unix socket — superuser only, no password (container-internal).
local   all             postgres                                trust

# pgBackRest runs inside the same container as PG, over the local socket.
local   replication     pgbackrest                              scram-sha-256
local   all             pgbackrest                              scram-sha-256

# App user — only inside the docker network, password required.
host    {db}             {user}            samenet                 scram-sha-256

# Default-deny everything else.
host    all             all             all                     reject
"#,
        db = db_name,
        user = app_user,
    )
}
```

- [ ] **Step 4: Update `src/postgres/mod.rs`**

Replace `src/postgres/mod.rs` with:

```rust
pub mod conf;
pub mod hba;
```

- [ ] **Step 5: Run — expect pass**

Run:
```bash
cargo test --test pg_hba_test
```
Expected: 4 passed.

- [ ] **Step 6: Commit**

```bash
git add .
git commit -m "feat(postgres): generate pg_hba.conf with default-deny and scram"
```

---

## Task 7: pgbackrest.conf generation (TDD)

**Files:**
- Create: `src/pgbackrest/mod.rs`
- Create: `src/pgbackrest/conf.rs`
- Create: `tests/pgbackrest_conf_test.rs`
- Modify: `src/lib.rs`

**Background — `S3Settings` model:** S3 creds live in the global config (Task 9) but the `pgbackrest.conf` generator must accept them as plain values to stay pure-functional. We define a small `S3Settings` struct here; later the global config layer reuses this exact type via `pub use`.

- [ ] **Step 1: Write the failing test**

Create `tests/pgbackrest_conf_test.rs`:

```rust
use pgforge::pgbackrest::conf::{S3Settings, generate_pgbackrest_conf};

fn s3_fixture() -> S3Settings {
    S3Settings {
        bucket: "pgforge-bk".into(),
        region: "eu-central-1".into(),
        endpoint: "s3.eu-central-1.amazonaws.com".into(),
        access_key: "AKIAFAKE".into(),
        secret_key: "secret".into(),
    }
}

#[test]
fn conf_includes_s3_repo_settings() {
    let conf = generate_pgbackrest_conf("billing", &s3_fixture());
    assert!(conf.contains("repo1-type=s3"));
    assert!(conf.contains("repo1-s3-bucket=pgforge-bk"));
    assert!(conf.contains("repo1-s3-region=eu-central-1"));
    assert!(conf.contains("repo1-s3-key=AKIAFAKE"));
    assert!(conf.contains("repo1-s3-key-secret=secret"));
}

#[test]
fn conf_namespaces_repo_path_per_instance() {
    let conf = generate_pgbackrest_conf("billing", &s3_fixture());
    assert!(conf.contains("repo1-path=/pgforge/billing"));

    let conf2 = generate_pgbackrest_conf("analytics", &s3_fixture());
    assert!(conf2.contains("repo1-path=/pgforge/analytics"));
}

#[test]
fn conf_has_main_stanza_with_local_socket() {
    let conf = generate_pgbackrest_conf("billing", &s3_fixture());
    assert!(conf.contains("[main]"));
    assert!(conf.contains("pg1-path=/var/lib/postgresql/data/pgdata"));
    assert!(conf.contains("pg1-user=pgbackrest"));
    assert!(conf.contains("pg1-socket-path=/var/run/postgresql"));
}

#[test]
fn conf_enables_async_archive_push_and_zstd() {
    let conf = generate_pgbackrest_conf("billing", &s3_fixture());
    assert!(conf.contains("archive-async=y"));
    assert!(conf.contains("compress-type=zst"));
    assert!(conf.contains("spool-path=/var/spool/pgbackrest"));
}
```

- [ ] **Step 2: Run — expect failure**

Run:
```bash
cargo test --test pgbackrest_conf_test
```
Expected: module not found.

- [ ] **Step 3: Implement `src/pgbackrest/mod.rs`**

Write to `src/pgbackrest/mod.rs`:

```rust
pub mod conf;
```

- [ ] **Step 4: Implement `src/pgbackrest/conf.rs`**

Write to `src/pgbackrest/conf.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct S3Settings {
    pub bucket: String,
    pub region: String,
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: String,
}

/// Render `pgbackrest.conf` for one instance. Each instance gets its own
/// `repo1-path` (`/pgforge/<instance>`) so a single S3 bucket can host many.
pub fn generate_pgbackrest_conf(instance_name: &str, s3: &S3Settings) -> String {
    format!(
        r#"# Generated by pgforge — do not edit by hand.

[global]
# --- Repository: S3 ---------------------------------------------------------
repo1-type=s3
repo1-s3-bucket={bucket}
repo1-s3-region={region}
repo1-s3-endpoint={endpoint}
repo1-s3-key={akey}
repo1-s3-key-secret={skey}
repo1-s3-uri-style=host
repo1-path=/pgforge/{instance}
repo1-cipher-type=none
repo1-storage-verify-tls=y

# --- Retention --------------------------------------------------------------
repo1-retention-full=2
repo1-retention-full-type=count
repo1-retention-diff=7

# --- Performance / archiving ------------------------------------------------
compress-type=zst
compress-level=3
process-max=4
archive-async=y
spool-path=/var/spool/pgbackrest
start-fast=y
stop-auto=y

# --- Logging ----------------------------------------------------------------
log-path=/var/log/pgbackrest
log-level-console=info
log-level-file=detail

[global:archive-push]
process-max=2

[main]
pg1-path=/var/lib/postgresql/data/pgdata
pg1-port=5432
pg1-user=pgbackrest
pg1-database=postgres
pg1-socket-path=/var/run/postgresql
"#,
        bucket = s3.bucket,
        region = s3.region,
        endpoint = s3.endpoint,
        akey = s3.access_key,
        skey = s3.secret_key,
        instance = instance_name,
    )
}
```

- [ ] **Step 5: Wire `pgbackrest` into `src/lib.rs`**

Replace `src/lib.rs` with:

```rust
//! pgforge — hardened PostgreSQL provisioner for a single host.

pub mod domain;
pub mod error;
pub mod pgbackrest;
pub mod postgres;
```

- [ ] **Step 6: Run — expect pass**

Run:
```bash
cargo test --test pgbackrest_conf_test
```
Expected: 4 passed.

- [ ] **Step 7: Commit**

```bash
git add .
git commit -m "feat(pgbackrest): generate per-instance config with S3 namespacing"
```

---

## Task 8: Port allocator (TDD)

**Files:**
- Create: `src/ports.rs`
- Modify: `src/lib.rs`
- Create: `tests/ports_test.rs`

**Design note:** Allocator is a pure function over `taken: &HashSet<u16>` plus a probe trait `IsBindable`. Real implementation probes via `TcpListener::bind`, tests inject a mock probe. This keeps the algorithm pure-testable.

- [ ] **Step 1: Write the failing test**

Create `tests/ports_test.rs`:

```rust
use pgforge::error::PgForgeError;
use pgforge::ports::{IsBindable, allocate_port};
use std::collections::HashSet;

struct AllFree;
impl IsBindable for AllFree {
    fn is_bindable(&self, _port: u16) -> bool { true }
}

struct NoneFree;
impl IsBindable for NoneFree {
    fn is_bindable(&self, _port: u16) -> bool { false }
}

struct OnlyOddFree;
impl IsBindable for OnlyOddFree {
    fn is_bindable(&self, port: u16) -> bool { port % 2 == 1 }
}

#[test]
fn allocates_first_port_in_range_when_all_free() {
    let p = allocate_port(5433, 5500, &HashSet::new(), &AllFree).unwrap();
    assert_eq!(p, 5433);
}

#[test]
fn skips_taken_ports() {
    let taken: HashSet<u16> = [5433, 5434].iter().copied().collect();
    let p = allocate_port(5433, 5500, &taken, &AllFree).unwrap();
    assert_eq!(p, 5435);
}

#[test]
fn skips_unbindable_ports() {
    // Even ports unbindable → first odd >= 5433 is 5433
    let p = allocate_port(5433, 5500, &HashSet::new(), &OnlyOddFree).unwrap();
    assert_eq!(p, 5433);
    // Forcing start=5434 → next odd is 5435
    let p = allocate_port(5434, 5500, &HashSet::new(), &OnlyOddFree).unwrap();
    assert_eq!(p, 5435);
}

#[test]
fn errors_when_no_port_available() {
    let err = allocate_port(5433, 5435, &HashSet::new(), &NoneFree).unwrap_err();
    matches!(err, PgForgeError::NoFreePort { start: 5433, end: 5435 });
}
```

- [ ] **Step 2: Run — expect failure**

Run:
```bash
cargo test --test ports_test
```
Expected: module not found.

- [ ] **Step 3: Implement `src/ports.rs`**

Write to `src/ports.rs`:

```rust
use crate::error::{PgForgeError, Result};
use std::collections::HashSet;
use std::net::TcpListener;

/// Abstraction over "can I bind to this port right now?" so the allocator
/// algorithm stays unit-testable without touching the OS.
pub trait IsBindable {
    fn is_bindable(&self, port: u16) -> bool;
}

/// Real probe: tries to bind to 127.0.0.1:port and immediately drops the
/// listener. If the bind succeeds, the port is free at this instant.
pub struct TcpProbe;

impl IsBindable for TcpProbe {
    fn is_bindable(&self, port: u16) -> bool {
        TcpListener::bind(("127.0.0.1", port)).is_ok()
    }
}

/// Find the first port in `[start, end)` that is neither in `taken` nor
/// currently in use according to `probe`.
pub fn allocate_port<P: IsBindable>(
    start: u16,
    end: u16,
    taken: &HashSet<u16>,
    probe: &P,
) -> Result<u16> {
    for p in start..end {
        if taken.contains(&p) {
            continue;
        }
        if probe.is_bindable(p) {
            return Ok(p);
        }
    }
    Err(PgForgeError::NoFreePort { start, end })
}
```

- [ ] **Step 4: Wire `ports` into `src/lib.rs`**

Replace `src/lib.rs` with:

```rust
//! pgforge — hardened PostgreSQL provisioner for a single host.

pub mod domain;
pub mod error;
pub mod pgbackrest;
pub mod ports;
pub mod postgres;
```

- [ ] **Step 5: Run — expect pass**

Run:
```bash
cargo test --test ports_test
```
Expected: 4 passed.

- [ ] **Step 6: Commit**

```bash
git add .
git commit -m "feat(ports): port allocator with bindable probe"
```

---

## Task 9: Global config load/save (TDD)

**Files:**
- Create: `src/config/mod.rs`
- Create: `src/config/global.rs`
- Modify: `src/lib.rs`
- Create: `tests/global_config_test.rs`

**Design:** `GlobalConfig` lives at `~/.config/pgforge/config.toml` on Linux and `~/Library/Application Support/pgforge/config.toml` on macOS — both resolved via the `directories` crate. Loading is *forgiving*: if the file doesn't exist, return `GlobalConfig::default()`. Saving creates parent dirs.

- [ ] **Step 1: Write the failing test**

Create `tests/global_config_test.rs`:

```rust
use pgforge::config::global::GlobalConfig;
use pgforge::pgbackrest::conf::S3Settings;
use tempfile::TempDir;

fn s3() -> S3Settings {
    S3Settings {
        bucket: "b".into(),
        region: "eu-central-1".into(),
        endpoint: "s3.eu-central-1.amazonaws.com".into(),
        access_key: "a".into(),
        secret_key: "s".into(),
    }
}

#[test]
fn load_returns_default_when_file_missing() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    let cfg = GlobalConfig::load_from(&path).unwrap();
    assert!(cfg.s3.is_none());
    assert_eq!(cfg.port_range_start, 5433);
    assert_eq!(cfg.port_range_end, 5500);
}

#[test]
fn save_then_load_round_trips() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("nested").join("config.toml");
    let cfg = GlobalConfig {
        s3: Some(s3()),
        port_range_start: 6000,
        port_range_end: 6100,
    };
    cfg.save_to(&path).unwrap();
    let loaded = GlobalConfig::load_from(&path).unwrap();
    assert_eq!(loaded, cfg);
}

#[test]
fn malformed_config_returns_typed_error() {
    use pgforge::error::PgForgeError;
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "this is not = valid toml [[[").unwrap();
    let err = GlobalConfig::load_from(&path).unwrap_err();
    assert!(matches!(err, PgForgeError::ConfigMalformed { .. }), "got {err:?}");
}
```

- [ ] **Step 2: Run — expect failure**

Run:
```bash
cargo test --test global_config_test
```
Expected: module not found.

- [ ] **Step 3: Create `src/config/mod.rs`**

Write to `src/config/mod.rs`:

```rust
pub mod global;
```

- [ ] **Step 4: Implement `src/config/global.rs`**

Write to `src/config/global.rs`:

```rust
use crate::error::{PgForgeError, Result};
use crate::pgbackrest::conf::S3Settings;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GlobalConfig {
    pub s3: Option<S3Settings>,
    pub port_range_start: u16,
    pub port_range_end: u16,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            s3: None,
            port_range_start: 5433,
            port_range_end: 5500,
        }
    }
}

impl GlobalConfig {
    /// Resolve the canonical platform-specific path for the global config.
    pub fn default_path() -> PathBuf {
        ProjectDirs::from("dev", "pgforge", "pgforge")
            .map(|p| p.config_dir().join("config.toml"))
            .unwrap_or_else(|| PathBuf::from("config.toml"))
    }

    pub fn load() -> Result<Self> {
        Self::load_from(&Self::default_path())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path).map_err(|e| PgForgeError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        toml::from_str(&raw).map_err(|source| PgForgeError::ConfigMalformed {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::default_path())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| PgForgeError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let raw = toml::to_string_pretty(self).map_err(|e| {
            PgForgeError::Anyhow(anyhow::anyhow!("serialize global config: {e}"))
        })?;
        std::fs::write(path, raw).map_err(|e| PgForgeError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        Ok(())
    }
}
```

- [ ] **Step 5: Wire `config` into `src/lib.rs`**

Replace `src/lib.rs` with:

```rust
//! pgforge — hardened PostgreSQL provisioner for a single host.

pub mod config;
pub mod domain;
pub mod error;
pub mod pgbackrest;
pub mod ports;
pub mod postgres;
```

- [ ] **Step 6: Run — expect pass**

Run:
```bash
cargo test --test global_config_test
```
Expected: 3 passed.

- [ ] **Step 7: Commit**

```bash
git add .
git commit -m "feat(config): GlobalConfig with default-on-missing loading"
```

---

## Task 10: Instance domain + state load/save (TDD)

**Files:**
- Create: `src/domain/instance.rs`
- Modify: `src/domain/mod.rs`
- Create: `src/state/mod.rs`
- Create: `src/state/instance.rs`
- Modify: `src/lib.rs`
- Create: `tests/instance_state_test.rs`

**Design:** `Instance` is the in-memory representation (immutable after creation). `InstanceState` wraps an `Instance` plus mutable bits (currently just `created_at`). Serialized state lives at `<state_root>/instances/<name>/state.toml`. `state_root` defaults to `~/.local/share/pgforge` on Linux and `~/Library/Application Support/pgforge` on macOS, but is overridable for tests.

- [ ] **Step 1: Write the failing test**

Create `tests/instance_state_test.rs`:

```rust
use pgforge::domain::instance::Instance;
use pgforge::domain::preset::Preset;
use pgforge::state::instance::InstanceState;
use tempfile::TempDir;

fn fixture(name: &str) -> InstanceState {
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
        },
        created_at: "2026-05-11T08:00:00Z".into(),
    }
}

#[test]
fn instance_name_validation_rejects_uppercase() {
    let err = Instance::validate_name("Billing").unwrap_err();
    assert!(matches!(err, pgforge::error::PgForgeError::InvalidInstanceName(_)));
}

#[test]
fn instance_name_validation_accepts_alpha_numeric_underscore_dash() {
    Instance::validate_name("billing").unwrap();
    Instance::validate_name("billing-staging").unwrap();
    Instance::validate_name("billing_2").unwrap();
}

#[test]
fn instance_name_validation_rejects_starting_digit() {
    assert!(Instance::validate_name("2billing").is_err());
}

#[test]
fn save_then_load_round_trips() {
    let dir = TempDir::new().unwrap();
    let state_root = dir.path();
    let s = fixture("billing");
    s.save_under(state_root).unwrap();
    let loaded = InstanceState::load_under(state_root, "billing").unwrap();
    assert_eq!(s, loaded);
}

#[test]
fn list_returns_all_instances() {
    let dir = TempDir::new().unwrap();
    let state_root = dir.path();
    fixture("billing").save_under(state_root).unwrap();
    fixture("analytics").save_under(state_root).unwrap();
    let mut names = InstanceState::list_under(state_root).unwrap();
    names.sort();
    assert_eq!(names, vec!["analytics".to_string(), "billing".to_string()]);
}

#[test]
fn list_returns_empty_when_state_root_missing() {
    let dir = TempDir::new().unwrap();
    let state_root = dir.path().join("does-not-exist");
    let names = InstanceState::list_under(&state_root).unwrap();
    assert!(names.is_empty());
}
```

- [ ] **Step 2: Run — expect failure**

Run:
```bash
cargo test --test instance_state_test
```
Expected: modules don't exist.

- [ ] **Step 3: Implement `src/domain/instance.rs`**

Write to `src/domain/instance.rs`:

```rust
use crate::domain::preset::Preset;
use crate::error::{PgForgeError, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// Immutable description of one PG instance managed by pgforge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    pub name: String,
    pub db_name: String,
    pub app_user: String,
    pub app_password: String,
    pub pgbackrest_password: String,
    pub preset: Preset,
    pub pg_version: u8,
    pub host_port: u16,
}

impl Instance {
    /// Names must be filesystem-safe, DNS-safe, and short enough to fit a
    /// container name. Conservative regex: lowercase start, then
    /// alphanumeric / `_` / `-`, total length 1..=63.
    pub fn validate_name(name: &str) -> Result<()> {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r"^[a-z][a-z0-9_-]{0,62}$").unwrap());
        if re.is_match(name) {
            Ok(())
        } else {
            Err(PgForgeError::InvalidInstanceName(name.to_string()))
        }
    }
}
```

- [ ] **Step 4: Update `src/domain/mod.rs`**

Replace `src/domain/mod.rs` with:

```rust
pub mod instance;
pub mod platform;
pub mod preset;
```

- [ ] **Step 5: Implement `src/state/mod.rs`**

Write to `src/state/mod.rs`:

```rust
pub mod instance;
```

- [ ] **Step 6: Implement `src/state/instance.rs`**

Write to `src/state/instance.rs`:

```rust
use crate::domain::instance::Instance;
use crate::error::{PgForgeError, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceState {
    pub instance: Instance,
    pub created_at: String, // ISO-8601, kept as String to avoid pulling chrono yet
}

impl InstanceState {
    pub fn default_state_root() -> PathBuf {
        ProjectDirs::from("dev", "pgforge", "pgforge")
            .map(|p| p.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn dir_under(state_root: &Path, name: &str) -> PathBuf {
        state_root.join("instances").join(name)
    }

    fn file_under(state_root: &Path, name: &str) -> PathBuf {
        Self::dir_under(state_root, name).join("state.toml")
    }

    pub fn save_under(&self, state_root: &Path) -> Result<()> {
        Instance::validate_name(&self.instance.name)?;
        let dir = Self::dir_under(state_root, &self.instance.name);
        std::fs::create_dir_all(&dir).map_err(|e| PgForgeError::Io {
            path: dir.clone(),
            source: e,
        })?;
        let file = Self::file_under(state_root, &self.instance.name);
        let raw = toml::to_string_pretty(self).map_err(|e| {
            PgForgeError::Anyhow(anyhow::anyhow!("serialize instance state: {e}"))
        })?;
        std::fs::write(&file, raw).map_err(|e| PgForgeError::Io {
            path: file,
            source: e,
        })
    }

    pub fn load_under(state_root: &Path, name: &str) -> Result<Self> {
        Instance::validate_name(name)?;
        let file = Self::file_under(state_root, name);
        if !file.exists() {
            return Err(PgForgeError::InstanceNotFound(name.to_string()));
        }
        let raw = std::fs::read_to_string(&file).map_err(|e| PgForgeError::Io {
            path: file.clone(),
            source: e,
        })?;
        toml::from_str(&raw).map_err(|source| PgForgeError::ConfigMalformed {
            path: file,
            source,
        })
    }

    pub fn list_under(state_root: &Path) -> Result<Vec<String>> {
        let dir = state_root.join("instances");
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let entries = std::fs::read_dir(&dir).map_err(|e| PgForgeError::Io {
            path: dir.clone(),
            source: e,
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| PgForgeError::Io {
                path: dir.clone(),
                source: e,
            })?;
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if let Some(name) = entry.file_name().to_str() {
                    out.push(name.to_string());
                }
            }
        }
        Ok(out)
    }

    pub fn exists_under(state_root: &Path, name: &str) -> bool {
        Self::file_under(state_root, name).exists()
    }
}
```

- [ ] **Step 7: Wire `state` into `src/lib.rs`**

Replace `src/lib.rs` with:

```rust
//! pgforge — hardened PostgreSQL provisioner for a single host.

pub mod config;
pub mod domain;
pub mod error;
pub mod pgbackrest;
pub mod ports;
pub mod postgres;
pub mod state;
```

- [ ] **Step 8: Run — expect pass**

Run:
```bash
cargo test --test instance_state_test
```
Expected: 6 passed.

- [ ] **Step 9: Commit**

```bash
git add .
git commit -m "feat(state): Instance + InstanceState with on-disk round-trip"
```

---

## Task 11: Docker engine trait (no Docker required to compile)

**Files:**
- Create: `src/docker/mod.rs`
- Create: `src/docker/engine.rs`
- Modify: `src/lib.rs`

**Design:** A small async trait so unit tests of `commands::create` (Task 14) don't need Docker. Only the methods the create flow actually calls — keep the surface tight, expand in later plans (Plan 2 will add `exec`, Plan 4 will add `restart` etc.).

- [ ] **Step 1: Create `src/docker/mod.rs`**

Write to `src/docker/mod.rs`:

```rust
pub mod bollard_engine;
pub mod engine;
pub mod image;
```

- [ ] **Step 2: Create `src/docker/engine.rs`**

Write to `src/docker/engine.rs`:

```rust
use crate::error::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct BindMount {
    pub host_path: PathBuf,
    pub container_path: String,
    pub read_only: bool,
}

#[derive(Debug, Clone)]
pub struct CreateContainerSpec {
    pub container_name: String,
    pub image: String,
    pub env: HashMap<String, String>,
    pub binds: Vec<BindMount>,
    pub volumes: Vec<NamedVolume>,
    pub host_port: u16,
    pub container_port: u16,
    pub memory_mb: u32,
    pub network: String,
    pub shm_size_mb: u32,
}

#[derive(Debug, Clone)]
pub struct NamedVolume {
    pub volume_name: String,
    pub container_path: String,
}

#[derive(Debug, Clone)]
pub struct BuildImageSpec {
    /// Tag the resulting image will be saved under, e.g. "pgforge/postgres:18".
    pub tag: String,
    /// Dockerfile contents.
    pub dockerfile: String,
}

#[async_trait]
pub trait DockerEngine: Send + Sync {
    async fn build_image(&self, spec: &BuildImageSpec) -> Result<()>;
    async fn ensure_network(&self, name: &str) -> Result<()>;
    async fn create_container(&self, spec: &CreateContainerSpec) -> Result<String>;
    async fn start_container(&self, id: &str) -> Result<()>;
    async fn container_exists(&self, name: &str) -> Result<bool>;
}
```

- [ ] **Step 3: Stub `src/docker/bollard_engine.rs`**

Write to `src/docker/bollard_engine.rs` (we fully fill it in Task 12):

```rust
use crate::docker::engine::{BuildImageSpec, CreateContainerSpec, DockerEngine};
use crate::error::Result;
use async_trait::async_trait;

pub struct BollardEngine;

impl BollardEngine {
    pub fn connect() -> Result<Self> {
        Ok(Self)
    }
}

#[async_trait]
impl DockerEngine for BollardEngine {
    async fn build_image(&self, _spec: &BuildImageSpec) -> Result<()> {
        unimplemented!("filled in Task 12")
    }
    async fn ensure_network(&self, _name: &str) -> Result<()> {
        unimplemented!("filled in Task 13")
    }
    async fn create_container(&self, _spec: &CreateContainerSpec) -> Result<String> {
        unimplemented!("filled in Task 13")
    }
    async fn start_container(&self, _id: &str) -> Result<()> {
        unimplemented!("filled in Task 13")
    }
    async fn container_exists(&self, _name: &str) -> Result<bool> {
        unimplemented!("filled in Task 13")
    }
}
```

- [ ] **Step 4: Stub `src/docker/image.rs`**

Write to `src/docker/image.rs` (filled in Task 12):

```rust
/// Render the Dockerfile that bakes pgbackrest + cron on top of the official
/// postgres image. Pure function — output is deterministic per pg_version.
pub fn dockerfile(pg_version: u8) -> String {
    format!(
        r#"FROM postgres:{ver}-bookworm

ENV DEBIAN_FRONTEND=noninteractive

RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        pgbackrest cron tini ca-certificates tzdata; \
    rm -rf /var/lib/apt/lists/*; \
    mkdir -p /var/spool/pgbackrest /var/log/pgbackrest /etc/pgbackrest; \
    chown -R postgres:postgres /var/spool/pgbackrest /var/log/pgbackrest /etc/pgbackrest

# Tag of the image is set at build time by pgforge: pgforge/postgres:{ver}
"#,
        ver = pg_version
    )
}
```

- [ ] **Step 5: Wire `docker` into `src/lib.rs`**

Replace `src/lib.rs` with:

```rust
//! pgforge — hardened PostgreSQL provisioner for a single host.

pub mod config;
pub mod docker;
pub mod domain;
pub mod error;
pub mod pgbackrest;
pub mod ports;
pub mod postgres;
pub mod state;
```

- [ ] **Step 6: Build to verify compile**

Run:
```bash
cargo build
```
Expected: clean build. The `unimplemented!()` panics never trigger because nothing calls them yet.

- [ ] **Step 7: Commit**

```bash
git add .
git commit -m "feat(docker): add DockerEngine trait + BollardEngine stub + Dockerfile renderer"
```

---

## Task 12: Implement `BollardEngine::build_image`

**Files:**
- Modify: `src/docker/bollard_engine.rs`

**Background — bollard build_image:** `bollard::Docker::build_image` takes `BuildImageOptions`, an optional registry creds map, and a `Body` of TAR bytes containing the build context. Minimum viable build context is a TAR with one file: `Dockerfile`. We construct the TAR in-memory using `tar` crate — but to keep deps small, we hand-roll a minimal TAR (USTAR with one regular file) since the format is simple.

Actually simpler: add the `tar` dep. It's tiny.

- [ ] **Step 1: Add `tar` to `Cargo.toml`**

Add to `[dependencies]` in `Cargo.toml`:

```toml
tar = "0.4"
```

Run:
```bash
cargo build
```
Expected: pulls in `tar`, builds clean.

- [ ] **Step 2: Replace `src/docker/bollard_engine.rs` with the bollard-backed implementation**

Write to `src/docker/bollard_engine.rs`:

```rust
use crate::docker::engine::{BuildImageSpec, CreateContainerSpec, DockerEngine};
use crate::error::{PgForgeError, Result};
use async_trait::async_trait;
use bollard::Docker;
use bollard::image::BuildImageOptions;
use futures_util::StreamExt;
use std::io::Cursor;

pub struct BollardEngine {
    docker: Docker,
}

impl BollardEngine {
    pub fn connect() -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| PgForgeError::Docker(format!("connect: {e}")))?;
        Ok(Self { docker })
    }

    /// Wrap a single Dockerfile into a TAR build context as bollard expects.
    fn make_tar_context(dockerfile: &str) -> Result<Vec<u8>> {
        let buf = Cursor::new(Vec::new());
        let mut builder = tar::Builder::new(buf);
        let bytes = dockerfile.as_bytes();
        let mut header = tar::Header::new_gnu();
        header.set_path("Dockerfile").map_err(|e| {
            PgForgeError::Docker(format!("tar set_path: {e}"))
        })?;
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, bytes).map_err(|e| {
            PgForgeError::Docker(format!("tar append: {e}"))
        })?;
        let cursor = builder.into_inner().map_err(|e| {
            PgForgeError::Docker(format!("tar finish: {e}"))
        })?;
        Ok(cursor.into_inner())
    }
}

#[async_trait]
impl DockerEngine for BollardEngine {
    async fn build_image(&self, spec: &BuildImageSpec) -> Result<()> {
        let tar_bytes = Self::make_tar_context(&spec.dockerfile)?;
        let opts = BuildImageOptions {
            t: spec.tag.clone(),
            rm: true,
            forcerm: true,
            ..Default::default()
        };
        let mut stream = self.docker.build_image(opts, None, Some(tar_bytes.into()));
        while let Some(item) = stream.next().await {
            match item {
                Ok(info) => {
                    if let Some(stream) = info.stream {
                        let trimmed = stream.trim();
                        if !trimmed.is_empty() {
                            tracing::debug!(target: "pgforge::docker::build", "{trimmed}");
                        }
                    }
                    if let Some(err) = info.error {
                        return Err(PgForgeError::Docker(format!("build_image: {err}")));
                    }
                }
                Err(e) => return Err(PgForgeError::Docker(format!("build_image stream: {e}"))),
            }
        }
        Ok(())
    }

    async fn ensure_network(&self, _name: &str) -> Result<()> {
        unimplemented!("filled in Task 13")
    }
    async fn create_container(&self, _spec: &CreateContainerSpec) -> Result<String> {
        unimplemented!("filled in Task 13")
    }
    async fn start_container(&self, _id: &str) -> Result<()> {
        unimplemented!("filled in Task 13")
    }
    async fn container_exists(&self, _name: &str) -> Result<bool> {
        unimplemented!("filled in Task 13")
    }
}
```

- [ ] **Step 3: Build to verify**

Run:
```bash
cargo build
```
Expected: clean build. (Field/variant names from bollard 0.18 — if the API has shifted, the compiler will tell you exactly what changed.)

- [ ] **Step 4: Commit**

```bash
git add .
git commit -m "feat(docker): implement BollardEngine::build_image with in-memory TAR context"
```

---

## Task 13: Implement remaining `BollardEngine` methods

**Files:**
- Modify: `src/docker/bollard_engine.rs`

- [ ] **Step 1: Replace the four `unimplemented!()` methods with real impls**

Edit `src/docker/bollard_engine.rs` — replace the `ensure_network`, `create_container`, `start_container`, and `container_exists` methods with:

```rust
async fn ensure_network(&self, name: &str) -> Result<()> {
    use bollard::network::{CreateNetworkOptions, ListNetworksOptions};
    let mut filters = std::collections::HashMap::new();
    filters.insert("name".to_string(), vec![name.to_string()]);
    let nets = self
        .docker
        .list_networks(Some(ListNetworksOptions { filters }))
        .await
        .map_err(|e| PgForgeError::Docker(format!("list_networks: {e}")))?;
    if nets.iter().any(|n| n.name.as_deref() == Some(name)) {
        return Ok(());
    }
    let opts = CreateNetworkOptions {
        name: name.to_string(),
        driver: "bridge".to_string(),
        ..Default::default()
    };
    self.docker
        .create_network(opts)
        .await
        .map_err(|e| PgForgeError::Docker(format!("create_network({name}): {e}")))?;
    Ok(())
}

async fn create_container(&self, spec: &CreateContainerSpec) -> Result<String> {
    use bollard::container::{Config, CreateContainerOptions};
    use bollard::secret::{HostConfig, Mount, MountTypeEnum, PortBinding};
    use std::collections::HashMap;

    let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
    port_bindings.insert(
        format!("{}/tcp", spec.container_port),
        Some(vec![PortBinding {
            host_ip: Some("127.0.0.1".to_string()),
            host_port: Some(spec.host_port.to_string()),
        }]),
    );

    let mut exposed_ports = HashMap::new();
    exposed_ports.insert(format!("{}/tcp", spec.container_port), HashMap::new());

    let mut mounts = Vec::new();
    for b in &spec.binds {
        mounts.push(Mount {
            target: Some(b.container_path.clone()),
            source: Some(b.host_path.to_string_lossy().to_string()),
            typ: Some(MountTypeEnum::BIND),
            read_only: Some(b.read_only),
            ..Default::default()
        });
    }
    for v in &spec.volumes {
        mounts.push(Mount {
            target: Some(v.container_path.clone()),
            source: Some(v.volume_name.clone()),
            typ: Some(MountTypeEnum::VOLUME),
            ..Default::default()
        });
    }

    let env: Vec<String> = spec
        .env
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();

    let host_config = HostConfig {
        port_bindings: Some(port_bindings),
        mounts: Some(mounts),
        memory: Some((spec.memory_mb as i64) * 1024 * 1024),
        memory_swap: Some((spec.memory_mb as i64) * 1024 * 1024),
        shm_size: Some((spec.shm_size_mb as i64) * 1024 * 1024),
        network_mode: Some(spec.network.clone()),
        restart_policy: Some(bollard::secret::RestartPolicy {
            name: Some(bollard::secret::RestartPolicyNameEnum::UNLESS_STOPPED),
            ..Default::default()
        }),
        ..Default::default()
    };

    let cfg = Config {
        image: Some(spec.image.clone()),
        env: Some(env),
        exposed_ports: Some(exposed_ports),
        host_config: Some(host_config),
        ..Default::default()
    };

    let opts = CreateContainerOptions {
        name: spec.container_name.clone(),
        platform: None,
    };

    let res = self
        .docker
        .create_container(Some(opts), cfg)
        .await
        .map_err(|e| PgForgeError::Docker(format!("create_container: {e}")))?;
    Ok(res.id)
}

async fn start_container(&self, id: &str) -> Result<()> {
    self.docker
        .start_container::<String>(id, None)
        .await
        .map_err(|e| PgForgeError::Docker(format!("start_container({id}): {e}")))
}

async fn container_exists(&self, name: &str) -> Result<bool> {
    use bollard::container::ListContainersOptions;
    let mut filters = std::collections::HashMap::new();
    filters.insert("name".to_string(), vec![format!("^/{name}$")]);
    let opts = ListContainersOptions {
        all: true,
        filters,
        ..Default::default()
    };
    let list = self
        .docker
        .list_containers(Some(opts))
        .await
        .map_err(|e| PgForgeError::Docker(format!("list_containers: {e}")))?;
    Ok(!list.is_empty())
}
```

- [ ] **Step 2: Build to verify**

Run:
```bash
cargo build
```
Expected: clean build. If a bollard struct field renamed in 0.18+ (e.g. `MountTypeEnum::BIND` vs `Bind`), the compiler error will name it — fix and rebuild.

- [ ] **Step 3: Commit**

```bash
git add .
git commit -m "feat(docker): implement BollardEngine network + container ops"
```

---

## Task 14: `pgforge create` orchestration

**Files:**
- Create: `src/commands/mod.rs`
- Create: `src/commands/create.rs`
- Modify: `src/lib.rs`

**Design:** The orchestration layer is *not* unit-tested at this task — it's covered end-to-end by Task 15's gated integration test. We do, however, write a small unit test that asserts the per-instance host directory layout (where bind-mounts come from), since that part is pure path arithmetic.

- [ ] **Step 1: Create `src/commands/mod.rs`**

Write to `src/commands/mod.rs`:

```rust
pub mod create;
```

- [ ] **Step 2: Implement `src/commands/create.rs`**

Write to `src/commands/create.rs`:

```rust
use crate::config::global::GlobalConfig;
use crate::docker::bollard_engine::BollardEngine;
use crate::docker::engine::{
    BindMount, BuildImageSpec, CreateContainerSpec, DockerEngine, NamedVolume,
};
use crate::docker::image::dockerfile;
use crate::domain::instance::Instance;
use crate::domain::platform::current_platform;
use crate::domain::preset::Preset;
use crate::error::{PgForgeError, Result};
use crate::pgbackrest::conf::generate_pgbackrest_conf;
use crate::ports::{TcpProbe, allocate_port};
use crate::postgres::conf::generate_postgresql_conf;
use crate::postgres::hba::generate_pg_hba;
use crate::state::instance::InstanceState;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct CreateArgs {
    pub name: String,
    pub preset: Preset,
    pub pg_version: u8,
    pub app_user: String,
    pub app_password: String,
    pub pgbackrest_password: String,
    /// When None, uses GlobalConfig::default_path() and InstanceState::default_state_root().
    /// Tests set this to a TempDir.
    pub override_state_root: Option<PathBuf>,
}

pub struct ConfigLayout {
    pub root: PathBuf,
    pub postgresql_conf: PathBuf,
    pub pg_hba: PathBuf,
    pub pgbackrest_conf: PathBuf,
}

impl ConfigLayout {
    pub fn for_instance(state_root: &Path, name: &str) -> Self {
        let root = state_root.join("instances").join(name).join("conf");
        Self {
            postgresql_conf: root.join("postgresql.conf"),
            pg_hba: root.join("pg_hba.conf"),
            pgbackrest_conf: root.join("pgbackrest.conf"),
            root,
        }
    }
}

/// Top-level entry called by main.rs / CLI.
pub async fn run(args: CreateArgs) -> Result<InstanceState> {
    Instance::validate_name(&args.name)?;

    let state_root = args
        .override_state_root
        .clone()
        .unwrap_or_else(InstanceState::default_state_root);
    let global_cfg = GlobalConfig::load()?;
    let s3 = global_cfg
        .s3
        .as_ref()
        .ok_or_else(|| PgForgeError::Anyhow(anyhow::anyhow!(
            "S3 settings missing in global config (~/.config/pgforge/config.toml). Add an [s3] section."
        )))?;

    if InstanceState::exists_under(&state_root, &args.name) {
        return Err(PgForgeError::InstanceExists(args.name.clone()));
    }

    let docker = BollardEngine::connect()?;
    run_with_engine(args, &docker, state_root, global_cfg, s3.clone()).await
}

/// Inner function — engine injected so integration tests can swap it.
pub async fn run_with_engine<E: DockerEngine>(
    args: CreateArgs,
    docker: &E,
    state_root: PathBuf,
    global_cfg: GlobalConfig,
    s3: crate::pgbackrest::conf::S3Settings,
) -> Result<InstanceState> {
    let plat = current_platform();
    let tuning = args.preset.tuning();

    // 1. Allocate a port avoiding ones we've handed out before.
    let taken: HashSet<u16> = InstanceState::list_under(&state_root)?
        .into_iter()
        .filter_map(|n| InstanceState::load_under(&state_root, &n).ok())
        .map(|s| s.instance.host_port)
        .collect();
    let host_port = allocate_port(
        global_cfg.port_range_start,
        global_cfg.port_range_end,
        &taken,
        &TcpProbe,
    )?;

    // 2. Render configs and write them to the per-instance config dir on host.
    let layout = ConfigLayout::for_instance(&state_root, &args.name);
    std::fs::create_dir_all(&layout.root).map_err(|e| PgForgeError::Io {
        path: layout.root.clone(),
        source: e,
    })?;
    std::fs::write(&layout.postgresql_conf, generate_postgresql_conf(args.preset, plat))
        .map_err(|e| PgForgeError::Io { path: layout.postgresql_conf.clone(), source: e })?;
    std::fs::write(&layout.pg_hba, generate_pg_hba(&args.name, &args.app_user))
        .map_err(|e| PgForgeError::Io { path: layout.pg_hba.clone(), source: e })?;
    std::fs::write(&layout.pgbackrest_conf, generate_pgbackrest_conf(&args.name, &s3))
        .map_err(|e| PgForgeError::Io { path: layout.pgbackrest_conf.clone(), source: e })?;

    // 3. Make sure the per-version image exists.
    docker
        .build_image(&BuildImageSpec {
            tag: format!("pgforge/postgres:{}", args.pg_version),
            dockerfile: dockerfile(args.pg_version),
        })
        .await?;

    // 4. Make sure the shared docker network exists.
    docker.ensure_network("pgforge_net").await?;

    // 5. Create the container.
    let mut env = HashMap::new();
    env.insert("POSTGRES_USER".into(), args.app_user.clone());
    env.insert("POSTGRES_PASSWORD".into(), args.app_password.clone());
    env.insert("POSTGRES_DB".into(), args.name.clone());
    env.insert("PGDATA".into(), "/var/lib/postgresql/data/pgdata".into());

    let binds = vec![
        BindMount {
            host_path: layout.postgresql_conf.clone(),
            container_path: "/etc/postgresql/postgresql.conf".into(),
            read_only: true,
        },
        BindMount {
            host_path: layout.pg_hba.clone(),
            container_path: "/etc/postgresql/pg_hba.conf".into(),
            read_only: true,
        },
        BindMount {
            host_path: layout.pgbackrest_conf.clone(),
            container_path: "/etc/pgbackrest/pgbackrest.conf".into(),
            read_only: true,
        },
    ];
    let volumes = vec![NamedVolume {
        volume_name: format!("pgforge_data_{}", args.name),
        container_path: "/var/lib/postgresql/data".into(),
    }];

    let spec = CreateContainerSpec {
        container_name: format!("pgforge_{}", args.name),
        image: format!("pgforge/postgres:{}", args.pg_version),
        env,
        binds,
        volumes,
        host_port,
        container_port: 5432,
        memory_mb: tuning.ram_mb,
        network: "pgforge_net".into(),
        shm_size_mb: 256,
    };
    let id = docker.create_container(&spec).await?;

    // 6. Start it.
    docker.start_container(&id).await?;

    // 7. Persist state.
    let state = InstanceState {
        instance: Instance {
            name: args.name.clone(),
            db_name: args.name.clone(),
            app_user: args.app_user,
            app_password: args.app_password,
            pgbackrest_password: args.pgbackrest_password,
            preset: args.preset,
            pg_version: args.pg_version,
            host_port,
        },
        created_at: now_rfc3339(),
    };
    state.save_under(&state_root)?;

    Ok(state)
}

fn now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Cheap ISO-8601 stamp without pulling chrono. Resolution: seconds, UTC.
    // Format: 1970-01-01T00:00:00Z + offset. Good enough for state metadata.
    let days = secs / 86400;
    let rem = secs % 86400;
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    let ss = rem % 60;
    let (y, m, d) = days_to_ymd(days as i64);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn days_to_ymd(days_from_epoch: i64) -> (i64, u32, u32) {
    // Civil-from-days algorithm by Howard Hinnant (public domain).
    let z = days_from_epoch + 719468;
    let era = if z >= 0 { z / 146097 } else { (z - 146096) / 146097 };
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn config_layout_is_per_instance() {
        let layout = ConfigLayout::for_instance(Path::new("/state"), "billing");
        assert_eq!(
            layout.postgresql_conf,
            PathBuf::from("/state/instances/billing/conf/postgresql.conf"),
        );
        assert_eq!(
            layout.pg_hba,
            PathBuf::from("/state/instances/billing/conf/pg_hba.conf"),
        );
        assert_eq!(
            layout.pgbackrest_conf,
            PathBuf::from("/state/instances/billing/conf/pgbackrest.conf"),
        );
    }

    #[test]
    fn now_rfc3339_starts_with_2_and_ends_with_z() {
        let s = now_rfc3339();
        assert!(s.starts_with('2'));
        assert!(s.ends_with('Z'));
        assert_eq!(s.len(), 20);
    }
}
```

- [ ] **Step 3: Wire `commands` into `src/lib.rs`**

Replace `src/lib.rs` with:

```rust
//! pgforge — hardened PostgreSQL provisioner for a single host.

pub mod commands;
pub mod config;
pub mod docker;
pub mod domain;
pub mod error;
pub mod pgbackrest;
pub mod ports;
pub mod postgres;
pub mod state;
```

- [ ] **Step 4: Build and run unit tests**

Run:
```bash
cargo build
cargo test --lib
```
Expected: `commands::create` unit tests pass (2 tests).

- [ ] **Step 5: Commit**

```bash
git add .
git commit -m "feat(commands): pgforge create orchestration with engine-injectable runner"
```

---

## Task 15: CLI dispatch with clap

**Files:**
- Create: `src/cli.rs`
- Modify: `src/main.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Create `src/cli.rs`**

Write to `src/cli.rs`:

```rust
use crate::commands::create::{CreateArgs, run as run_create};
use crate::domain::preset::Preset;
use crate::error::Result;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "pgforge", version, about = "Hardened single-host PostgreSQL provisioner")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create a new hardened PG instance.
    Create {
        /// Instance name (lowercase, [a-z][a-z0-9_-]{0,62}).
        #[arg(long)]
        name: String,
        /// Preset: tiny | small | medium | large.
        #[arg(long, value_parser = parse_preset)]
        preset: Preset,
        /// PostgreSQL major version (e.g. 18).
        #[arg(long)]
        version: u8,
        /// App user name. Default: leads.
        #[arg(long, default_value = "leads")]
        app_user: String,
        /// App password (set via env PGFORGE_APP_PASSWORD or this flag).
        #[arg(long, env = "PGFORGE_APP_PASSWORD")]
        app_password: String,
        /// pgbackrest replication user password.
        #[arg(long, env = "PGFORGE_PGBACKREST_PASSWORD")]
        pgbackrest_password: String,
    },
}

fn parse_preset(s: &str) -> Result<Preset, String> {
    use std::str::FromStr;
    Preset::from_str(s)
}

pub async fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        None => {
            // TUI is added in Plan 5. Until then: print help.
            println!("pgforge: TUI not yet implemented (Plan 5). Run `pgforge --help`.");
            Ok(())
        }
        Some(Command::Create {
            name,
            preset,
            version,
            app_user,
            app_password,
            pgbackrest_password,
        }) => {
            let state = run_create(CreateArgs {
                name,
                preset,
                pg_version: version,
                app_user,
                app_password,
                pgbackrest_password,
                override_state_root: None,
            })
            .await?;
            let i = &state.instance;
            println!(
                "Instance ready:\n  postgresql://{}:***@127.0.0.1:{}/{}",
                i.app_user, i.host_port, i.db_name
            );
            Ok(())
        }
    }
}
```

- [ ] **Step 2: Wire `cli` into `src/lib.rs`**

Replace `src/lib.rs` with:

```rust
//! pgforge — hardened PostgreSQL provisioner for a single host.

pub mod cli;
pub mod commands;
pub mod config;
pub mod docker;
pub mod domain;
pub mod error;
pub mod pgbackrest;
pub mod ports;
pub mod postgres;
pub mod state;
```

- [ ] **Step 3: Update `src/main.rs` to dispatch**

Replace `src/main.rs` with:

```rust
use clap::Parser;
use pgforge::cli::{Cli, dispatch};
use pgforge::error::Result;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_target(false)
        .init();

    let cli = Cli::parse();
    dispatch(cli).await
}
```

- [ ] **Step 4: Verify CLI parses correctly**

Run:
```bash
cargo run -- --help
cargo run -- create --help
```
Expected: clap renders top-level help and the `create` subcommand's flags. No execution beyond parsing.

- [ ] **Step 5: Commit**

```bash
git add .
git commit -m "feat(cli): clap dispatcher with create subcommand"
```

---

## Task 16: End-to-end smoke test (gated) + README

**Files:**
- Create: `tests/create_e2e_test.rs`
- Modify: `README.md`

**Design:** This test actually starts a container. It runs only when `PGFORGE_E2E=1` is set so CI doesn't accidentally pull a 200 MB image. Cleanup is best-effort (we drop the container at the end via `docker rm -f` shell-out — keeps the test code small).

- [ ] **Step 1: Write `tests/create_e2e_test.rs`**

Create `tests/create_e2e_test.rs`:

```rust
//! End-to-end test: actually create a tiny PG instance via pgforge,
//! connect to it, drop it. Gated by `PGFORGE_E2E=1` so CI doesn't run it.
//!
//! Requires: a running Docker engine (Docker Desktop, OrbStack, etc.) and
//! S3 creds in the global config OR a stubbed config in the temp dir.

use pgforge::commands::create::{CreateArgs, run_with_engine};
use pgforge::config::global::GlobalConfig;
use pgforge::docker::bollard_engine::BollardEngine;
use pgforge::domain::preset::Preset;
use pgforge::pgbackrest::conf::S3Settings;
use std::time::Duration;
use tempfile::TempDir;

fn fake_s3() -> S3Settings {
    S3Settings {
        bucket: "pgforge-e2e-stub".into(),
        region: "eu-central-1".into(),
        endpoint: "s3.eu-central-1.amazonaws.com".into(),
        access_key: "AKIAFAKE".into(),
        secret_key: "secret".into(),
    }
}

#[tokio::test]
async fn create_tiny_instance_then_cleanup() {
    if std::env::var("PGFORGE_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping: set PGFORGE_E2E=1 to run");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let docker = BollardEngine::connect().expect("docker engine reachable");
    let global = GlobalConfig {
        s3: Some(fake_s3()),
        ..Default::default()
    };
    let name = format!("pgforge_e2e_{}", uniq_suffix());
    let res = run_with_engine(
        CreateArgs {
            name: name.clone(),
            preset: Preset::Tiny,
            pg_version: 18,
            app_user: "leads".into(),
            app_password: "pw".into(),
            pgbackrest_password: "rpw".into(),
            override_state_root: Some(tmp.path().to_path_buf()),
        },
        &docker,
        tmp.path().to_path_buf(),
        global,
        fake_s3(),
    )
    .await;

    // Cleanup before assert so a failure leaves no residue.
    let container = format!("pgforge_{}", name);
    let _ = std::process::Command::new("docker")
        .args(["rm", "-f", &container])
        .output();
    let _ = std::process::Command::new("docker")
        .args(["volume", "rm", "-f", &format!("pgforge_data_{}", name)])
        .output();

    let state = res.expect("create should succeed");
    assert_eq!(state.instance.preset, Preset::Tiny);
    assert!(state.instance.host_port >= 5433);

    // Optionally: poll the host port until PG accepts connections.
    poll_pg_ready(state.instance.host_port).await;
}

fn uniq_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .to_string()
}

async fn poll_pg_ready(port: u16) {
    use tokio::net::TcpStream;
    for _ in 0..30 {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("postgres on port {port} did not accept TCP within 30s");
}
```

- [ ] **Step 2: Verify it compiles (without running)**

Run:
```bash
cargo test --no-run
```
Expected: builds clean. The test compiles but does nothing at runtime unless `PGFORGE_E2E=1`.

- [ ] **Step 3: Manual smoke test (optional but strongly recommended)**

If Docker is running on your machine:

Run:
```bash
PGFORGE_E2E=1 cargo test --test create_e2e_test -- --nocapture
```
Expected: ~60-90s on first run (image build), then `test create_tiny_instance_then_cleanup ... ok`. Inspect with `docker ps`, `docker logs pgforge_pgforge_e2e_<suffix>` while it's running.

- [ ] **Step 4: Replace `README.md` with a real quickstart**

Write to `README.md`:

````markdown
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
````

- [ ] **Step 5: Final verification — full test suite passes**

Run:
```bash
cargo test
```
Expected: every test (unit + integration except the gated E2E) passes.

- [ ] **Step 6: Final commit**

```bash
git add .
git commit -m "feat: e2e smoke test and quickstart README"
```

---

## Self-review checklist

- [x] **Spec coverage:** Foundation + `create` end-to-end. Snapshot/restore/clone/upgrade/TUI are explicitly deferred to plans 2–5 and called out at the top.
- [x] **No placeholders:** Every step has either a complete code block, an exact command with expected output, or a commit message. No "TBD" / "implement later".
- [x] **Type consistency:** `Instance` fields (`name`, `db_name`, `app_user`, `app_password`, `pgbackrest_password`, `preset`, `pg_version`, `host_port`) are identical across Tasks 10, 11, 14, 15, 16. `S3Settings` shape (Task 7) reused unchanged in Tasks 9, 14, 16. `DockerEngine` trait surface defined in Task 11 matches usage in Task 14 verbatim.
- [x] **TDD where it matters:** Pure functions (`Preset::tuning`, conf generators, port allocator, name validator, state round-trip) are TDD'd. Engine integration is covered by the gated E2E test rather than mocked-out unit tests of the orchestrator (mocks would test the mock, not the system).
- [x] **Frequent commits:** 16 commits, one per task, each leaves the build green.

## Known follow-ups for Plan 2+

- The E2E test relies on the `docker` CLI for cleanup (it's the smallest reliable way to drop a container + named volume). If we ever ship without depending on the CLI, fold cleanup into `BollardEngine`.
- `now_rfc3339()` is hand-rolled to avoid pulling `chrono`. Plan 2 likely adds proper time handling for `--target-time` parsing in PITR — at that point swap both call sites to `chrono`.
- The `create` flow doesn't yet persist a `cron` job inside the container for scheduled backups — Plan 2 adds the `entrypoint.sh` and crontab generation, since they only matter once snapshot/restore exist.
- TLS for client connections is deferred. Plan 5 (TUI) is a natural place to add a "rotate certs" action.
