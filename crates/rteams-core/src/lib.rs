//! rteams-core: pure domain model, event log, task state machine and projections.
//!
//! No I/O, no clock, no id generation: callers supply timestamps and ids.
//! The event log is the single source of truth; Task state, kanban, inbox,
//! briefing and process list are all derived from it. Messages and task
//! comments share one stream (see `Scope`). Deletion is a single `archived` flag.

pub mod command;
pub mod domain;
pub mod error;
pub mod event;
pub mod lifecycle;
pub mod projection;
pub mod state;
pub mod store;

pub use command::Command;
pub use domain::{Member, Message, ProcessEntry, Scope, Task, Team};
pub use error::CoreError;
pub use event::{Event, EventKind};
pub use lifecycle::Status;
pub use projection::{
    briefing, inbox, kanban, process_list, Briefing, BriefingItem, Card, Inbox, KanbanBoard, Reason,
};
pub use state::State;
pub use store::MemStore;
