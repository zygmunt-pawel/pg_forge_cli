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
}

#[derive(Debug, Clone)]
pub struct SnapshotsView {
    pub list: Vec<SnapshotRecord>,
    pub pitr: PitrWindow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Snapshot, Clone, Rotate, Upgrade, Restore,
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

#[derive(Debug, Clone)]
pub enum PendingDestructiveOp {
    Rotate { name: String },
    Upgrade { name: String, to: u8 },
    Restore { source: String, as_: String, target_time: Option<String> },
}

#[derive(Debug, Clone)]
pub enum Modal {
    CloneAs { source: String, input: TextField },
    UpgradeTo { source: String, input: TextField },
    RestoreAs { source: String, as_input: TextField, target_time: TextField, focus: u8 },
    Confirm { kind: PendingDestructiveOp, prompt: String },
    Snapshots { name: String, view: SnapshotsView },
    ErrorDetail { msg: String },
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
