//! rteams-store: durable, multi-process-safe persistence for the rteams-core event log.
//!
//! One team = one directory:
//!
//! ```text
//! <root>/<team>/events.jsonl   append-only, one `Event` per line (the single source of truth)
//! <root>/<team>/.lock          per-team advisory lock (fs2), held only while writing
//! <root>/<team>/tasks/<displayId>.json   write-only snapshot per task, for humans and grep
//! ```
//!
//! No database: `State`, kanban, briefing and inbox are folded/derived from the log, exactly as
//! with `MemStore`. A write is `lock -> catch up with other writers -> decide -> append+fsync ->
//! unlock`. A crash mid-append leaves an unterminated last line, which is ignored by readers and
//! truncated by the next writer. Optional command ids make retried commands idempotent; the id is
//! stored on the same log line as the event, so dedup and append are atomic. Task snapshots are
//! never read back, so they cannot become a second source of truth.

mod file;

pub use file::{Executed, FileStore};

use rteams_core::{Command, CoreError, Event, MemStore, State};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("io error on {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("corrupt event log {path} at line {line}: {reason}")]
    Corrupt {
        path: std::path::PathBuf,
        line: u64,
        reason: String,
    },
    #[error("invalid team name {0:?}: use 1-64 chars of [A-Za-z0-9._-], not starting with '.'")]
    InvalidTeamName(String),
}

/// What every backend gives callers: `execute` = decide, append, apply; reads see the full log.
/// Reads take `&mut self` because a persistent store first catches up with other writers.
pub trait Store {
    fn execute(&mut self, actor: &str, ts: i64, cmd: Command) -> Result<Event, StoreError>;
    fn events(&mut self) -> Result<&[Event], StoreError>;
    fn state(&mut self) -> Result<&State, StoreError>;
}

impl Store for MemStore {
    fn execute(&mut self, actor: &str, ts: i64, cmd: Command) -> Result<Event, StoreError> {
        Ok(MemStore::execute(self, actor, ts, cmd)?.clone())
    }
    fn events(&mut self) -> Result<&[Event], StoreError> {
        Ok(MemStore::events(self))
    }
    fn state(&mut self) -> Result<&State, StoreError> {
        Ok(MemStore::state(self))
    }
}
