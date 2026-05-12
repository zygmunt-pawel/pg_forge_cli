//! Plan 5 — interactive ratatui dashboard. See
//! docs/plans/2026-05-12-plan-5-tui.md and
//! docs/superpowers/specs/2026-05-12-plan-5-tui-design.md.

pub mod app;
pub mod clipboard;
pub mod events;
pub mod ops;
pub mod refresh;
pub mod ui;

use crate::error::Result;

/// Top-level entry. Implemented in Phase 7.
pub async fn run() -> Result<()> {
    todo!("Phase 7")
}
