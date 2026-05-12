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

    /// Stub — implemented in Phase 2 piece by piece.
    pub fn apply_event(&mut self, _ev: Event) {
        // Phase 2 fills this in.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
