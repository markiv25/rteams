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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime: Option<String>,
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
    /// Only the fields that changed are present, so old logs and readers are unaffected.
    TaskUpdated {
        task: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subject: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_team: Option<String>,
    },
    /// Only the fields that changed are present.
    TeamConfigured {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lead_prompt: Option<String>,
    },
    MemberUpdated {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icon: Option<String>,
    },
    MemberRemoved {
        name: String,
        removed: bool,
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
    TaskClarificationSet {
        task: String,
        question: Option<String>,
    },
    TaskFileAttached {
        task: String,
        name: String,
        path: String,
    },
    TasksLinked {
        task: String,
        related: String,
        linked: bool,
    },
    ReviewStarted {
        task: String,
    },
    ColumnOrderSet {
        column: String,
        order: Vec<String>,
    },
    TeamDeleted {
        deleted: bool,
    },
    MessageReacted {
        message_seq: u64,
        emoji: String,
    },
    ScheduleSet {
        schedule: crate::Schedule,
    },
    ScheduleRemoved {
        id: String,
    },
    PermissionRequested {
        id: String,
        member: String,
        tool: String,
        input: serde_json::Value,
    },
    PermissionDecided {
        id: String,
        allow: bool,
        note: Option<String>,
    },
}
