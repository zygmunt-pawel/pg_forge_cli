//! Event enum + the state-machine types referenced by AppState. Pure
//! data; no async, no terminal. Tests in app.rs and modal handler tests
//! exercise these.

use crate::commands::ls::InstanceSummary;
use crate::commands::snapshots::PitrWindow;
use crate::commands::status::InstanceStatus;
use crate::domain::snapshot::SnapshotRecord;
use crate::error::PgForgeError;
use crossterm::event::KeyEvent;
use std::time::Instant;

#[derive(Debug)]
pub enum Event {
    Key(KeyEvent),
    Tick,
    InstancesListed(Vec<InstanceSummary>),
    StatusRefreshed { name: String, status: InstanceStatus },
    SnapshotsRefreshed { name: String, view: SnapshotsView },
    OpStarted { instance: String, kind: OpKind },
    OpFinished { instance: String, kind: OpKind, result: std::result::Result<(), String> },
    RefreshFailed { name: String, err: String },
    /// `pgforge self-update` finished. `upgraded` is true when the
    /// binary was actually replaced; false means we were already on
    /// the latest tag and nothing was touched.
    SelfUpdateDone {
        upgraded: bool,
        latest_tag: String,
        current_version: String,
    },
    SelfUpdateFailed { msg: String },
    DiskHealthRefreshed(crate::disk::health::DiskHealth),
}

#[derive(Debug, Clone)]
pub struct SnapshotsView {
    pub list: Vec<SnapshotRecord>,
    pub pitr: PitrWindow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Snapshot, Clone, Rotate, Upgrade, Restore, Destroy, Create, Resize,
    /// Sentinel for clipboard-copy failures. NEVER appears in
    /// `AppState::in_progress` (clipboard is sync, runs inline in the
    /// main loop), only in `OpError::kind` for rendering.
    Clipboard,
}

impl OpKind {
    pub fn label(&self) -> &'static str {
        match self {
            OpKind::Snapshot  => "snapshot",
            OpKind::Clone     => "clone",
            OpKind::Rotate    => "rotate",
            OpKind::Upgrade   => "upgrade",
            OpKind::Restore   => "restore",
            OpKind::Destroy   => "destroy",
            OpKind::Create    => "create",
            OpKind::Resize    => "resize",
            OpKind::Clipboard => "copy",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunningOp { pub kind: OpKind, pub started_at: Instant }

#[derive(Debug, Clone)]
pub struct OpError {
    pub instance: String,
    pub kind: OpKind,
    pub msg: String,
    pub at: Instant,
}

#[derive(Debug, Clone, Copy)]
pub enum FlashKind { Info, Success }

#[derive(Debug, Clone)]
pub struct Flash {
    pub msg: String,
    pub kind: FlashKind,
    pub at: Instant,
}

#[derive(Debug, Clone, Default)]
pub struct TextField { pub buf: String, pub cursor: usize }

impl TextField {
    pub fn insert_char(&mut self, c: char) {
        self.buf.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 { return; }
        let prev = self.buf[..self.cursor].chars().next_back().unwrap();
        self.cursor -= prev.len_utf8();
        self.buf.remove(self.cursor);
    }

    pub fn move_left(&mut self) {
        if self.cursor == 0 { return; }
        let prev = self.buf[..self.cursor].chars().next_back().unwrap();
        self.cursor -= prev.len_utf8();
    }

    pub fn move_right(&mut self) {
        if self.cursor >= self.buf.len() { return; }
        let next = self.buf[self.cursor..].chars().next().unwrap();
        self.cursor += next.len_utf8();
    }
}

/// Drained by the main loop; spawned as a `pgforge create` op task.
/// Lives in `AppState::pending_creates` because the parameter list is
/// larger than the (name, kind) pair used by other ops.
#[derive(Debug, Clone)]
pub struct CreateRequest {
    pub name: String,
    pub app_user: String,
    pub app_password: String,
    pub pgbackrest_password: String,
    pub pg_version: u8,
    pub preset: crate::domain::preset::Preset,
    pub no_backup: bool,
    pub retain_days: u32,
    pub snapshot_hour: Option<u8>,
}

#[derive(Debug, Clone)]
pub enum PendingDestructiveOp {
    Rotate { name: String },
    Upgrade { name: String, to: u8 },
    Restore { source: String, as_: String, target_time: Option<String> },
    Destroy { name: String, delete_backups: bool },
    Resize { name: String, new_preset: crate::domain::preset::Preset },
}

#[derive(Debug, Clone)]
pub enum Modal {
    /// Centred menu of every per-instance action. Opened by `a` at the
    /// top level. Each listed key delegates to an `open_*_for_selected`
    /// method on AppState and closes itself first.
    ActionsMenu { instance_name: String },
    CloneAs { source: String, input: TextField },
    UpgradeTo { source: String, input: TextField },
    RestoreAs {
        source: String,
        as_input: TextField,
        minutes_ago: u32,
        focus: u8,
        /// Captured at modal-open from snapshots[source].pitr.earliest.
        /// Used to cap minutes_ago so the user can't pick a target_time
        /// before the earliest full backup. None when the instance
        /// has no full backups yet.
        pitr_earliest: Option<String>,
        /// Container uptime in minutes at modal-open. Acts as the
        /// absolute upper bound when there's no PITR window yet
        /// (freshly-created instance) — you can't restore to a point
        /// before the container was even born. None means uptime
        /// wasn't available; in that case the picker falls back to
        /// 0 (latest only) when pitr_earliest is also None.
        uptime_cap_min: Option<u32>,
    },
    Confirm { kind: PendingDestructiveOp, prompt: String },
    Snapshots { name: String, view: SnapshotsView },
    ErrorDetail { msg: String },
    /// Wizard for `pgforge create`. All values pre-filled with generated
    /// defaults; user can edit name / app_user / pg_version, cycle
    /// preset with arrow keys, toggle no_backup with space. Password is
    /// always generated (never editable) — shown ONCE on success.
    Create {
        name: TextField,
        app_user: TextField,
        pg_version: TextField,
        preset: crate::domain::preset::Preset,
        no_backup: bool,
        /// Retention window in days for pgbackrest full backups.
        /// 0 = keep forever. Default 30. Numeric cycler on focus==5.
        retain_days: u32,
        /// Auto-snapshot hour (0..=23 local time). None = manual only.
        /// Default Some(3) = 03:00. Numeric cycler on focus==6.
        snapshot_hour: Option<u8>,
        /// 0=name, 1=app_user, 2=pg_version, 3=preset, 4=no_backup, 5=retain_days, 6=snapshot_hour
        focus: u8,
        /// Pre-generated password, stashed here so submit can pick it
        /// up without re-generating. Not displayed in the modal body
        /// (that would defeat the show-once-on-success flow).
        generated_password: String,
        /// Pre-generated pgbackrest password (only used when backup_enabled).
        generated_pgbackrest_password: String,
    },
    /// Shown after a successful `pgforge create` from the TUI wizard.
    /// Contains the full connection URI with the generated password
    /// embedded — user selects with mouse and Cmd+C to copy.
    CreatedSuccess { name: String, uri: String },
    /// Plain "show the URI on screen" modal, opened by `[Enter]` on the
    /// instance list. Same render as CreatedSuccess but a different
    /// header ("Connection string" vs "Instance ready"), used purely
    /// for retrieval — user selects with mouse + Cmd+C to copy.
    ConnectionString { name: String, uri: String },
    /// Global keybind reference. Opened by `?` when there is no
    /// last_op_error to detail (otherwise `?` shows the error).
    Help,
    /// Preset-resize wizard. ← → / space cycles `new` through tiny ↔
    /// small ↔ medium ↔ large; Enter transitions to Confirm. `current`
    /// is shown read-only so the user knows what they're changing from.
    ResizeTo { name: String, current: crate::domain::preset::Preset, new: crate::domain::preset::Preset },
    /// Change auto-snapshot hour on an existing instance. `new = None`
    /// means "manual only". Numeric cycler: ← → ±1, digits jump, Backspace
    /// resets to None.
    ScheduleEdit { name: String, current: Option<u8>, new: Option<u8> },
}

/// Map a PgForgeError (or anyhow) into the string carried by
/// Event::OpFinished{result: Err(_)}. Centralized so ops::spawn_op
/// doesn't sprinkle Display-formatting at call sites.
pub fn err_to_string(e: PgForgeError) -> String {
    e.to_string()
}

#[cfg(test)]
mod text_field_tests {
    use super::TextField;

    #[test]
    fn insert_char_at_end() {
        let mut t = TextField::default();
        t.insert_char('a');
        t.insert_char('b');
        assert_eq!(t.buf, "ab");
        assert_eq!(t.cursor, 2);
    }

    #[test]
    fn backspace_removes_previous_char() {
        let mut t = TextField { buf: "abc".into(), cursor: 3 };
        t.backspace();
        assert_eq!(t.buf, "ab");
        assert_eq!(t.cursor, 2);
    }

    #[test]
    fn backspace_at_zero_is_noop() {
        let mut t = TextField::default();
        t.backspace();
        assert_eq!(t.buf, "");
        assert_eq!(t.cursor, 0);
    }

    #[test]
    fn move_left_and_right() {
        let mut t = TextField { buf: "abc".into(), cursor: 3 };
        t.move_left(); assert_eq!(t.cursor, 2);
        t.move_left(); assert_eq!(t.cursor, 1);
        t.move_right(); assert_eq!(t.cursor, 2);
        t.move_right(); assert_eq!(t.cursor, 3);
        t.move_right(); assert_eq!(t.cursor, 3); // clamped
    }

    #[test]
    fn utf8_multibyte_safe() {
        // Polish: 'ł' is 2 bytes UTF-8.
        let mut t = TextField::default();
        t.insert_char('a');
        t.insert_char('ł');
        t.insert_char('b');
        assert_eq!(t.buf, "ałb");
        assert_eq!(t.cursor, 4); // 1 + 2 + 1 bytes
        t.backspace();           // removes 'b'
        assert_eq!(t.buf, "ał");
        assert_eq!(t.cursor, 3);
        t.backspace();           // removes 'ł' (2 bytes)
        assert_eq!(t.buf, "a");
        assert_eq!(t.cursor, 1);
    }
}
