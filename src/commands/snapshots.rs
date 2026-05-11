use crate::error::Result;
use crate::state::instance::InstanceState;
use crate::state::snapshots::SnapshotsFile;
use std::path::PathBuf;

pub fn run(
    instance: &str,
    override_state_root: Option<PathBuf>,
) -> Result<Vec<crate::domain::snapshot::SnapshotRecord>> {
    let state_root = override_state_root
        .clone()
        .unwrap_or_else(InstanceState::default_state_root);
    // Ensures instance exists; errors if not
    let _ = InstanceState::load_under(&state_root, instance)?;
    let file = SnapshotsFile::load_for(&state_root, instance)?;
    Ok(file.snapshots)
}
