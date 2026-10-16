/// Requests validated by `State::decide`; each yields at most one event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    CreateTeam {
        name: String,
    },
    AddMember {
        name: String,
        role: String,
        runtime: Option<String>,
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
    /// Edit subject and/or description; `None` leaves a field unchanged.
    UpdateTask {
        task: String,
        subject: Option<String>,
        description: Option<String>,
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
        /// One of do / ask / delegate.
        mode: Option<String>,
        /// Sender's team for a cross-team message.
        from_team: Option<String>,
    },
    /// `None` leaves a field unchanged.
    ConfigureTeam {
        project_path: Option<String>,
        lead_prompt: Option<String>,
    },
    /// `None` leaves a field unchanged.
    UpdateMember {
        name: String,
        role: Option<String>,
        prompt: Option<String>,
        model: Option<String>,
        runtime: Option<String>,
        worktree: Option<bool>,
        /// Avatar icon; `Some("")` clears it back to the default.
        icon: Option<String>,
    },
    /// Soft remove (`true`) or restore (`false`).
    RemoveMember {
        name: String,
        removed: bool,
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
    /// `None` clears the question.
    SetClarification {
        task: String,
        question: Option<String>,
    },
    AttachFile {
        task: String,
        name: String,
        path: String,
    },
    /// Symmetric link (`linked`) or unlink between two tasks.
    LinkTasks {
        task: String,
        related: String,
        linked: bool,
    },
    StartReview {
        task: String,
    },
    SetColumnOrder {
        column: String,
        order: Vec<String>,
    },
    DeleteTeam {
        deleted: bool,
    },
    /// Toggle: the same actor + emoji twice removes the reaction.
    React {
        message_seq: u64,
        emoji: String,
    },
    /// Upsert by id.
    SetSchedule {
        schedule: crate::Schedule,
    },
    RemoveSchedule {
        id: String,
    },
    RequestPermission {
        id: String,
        member: String,
        tool: String,
        input: serde_json::Value,
    },
    DecidePermission {
        id: String,
        allow: bool,
        note: Option<String>,
    },
}
