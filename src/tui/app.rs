//! AppState — single-owner mutable state for the TUI main loop.
//! `apply_event` is pure modulo Instant::now(): tests inject events and
//! assert state. No terminal, no docker, no tokio in this file.

use crate::commands::ls::InstanceSummary;
use crate::commands::status::InstanceStatus;
use crate::tui::events::*;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

pub struct AppState {
    pub instances: Vec<InstanceSummary>,
    pub statuses: HashMap<String, InstanceStatus>,
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
                if let Some(n) = prev_name {
                    if let Some(i) = self.instances.iter().position(|r| r.name == n) {
                        self.selected = i;
                        return;
                    }
                }
                if self.instances.is_empty() {
                    self.selected = 0;
                } else if self.selected >= self.instances.len() {
                    self.selected = self.instances.len() - 1;
                }
            }
            Event::StatusRefreshed { name, status } => {
                self.stale_status.remove(&name);
                self.statuses.insert(name, status);
            }
            Event::SnapshotsRefreshed { name, view } => {
                self.snapshots.insert(name, view);
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
                if let Some(f) = &self.flash {
                    if self.now.duration_since(f.at) > std::time::Duration::from_secs(3) {
                        self.flash = None;
                    }
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
                if let Some(n) = self.selected_name().map(str::to_string) {
                    if !self.in_progress.contains_key(&n) {
                        self.pending_ops.push((n, OpKind::Snapshot));
                    }
                }
            }
            KeyCode::Char('c') => {
                if let Some(n) = self.selected_name().map(str::to_string) {
                    self.modal = Some(Modal::CloneAs { source: n, input: TextField::default() });
                }
            }
            KeyCode::Char('u') => {
                if let Some(n) = self.selected_name().map(str::to_string) {
                    self.modal = Some(Modal::UpgradeTo { source: n, input: TextField::default() });
                }
            }
            KeyCode::Char('r') => {
                if let Some(n) = self.selected_name().map(str::to_string) {
                    self.modal = Some(Modal::RestoreAs {
                        source: n,
                        as_input: TextField::default(),
                        target_time: TextField::default(),
                        focus: 0,
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
            KeyCode::Char('?') => {
                if let Some(e) = &self.last_op_error {
                    self.modal = Some(Modal::ErrorDetail { msg: e.msg.clone() });
                }
            }
            KeyCode::Char('e') => {
                if let Some(n) = self.selected_name().map(str::to_string) {
                    if let Some(v) = self.snapshots.get(&n).cloned() {
                        self.modal = Some(Modal::Snapshots { name: n, view: v });
                    }
                }
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
            Some(Modal::RestoreAs { as_input, target_time, focus, .. }) => match k.code {
                KeyCode::Tab => { *focus = (*focus + 1) % 2; Action::Nothing }
                KeyCode::Char(c) if !c.is_control() => {
                    if *focus == 0 { as_input.insert_char(c); } else { target_time.insert_char(c); }
                    Action::Nothing
                }
                KeyCode::Backspace => {
                    if *focus == 0 { as_input.backspace(); } else { target_time.backspace(); }
                    Action::Nothing
                }
                KeyCode::Left => {
                    if *focus == 0 { as_input.move_left(); } else { target_time.move_left(); }
                    Action::Nothing
                }
                KeyCode::Right => {
                    if *focus == 0 { as_input.move_right(); } else { target_time.move_right(); }
                    Action::Nothing
                }
                KeyCode::Enter => Action::Submit,
                _ => Action::Nothing,
            },
            Some(Modal::Confirm { .. }) => match k.code {
                KeyCode::Char('y') | KeyCode::Enter => Action::ConfirmYes,
                KeyCode::Char('n') => Action::ConfirmNo,
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
        use std::str::FromStr;
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
            Some(Modal::RestoreAs { source, as_input, target_time, focus }) => {
                let as_ = as_input.buf.clone();
                let tt = if target_time.buf.is_empty() { None } else { Some(target_time.buf.clone()) };
                if let Err(msg) = validate_instance_name(&as_) {
                    self.last_op_error = Some(OpError {
                        instance: source.clone(), kind: OpKind::Restore, msg, at: Instant::now(),
                    });
                    self.modal = Some(Modal::RestoreAs { source, as_input, target_time, focus });
                    return;
                }
                if let Some(ref ts) = tt {
                    if let Err(e) = <jiff::Timestamp as FromStr>::from_str(ts) {
                        self.last_op_error = Some(OpError {
                            instance: source.clone(), kind: OpKind::Restore,
                            msg: format!("invalid target_time RFC3339: {e}"), at: Instant::now(),
                        });
                        self.modal = Some(Modal::RestoreAs { source, as_input, target_time, focus });
                        return;
                    }
                }
                self.modal = Some(Modal::Confirm {
                    kind: PendingDestructiveOp::Restore { source: source.clone(), as_: as_.clone(), target_time: tt },
                    prompt: format!("Restore {source} → new instance {as_}? Takes minutes."),
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
            }
        }
    }
}

/// Validator reused by clone+restore. Delegates to
/// `domain::instance::Instance::validate_name` (single source of truth).
fn validate_instance_name(s: &str) -> std::result::Result<(), String> {
    use crate::domain::instance::Instance;
    Instance::validate_name(s).map_err(|e| e.to_string())
}

#[cfg(test)]
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
    fn key_u_opens_upgrade_to_modal() {
        let mut s = AppState::default();
        s.instances = vec![row("alpha")];
        s.apply_event(key(KeyCode::Char('u')));
        assert!(matches!(s.modal, Some(Modal::UpgradeTo { ref source, .. }) if source == "alpha"));
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
        for c in ['s', 'c', 'u', 'r', 'R'] {
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
    fn restore_as_with_no_target_time_transitions_to_confirm() {
        let mut s = AppState::default();
        let mut a = TextField::default();
        for c in "gamma".chars() { a.insert_char(c); }
        s.modal = Some(Modal::RestoreAs {
            source: "alpha".into(),
            as_input: a,
            target_time: TextField::default(),
            focus: 0,
        });
        s.apply_event(key(KeyCode::Enter));
        assert!(matches!(s.modal, Some(Modal::Confirm {
            kind: PendingDestructiveOp::Restore { ref source, ref as_, target_time: None }, ..
        }) if source == "alpha" && as_ == "gamma"));
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
}
