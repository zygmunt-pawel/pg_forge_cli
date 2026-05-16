# pgforge Linux port — retarget from macOS to Linux (v0.2.0)

**Date:** 2026-05-16
**Status:** design approved, no backwards compatibility with macOS

## Purpose

Retarget pgforge to run **natively on Linux** as its primary (and only)
platform. All macOS-specific code is removed; no compatibility shims are kept.
Existing macOS deployments stay on `v0.1.27` indefinitely — they are not
migrated, by the operator's explicit decision.

Linux also materially improves the durability story (the macOS+Docker-Desktop
fsync caveat documented in v0.1.x goes away — Linux with native Docker has
honest fsync), which was the original motivation for the move.

Released as **`v0.2.0`** (minor bump: new platform target, the macOS asset
disappears from `releases/latest`; no CLI surface or `state.toml` format
changes).

## Scope

### Removed (macOS-only code, deleted outright — no `#[cfg]` gates)

- `src/commands/schedule.rs` — entire ~277-line launchd implementation.
- `tests/schedule_test.rs` — imports `render_plist`/`launchctl_*` which
  disappear; deleted with the impl.
- `tests/platform_test.rs` — imports `Platform::MacOs` which goes away with
  the enum simplification (see below); deleted.
- `src/docker/bollard_engine.rs` Colima fallback block (lines ~22-35) —
  contains an `unsafe std::env::set_var(...)` that becomes warning/clippy
  noise on Linux; deleted, not just left dead. On Linux `/var/run/docker.sock`
  is the only path that matters.
- All macOS-specific strings in `src/cli.rs`: the `"Manage the macOS
  LaunchAgent"` doc on the `Schedule` enum, the `"Installed LaunchAgent at"`
  println in the dispatch arm, the `Snapshot --due` doc referring to "the
  LaunchAgent installed by pgforge schedule install".
- README: the entire Colima / OrbStack / Docker Desktop install section, the
  `xattr quarantine` line, the macOS+Docker-Desktop fsync caveat, the
  "Plans/macOS" historical notes.

### Simplified

- `src/domain/platform.rs` — the `Platform` enum and `current_platform()`
  helper were only ever consumed in `postgres/conf.rs` as `let _ = platform;`
  (the value never affected output). Delete the enum + helper entirely; drop
  the parameter from `generate_postgresql_conf_with_archive` and update its
  call sites (`create.rs`, `restore.rs`, `rotate.rs`). One small ripple, no
  behaviour change.
- `src/tui/clipboard.rs` — the `cfg!(target_os = "macos")` branch that
  appends `.local` to the hostname disappears; the OSC52 fallback path
  (which already exists in the code) becomes the primary clipboard mechanism
  for the typical headless-server case.

### Unchanged (already OS-agnostic — verified)

State files (`state.toml` / `snapshots.toml`), `atomic_write` / `fsync_dir`
/ `LockedStateRoot` (all already `#[cfg(unix)]`-guarded; Linux *is* unix),
fs2 advisory locking, the entire bollard/Docker integration (apart from the
Colima fallback deletion), port allocation, ratatui TUI rendering, every
command except `schedule`. The `directories` crate already returns the
correct XDG paths on Linux (`~/.local/share/pgforge/` for state,
`~/.config/pgforge/` for the global config) — no changes needed.

## Section 2 — `src/commands/schedule.rs` rewritten for systemd user timer

Same public API (`install` / `uninstall` / `status` + the `AGENT_LABEL` const
+ a `ScheduleStatus` struct) so the CLI dispatch arm doesn't change shape.
Internals are replaced.

`pgforge schedule install`:

- Resolves the absolute path of the current `pgforge` binary via
  `std::env::current_exe()`.
- Generates `~/.config/systemd/user/pgforge-snapshot-due.service`:
  ```ini
  [Unit]
  Description=pgforge: snapshot every backup-enabled instance whose hour is due

  [Service]
  Type=oneshot
  ExecStart=/absolute/path/to/pgforge snapshot --due
  Environment=PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
  ```
- Generates `~/.config/systemd/user/pgforge-snapshot-due.timer`:
  ```ini
  [Unit]
  Description=Run pgforge snapshot --due every 5 minutes

  [Timer]
  OnBootSec=2min
  OnUnitActiveSec=5min
  Unit=pgforge-snapshot-due.service

  [Install]
  WantedBy=timers.target
  ```
  Note on `OnUnitActiveSec` semantics: the next firing is 5 min after the
  timer last *activated*, not after the previous run finished. For a
  `snapshot --due` tick that typically completes in seconds this is
  irrelevant; for a backlog of S3 backups that takes >5 min the next
  activation overlaps. Acceptable for a daily-snapshot trigger; documented
  here so it isn't read later as a bug.
- Runs:
  ```bash
  systemctl --user daemon-reload
  systemctl --user enable --now pgforge-snapshot-due.timer
  ```
- Detects linger via `loginctl show-user $USER -p Linger` and parses
  `Linger=yes`/`no`. If `no`, prints a loud stderr warning:
  > `WARNING: linger is not enabled for $USER — the timer will only fire
  > while you are logged in. Run \`sudo loginctl enable-linger $USER\` to
  > have it fire on a headless server.`
  The install itself does NOT attempt `sudo loginctl enable-linger` — `sudo`
  from a CLI install is a footgun.

`pgforge schedule uninstall`:

- `systemctl --user disable --now pgforge-snapshot-due.timer` (idempotent —
  tolerates "doesn't exist" / "not loaded").
- Removes both unit files if they exist.
- `systemctl --user daemon-reload`.

`pgforge schedule status`:

- Runs `systemctl --user is-enabled pgforge-snapshot-due.timer` and
  `is-active`; parses the trivial output (`enabled`/`active`/error).
- Runs `systemctl --user show pgforge-snapshot-due.timer -p NextElapseUSecRealtime`
  to extract the next scheduled firing (best-effort: surface "unknown" if
  parsing fails).
- Reads `loginctl show-user $USER -p Linger`.
- Prints a 4-line summary:
  ```
  Unit:    pgforge-snapshot-due.timer (enabled, active)
  Next:    2026-05-16 03:02:00 UTC
  Linger:  enabled
  Logs:    journalctl --user -u pgforge-snapshot-due
  ```

A small private helper (`fn run_systemctl(args: &[&str]) -> Result<Output>`)
captures stdout+stderr like the old `run_launchctl` did. All path resolution
goes through `~/.config/systemd/user/` via `$HOME`.

## Section 3 — `self_update.rs` + release CI

### `src/commands/self_update.rs`

Two precise edits (there is no constant — the asset name is embedded in a
format string and the doc comment):

1. **Line ~53**, the URL format string:
   ```rust
   "https://github.com/{repo}/releases/download/{tag}/pgforge"
   ```
   becomes
   ```rust
   "https://github.com/{repo}/releases/download/{tag}/pgforge-linux-x86_64"
   ```
2. **Module doc comment** (line ~8):
   `"Direct download of the universal macOS binary asset."`
   becomes
   `"Direct download of the Linux x86_64 binary asset."`

Everything else (the `curl` shell-out, the atomic rename, the
already-on-latest fast path) stays.

### `.github/workflows/release.yml`

Replace the entire `build-macos-universal` job with a single Linux job; keep
the `on: push: tags: ['v*']` trigger and `permissions: contents: write`.

```yaml
jobs:
  build-linux-x86_64:
    name: Build Linux x86_64 binary
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: x86_64-unknown-linux-gnu

      - name: Cache cargo
        uses: Swatinem/rust-cache@v2

      - name: Build (release)
        run: cargo build --release --target x86_64-unknown-linux-gnu

      - name: Strip + smoke test + checksum + tarball
        run: |
          mkdir -p dist
          cp target/x86_64-unknown-linux-gnu/release/pgforge dist/pgforge-linux-x86_64
          strip dist/pgforge-linux-x86_64
          ./dist/pgforge-linux-x86_64 --version    # catches broken linkage
          (cd dist && tar -czf pgforge-linux-x86_64.tar.gz pgforge-linux-x86_64 \
             && sha256sum pgforge-linux-x86_64 pgforge-linux-x86_64.tar.gz > SHA256SUMS)
          ls -lah dist/

      - name: Upload to release
        uses: softprops/action-gh-release@v2
        with:
          files: |
            dist/pgforge-linux-x86_64
            dist/pgforge-linux-x86_64.tar.gz
            dist/SHA256SUMS
          generate_release_notes: true
          draft: false
          prerelease: false
```

Glibc (the default on `ubuntu-latest`) — covers Ubuntu / Debian / Fedora /
RHEL / Arch defaults. musl-static and ARM Linux are explicitly deferred
(see *Out of scope*).

Also add a CI test job (separate workflow `ci.yml`, triggers on push to
`main` + on PRs):

```yaml
name: ci
on: { push: { branches: [main] }, pull_request: {} }
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --quiet
      - run: cargo clippy -- -D warnings
```

Currently zero CI test coverage; this is free safety.

## Section 4 — README rewrite

Keep: title, "What it does", Configuration, Quick start, Commands, TUI mode,
"How it works", "Upgrading existing instances", "Building from source".

Rewrite:

- **Install section** — drop the entire Colima / OrbStack / Docker Desktop /
  `xattr quarantine` content. Replace with ~10 lines: install Docker for
  your distro (link to docker.com docs), `sudo usermod -aG docker $USER`
  + log out/in, `curl + chmod` the `pgforge-linux-x86_64` asset to
  `~/.local/bin/pgforge`, `pgforge --version`. Note that on a fresh system
  without docker-group membership every pgforge command fails with a
  permission-denied trying to open the docker socket — make this loud.
- **Scheduled backups section** — add a one-liner:
  > After `pgforge schedule install`, run `sudo loginctl enable-linger
  > $USER` once so the timer fires when you're not logged in (typical
  > server case).
- **Caveats section** — delete the macOS+Docker-Desktop fsync caveat.
  Keep the "No HA" + "Restored instances are inert by design" caveats.
- No "migration from macOS" section. Fresh-install only.

## Section 5 — `arboard` / clipboard on a headless server

`arboard::Clipboard::new()` fails at runtime on a headless Linux box (no X11
or Wayland display). The TUI's clipboard helper (`src/tui/clipboard.rs`)
already has an OSC52 fallback that pushes the copied string through the
terminal escape sequence — that becomes the primary clipboard path on the
typical headless server, and it works through SSH.

No code change required beyond removing the `cfg!(target_os = "macos")`
hostname-suffix branch. Document in the TUI section of the README:

> Clipboard on a headless server uses OSC52 (terminal escape sequence) —
> works through SSH if your local terminal supports it (most do: iTerm2,
> kitty, WezTerm, Alacritty, tmux ≥ 3.3). If your terminal doesn't, the
> copy is a no-op (a flash warning shows).

Consider gating `arboard` behind a `desktop` Cargo feature in a follow-up
(`arboard` pulls X11/Wayland link deps that aren't needed on a headless
server). Not in scope for this port — it works fine as-is on headless,
just with one extra crate.

## Section 6 — Tests + CI

- Existing unit + integration tests pass 1:1 on Linux (all already
  `#[cfg(unix)]`-correct).
- **Deletions**: `tests/schedule_test.rs`, `tests/platform_test.rs` —
  these import symbols that disappear; deleted with their impl files.
- **New**: `tests/schedule_systemd_test.rs` — pure-fn tests over the
  generators for the `.service` and `.timer` file contents (assert presence
  of `Type=oneshot`, `ExecStart=`, `OnUnitActiveSec=5min`,
  `WantedBy=timers.target`, the absolute exe path) — analogous to the old
  `plist_contains_label_and_program` test, ~5 tests.
- **Soft-failure helpers** (the old `launchctl_is_*` substring matchers)
  are not replicated for `systemctl --user` — its exit codes are well-
  behaved enough that string-scraping isn't needed.
- Real `systemctl --user` invocations remain untested in `cargo test` (CI
  has no user-systemd session); covered by manual verification on a real
  Linux box. This matches how the old launchd code was tested.
- Add the `ci.yml` workflow described in §3 so `cargo test` actually runs
  somewhere reproducible.

## Files touched

| File | Action |
|---|---|
| `src/commands/schedule.rs` | rewritten (launchd → systemd user timer) |
| `src/cli.rs` | remove macOS strings (Schedule doc, "LaunchAgent" println, Snapshot --due doc) |
| `src/commands/self_update.rs` | URL format string + doc comment |
| `src/domain/platform.rs` | deleted |
| `src/domain/mod.rs` | drop `pub mod platform;` |
| `src/postgres/conf.rs` | drop `platform: Platform` parameter |
| `src/commands/create.rs`, `restore.rs`, `rotate.rs` | drop `current_platform()` call sites |
| `src/docker/bollard_engine.rs` | delete the Colima fallback block (lines ~22-35) |
| `src/tui/clipboard.rs` | drop the `cfg!(target_os = "macos")` branch |
| `tests/schedule_test.rs` | deleted |
| `tests/platform_test.rs` | deleted |
| `tests/schedule_systemd_test.rs` | new — generator tests |
| `README.md` | rewritten install section, drop macOS caveat, scheduled-backups linger note |
| `.github/workflows/release.yml` | replaced (macOS → Linux x86_64), `--version` smoke step |
| `.github/workflows/ci.yml` | new — `cargo test` + `cargo clippy -D warnings` on PRs and main pushes |
| `Cargo.toml` | version bump 0.1.27 → 0.2.0 |
| `Cargo.lock` | regenerated |

## Out of scope

- **No backwards compatibility with macOS.** No `#[cfg]` gates, no dual
  shipping. The macOS asset disappears from `releases/latest`.
- **No migration of existing macmini instances.** Operator's decision; the
  macmini's `pg-a002` stays on `v0.1.27` indefinitely.
- **musl-static binary.** `pgforge-linux-x86_64` is glibc against
  `ubuntu-latest`; add a musl asset only if a real glibc-version complaint
  arrives.
- **ARM Linux (`aarch64-unknown-linux-gnu`).** Single x86_64 asset for now;
  add ARM when there's a real deployment for it.
- **Package-manager installs** (apt/dnf/AUR/Homebrew tap). Curl-and-chmod
  is the only install path for v0.2.0.
- **Gating `arboard` behind a `desktop` feature.** Real cleanup but not
  required for the port; the OSC52 fallback already works on headless
  servers.
- **Auto-detecting and offering to run `loginctl enable-linger`.** Sudo
  from a CLI install is a footgun; a loud stderr warning is the chosen
  mitigation.
- **A Linux equivalent of the macOS `xattr -d com.apple.quarantine` step.**
  Not needed — Linux has no equivalent quarantine mechanism on `curl`'d
  binaries.
