//! AppState — single-owner mutable state for the TUI main loop.
//! `apply_event` is pure modulo Instant::now(): tests inject events and
//! assert state. No terminal, no docker, no tokio in this file.

use crate::commands::ls::InstanceSummary;
use crate::commands::status::InstanceStatus;
use crate::tui::events::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

/// Number of CPU samples to keep per instance for the sparkline. At
/// 2s per status_poller tick this is 60s of history — enough to see
/// short-term spikes without bloating memory.
pub const CPU_HISTORY_LEN: usize = 30;

pub struct AppState {
    pub instances: Vec<InstanceSummary>,
    pub statuses: HashMap<String, InstanceStatus>,
    /// Recent CPU% samples per instance — newest on the back. Updated
    /// by `apply_event(StatusRefreshed)`; clamped to CPU_HISTORY_LEN.
    /// Stored as u64 (× 10 for one decimal of precision) because that's
    /// what ratatui::Sparkline takes; render divides by 10.
    pub cpu_history: HashMap<String, VecDeque<u64>>,
    /// Flag drained by the main loop: when true, spawn an async
    /// pgforge self-update check (curl latest binary + atomic
    /// replace). Set by the `[u]` keybind.
    pub pending_self_update: bool,
    pub snapshots: HashMap<String, SnapshotsView>,
    pub in_progress: HashMap<String, RunningOp>,
    pub selected: usize,
    pub modal: Option<Modal>,
    pub last_op_error: Option<OpError>,
    pub flash: Option<Flash>,
    pub stale_status: HashSet<String>,
    pub now: Instant,
    pub should_quit: bool,
    pub pending_ops: Vec<(String, OpKind)>,
    pub pending_clipboard: Vec<String>,
    pub refresh_requests: Vec<String>,
    /// Drained by the main loop and dispatched to `ops::spawn_create`.
    /// Separate from `pending_ops` because Create takes more args than
    /// (encoded_string, kind).
    pub pending_creates: Vec<CreateRequest>,
    /// Names whose post-Create CreatedSuccess modal still needs to be
    /// opened. Drained by the main loop, which loads state.toml to
    /// build the connection URI before showing the modal — kept out of
    /// `apply_event` to preserve its purity.
    pub pending_show_created: Vec<String>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            instances: Vec::new(),
            statuses: HashMap::new(),
            snapshots: HashMap::new(),
            in_progress: HashMap::new(),
            selected: 0,
            modal: None,
            last_op_error: None,
            flash: None,
            stale_status: HashSet::new(),
            now: Instant::now(),
            should_quit: false,
            pending_ops: Vec::new(),
            pending_clipboard: Vec::new(),
            refresh_requests: Vec::new(),
            pending_creates: Vec::new(),
            pending_show_created: Vec::new(),
            cpu_history: HashMap::new(),
            pending_self_update: false,
        }
    }
}

impl AppState {
    /// Name of the currently selected instance, or None if the list is empty.
    pub fn selected_name(&self) -> Option<&str> {
        self.instances.get(self.selected).map(|i| i.name.as_str())
    }

    /// Selected instance row.
    pub fn selected_instance(&self) -> Option<&InstanceSummary> {
        self.instances.get(self.selected)
    }

    pub fn apply_event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) => self.handle_key(k),
            Event::InstancesListed(rows) => {
                let prev_name = self.selected_name().map(str::to_string);
                self.instances = rows;
                // Re-anchor selection by name if possible, else clamp.
                if let Some(n) = prev_name
                    && let Some(i) = self.instances.iter().position(|r| r.name == n) {
                    self.selected = i;
                    return;
                }
                if self.instances.is_empty() {
                    self.selected = 0;
                } else if self.selected >= self.instances.len() {
                    self.selected = self.instances.len() - 1;
                }
            }
            Event::StatusRefreshed { name, status } => {
                self.stale_status.remove(&name);
                // Heartbeat detection: previous run was up, this one is
                // down → loud flash so user notices the crash even if
                // they're not looking at the right pane.
                let prev_running = self.statuses.get(&name).map(|s| s.running).unwrap_or(false);
                if prev_running && !status.running {
                    self.last_op_error = Some(OpError {
                        instance: name.clone(),
                        kind: OpKind::Snapshot, // placeholder; no dedicated Watchdog kind
                        msg: format!("{name} went down (container is no longer running)"),
                        at: Instant::now(),
                    });
                }
                // Sparkline history — append latest CPU sample, drop oldest
                // beyond CPU_HISTORY_LEN. Store stopped instances as 0
                // so the chart doesn't go blank during transient
                // restarts.
                let sample = status.cpu_percent.map(|p| (p * 10.0) as u64).unwrap_or(0);
                let hist = self.cpu_history.entry(name.clone()).or_default();
                hist.push_back(sample);
                while hist.len() > CPU_HISTORY_LEN { hist.pop_front(); }
                self.statuses.insert(name, status);
            }
            Event::SnapshotsRefreshed { name, view } => {
                self.snapshots.insert(name, view);
            }
            Event::SelfUpdateDone { upgraded, latest_tag, current_version } => {
                let msg = if upgraded {
                    format!("pgforge updated {current_version} → {latest_tag}. Restart pgforge to use the new binary.")
                } else {
                    format!("pgforge already on {current_version} (latest is {latest_tag})")
                };
                self.flash = Some(Flash {
                    msg,
                    kind: FlashKind::Success,
                    at: Instant::now(),
                });
            }
            Event::SelfUpdateFailed { msg } => {
                self.last_op_error = Some(OpError {
                    instance: "pgforge".into(),
                    kind: OpKind::Snapshot, // no dedicated SelfUpdate kind for OpError
                    msg: format!("self-update failed: {msg}"),
                    at: Instant::now(),
                });
            }
            Event::RefreshFailed { name, err } => {
                tracing::warn!(target: "pgforge::tui", "refresh failed for {name}: {err}");
                self.stale_status.insert(name);
            }
            Event::OpStarted { instance, kind } => {
                self.in_progress.insert(
                    instance,
                    RunningOp { kind, started_at: Instant::now() },
                );
            }
            Event::OpFinished { instance, kind, result } => {
                self.in_progress.remove(&instance);
                match result {
                    Ok(()) => {
                        self.flash = Some(Flash {
                            msg: format!("{} on {} done", kind.label(), instance),
                            kind: FlashKind::Success,
                            at: Instant::now(),
                        });
                        // Destroyed instances have no state to refresh.
                        if kind != OpKind::Destroy {
                            self.refresh_requests.push(instance.clone());
                        }
                        // Newly-created instances trigger a one-time
                        // CreatedSuccess modal so the user can copy
                        // the URI before it lives only in state.toml.
                        if kind == OpKind::Create {
                            self.pending_show_created.push(instance.clone());
                        }
                    }
                    Err(msg) => {
                        self.last_op_error = Some(OpError {
                            instance, kind, msg, at: Instant::now(),
                        });
                    }
                }
            }
            Event::Tick => {
                self.now = Instant::now();
                if let Some(f) = &self.flash
                    && self.now.duration_since(f.at) > std::time::Duration::from_secs(3) {
                    self.flash = None;
                }
                // last_op_error is sticky — user clears with Esc.
            }
        }
    }

    fn handle_key(&mut self, k: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        if self.modal.is_some() {
            self.handle_modal_key(k);
            return;
        }
        // No modal — global keymap.
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected + 1 < self.instances.len() {
                    self.selected += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc => { self.last_op_error = None; }
            KeyCode::Char('s') => {
                if let Some(n) = self.selected_name().map(str::to_string)
                    && !self.in_progress.contains_key(&n) {
                    self.pending_ops.push((n, OpKind::Snapshot));
                }
            }
            KeyCode::Char('c') => {
                if let Some(n) = self.selected_name().map(str::to_string) {
                    self.modal = Some(Modal::CloneAs { source: n, input: TextField::default() });
                }
            }
            KeyCode::Char('t') => {
                // Auto-snapshot time editor. Reads current value off the
                // freshly-loaded state.toml (live values aren't on the
                // ls summary). Empty list → no-op.
                if let Some(n) = self.selected_name().map(str::to_string) {
                    let root = crate::state::instance::InstanceState::default_state_root();
                    if let Ok(state) = crate::state::instance::InstanceState::load_under(&root, &n) {
                        let cur = state.instance.snapshot_hour;
                        self.modal = Some(Modal::ScheduleEdit {
                            name: n,
                            current: cur,
                            new: cur,
                        });
                    }
                }
            }
            KeyCode::Char('p') => {
                // Preset resize wizard — cycle to a different tuning
                // (tiny/small/medium/large). Preview parameters live
                // in the modal; submit transitions to Confirm.
                if let Some(inst) = self.selected_instance() {
                    use std::str::FromStr;
                    if let Ok(cur) = crate::domain::preset::Preset::from_str(&inst.preset_label) {
                        let new = next_preset(cur);
                        self.modal = Some(Modal::ResizeTo {
                            name: inst.name.clone(),
                            current: cur,
                            new,
                        });
                    }
                }
            }
            KeyCode::Char('u') => {
                // pg_upgrade (major-version pg) is rare enough that it
                // lives only in the CLI now. `[u]` in the TUI means
                // "self-update pgforge from GitHub releases".
                if !self.pending_self_update {
                    self.pending_self_update = true;
                    self.flash = Some(Flash {
                        msg: "checking for pgforge update…".into(),
                        kind: FlashKind::Info,
                        at: Instant::now(),
                    });
                }
            }
            KeyCode::Char('r') => {
                if let Some(n) = self.selected_name().map(str::to_string) {
                    let pitr_earliest = self
                        .snapshots
                        .get(&n)
                        .and_then(|v| v.pitr.earliest.clone());
                    // Also capture container uptime — fresh instances
                    // without any full backup have no PITR window yet,
                    // but they STILL have an absolute upper bound on
                    // "how far back can you restore": the container
                    // birth. submit_modal will use whichever cap is
                    // tighter.
                    let uptime_min = self
                        .statuses
                        .get(&n)
                        .and_then(|s| s.uptime_seconds)
                        .map(|s| (s / 60) as u32);
                    self.modal = Some(Modal::RestoreAs {
                        source: n,
                        as_input: TextField::default(),
                        minutes_ago: 0,
                        focus: 0,
                        pitr_earliest,
                        uptime_cap_min: uptime_min,
                    });
                }
            }
            KeyCode::Char('R') => {
                if let Some(n) = self.selected_name().map(str::to_string) {
                    self.modal = Some(Modal::Confirm {
                        kind: PendingDestructiveOp::Rotate { name: n.clone() },
                        prompt: format!("Rotate {n}? Container restarts (~10s downtime), volume preserved."),
                    });
                }
            }
            KeyCode::Char('d') => {
                if let Some(n) = self.selected_name().map(str::to_string) {
                    self.modal = Some(Modal::Confirm {
                        kind: PendingDestructiveOp::Destroy { name: n.clone(), delete_backups: false },
                        prompt: format!(
                            "Destroy {n}? This drops the container AND its data volume. S3 backups are RETAINED — restore later with `pgforge restore`. Press [D] (shift) to also wipe S3 backups."
                        ),
                    });
                }
            }
            KeyCode::Char('D') => {
                if let Some(n) = self.selected_name().map(str::to_string) {
                    self.modal = Some(Modal::Confirm {
                        kind: PendingDestructiveOp::Destroy { name: n.clone(), delete_backups: true },
                        prompt: format!(
                            "Destroy {n} + DELETE ALL S3 BACKUPS? This is permanent: container, volume, full backups, WAL archives, PITR window — all gone. No recovery."
                        ),
                    });
                }
            }
            KeyCode::Char('?') => {
                if let Some(e) = &self.last_op_error {
                    self.modal = Some(Modal::ErrorDetail { msg: e.msg.clone() });
                }
            }
            KeyCode::Char('e') => {
                if let Some(n) = self.selected_name().map(str::to_string)
                    && let Some(v) = self.snapshots.get(&n).cloned() {
                    self.modal = Some(Modal::Snapshots { name: n, view: v });
                }
            }
            KeyCode::Enter => {
                if let Some(n) = self.selected_name().map(str::to_string) {
                    self.pending_clipboard.push(n);
                }
            }
            KeyCode::Char('n') => {
                // Open the Create wizard with pre-generated defaults.
                use crate::util::random;
                let mut name = random::instance_name();
                // Avoid collisions with existing instances (extremely unlikely
                // but cheap to be safe — 4 hex × random = ~65k space).
                let existing: std::collections::HashSet<&str> =
                    self.instances.iter().map(|r| r.name.as_str()).collect();
                for _ in 0..16 {
                    if !existing.contains(name.as_str()) { break; }
                    name = random::instance_name();
                }
                let name_field = TextField { buf: name, cursor: 0 };
                let app_user_field = TextField { buf: "app".into(), cursor: 0 };
                let pg_field = TextField { buf: "18".into(), cursor: 0 };
                self.modal = Some(Modal::Create {
                    name: name_field,
                    app_user: app_user_field,
                    pg_version: pg_field,
                    preset: crate::domain::preset::Preset::Small,
                    no_backup: true, // safer default — S3 isn't configured on most fresh setups
                    retain_days: 30,
                    snapshot_hour: Some(3),
                    focus: 0,
                    generated_password: random::password(24),
                    generated_pgbackrest_password: random::password(24),
                });
            }
            _ => {}
        }
    }

    fn handle_modal_key(&mut self, k: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        if k.code == KeyCode::Esc { self.modal = None; return; }

        // Decide what action to take, dropping the borrow before mutating elsewhere.
        enum Action { Nothing, Submit, ConfirmYes, ConfirmNo }
        let action = match &mut self.modal {
            Some(Modal::CloneAs { input, .. }) | Some(Modal::UpgradeTo { input, .. }) => match k.code {
                KeyCode::Char(c) if !c.is_control() => { input.insert_char(c); Action::Nothing }
                KeyCode::Backspace => { input.backspace(); Action::Nothing }
                KeyCode::Left => { input.move_left(); Action::Nothing }
                KeyCode::Right => { input.move_right(); Action::Nothing }
                KeyCode::Enter => Action::Submit,
                _ => Action::Nothing,
            },
            Some(Modal::RestoreAs { as_input, minutes_ago, focus, pitr_earliest, uptime_cap_min, .. }) => {
                let cap = effective_restore_cap(pitr_earliest.as_deref(), *uptime_cap_min);
                let clamp = |m: u32| -> u32 { m.min(cap) };
                match k.code {
                    KeyCode::Tab => { *focus = (*focus + 1) % 2; Action::Nothing }
                    KeyCode::Enter => Action::Submit,
                    _ if *focus == 0 => match k.code {
                        KeyCode::Char(c) if !c.is_control() => { as_input.insert_char(c); Action::Nothing }
                        KeyCode::Backspace => { as_input.backspace(); Action::Nothing }
                        KeyCode::Left  => { as_input.move_left();  Action::Nothing }
                        KeyCode::Right => { as_input.move_right(); Action::Nothing }
                        _ => Action::Nothing,
                    },
                    // focus == 1: minutes_ago picker. ← decrements (saturating
                    // at 0 = "latest"), → increments. Space cycles forward in
                    // bigger steps; digit keys append. All bumps clamped to
                    // the PITR window (no point picking a time before the
                    // earliest full backup).
                    _ => match k.code {
                        KeyCode::Left  => { *minutes_ago = minutes_ago.saturating_sub(1); Action::Nothing }
                        KeyCode::Right => { *minutes_ago = clamp(minutes_ago.saturating_add(1)); Action::Nothing }
                        KeyCode::Char(' ') => { *minutes_ago = clamp(minutes_ago.saturating_add(5)); Action::Nothing }
                        KeyCode::Backspace => { *minutes_ago /= 10; Action::Nothing }
                        KeyCode::Char(d) if d.is_ascii_digit() => {
                            let v = d.to_digit(10).unwrap();
                            *minutes_ago = clamp(minutes_ago.saturating_mul(10).saturating_add(v));
                            Action::Nothing
                        }
                        _ => Action::Nothing,
                    }
                }
            },
            Some(Modal::Confirm { kind, prompt }) => match k.code {
                KeyCode::Char('y') | KeyCode::Enter => Action::ConfirmYes,
                KeyCode::Char('n') => Action::ConfirmNo,
                // Shift+D upgrades a Destroy confirm to the
                // wipe-S3-backups variant in-place. Documented in the
                // initial prompt. Other destructive kinds ignore it.
                KeyCode::Char('D') => {
                    if let PendingDestructiveOp::Destroy { name, delete_backups } = kind
                        && !*delete_backups {
                        *delete_backups = true;
                        *prompt = format!(
                            "Destroy {name} + DELETE ALL S3 BACKUPS? This is permanent: container, volume, full backups, WAL archives, PITR window — all gone. No recovery."
                        );
                    }
                    Action::Nothing
                }
                _ => Action::Nothing,
            },
            Some(Modal::Create { name, app_user, pg_version, preset, no_backup, retain_days, snapshot_hour, focus, .. }) => {
                // 7 fields, cycle with Tab. ←→/space cycle the cycler fields.
                match k.code {
                    KeyCode::Tab       => { *focus = (*focus + 1) % 7; Action::Nothing }
                    KeyCode::BackTab   => { *focus = (*focus + 6) % 7; Action::Nothing }
                    KeyCode::Enter     => Action::Submit,
                    KeyCode::Char(' ') => {
                        match *focus {
                            3 => { *preset = next_preset(*preset); Action::Nothing }
                            4 => { *no_backup = !*no_backup; Action::Nothing }
                            5 => { *retain_days = retain_days.saturating_add(1); Action::Nothing }
                            6 => { *snapshot_hour = bump_snapshot_hour(*snapshot_hour, 1); Action::Nothing }
                            0 => { name.insert_char(' '); Action::Nothing }
                            1 => { app_user.insert_char(' '); Action::Nothing }
                            2 => { pg_version.insert_char(' '); Action::Nothing }
                            _ => Action::Nothing,
                        }
                    }
                    KeyCode::Left | KeyCode::Right if *focus == 3 => {
                        *preset = match k.code {
                            KeyCode::Right => next_preset(*preset),
                            _              => prev_preset(*preset),
                        };
                        Action::Nothing
                    }
                    KeyCode::Left | KeyCode::Right if *focus == 5 => {
                        if k.code == KeyCode::Right {
                            *retain_days = retain_days.saturating_add(1);
                        } else {
                            *retain_days = retain_days.saturating_sub(1);
                        }
                        Action::Nothing
                    }
                    KeyCode::Left | KeyCode::Right if *focus == 6 => {
                        let delta = if k.code == KeyCode::Right { 1 } else { -1 };
                        *snapshot_hour = bump_snapshot_hour(*snapshot_hour, delta);
                        Action::Nothing
                    }
                    KeyCode::Char(d) if *focus == 5 && d.is_ascii_digit() => {
                        let v = d.to_digit(10).unwrap();
                        *retain_days = retain_days.saturating_mul(10).saturating_add(v);
                        Action::Nothing
                    }
                    KeyCode::Char(d) if *focus == 6 && d.is_ascii_digit() => {
                        let v = d.to_digit(10).unwrap();
                        let cur = snapshot_hour.unwrap_or(0) as u32;
                        let next = cur.saturating_mul(10).saturating_add(v).min(23) as u8;
                        *snapshot_hour = Some(next);
                        Action::Nothing
                    }
                    KeyCode::Backspace if *focus == 5 => {
                        *retain_days /= 10;
                        Action::Nothing
                    }
                    KeyCode::Backspace if *focus == 6 => {
                        // Cycle hour → off → 23 → 22 → … via backspace? Keep simple: set None.
                        *snapshot_hour = None;
                        Action::Nothing
                    }
                    KeyCode::Char(c) if !c.is_control() => {
                        let field = match *focus {
                            0 => Some(&mut *name),
                            1 => Some(&mut *app_user),
                            2 => Some(&mut *pg_version),
                            _ => None,
                        };
                        if let Some(f) = field { f.insert_char(c); }
                        Action::Nothing
                    }
                    KeyCode::Backspace => {
                        let field = match *focus {
                            0 => Some(&mut *name),
                            1 => Some(&mut *app_user),
                            2 => Some(&mut *pg_version),
                            _ => None,
                        };
                        if let Some(f) = field { f.backspace(); }
                        Action::Nothing
                    }
                    KeyCode::Left | KeyCode::Right => {
                        let field = match *focus {
                            0 => Some(&mut *name),
                            1 => Some(&mut *app_user),
                            2 => Some(&mut *pg_version),
                            _ => None,
                        };
                        if let Some(f) = field {
                            if k.code == KeyCode::Left { f.move_left(); } else { f.move_right(); }
                        }
                        Action::Nothing
                    }
                    _ => Action::Nothing,
                }
            }
            Some(Modal::CreatedSuccess { .. }) | Some(Modal::ConnectionString { .. }) => {
                // No interaction beyond Esc (handled at top of this fn).
                // User selects URI text with mouse and Cmd+C / Ctrl+C —
                // every terminal supports that, no escape-sequence games.
                Action::Nothing
            }
            Some(Modal::ResizeTo { new, .. }) => match k.code {
                KeyCode::Char(' ') | KeyCode::Right => { *new = next_preset(*new); Action::Nothing }
                KeyCode::Left => { *new = prev_preset(*new); Action::Nothing }
                KeyCode::Enter => Action::Submit,
                _ => Action::Nothing,
            },
            Some(Modal::ScheduleEdit { new, .. }) => match k.code {
                KeyCode::Left  => { *new = bump_snapshot_hour(*new, -1); Action::Nothing }
                KeyCode::Right | KeyCode::Char(' ') => { *new = bump_snapshot_hour(*new, 1); Action::Nothing }
                KeyCode::Char(d) if d.is_ascii_digit() => {
                    let v = d.to_digit(10).unwrap();
                    let cur = new.unwrap_or(0) as u32;
                    let next = cur.saturating_mul(10).saturating_add(v).min(23) as u8;
                    *new = Some(next);
                    Action::Nothing
                }
                KeyCode::Backspace => { *new = None; Action::Nothing }
                KeyCode::Enter => Action::Submit,
                _ => Action::Nothing,
            },
            Some(Modal::Snapshots { .. }) | Some(Modal::ErrorDetail { .. }) | None => Action::Nothing,
        };
        match action {
            Action::Nothing => {}
            Action::Submit => self.submit_modal(),
            Action::ConfirmYes => self.confirm_modal(true),
            Action::ConfirmNo => self.confirm_modal(false),
        }
    }

    fn submit_modal(&mut self) {
        let taken = self.modal.take();
        match taken {
            Some(Modal::CloneAs { source, input }) => {
                match validate_instance_name(&input.buf) {
                    Ok(()) => self.pending_ops.push((format!("{source}:{}", input.buf), OpKind::Clone)),
                    Err(msg) => {
                        self.last_op_error = Some(OpError {
                            instance: source.clone(), kind: OpKind::Clone, msg, at: Instant::now(),
                        });
                        // Put it back so user can fix.
                        self.modal = Some(Modal::CloneAs { source, input });
                    }
                }
            }
            Some(Modal::UpgradeTo { source, input }) => {
                let current = self.instances.iter()
                    .find(|r| r.name == source).map(|r| r.pg_version).unwrap_or(0);
                match input.buf.parse::<u8>() {
                    Ok(to) if to > current => {
                        self.modal = Some(Modal::Confirm {
                            kind: PendingDestructiveOp::Upgrade { name: source.clone(), to },
                            prompt: format!(
                                "Upgrade {source} from PG{current} to PG{to}? Takes several minutes; an auto pre-snapshot is taken first."
                            ),
                        });
                    }
                    Ok(to) => {
                        self.last_op_error = Some(OpError {
                            instance: source.clone(), kind: OpKind::Upgrade,
                            msg: format!("target version {to} must be greater than current PG{current}"),
                            at: Instant::now(),
                        });
                        self.modal = Some(Modal::UpgradeTo { source, input });
                    }
                    Err(_) => {
                        self.last_op_error = Some(OpError {
                            instance: source.clone(), kind: OpKind::Upgrade,
                            msg: format!("invalid version: {:?}", input.buf), at: Instant::now(),
                        });
                        self.modal = Some(Modal::UpgradeTo { source, input });
                    }
                }
            }
            Some(Modal::RestoreAs { source, as_input, minutes_ago, focus, pitr_earliest, uptime_cap_min }) => {
                let as_ = as_input.buf.clone();
                if let Err(msg) = validate_instance_name(&as_) {
                    self.last_op_error = Some(OpError {
                        instance: source.clone(), kind: OpKind::Restore, msg, at: Instant::now(),
                    });
                    self.modal = Some(Modal::RestoreAs { source, as_input, minutes_ago, focus, pitr_earliest, uptime_cap_min });
                    return;
                }
                // 0 = "latest archived WAL" — no --target-time. Else compute
                // an absolute timestamp by subtracting from now() and feed it
                // to pgbackrest as RFC3339.
                let tt = if minutes_ago == 0 {
                    None
                } else {
                    let delta = jiff::SignedDuration::from_secs((minutes_ago as i64) * 60);
                    let target = jiff::Timestamp::now() - delta;
                    Some(target.to_string())
                };
                let target_display = match &tt {
                    Some(t) => format!("at {t}"),
                    None    => "at latest archived WAL".to_string(),
                };
                self.modal = Some(Modal::Confirm {
                    kind: PendingDestructiveOp::Restore {
                        source: source.clone(),
                        as_: as_.clone(),
                        target_time: tt,
                    },
                    prompt: format!("Restore {source} → new instance {as_} {target_display}? Takes minutes."),
                });
            }
            Some(Modal::Create {
                name, app_user, pg_version, preset, no_backup, retain_days, snapshot_hour, focus,
                generated_password, generated_pgbackrest_password,
            }) => {
                let fail = |state: &mut AppState, msg: String,
                            name, app_user, pg_version, preset, no_backup, retain_days, snapshot_hour, focus,
                            generated_password, generated_pgbackrest_password| {
                    state.last_op_error = Some(OpError {
                        instance: "create wizard".into(),
                        kind: OpKind::Create,
                        msg,
                        at: Instant::now(),
                    });
                    state.modal = Some(Modal::Create {
                        name, app_user, pg_version, preset, no_backup, retain_days, snapshot_hour, focus,
                        generated_password, generated_pgbackrest_password,
                    });
                };

                if let Err(msg) = validate_instance_name(&name.buf) {
                    fail(self, format!("instance name: {msg}"),
                         name, app_user, pg_version, preset, no_backup, retain_days, snapshot_hour, focus,
                         generated_password, generated_pgbackrest_password);
                    return;
                }
                let ver = match pg_version.buf.parse::<u8>() {
                    Ok(v) if v > 0 => v,
                    _ => {
                        fail(self, format!("invalid pg version {:?} — expected 13..=18", pg_version.buf),
                             name, app_user, pg_version, preset, no_backup, retain_days, snapshot_hour, focus,
                             generated_password, generated_pgbackrest_password);
                        return;
                    }
                };
                if app_user.buf.is_empty() {
                    fail(self, "App user cannot be empty.".into(),
                         name, app_user, pg_version, preset, no_backup, retain_days, snapshot_hour, focus,
                         generated_password, generated_pgbackrest_password);
                    return;
                }
                if let Some(bad) = app_user.buf.chars()
                    .find(|c| !(c.is_ascii_alphanumeric() || *c == '_'))
                {
                    let printable = if bad == ' ' { "space".to_string() } else { format!("{:?}", bad) };
                    fail(self, format!(
                        "App user {:?} has invalid char {} — only letters, digits and `_` are allowed (no `-`, no spaces).",
                        app_user.buf, printable,
                    ), name, app_user, pg_version, preset, no_backup, retain_days, snapshot_hour, focus,
                       generated_password, generated_pgbackrest_password);
                    return;
                }
                self.pending_creates.push(CreateRequest {
                    name: name.buf.clone(),
                    app_user: app_user.buf.clone(),
                    app_password: generated_password,
                    pgbackrest_password: generated_pgbackrest_password,
                    pg_version: ver,
                    preset,
                    no_backup,
                    retain_days,
                    snapshot_hour,
                });
            }
            Some(Modal::ScheduleEdit { name, current, new }) => {
                if current == new {
                    // No change — just close.
                    self.modal = None;
                    return;
                }
                // Persist the change to state.toml. This is sync I/O but
                // small (single TOML write under the state-root lock),
                // acceptable in apply_event. update_under re-loads inside the
                // lock so a concurrent launchd snapshot tick can't be
                // clobbered (and vice versa).
                let root = crate::state::instance::InstanceState::default_state_root();
                match crate::state::instance::InstanceState::update_under(&root, &name, |s| {
                    s.instance.snapshot_hour = new;
                    Ok(())
                }) {
                    Ok(_) => {
                        let msg = match new {
                            Some(h) => format!("auto-snapshot for {name}: {:02}:00 local", h),
                            None    => format!("auto-snapshot for {name}: disabled (manual only)"),
                        };
                        self.flash = Some(Flash {
                            msg, kind: FlashKind::Success, at: Instant::now(),
                        });
                    }
                    Err(e) => {
                        self.last_op_error = Some(OpError {
                            instance: name.clone(),
                            kind: OpKind::Snapshot,
                            msg: format!("update state.toml: {e}"),
                            at: Instant::now(),
                        });
                    }
                }
            }
            Some(Modal::ResizeTo { name, current, new }) => {
                if current == new {
                    self.last_op_error = Some(OpError {
                        instance: name.clone(),
                        kind: OpKind::Resize,
                        msg: format!("already on preset {:?} — cycle ← → to pick a different one", current),
                        at: Instant::now(),
                    });
                    self.modal = Some(Modal::ResizeTo { name, current, new });
                    return;
                }
                self.modal = Some(Modal::Confirm {
                    kind: PendingDestructiveOp::Resize { name: name.clone(), new_preset: new },
                    prompt: format!(
                        "Resize {name} from {current:?} to {new:?}? Container will be recreated with the new memory limit (~10s downtime); volume preserved.",
                    ),
                });
            }
            Some(other) => { self.modal = Some(other); /* should not happen */ }
            None => {}
        }
    }

    fn confirm_modal(&mut self, yes: bool) {
        let taken = self.modal.take();
        if !yes { return; }
        if let Some(Modal::Confirm { kind, .. }) = taken {
            match kind {
                PendingDestructiveOp::Rotate { name } => {
                    self.pending_ops.push((name, OpKind::Rotate));
                }
                PendingDestructiveOp::Upgrade { name, to } => {
                    self.pending_ops.push((format!("{name}@{to}"), OpKind::Upgrade));
                }
                PendingDestructiveOp::Restore { source, as_, target_time } => {
                    let key = match target_time {
                        Some(t) => format!("{source}:{as_}@{t}"),
                        None    => format!("{source}:{as_}"),
                    };
                    self.pending_ops.push((key, OpKind::Restore));
                }
                PendingDestructiveOp::Destroy { name, delete_backups } => {
                    // Encoding: "name" = keep S3 backups; "name@delete" = wipe.
                    // ops::spawn decodes via parse_at — a literal "delete" suffix
                    // can't collide with a u8 version (the only other thing that
                    // uses @) because OpKind tags the destination dispatcher.
                    let key = if delete_backups { format!("{name}@delete") } else { name };
                    self.pending_ops.push((key, OpKind::Destroy));
                }
                PendingDestructiveOp::Resize { name, new_preset } => {
                    // Encoding: "name@<preset-name>" — ops::spawn maps the
                    // preset string back via FromStr.
                    let p = format!("{:?}", new_preset).to_lowercase();
                    self.pending_ops.push((format!("{name}@{p}"), OpKind::Resize));
                }
            }
        }
    }
}

/// Validator reused by clone+restore. Delegates to
/// `domain::instance::Instance::validate_name` (single source of truth).
/// Resolve PITR earliest (RFC3339) into "max minutes_ago" the user can
/// pick in the Restore wizard, so they can't request a target_time
/// before the oldest available backup. None when there's no PITR data
/// yet — in that case we don't cap (defensive; submit_modal would just
/// pass the raw timestamp through to pgbackrest which would error out
/// loudly).
/// Resolve the maximum sensible "minutes ago" the Restore picker
/// should allow. Combines two upper bounds, picking the tighter one:
///
///   - PITR earliest (if pgbackrest has a full backup yet): you can't
///     restore to a time before the oldest full.
///   - Container uptime (always): you can't restore to a time before
///     the container itself existed — there's literally no data.
///
/// Returns 0 when neither bound is known AND nothing is finite yet —
/// the only safe value being "latest archived WAL".
pub(crate) fn effective_restore_cap(earliest: Option<&str>, uptime_min: Option<u32>) -> u32 {
    let from_pitr = pitr_max_minutes_ago(earliest);
    match (from_pitr, uptime_min) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None)    => a,
        (None, Some(b))    => b,
        (None, None)       => 0,
    }
}

pub(crate) fn pitr_max_minutes_ago(earliest: Option<&str>) -> Option<u32> {
    use std::str::FromStr;
    let e = earliest?;
    let earliest = jiff::Timestamp::from_str(e).ok()?;
    let now = jiff::Timestamp::now();
    let secs = earliest.duration_until(now).as_secs();
    if secs <= 0 { return Some(0); }
    Some((secs as u64 / 60) as u32)
}

/// Cycle snapshot_hour through None (off) → 0 → 1 → … → 23 → None
/// when delta is ±1. None acts as a "before 0 / after 23" sentinel.
fn bump_snapshot_hour(cur: Option<u8>, delta: i32) -> Option<u8> {
    match (cur, delta) {
        (None, d) if d > 0 => Some(0),
        (None, _)          => Some(23),
        (Some(h), d) if d > 0 => {
            if h >= 23 { None } else { Some(h + 1) }
        }
        (Some(h), _) => {
            if h == 0 { None } else { Some(h - 1) }
        }
    }
}

fn next_preset(p: crate::domain::preset::Preset) -> crate::domain::preset::Preset {
    use crate::domain::preset::Preset::*;
    match p { Tiny => Small, Small => Medium, Medium => Large, Large => Tiny }
}

fn prev_preset(p: crate::domain::preset::Preset) -> crate::domain::preset::Preset {
    use crate::domain::preset::Preset::*;
    match p { Tiny => Large, Small => Tiny, Medium => Small, Large => Medium }
}

fn validate_instance_name(s: &str) -> std::result::Result<(), String> {
    use crate::domain::instance::Instance;
    Instance::validate_name(s).map_err(|e| e.to_string())
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)] // reason: test helpers use mutation for readability
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// Convenience: build a minimal InstanceSummary for tests.
    pub(crate) fn row(name: &str) -> InstanceSummary {
        InstanceSummary {
            name: name.to_string(),
            pg_version: 18,
            preset_label: "tiny".to_string(),
            host_port: 5432,
            backup_enabled: true,
            backup_failing: false,
            running: true,
        }
    }

    fn key(code: KeyCode) -> Event { Event::Key(KeyEvent::new(code, KeyModifiers::NONE)) }

    #[test]
    fn default_state_is_empty_and_not_quitting() {
        let s = AppState::default();
        assert!(s.instances.is_empty());
        assert_eq!(s.selected, 0);
        assert!(s.modal.is_none());
        assert!(s.flash.is_none());
        assert!(s.last_op_error.is_none());
        assert!(!s.should_quit);
    }

    #[test]
    fn selected_name_on_empty_list_returns_none() {
        let s = AppState::default();
        assert!(s.selected_name().is_none());
    }

    // --- Task 2.1: Navigation keys ---

    #[test]
    fn down_arrow_moves_selection_forward() {
        let mut s = AppState::default();
        s.instances = vec![row("a"), row("b"), row("c")];
        s.apply_event(key(KeyCode::Down));
        assert_eq!(s.selected, 1);
        s.apply_event(key(KeyCode::Down));
        assert_eq!(s.selected, 2);
        s.apply_event(key(KeyCode::Down));
        assert_eq!(s.selected, 2, "selected clamps at last");
    }

    #[test]
    fn up_arrow_and_k_move_back() {
        let mut s = AppState::default();
        s.instances = vec![row("a"), row("b"), row("c")];
        s.selected = 2;
        s.apply_event(key(KeyCode::Up));
        assert_eq!(s.selected, 1);
        s.apply_event(key(KeyCode::Char('k')));
        assert_eq!(s.selected, 0);
        s.apply_event(key(KeyCode::Up));
        assert_eq!(s.selected, 0, "clamps at zero");
    }

    #[test]
    fn j_moves_down_like_down_arrow() {
        let mut s = AppState::default();
        s.instances = vec![row("a"), row("b")];
        s.apply_event(key(KeyCode::Char('j')));
        assert_eq!(s.selected, 1);
    }

    #[test]
    fn q_sets_should_quit() {
        let mut s = AppState::default();
        s.apply_event(key(KeyCode::Char('q')));
        assert!(s.should_quit);
    }

    #[test]
    fn esc_with_no_modal_does_not_quit() {
        let mut s = AppState::default();
        s.apply_event(key(KeyCode::Esc));
        assert!(!s.should_quit);
    }

    // --- Task 2.2: InstancesListed merge ---

    #[test]
    fn instances_listed_replaces_list_and_preserves_selection_by_name() {
        let mut s = AppState::default();
        s.instances = vec![row("a"), row("b"), row("c")];
        s.selected = 1; // "b" selected
        s.apply_event(Event::InstancesListed(vec![row("a"), row("b"), row("d")]));
        assert_eq!(s.instances.iter().map(|i| &i.name).collect::<Vec<_>>(),
                   vec!["a", "b", "d"]);
        assert_eq!(s.selected, 1, "still on b");
    }

    #[test]
    fn instances_listed_clamps_selection_when_list_shrinks() {
        let mut s = AppState::default();
        s.instances = vec![row("a"), row("b"), row("c")];
        s.selected = 2; // "c"
        s.apply_event(Event::InstancesListed(vec![row("a"), row("b")]));
        assert_eq!(s.selected, 1, "clamped to last");
    }

    #[test]
    fn instances_listed_resets_to_zero_when_list_empty() {
        let mut s = AppState::default();
        s.instances = vec![row("a")];
        s.selected = 0;
        s.apply_event(Event::InstancesListed(vec![]));
        assert_eq!(s.selected, 0);
        assert!(s.instances.is_empty());
    }

    // --- Task 2.3: StatusRefreshed / SnapshotsRefreshed / RefreshFailed ---

    fn empty_status(name: &str) -> InstanceStatus {
        InstanceStatus {
            name: name.into(), running: true, host_port: 5432,
            ..Default::default()
        }
    }

    #[test]
    fn status_refreshed_inserts_and_clears_stale() {
        let mut s = AppState::default();
        s.stale_status.insert("a".into());
        s.apply_event(Event::StatusRefreshed { name: "a".into(), status: empty_status("a") });
        assert!(s.statuses.contains_key("a"));
        assert!(!s.stale_status.contains("a"), "successful refresh clears stale flag");
    }

    #[test]
    fn snapshots_refreshed_inserts() {
        let mut s = AppState::default();
        s.apply_event(Event::SnapshotsRefreshed {
            name: "a".into(),
            view: SnapshotsView { list: Vec::new(), pitr: Default::default() },
        });
        assert!(s.snapshots.contains_key("a"));
    }

    #[test]
    fn refresh_failed_marks_stale_without_touching_statuses() {
        let mut s = AppState::default();
        s.statuses.insert("a".into(), empty_status("a"));
        s.apply_event(Event::RefreshFailed { name: "a".into(), err: "boom".into() });
        assert!(s.stale_status.contains("a"));
        assert!(s.statuses.contains_key("a"), "stale doesn't drop the last-good status");
    }

    // --- Task 2.4: OpStarted / OpFinished lifecycle ---

    #[test]
    fn op_started_takes_per_instance_lock() {
        let mut s = AppState::default();
        s.apply_event(Event::OpStarted { instance: "a".into(), kind: OpKind::Snapshot });
        assert_eq!(s.in_progress.get("a").map(|r| r.kind), Some(OpKind::Snapshot));
    }

    #[test]
    fn op_finished_ok_clears_lock_and_sets_flash() {
        let mut s = AppState::default();
        s.in_progress.insert(
            "a".into(),
            RunningOp { kind: OpKind::Snapshot, started_at: Instant::now() },
        );
        s.apply_event(Event::OpFinished {
            instance: "a".into(),
            kind: OpKind::Snapshot,
            result: Ok(()),
        });
        assert!(!s.in_progress.contains_key("a"));
        assert!(matches!(s.flash, Some(Flash { kind: FlashKind::Success, .. })));
        assert!(s.last_op_error.is_none());
    }

    #[test]
    fn op_finished_err_clears_lock_and_sets_error_no_flash() {
        let mut s = AppState::default();
        s.in_progress.insert(
            "a".into(),
            RunningOp { kind: OpKind::Upgrade, started_at: Instant::now() },
        );
        s.apply_event(Event::OpFinished {
            instance: "a".into(),
            kind: OpKind::Upgrade,
            result: Err("pg_upgrade exit 1".into()),
        });
        assert!(!s.in_progress.contains_key("a"));
        assert!(s.flash.is_none());
        let e = s.last_op_error.as_ref().expect("error set");
        assert_eq!(e.instance, "a");
        assert_eq!(e.kind, OpKind::Upgrade);
        assert!(e.msg.contains("pg_upgrade"));
    }

    // --- Task 2.5: Tick — auto-expire flash, refresh `now` ---

    use std::time::Duration;

    #[test]
    fn tick_expires_flash_after_3s() {
        let mut s = AppState::default();
        let old = Instant::now() - Duration::from_secs(4);
        s.flash = Some(Flash { msg: "x".into(), kind: FlashKind::Success, at: old });
        s.apply_event(Event::Tick);
        assert!(s.flash.is_none(), "flash >3s old expired");
    }

    #[test]
    fn tick_keeps_recent_flash() {
        let mut s = AppState::default();
        s.flash = Some(Flash { msg: "x".into(), kind: FlashKind::Success, at: Instant::now() });
        s.apply_event(Event::Tick);
        assert!(s.flash.is_some());
    }

    #[test]
    fn tick_does_not_expire_last_op_error() {
        let mut s = AppState::default();
        let old = Instant::now() - Duration::from_secs(60);
        s.last_op_error = Some(OpError {
            instance: "a".into(), kind: OpKind::Snapshot, msg: "x".into(), at: old,
        });
        s.apply_event(Event::Tick);
        assert!(s.last_op_error.is_some(), "errors are sticky (user clears via Esc)");
    }

    // --- Task 2.6: Esc clears last_op_error ---

    #[test]
    fn esc_clears_last_op_error_when_no_modal() {
        let mut s = AppState::default();
        s.last_op_error = Some(OpError {
            instance: "a".into(), kind: OpKind::Snapshot, msg: "x".into(), at: Instant::now(),
        });
        s.apply_event(key(KeyCode::Esc));
        assert!(s.last_op_error.is_none());
    }

    // --- Task 3.1: Open modal on op keys ---

    #[test]
    fn key_c_opens_clone_as_modal_for_selected() {
        let mut s = AppState::default();
        s.instances = vec![row("alpha")];
        s.apply_event(key(KeyCode::Char('c')));
        assert!(matches!(s.modal, Some(Modal::CloneAs { ref source, .. }) if source == "alpha"));
    }

    #[test]
    fn key_u_triggers_self_update() {
        // [u] in the TUI is `pgforge self-update` now (pg_upgrade moved
        // to CLI). Setting the flag is enough; the main loop drains it
        // into an async task — apply_event stays pure.
        let mut s = AppState::default();
        s.apply_event(key(KeyCode::Char('u')));
        assert!(s.pending_self_update);
        assert!(matches!(s.flash, Some(Flash { kind: FlashKind::Info, .. })));
        assert!(s.modal.is_none(), "self-update doesn't open a modal");
    }

    #[test]
    fn self_update_done_upgraded_sets_success_flash() {
        let mut s = AppState::default();
        s.apply_event(Event::SelfUpdateDone {
            upgraded: true,
            latest_tag: "v0.2.0".into(),
            current_version: "v0.1.11".into(),
        });
        let f = s.flash.as_ref().expect("flash set");
        assert!(matches!(f.kind, FlashKind::Success));
        assert!(f.msg.contains("v0.1.11"));
        assert!(f.msg.contains("v0.2.0"));
        assert!(f.msg.contains("Restart"));
    }

    #[test]
    fn self_update_done_noop_flash_when_already_latest() {
        let mut s = AppState::default();
        s.apply_event(Event::SelfUpdateDone {
            upgraded: false,
            latest_tag: "v0.1.11".into(),
            current_version: "v0.1.11".into(),
        });
        let f = s.flash.as_ref().expect("flash set");
        assert!(f.msg.contains("already on"));
    }

    #[test]
    fn self_update_failed_sets_op_error() {
        let mut s = AppState::default();
        s.apply_event(Event::SelfUpdateFailed { msg: "curl: HTTP 404".into() });
        let e = s.last_op_error.as_ref().expect("error set");
        assert!(e.msg.contains("curl"));
    }

    #[test]
    fn key_r_opens_restore_as_modal() {
        let mut s = AppState::default();
        s.instances = vec![row("alpha")];
        s.apply_event(key(KeyCode::Char('r')));
        assert!(matches!(s.modal, Some(Modal::RestoreAs { ref source, .. }) if source == "alpha"));
    }

    #[test]
    fn key_shift_r_opens_confirm_rotate() {
        let mut s = AppState::default();
        s.instances = vec![row("alpha")];
        s.apply_event(Event::Key(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT)));
        assert!(matches!(s.modal, Some(Modal::Confirm { kind: PendingDestructiveOp::Rotate { ref name }, .. }) if name == "alpha"));
    }

    #[test]
    fn key_question_opens_error_detail_when_error_present() {
        let mut s = AppState::default();
        s.last_op_error = Some(OpError { instance:"a".into(), kind: OpKind::Upgrade, msg: "boom".into(), at: Instant::now() });
        s.apply_event(key(KeyCode::Char('?')));
        assert!(matches!(s.modal, Some(Modal::ErrorDetail { ref msg }) if msg == "boom"));
    }

    #[test]
    fn key_question_without_error_is_noop() {
        let mut s = AppState::default();
        s.apply_event(key(KeyCode::Char('?')));
        assert!(s.modal.is_none());
    }

    #[test]
    fn key_e_opens_snapshots_modal_when_snapshots_loaded() {
        let mut s = AppState::default();
        s.instances = vec![row("alpha")];
        s.snapshots.insert("alpha".into(), SnapshotsView {
            list: Vec::new(), pitr: Default::default(),
        });
        s.apply_event(key(KeyCode::Char('e')));
        assert!(matches!(s.modal, Some(Modal::Snapshots { ref name, .. }) if name == "alpha"));
    }

    #[test]
    fn op_keys_noop_when_no_instance_selected() {
        let mut s = AppState::default();
        // [u] is now self-update; doesn't require a selected instance.
        for c in ['s', 'c', 'r', 'R'] {
            let kc = if c == 'R' {
                Event::Key(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT))
            } else { key(KeyCode::Char(c)) };
            s.apply_event(kc);
        }
        assert!(s.modal.is_none());
        assert!(s.in_progress.is_empty());
    }

    // --- Task 3.2: [s] triggers Snapshot op event (no modal) ---

    #[test]
    fn key_s_enqueues_snapshot_op_for_selected() {
        let mut s = AppState::default();
        s.instances = vec![row("alpha")];
        s.apply_event(key(KeyCode::Char('s')));
        assert_eq!(s.pending_ops, vec![("alpha".into(), OpKind::Snapshot)]);
        assert!(s.modal.is_none(), "snapshot has no modal");
    }

    #[test]
    fn key_s_noop_when_op_in_progress_on_same_instance() {
        let mut s = AppState::default();
        s.instances = vec![row("alpha")];
        s.in_progress.insert("alpha".into(), RunningOp { kind: OpKind::Snapshot, started_at: Instant::now() });
        s.apply_event(key(KeyCode::Char('s')));
        assert!(s.pending_ops.is_empty(), "per-instance lock prevents enqueue");
    }

    // --- Task 3.3: Modal-on key dispatch + Esc closes modal ---

    #[test]
    fn esc_in_modal_closes_modal() {
        let mut s = AppState::default();
        s.modal = Some(Modal::CloneAs { source: "a".into(), input: TextField::default() });
        s.apply_event(key(KeyCode::Esc));
        assert!(s.modal.is_none());
    }

    #[test]
    fn typing_in_clone_as_modal_appends_to_input() {
        let mut s = AppState::default();
        s.modal = Some(Modal::CloneAs { source: "a".into(), input: TextField::default() });
        s.apply_event(key(KeyCode::Char('x')));
        s.apply_event(key(KeyCode::Char('y')));
        if let Some(Modal::CloneAs { input, .. }) = &s.modal {
            assert_eq!(input.buf, "xy");
        } else { panic!("modal closed"); }
    }

    #[test]
    fn backspace_in_modal() {
        let mut s = AppState::default();
        let mut f = TextField::default();
        f.insert_char('a'); f.insert_char('b');
        s.modal = Some(Modal::CloneAs { source: "x".into(), input: f });
        s.apply_event(key(KeyCode::Backspace));
        if let Some(Modal::CloneAs { input, .. }) = &s.modal {
            assert_eq!(input.buf, "a");
        } else { panic!(); }
    }

    // --- Task 3.4: Modal submit — validation + transitions ---

    #[test]
    fn clone_as_enter_with_valid_name_enqueues_clone() {
        let mut s = AppState::default();
        let mut f = TextField::default();
        for c in "beta".chars() { f.insert_char(c); }
        s.modal = Some(Modal::CloneAs { source: "alpha".into(), input: f });
        s.apply_event(key(KeyCode::Enter));
        assert!(s.modal.is_none());
        assert_eq!(s.pending_ops.last(), Some(&("alpha:beta".to_string(), OpKind::Clone)));
    }

    #[test]
    fn clone_as_invalid_name_keeps_modal_and_flashes_error() {
        let mut s = AppState::default();
        let mut f = TextField::default();
        f.insert_char('-'); // invalid (must start with [a-z])
        s.modal = Some(Modal::CloneAs { source: "alpha".into(), input: f });
        s.apply_event(key(KeyCode::Enter));
        assert!(s.modal.is_some(), "modal stays open on validation failure");
        assert!(s.last_op_error.is_some(), "validation surfaces as error");
    }

    #[test]
    fn upgrade_to_valid_version_transitions_to_confirm() {
        let mut s = AppState::default();
        s.instances = vec![{
            let mut r = row("alpha"); r.pg_version = 17; r
        }];
        let mut f = TextField::default();
        for c in "18".chars() { f.insert_char(c); }
        s.modal = Some(Modal::UpgradeTo { source: "alpha".into(), input: f });
        s.apply_event(key(KeyCode::Enter));
        assert!(matches!(s.modal, Some(Modal::Confirm {
            kind: PendingDestructiveOp::Upgrade { ref name, to: 18 }, ..
        }) if name == "alpha"));
    }

    #[test]
    fn upgrade_to_smaller_version_rejects() {
        let mut s = AppState::default();
        s.instances = vec![{ let mut r = row("alpha"); r.pg_version = 18; r }];
        let mut f = TextField::default();
        f.insert_char('1'); f.insert_char('7');
        s.modal = Some(Modal::UpgradeTo { source: "alpha".into(), input: f });
        s.apply_event(key(KeyCode::Enter));
        assert!(matches!(s.modal, Some(Modal::UpgradeTo { .. })), "modal stays open");
        assert!(s.last_op_error.is_some());
    }

    #[test]
    fn restore_as_minutes_ago_zero_means_latest() {
        let mut s = AppState::default();
        let mut a = TextField::default();
        for c in "gamma".chars() { a.insert_char(c); }
        s.modal = Some(Modal::RestoreAs {
            source: "alpha".into(),
            as_input: a,
            minutes_ago: 0,
            focus: 0,
            pitr_earliest: None,
            uptime_cap_min: None,
        });
        s.apply_event(key(KeyCode::Enter));
        assert!(matches!(s.modal, Some(Modal::Confirm {
            kind: PendingDestructiveOp::Restore { ref source, ref as_, target_time: None }, ..
        }) if source == "alpha" && as_ == "gamma"));
    }

    #[test]
    fn restore_as_picker_caps_at_pitr_earliest() {
        // PITR earliest 10 minutes ago → user can pick 0..=10, not more.
        let earliest_10m_ago = (jiff::Timestamp::now()
            - jiff::SignedDuration::from_secs(10 * 60))
            .to_string();
        let mut s = AppState::default();
        s.modal = Some(Modal::RestoreAs {
            source: "alpha".into(),
            as_input: TextField::default(),
            minutes_ago: 9,
            focus: 1,
            pitr_earliest: Some(earliest_10m_ago),
            uptime_cap_min: None,
        });
        // Pressing → bumps to 10 (cap).
        s.apply_event(key(KeyCode::Right));
        if let Some(Modal::RestoreAs { minutes_ago, .. }) = s.modal {
            assert_eq!(minutes_ago, 10);
        } else { panic!("modal closed unexpectedly"); }
        // Another → should stay at 10.
        s.apply_event(key(KeyCode::Right));
        if let Some(Modal::RestoreAs { minutes_ago, .. }) = s.modal {
            assert_eq!(minutes_ago, 10, "saturate at PITR earliest");
        } else { panic!(); }
        // Typing a big number also gets clamped.
        s.apply_event(key(KeyCode::Char('5')));
        s.apply_event(key(KeyCode::Char('0')));
        s.apply_event(key(KeyCode::Char('0')));
        if let Some(Modal::RestoreAs { minutes_ago, .. }) = s.modal {
            assert_eq!(minutes_ago, 10);
        } else { panic!(); }
    }

    #[test]
    fn restore_as_minutes_ago_nonzero_produces_target_timestamp() {
        let mut s = AppState::default();
        let mut a = TextField::default();
        for c in "gamma".chars() { a.insert_char(c); }
        s.modal = Some(Modal::RestoreAs {
            source: "alpha".into(),
            as_input: a,
            minutes_ago: 5,
            focus: 0,
            pitr_earliest: None,
            uptime_cap_min: None,
        });
        s.apply_event(key(KeyCode::Enter));
        let Some(Modal::Confirm { kind: PendingDestructiveOp::Restore { target_time, .. }, .. }) = &s.modal
            else { panic!("not in Confirm"); };
        let ts = target_time.as_ref().expect("target_time set");
        // RFC3339-ish — contains a `T` and ends with `Z` or `+...`.
        assert!(ts.contains('T'));
    }

    // --- Task 3.5: Confirm modal — y/n dispatch ---

    #[test]
    fn confirm_yes_for_rotate_enqueues_op() {
        let mut s = AppState::default();
        s.modal = Some(Modal::Confirm {
            kind: PendingDestructiveOp::Rotate { name: "alpha".into() },
            prompt: "...".into(),
        });
        s.apply_event(key(KeyCode::Char('y')));
        assert!(s.modal.is_none());
        assert_eq!(s.pending_ops, vec![("alpha".into(), OpKind::Rotate)]);
    }

    #[test]
    fn confirm_no_closes_modal_no_op() {
        let mut s = AppState::default();
        s.modal = Some(Modal::Confirm {
            kind: PendingDestructiveOp::Rotate { name: "alpha".into() },
            prompt: "...".into(),
        });
        s.apply_event(key(KeyCode::Char('n')));
        assert!(s.modal.is_none());
        assert!(s.pending_ops.is_empty());
    }

    #[test]
    fn confirm_yes_for_upgrade_enqueues_op_with_to_version() {
        let mut s = AppState::default();
        s.modal = Some(Modal::Confirm {
            kind: PendingDestructiveOp::Upgrade { name: "alpha".into(), to: 18 },
            prompt: "...".into(),
        });
        s.apply_event(key(KeyCode::Char('y')));
        assert_eq!(s.pending_ops, vec![("alpha@18".into(), OpKind::Upgrade)]);
    }

    #[test]
    fn confirm_yes_for_restore_encodes_source_as_target_time() {
        let mut s = AppState::default();
        s.modal = Some(Modal::Confirm {
            kind: PendingDestructiveOp::Restore {
                source: "alpha".into(), as_: "gamma".into(), target_time: None,
            },
            prompt: "...".into(),
        });
        s.apply_event(key(KeyCode::Char('y')));
        assert_eq!(s.pending_ops, vec![("alpha:gamma".into(), OpKind::Restore)]);
    }

    #[test]
    fn confirm_yes_for_restore_with_target_time() {
        let mut s = AppState::default();
        s.modal = Some(Modal::Confirm {
            kind: PendingDestructiveOp::Restore {
                source: "alpha".into(), as_: "gamma".into(),
                target_time: Some("2026-05-12T10:00:00Z".into()),
            },
            prompt: "...".into(),
        });
        s.apply_event(key(KeyCode::Char('y')));
        assert_eq!(s.pending_ops, vec![("alpha:gamma@2026-05-12T10:00:00Z".into(), OpKind::Restore)]);
    }

    #[test]
    fn key_enter_enqueues_clipboard_copy() {
        let mut s = AppState::default();
        s.instances = vec![row("alpha")];
        s.apply_event(key(KeyCode::Enter));
        assert_eq!(s.pending_clipboard, vec!["alpha".to_string()]);
    }

    #[test]
    fn op_finished_ok_enqueues_refresh_request() {
        let mut s = AppState::default();
        s.in_progress.insert("alpha".into(), RunningOp { kind: OpKind::Snapshot, started_at: Instant::now() });
        s.apply_event(Event::OpFinished { instance: "alpha".into(), kind: OpKind::Snapshot, result: Ok(()) });
        assert_eq!(s.refresh_requests, vec!["alpha".to_string()]);
    }
}
