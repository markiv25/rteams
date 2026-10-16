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
    #[error("process {0} is already registered and running")]
    ProcessRunning(u32),
    #[error("no running process with pid {0}")]
    ProcessNotFound(u32),
}
