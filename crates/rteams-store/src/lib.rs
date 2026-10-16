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

pub use file::FileStore;

use std::collections::HashMap;

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

/// Result of an execute: the event, and whether it was already in the log (idempotent replay).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Executed {
    pub event: Event,
    pub replayed: bool,
}

/// What every backend gives callers: `execute` = decide, append, apply; reads see the full log.
/// Reads take `&mut self` because a persistent store first catches up with other writers.
pub trait Store {
    /// Like `execute`, but a repeated `command_id` returns the original event instead of
    /// appending a second one. `None` is a plain, non-idempotent execute.
    fn execute_with_id(
        &mut self,
        actor: &str,
        ts: i64,
        command_id: Option<&str>,
        cmd: Command,
    ) -> Result<Executed, StoreError>;

    fn execute(&mut self, actor: &str, ts: i64, cmd: Command) -> Result<Event, StoreError> {
        Ok(self.execute_with_id(actor, ts, None, cmd)?.event)
    }

    /// The event a previous command with this id produced, if any. Lets callers short-circuit
    /// a retry before re-running checks that would now fail against the already-changed state.
    fn replay(&mut self, command_id: &str) -> Result<Option<Event>, StoreError>;

    fn events(&mut self) -> Result<&[Event], StoreError>;
    fn state(&mut self) -> Result<&State, StoreError>;
}

/// In-process store: core's `MemStore` plus the same command-id dedup the file store has.
/// For tests and ephemeral servers; nothing is shared or persisted.
#[derive(Debug, Default)]
pub struct MemoryStore {
    mem: MemStore,
    commands: HashMap<String, usize>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Store for MemoryStore {
    fn execute_with_id(
        &mut self,
        actor: &str,
        ts: i64,
        command_id: Option<&str>,
        cmd: Command,
    ) -> Result<Executed, StoreError> {
        if let Some(&i) = command_id.and_then(|id| self.commands.get(id)) {
            return Ok(Executed {
                event: self.mem.events()[i].clone(),
                replayed: true,
            });
        }
        let event = self.mem.execute(actor, ts, cmd)?.clone();
        if let Some(id) = command_id {
            self.commands
                .insert(id.to_string(), self.mem.events().len() - 1);
        }
        Ok(Executed {
            event,
            replayed: false,
        })
    }

    fn replay(&mut self, command_id: &str) -> Result<Option<Event>, StoreError> {
        Ok(self
            .commands
            .get(command_id)
            .map(|&i| self.mem.events()[i].clone()))
    }

    fn events(&mut self) -> Result<&[Event], StoreError> {
        Ok(self.mem.events())
    }

    fn state(&mut self) -> Result<&State, StoreError> {
        Ok(self.mem.state())
    }
}
