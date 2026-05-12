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
            // Modal handling — Phase 3 (Task 3.x).
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
            _ => {} // op keys + ? + e in Phase 3.
        }
    }
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
}
