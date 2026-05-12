# Plan 5 — TUI dashboard for pgforge (design)

Date: 2026-05-12
Status: design approved, plan-doc to follow (writing-plans)

## 1. Goal

`pgforge` invoked with no subcommand launches a lazydocker-style interactive
dashboard for managing the local fleet of hardened PostgreSQL instances. The
TUI reuses the Plan-4 command backends (`commands::ls`, `commands::status`,
`commands::snapshots`) and adds keybinds for the five mutating operations
(snapshot, clone, rotate, upgrade, restore) plus an Enter-to-copy connection
string.

Out of scope for Plan 5: external monitoring integration, multi-host views,
historical metrics charts, log tailing.

## 2. Scope (MVP for Plan 5)

Mutating operations available from the TUI:

- `[s]` snapshot — non-destructive, no confirm
- `[c]` clone   — opens `Modal::CloneAs` (text input for `--as`)
- `[R]` rotate  — destructive (container recreated), confirm prompt
- `[u]` upgrade — destructive, opens `Modal::UpgradeTo` then `Modal::Confirm`
- `[r]` restore — destructive, opens `Modal::RestoreAs` then `Modal::Confirm`
- `[Enter]` copy connection string to the system clipboard (arboard).
  Bottom-bar flash on success. **Password handling**: the app password is
  not stored in `state.toml` (it lives in init_sql which is wiped after
  bootstrap), so the copied URI is
  `postgresql://{app_user}:***@127.0.0.1:{host_port}/{db_name}` —
  consistent with what the CLI already prints. The user replaces `***`
  with their `PGPASSWORD` / `.pgpass` value. If clipboard write fails
  (e.g. no display server), open `Modal::ErrorDetail` containing the URI
  for manual copy and surface the clipboard error in `last_op_error`.
- `[q]`/`Esc` quit.
- `[?]` shows full error detail for `last_op_error` (if set).
- `[e]` open `Modal::Snapshots` with the full scrollable list.

Navigation: `↑`/`↓` or `j`/`k` on the instance list.

## 3. Architecture

### 3.1 Entry point

`src/cli.rs` `dispatch()` already has the `None` branch where TUI launches.
Replace the placeholder println! with `pgforge::tui::run().await`. If stdout
is not a tty (`std::io::IsTerminal`), print "use --help for commands"
instead — keeps `pgforge | head` and CI smoke tests usable.

### 3.2 Module layout

```
src/tui/
  mod.rs         — pub async fn run(); terminal setup/teardown; main loop
  app.rs         — AppState; apply_event(&mut self, Event)
  events.rs      — Event enum; key-event task; tick generator
  refresh.rs     — spawn_refresh_pollers(tx, instances_rx)
  ops.rs         — spawn_op(kind, name, tx)
  clipboard.rs   — copy_to_clipboard(&str) via arboard
  ui/
    mod.rs       — render(frame, &AppState); top-level Layout split
    list.rs      — left pane: instance list
    detail.rs    — right pane: status + snapshots + PITR
    bottom.rs    — bottom bar (keybinds / progress / error / flash)
    modal.rs     — enum Modal + render + handle_key per variant
```

`src/lib.rs` adds `pub mod tui;`.

Existing `commands::*` are **library-called** from `tui::ops` — no logic
duplication. Anything that takes args today (CloneArgs, UpgradeArgs, etc.)
is constructed by the op runner from `AppState` + modal input.

### 3.3 Dependencies (Cargo.toml additions)

```toml
ratatui   = "0.29"
crossterm = "0.28"
arboard   = "3"
```

Rationale:
- ratatui 0.29 with crossterm 0.28 backend — current stable line.
- arboard chosen over spawning `pbcopy`/`xclip` to keep one codepath; pure
  Rust on Linux & macOS, no runtime deps.

### 3.4 Concurrency model — actor / channels

Single-owner `AppState` lives in the main task. Background work (refresh
pollers, key-event reader, long-op runners) communicates only by sending
`Event`s into an `mpsc::UnboundedSender<Event>`. The main loop drains the
receiver and calls `state.apply_event(ev)` deterministically — no shared
mutable state, no locks, no holding-lock-across-await landmines.

Main loop pseudo:

```rust
loop {
    if state.should_quit { break; }
    terminal.draw(|f| ui::render(f, &state))?;
    tokio::select! {
        biased;
        Some(ev) = rx.recv()                  => state.apply_event(ev),
        _ = tokio::time::sleep(TICK_INTERVAL) => state.apply_event(Event::Tick),
    }
}
```

`TICK_INTERVAL` = 250 ms (drives flash auto-expire, spinner frames, elapsed
counters).

## 4. State and events

### 4.1 AppState

```rust
pub struct AppState {
    pub instances:    Vec<InstanceSummary>,             // from ls::run, sorted
    pub statuses:     HashMap<String, InstanceStatus>,  // per-name, optional
    pub snapshots:    HashMap<String, SnapshotsView>,   // per-name, optional
    pub in_progress:  HashMap<String, RunningOp>,       // per-instance lock
    pub selected:     usize,
    pub modal:        Option<Modal>,
    pub last_op_error: Option<OpError>,                 // sticky red bottom bar
    pub flash:        Option<Flash>,                    // auto-expire ~3s
    pub stale_status: HashSet<String>,                  // refresh-failed names; cleared on next successful StatusRefreshed
    pub now:          Instant,
    pub should_quit:  bool,
}

pub struct SnapshotsView { pub list: Vec<SnapshotRecord>, pub pitr: PitrWindow }
pub struct RunningOp { pub kind: OpKind, pub started_at: Instant }
pub enum OpKind { Snapshot, Clone, Rotate, Upgrade, Restore }
pub struct OpError { pub instance: String, pub kind: OpKind, pub msg: String, pub at: Instant }
pub enum FlashKind { Info, Success }
pub struct Flash { pub msg: String, pub kind: FlashKind, pub at: Instant }
```

### 4.2 Event enum

```rust
pub enum Event {
    Key(crossterm::event::KeyEvent),
    Tick,
    InstancesListed(Vec<InstanceSummary>),
    StatusRefreshed   { name: String, status: InstanceStatus },
    SnapshotsRefreshed{ name: String, view: SnapshotsView },
    OpStarted   { instance: String, kind: OpKind },
    OpFinished  { instance: String, kind: OpKind, result: Result<()> },
    RefreshFailed { name: String, err: String },  // quiet
}
```

`apply_event` is pure modulo `Instant::now()` — testable without ratatui or
any terminal.

## 5. Refresh strategy

Three independent pollers, spawned at startup, each with its own
`tokio::time::interval`. They share an `Arc<watch::Receiver<Vec<String>>>`
that the main loop updates whenever `InstancesListed` produces a new name
set — pollers always iterate the current set, not a stale snapshot.

| Poller             | Cadence | Action                                               |
|--------------------|---------|------------------------------------------------------|
| `ls_poller`        | 5 s     | `ls::run` → `Event::InstancesListed`                 |
| `status_poller`    | 2 s     | per name: `status::run_with_engine` → `StatusRefreshed`. Skips stopped instances (cheap docker.ps check first). |
| `snapshots_poller` | 15 s    | per name: `snapshots::run` + `snapshots::pitr_window` → `SnapshotsRefreshed` (pgbackrest exec is expensive) |

Poller errors emit `Event::RefreshFailed` — `tracing::warn!` and add the
name to `state.stale_status`. A successful `StatusRefreshed` for that name
clears it. The detail pane shows a `(stale)` indicator next to fields
that haven't refreshed for >2× their cadence.

**Post-op immediate refresh** (`refresh::refresh_one(name, tx)` in
`refresh.rs`) fires `status::run_with_engine` + `snapshots::run` +
`snapshots::pitr_window` for one instance, emitting the same
`StatusRefreshed` / `SnapshotsRefreshed` events. Called from
`apply_event(OpFinished{Ok})` so the user immediately sees the new state
without waiting for the next poller tick. After `Clone` / `Restore` we
*also* re-run `ls_poller`'s call (new instance appeared).

## 6. Long-op lifecycle

User-initiated mutating operations run as their own `tokio::task`.

```
[key 's' on instance A]
  apply_event(Key(s)):
    if state.in_progress.contains_key(A): return            // per-instance lock
    let tx2 = tx.clone();
    tokio::spawn(ops::run(OpKind::Snapshot, A.clone(), tx2));
    // OpStarted will arrive on the channel shortly

ops::run(OpKind::Snapshot, name, tx):
  tx.send(OpStarted{ name: name.clone(), kind: Snapshot }).ok();
  let result = commands::snapshot::run(SnapshotArgs { instance: name.clone(), .. })
                .await.map(|_| ());
  tx.send(OpFinished{ instance: name, kind: Snapshot, result }).ok();

apply_event(OpStarted{name, kind}):
  in_progress.insert(name, RunningOp{kind, started_at: now});

apply_event(OpFinished{name, kind, result}):
  in_progress.remove(&name);
  match result {
    Ok(()) => {
      state.flash = Some(Flash{ msg: format!("{kind:?} on {name} done"), kind: Success, at: now });
      // trigger immediate refresh of that instance:
      tokio::spawn(refresh::refresh_one(name, tx.clone()));
    }
    Err(e) => state.last_op_error = Some(OpError{instance:name, kind, msg: e.to_string(), at: now}),
  }
```

**Bottom bar selection.** When multiple ops are running, the bar shows the
op on the selected instance (if any) — otherwise the first by iteration —
suffixed with `(+N more)` when `in_progress.len() > 1`.

## 7. Error handling — two-tier

- **Refresh errors (quiet)**: `Event::RefreshFailed` → `tracing::warn!`,
  add name to `stale_status`. No bottom-bar noise — refresh will retry on
  the next tick anyway.
- **Op errors (loud)**: `Event::OpFinished { result: Err(_) }` → set
  `last_op_error`. Bottom bar renders red. Sticky: cleared on `Esc`, or
  replaced when a newer op fails. `[?]` opens `Modal::ErrorDetail` with the
  full message.

Docker-connect failure at TUI startup is fatal: print the error to stderr
and exit with code 1 (terminal not even initialized).

## 8. Modal architecture

`Option<Modal>` overlay; while `Some`, keys route to the modal's handler.
Modal is rendered with `Clear(area)` plus a bordered Block on a centered
rect (size depends on variant).

```rust
pub enum Modal {
    CloneAs    { source: String,                       input: TextField },
    UpgradeTo  { source: String,                       input: TextField },
    RestoreAs  { source: String, as_input: TextField, target_time: TextField, focus: u8 },
    Confirm    { kind: PendingDestructiveOp, prompt: String },
    Snapshots  { name: String, view: SnapshotsView },  // scrollable full list
    ErrorDetail{ msg: String },                        // [?] from bottom bar
}

pub enum PendingDestructiveOp {
    Rotate  { name: String },
    Upgrade { name: String, to: u8 },
    Restore { source: String, as_: String, target_time: Option<String> },
}

pub struct TextField { pub buf: String, pub cursor: usize }
```

**Validation**: on submit only (Enter). Reuses existing validators:
- instance name regex `[a-z][a-z0-9_-]{0,62}` (already in `domain::instance`),
- version `u8::from_str`,
- target_time `jiff::Timestamp::from_str`.

**Destructive flow** (Upgrade as example):
1. `[u]` on instance A → `Modal::UpgradeTo { source: A, input: "" }`.
2. User types `18`, Enter → validate `to > current_pg_version` → on OK
   → `Modal::Confirm { kind: Upgrade{A, 18}, prompt: "Upgrade A from PG17 to PG18? Takes several minutes; an auto pre-snapshot is taken first." }`.
3. `y` or `Enter` → close modal + `ops::spawn(Upgrade, A)`. `n`/`Esc` → close modal.

`[c]lone` and `[Enter]` (clipboard) skip Confirm — non-destructive.
`[R]otate` and `[r]estore` go through Confirm.

## 9. Layout

Steady state:
```
┌─ pgforge ─────────────────────────────────────────────────────────────────────┐
│ Instances (3)                  │ leads-staging   PG18 medium  :5432  ◉ running │
│ > leads-staging  PG18  ◉      │                                                │
│   leads-dev      PG17  ○      │ CPU: 12.4%        Mem: 384 / 1024 MiB          │
│   archive-2024   PG17  ◉      │ Conns: 3 active / 14 idle / 17 total           │
│                                │ DB:   12.4 GiB    PGDATA: 14.8 GiB             │
│                                │                                                │
│                                │ Snapshots (4)                                  │
│                                │   2026-05-12T08:00  full   pre-upgrade         │
│                                │   2026-05-11T18:00  diff                       │
│                                │   2026-05-10T18:00  diff                       │
│                                │   2026-05-09T08:00  full                       │
│                                │ PITR window: 2026-05-09T08:00 .. 2026-05-12T10:14│
├────────────────────────────────┴──────────────────────────────────────────────┤
│ [s]napshot  [c]lone  [R]otate  [u]pgrade  [r]estore  [↵] copy-uri  [q]uit     │
└───────────────────────────────────────────────────────────────────────────────┘
```

Bottom-bar variants (mutually exclusive, priority top-down):
```
│ ✗ upgrade leads-staging failed: pg_upgrade exit 1 — [?] details  [esc] clear │  red, sticky
│ ⠼ snapshot on leads-staging (12s)…  (+1 more)        [s/c/R/u/r] [q]uit      │  yellow
│ ✓ copied connection string to clipboard                                      │  green, 3s
│ [s]napshot  [c]lone  [R]otate  [u]pgrade  [r]estore  [↵] copy-uri  [q]uit   │  default
```

Modal example (UpgradeTo):
```
        ┌─ Upgrade leads-staging ───────────────────────┐
        │ Target version: _18_                          │
        │                                               │
        │ [Enter] continue   [Esc] cancel               │
        └───────────────────────────────────────────────┘
```

Modal sizes: CloneAs/UpgradeTo 60×9, RestoreAs 70×13, Confirm 60×7,
ErrorDetail 80×15, Snapshots 80×20 (whichever is smaller fits terminal).

## 10. Testing

| Subject                              | How                                                              |
|--------------------------------------|------------------------------------------------------------------|
| `AppState::apply_event` transitions  | Pure unit tests in `tui::app::tests` — feed event sequences, assert state. Coverage: navigation, lock acquire/release, modal open/close, error/flash expiry, refresh merge, stale tracking. |
| Modal input validation               | Unit tests in `tui::ui::modal::tests` — name regex, u8 parse, RFC3339 parse, upgrade `to > from`. |
| Refresh poller fan-out               | Mock `DockerEngine` + single tick — assert one event emitted per name. One smoke test per poller. |
| Long-op lifecycle                    | Apply `OpStarted` → `OpFinished{Ok}` → assert flash + lock cleared, no error. `Err` variant → assert `last_op_error` set, no flash. |
| Clipboard                            | Skipped on headless Linux CI (`#[cfg_attr(target_os = "linux", ignore)]` or feature gate). Manual smoke on macOS. |
| Terminal rendering                   | No snapshot tests for MVP. Manual `cargo run` on a real tty. |

Target: +20–30 unit tests on top of current 98. **No heavy E2E.** Keeps
the gate-pattern from Plan 4 (existing 98 tests must stay green; new TUI
tests are also fast — pure logic on AppState).

## 11. Non-goals (explicit)

- No log tailing inside the TUI.
- No multi-host or cluster view.
- No historical charts (CPU/memory over time) — current snapshot only.
- No theming / config file.
- No mouse support (keyboard only; mouse may select text in the terminal naturally).
- No headless / `--no-default-features` build for now — single binary stays single.

## 12. Risks and mitigations

- **Terminal restoration on panic.** Use `std::panic::set_hook` to call
  `disable_raw_mode + LeaveAlternateScreen` before the default hook prints
  the panic. Otherwise an unwind leaves the user's terminal in raw mode.
- **`docker stats` blocking call** in `status::run_with_engine` uses
  `std::process::Command` (blocking) — must be wrapped in
  `tokio::task::spawn_blocking` from the poller so it doesn't stall the
  runtime. This is the only sync syscall left in the status path.
- **Channel backpressure.** `mpsc::unbounded` is fine for our cadence
  (events arrive at most a few per second); upgrade to bounded only if
  profiling shows growth.
- **pgbackrest exec on every poll** — 15 s cadence + skip-if-stopped is
  enough; the alternative (in-container `inotify`) is over-engineering.

## 13. Open questions

None at this point — the three brainstorming questions (state shape,
error model, modal pattern) are resolved by the recommendations above.
