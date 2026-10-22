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
    /// A caller-supplied guard (authorization policy) refused the command under the lock.
    #[error("forbidden: {0}")]
    Forbidden(String),
    /// A write was attempted on a store opened with `open_read_only`.
    #[error("store is read-only")]
    ReadOnly,
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
    /// The one write path. Under the store's lock, after catching up with other writers:
    /// a repeated `command_id` returns the original event; otherwise the command is decided
    /// (domain errors first), then `guard` sees the same state and may refuse it with a reason
    /// (`StoreError::Forbidden`), and only then is the event appended. So check, decide and
    /// append are atomic with respect to other processes.
    fn execute_guarded(
        &mut self,
        actor: &str,
        ts: i64,
        command_id: Option<&str>,
        cmd: Command,
        guard: &dyn Fn(&State) -> Result<(), String>,
    ) -> Result<Executed, StoreError>;

    /// `execute_guarded` with no policy. `None` is a plain, non-idempotent execute.
    fn execute_with_id(
        &mut self,
        actor: &str,
        ts: i64,
        command_id: Option<&str>,
        cmd: Command,
    ) -> Result<Executed, StoreError> {
        self.execute_guarded(actor, ts, command_id, cmd, &|_| Ok(()))
    }

    fn execute(&mut self, actor: &str, ts: i64, cmd: Command) -> Result<Event, StoreError> {
        Ok(self.execute_with_id(actor, ts, None, cmd)?.event)
    }

    /// The directory holding every team of this store (sibling teams), if it has one.
    fn home(&self) -> Option<std::path::PathBuf> {
        None
    }

    /// This team's directory name (its stable id), if the store has one.
    fn team_dir(&self) -> Option<String> {
        None
    }

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
    fn execute_guarded(
        &mut self,
        actor: &str,
        ts: i64,
        command_id: Option<&str>,
        cmd: Command,
        guard: &dyn Fn(&State) -> Result<(), String>,
    ) -> Result<Executed, StoreError> {
        if let Some(&i) = command_id.and_then(|id| self.commands.get(id)) {
            return Ok(Executed {
                event: self.mem.events()[i].clone(),
                replayed: true,
            });
        }
        self.mem.state().decide(cmd.clone())?;
        guard(self.mem.state()).map_err(StoreError::Forbidden)?;
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

    fn events(&mut self) -> Result<&[Event], StoreError> {
        Ok(self.mem.events())
    }

    fn state(&mut self) -> Result<&State, StoreError> {
        Ok(self.mem.state())
    }
}
