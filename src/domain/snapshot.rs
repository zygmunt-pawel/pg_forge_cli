use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotKind {
    Full,
    Diff,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRecord {
    /// pgbackrest internal label, e.g. "20260511-141259F".
    pub label: String,
    pub kind: SnapshotKind,
    /// User-supplied label, e.g. "before-migration". Optional.
    pub user_label: Option<String>,
    /// ISO 8601 UTC.
    pub taken_at: String,
}
