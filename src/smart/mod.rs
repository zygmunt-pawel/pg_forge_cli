//! Host SMART hardware-health monitoring. Sibling to `src/disk/` (capacity).
//! Cache-and-read architecture: a daily systemd-user timer writes
//! `~/.local/state/pgforge/disk-smart.json`; TUI/CLI read that cache.
//!
//! This subsystem MUST NEVER panic — every fallible step degrades to
//! `SmartHealth::unknown(<reason>)`. Enforced by the file-level deny lints
//! below (matching `src/disk/mod.rs`).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

pub mod cache;
pub mod check;
pub mod install;
pub mod installed;
pub mod types;
