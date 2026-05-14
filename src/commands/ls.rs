//! `pgforge ls` — list every managed instance with a short status line.
//! Reads state.toml files under `state_root/instances/*/` and tags each with
//! whether its container is currently running (via docker ps).

use crate::docker::bollard_engine::BollardEngine;
use crate::docker::engine::DockerEngine;
use crate::error::Result;
use crate::state::instance::InstanceState;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct LsArgs {
    pub override_state_root: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct InstanceSummary {
    pub name: String,
    pub pg_version: u8,
    pub preset_label: String,
    pub host_port: u16,
    pub backup_enabled: bool,
    /// True when the last snapshot attempt is newer than the last success —
    /// backups are currently broken and need operator attention.
    pub backup_failing: bool,
    pub running: bool,
}

pub async fn run(args: LsArgs) -> Result<Vec<InstanceSummary>> {
    let state_root = args
        .override_state_root
        .clone()
        .unwrap_or_else(InstanceState::default_state_root);
    let docker = BollardEngine::connect()?;
    run_with_engine(args, &docker, state_root).await
}

pub async fn run_with_engine<E: DockerEngine>(
    _args: LsArgs,
    docker: &E,
    state_root: PathBuf,
) -> Result<Vec<InstanceSummary>> {
    let names = InstanceState::list_under(&state_root)?;
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        // Skip un-parseable state files instead of failing the whole listing
        // — a corrupt file shouldn't hide the other 5 healthy instances.
        let Ok(state) = InstanceState::load_under(&state_root, &name) else {
            tracing::warn!(
                target: "pgforge::ls",
                "skipping {name}: state.toml unparseable"
            );
            continue;
        };
        let container_name = format!("pgforge_{}", state.instance.name);
        let running = docker.container_running(&container_name).await.unwrap_or(false);
        out.push(InstanceSummary {
            name: state.instance.name.clone(),
            pg_version: state.instance.pg_version,
            preset_label: format!("{:?}", state.instance.preset).to_lowercase(),
            host_port: state.instance.host_port,
            backup_enabled: state.instance.backup_enabled,
            backup_failing: state.instance.backup_failing(),
            running,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Render the listing as a short text table (used by the CLI; the TUI will
/// build its own view from the Vec<InstanceSummary>).
pub fn render_table(rows: &[InstanceSummary]) -> String {
    if rows.is_empty() {
        return "No instances. Run `pgforge create --help` to create one.\n".into();
    }
    let mut s = String::new();
    s.push_str(&format!(
        "{:<24} {:<7} {:<8} {:<6} {:<8} {:<7}\n",
        "NAME", "PG", "PRESET", "PORT", "BACKUPS", "RUNNING"
    ));
    for r in rows {
        let backups = if !r.backup_enabled {
            "no"
        } else if r.backup_failing {
            "FAILING"
        } else {
            "yes"
        };
        s.push_str(&format!(
            "{:<24} {:<7} {:<8} {:<6} {:<8} {:<7}\n",
            truncate(&r.name, 24),
            r.pg_version,
            truncate(&r.preset_label, 8),
            r.host_port,
            backups,
            if r.running { "yes" } else { "no" }
        ));
    }
    s
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        // ASCII-safe truncation — instance names are validated to be ASCII
        // [a-z0-9_-]; preset labels are static Rust idents. char_indices
        // keeps the byte boundary correct in case that contract ever
        // loosens (see rust-safe-string-truncation skill).
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}
