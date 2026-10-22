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
        attachments: Vec<crate::Attachment>,
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
        /// Group chat id (the message goes to that chat; `to` must be `None`).
        chat: Option<String>,
        attachments: Vec<crate::Attachment>,
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
        /// low | medium | high; `Some("")` clears.
        effort: Option<String>,
        fast: Option<bool>,
        /// `Some(0)` clears.
        context_limit: Option<u32>,
        /// Replaces the whole list.
        mcp_allow: Option<Vec<String>>,
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
    /// Change the team's display name. The directory (team id) never changes.
    RenameTeam {
        name: String,
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
    CreateGroupChat {
        id: String,
        name: String,
        members: Vec<String>,
    },
    UpdateGroupChat {
        id: String,
        name: Option<String>,
        members: Option<Vec<String>>,
    },
    DeleteGroupChat {
        id: String,
    },
    AddReviewer {
        task: String,
        reviewer: String,
    },
    RemoveReviewer {
        task: String,
        reviewer: String,
    },
    /// Unarchive (the original's tool name).
    RestoreTask {
        task: String,
    },
    /// Move a task to a kanban column (status in snake_case) along legal transitions.
    SetColumn {
        task: String,
        column: String,
    },
    /// Archive every task in a column (approved or done only).
    ClearColumn {
        column: String,
    },
    /// status: starting | ready | working | idle | stuck.
    Heartbeat {
        member: String,
        status: String,
        task: Option<String>,
        note: Option<String>,
    },
    /// Ask the shell to launch these members (empty = everyone).
    LaunchRequest {
        members: Vec<String>,
    },
    /// Ask the shell to stop these members (empty = everyone).
    StopRequest {
        members: Vec<String>,
    },
}
