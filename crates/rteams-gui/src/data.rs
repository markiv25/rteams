//! Read adapter over the event log. OWNER: vector. echo's stub: the signatures below are what
//! `app.rs` and `board.rs` call; fill in the bodies (live refresh, error handling) freely, but
//! please keep these signatures, or tell echo before changing them.

use std::path::PathBuf;

use rteams_core::{kanban, KanbanBoard, State};
use rteams_store::{FileStore, Store};

/// Read-only view of one team's log. Never appends.
pub struct Data {
    store: FileStore,
    seen: usize,
}

impl Data {
    /// Open `<home>/<team>` and fold the existing log.
    pub fn open(home: PathBuf, team: &str) -> Result<Self, String> {
        let store = FileStore::open(&home, team).map_err(|e| e.to_string())?;
        Ok(Data { store, seen: 0 })
    }

    /// Catch up with other writers. `Ok(true)` when new events arrived since the last call
    /// (so the UI knows to repaint); the first call after `open` reports `true`.
    pub fn refresh(&mut self) -> Result<bool, String> {
        let n = self.store.events().map_err(|e| e.to_string())?.len();
        let changed = n != self.seen;
        self.seen = n;
        Ok(changed)
    }

    /// Current state as of the last `refresh`.
    pub fn state(&mut self) -> Result<&State, String> {
        self.store.state().map_err(|e| e.to_string())
    }

    /// Kanban projection (archived tasks hidden) as of the last `refresh`.
    pub fn board(&mut self) -> Result<KanbanBoard, String> {
        Ok(kanban(self.state()?))
    }
}
