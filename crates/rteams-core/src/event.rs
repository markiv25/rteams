use serde::{Deserialize, Serialize};

use crate::lifecycle::Status;

/// One append-only log entry. `seq` is 1-based and gapless per team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    /// Unix millis, supplied by the caller.
    pub ts: i64,
    pub actor: String,
    #[serde(flatten)]
    pub kind: EventKind,
}

/// Facts, not requests. Review events double as the transitions into
/// review / approved / needs_fix so a status change is never recorded twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    TeamCreated {
        name: String,
    },
    MemberAdded {
        name: String,
        role: String,
    },
    TaskCreated {
        task: String,
        subject: String,
        description: String,
        owner: Option<String>,
        #[serde(default)]
        blocked_by: Vec<String>,
    },
    /// Replaces the whole list, so it covers add and remove.
    TaskDependenciesSet {
        task: String,
        blocked_by: Vec<String>,
    },
    TaskAssigned {
        task: String,
        owner: Option<String>,
    },
    TaskTransitioned {
        task: String,
        from: Status,
        to: Status,
    },
    TaskBlocked {
        task: String,
        reason: String,
        from: Status,
    },
    TaskUnblocked {
        task: String,
        to: Status,
    },
    TaskArchived {
        task: String,
        archived: bool,
    },
    TaskCommented {
        task: String,
        text: String,
    },
    ReviewRequested {
        task: String,
        reviewer: String,
    },
    ReviewApproved {
        task: String,
        note: Option<String>,
    },
    ReviewChangesRequested {
        task: String,
        comment: String,
    },
    /// `to: None` is a broadcast.
    MessageSent {
        to: Option<String>,
        text: String,
    },
    ProcessRegistered {
        pid: u32,
        label: String,
        port: Option<u16>,
        url: Option<String>,
        command: Option<String>,
    },
    ProcessStopped {
        pid: u32,
    },
}
