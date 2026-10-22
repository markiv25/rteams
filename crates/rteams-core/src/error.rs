use crate::lifecycle::Status;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    #[error("illegal transition for task {task}: {from:?} -> {to:?}")]
    IllegalTransition {
        task: String,
        from: Status,
        to: Status,
    },
    #[error(
        "task {task}: {to:?} is reached through its dedicated command, not a plain transition"
    )]
    UseDedicatedCommand { task: String, to: Status },
    #[error("task {0} is not blocked")]
    NotBlocked(String),
    #[error("task {0} not found")]
    TaskNotFound(String),
    #[error("task update must change the subject or the description")]
    EmptyUpdate,
    #[error("task subject must not be empty")]
    EmptySubject,
    #[error("task {0} already exists")]
    TaskExists(String),
    #[error("task {task} cannot depend on {depends_on}: that would form a cycle")]
    DependencyCycle { task: String, depends_on: String },
    #[error("team already created")]
    TeamExists,
    #[error("team not created yet")]
    TeamNotCreated,
    #[error("member {0} already exists")]
    MemberExists(String),
    #[error("unknown member {0}")]
    UnknownMember(String),
    #[error("team must keep at least one lead; cannot remove {0}")]
    LastLead(String),
    #[error("member role must not be empty")]
    EmptyRole,
    #[error("message mode must be one of do, ask, delegate (got {0:?})")]
    InvalidMode(String),
    #[error("configuration update must change at least one field")]
    EmptyConfig,
    #[error("member update must change at least one field")]
    EmptyMemberUpdate,
    #[error("member runtime must not be empty")]
    EmptyRuntime,
    #[error("team is deleted; restore it first")]
    TeamDeleted,
    #[error("{0} must not be empty")]
    EmptyField(&'static str),
    #[error("task {task} must be in review to start a review (is {status:?})")]
    NotInReview { task: String, status: Status },
    #[error("task {task} cannot be linked to {related}")]
    InvalidLink { task: String, related: String },
    #[error("unknown kanban column {0:?}")]
    UnknownColumn(String),
    #[error("message {0} not found")]
    MessageNotFound(u64),
    #[error("schedule {0} not found")]
    ScheduleNotFound(String),
    #[error("schedule interval must be at least 1 minute")]
    InvalidInterval,
    #[error("permission request {0} already exists")]
    PermissionExists(String),
    #[error("permission request {0} not found")]
    PermissionNotFound(String),
    #[error("permission request {0} was already decided")]
    PermissionDecided(String),
    #[error("group chat {0} not found")]
    GroupChatNotFound(String),
    #[error("group chat {0} already exists")]
    GroupChatExists(String),
    #[error("a message to a group chat cannot also have a direct recipient")]
    ChatWithRecipient,
    #[error("group chat update must change the name or the members")]
    EmptyGroupUpdate,
    #[error("{reviewer} is already a reviewer of task {task}")]
    ReviewerExists { task: String, reviewer: String },
    #[error("{reviewer} is not a reviewer of task {task}")]
    ReviewerNotFound { task: String, reviewer: String },
    #[error("task {0} is not archived")]
    NotArchived(String),
    #[error("task {task} cannot be moved from {from:?} to {to:?}")]
    NoPath {
        task: String,
        from: Status,
        to: Status,
    },
    #[error("column {0:?} cannot be cleared (only approved or done)")]
    ColumnNotClearable(String),
    #[error("nothing to clear in column {0:?}")]
    ColumnEmpty(String),
    #[error("invalid heartbeat status {0:?} (starting, ready, working, idle, stuck)")]
    InvalidHeartbeat(String),
    #[error("invalid effort {0:?} (low, medium, high)")]
    InvalidEffort(String),
    #[error("process {0} is already registered and running")]
    ProcessRunning(u32),
    #[error("no running process with pid {0}")]
    ProcessNotFound(u32),
}
