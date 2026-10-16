/// Requests validated by `State::decide`; each yields at most one event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    CreateTeam {
        name: String,
    },
    AddMember {
        name: String,
        role: String,
    },
    CreateTask {
        id: String,
        subject: String,
        description: String,
        owner: Option<String>,
        blocked_by: Vec<String>,
    },
    SetDependencies {
        task: String,
        blocked_by: Vec<String>,
    },
    AssignTask {
        task: String,
        owner: Option<String>,
    },
    /// Plain transitions only: start, complete, reopen. Review/block have their own commands.
    Transition {
        task: String,
        to: crate::Status,
    },
    Block {
        task: String,
        reason: String,
    },
    Unblock {
        task: String,
    },
    ArchiveTask {
        task: String,
        archived: bool,
    },
    Comment {
        task: String,
        text: String,
    },
    RequestReview {
        task: String,
        reviewer: String,
    },
    ApproveReview {
        task: String,
        note: Option<String>,
    },
    RequestChanges {
        task: String,
        comment: String,
    },
    SendMessage {
        to: Option<String>,
        text: String,
    },
    RegisterProcess {
        pid: u32,
        label: String,
        port: Option<u16>,
        url: Option<String>,
        command: Option<String>,
    },
    StopProcess {
        pid: u32,
    },
}
