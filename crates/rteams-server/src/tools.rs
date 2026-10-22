//! Tool surface. Each write tool maps to exactly one core `Command`, so the state machine
//! in rteams-core is the only place lifecycle rules live. Writes name their `actor`.
//!
//! Folded on purpose: `task_start` also restarts needs_fix and reopens done tasks (all are
//! a plain transition to in_progress), so there is no separate set_status/transition tool.
//!
//! Policy lives here, not in core: review actions are limited to the task owner (request)
//! or the assigned reviewer (approve / request changes), with members whose role is `lead`
//! always allowed. Domain errors win over policy errors, so a wrong-state call reports
//! `illegal_transition` rather than `forbidden`. `review_start` is deliberately absent: a
//! task is already `review` once requested, and nothing would change on "start".

use std::time::{SystemTime, UNIX_EPOCH};

use rteams_core::{
    briefing, inbox, kanban, lead_briefing, member_briefing, process_list, Command, CoreError,
    EventKind, Scope, State, Status, Task,
};
use rteams_store::{Store, StoreError};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug)]
pub enum ToolError {
    /// A domain rule rejected the call (reported as an MCP tool error).
    Core(CoreError),
    /// Storage failed (I/O, corrupt log); reported as a tool error.
    Store(String),
    /// The actor is a valid member but not allowed to do this (reported as a tool error).
    Forbidden(String),
    UnknownTool(String),
    InvalidArguments(String),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::Core(e) => write!(f, "{e}"),
            ToolError::Store(m) => write!(f, "store error: {m}"),
            ToolError::Forbidden(m) => write!(f, "forbidden: {m}"),
            ToolError::UnknownTool(n) => write!(f, "unknown tool: {n}"),
            ToolError::InvalidArguments(m) => write!(f, "invalid arguments: {m}"),
        }
    }
}

impl std::error::Error for ToolError {}

impl From<CoreError> for ToolError {
    fn from(e: CoreError) -> Self {
        ToolError::Core(e)
    }
}

impl From<StoreError> for ToolError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Core(e) => ToolError::Core(e),
            StoreError::Forbidden(m) => ToolError::Forbidden(m),
            other => ToolError::Store(other.to_string()),
        }
    }
}

/// Stable machine-readable code for each domain error (exhaustive on purpose).
pub fn error_code(e: &CoreError) -> &'static str {
    match e {
        CoreError::IllegalTransition { .. } => "illegal_transition",
        CoreError::UseDedicatedCommand { .. } => "use_dedicated_command",
        CoreError::NotBlocked(_) => "not_blocked",
        CoreError::DependencyCycle { .. } => "dependency_cycle",
        CoreError::TaskNotFound(_) => "task_not_found",
        CoreError::TaskExists(_) => "task_exists",
        CoreError::EmptyUpdate => "empty_update",
        CoreError::EmptySubject => "empty_subject",
        CoreError::TeamExists => "team_exists",
        CoreError::TeamNotCreated => "team_not_created",
        CoreError::MemberExists(_) => "member_exists",
        CoreError::UnknownMember(_) => "unknown_member",
        CoreError::LastLead(_) => "last_lead",
        CoreError::EmptyRole => "empty_role",
        CoreError::InvalidMode(_) => "invalid_mode",
        CoreError::EmptyConfig => "empty_config",
        CoreError::EmptyMemberUpdate => "empty_member_update",
        CoreError::ProcessRunning(_) => "process_running",
        CoreError::ProcessNotFound(_) => "process_not_found",
        CoreError::EmptyRuntime => "empty_runtime",
        CoreError::TeamDeleted => "team_deleted",
        CoreError::EmptyField(_) => "empty_field",
        CoreError::NotInReview { .. } => "not_in_review",
        CoreError::InvalidLink { .. } => "invalid_link",
        CoreError::UnknownColumn(_) => "unknown_column",
        CoreError::MessageNotFound(_) => "message_not_found",
        CoreError::ScheduleNotFound(_) => "schedule_not_found",
        CoreError::InvalidInterval => "invalid_interval",
        CoreError::PermissionExists(_) => "permission_exists",
        CoreError::PermissionNotFound(_) => "permission_not_found",
        CoreError::PermissionDecided(_) => "permission_decided",
        CoreError::GroupChatNotFound(_) => "group_chat_not_found",
        CoreError::GroupChatExists(_) => "group_chat_exists",
        CoreError::ChatWithRecipient => "chat_with_recipient",
        CoreError::EmptyGroupUpdate => "empty_group_update",
        CoreError::ReviewerExists { .. } => "reviewer_exists",
        CoreError::ReviewerNotFound { .. } => "reviewer_not_found",
        CoreError::NotArchived(_) => "not_archived",
        CoreError::NoPath { .. } => "no_path",
        CoreError::ColumnNotClearable(_) => "column_not_clearable",
        CoreError::ColumnEmpty(_) => "column_empty",
        CoreError::InvalidHeartbeat(_) => "invalid_heartbeat",
        CoreError::InvalidEffort(_) => "invalid_effort",
    }
}

// ---- argument shapes ----

#[derive(Deserialize)]
struct MemberArg {
    name: String,
    role: String,
    #[serde(default)]
    runtime: Option<String>,
}

#[derive(Deserialize)]
struct TeamCreate {
    name: String,
    actor: String,
    #[serde(default)]
    members: Vec<MemberArg>,
}

#[derive(Deserialize)]
struct TaskCreate {
    actor: String,
    subject: String,
    #[serde(default)]
    description: String,
    owner: Option<String>,
    #[serde(default)]
    blocked_by: Vec<String>,
}

#[derive(Deserialize)]
struct SetDeps {
    task: String,
    actor: String,
    /// Replaces the whole list; empty clears it.
    #[serde(default)]
    blocked_by: Vec<String>,
}

#[derive(Deserialize)]
struct TaskRef {
    task: String,
}

#[derive(Deserialize)]
struct TaskAct {
    task: String,
    actor: String,
}

#[derive(Deserialize)]
struct TaskUpdate {
    task: String,
    actor: String,
    subject: Option<String>,
    description: Option<String>,
}

#[derive(Deserialize)]
struct MemberAdd {
    actor: String,
    name: String,
    role: String,
    runtime: Option<String>,
}

#[derive(Deserialize)]
struct TeamConfigure {
    actor: String,
    project_path: Option<String>,
    lead_prompt: Option<String>,
}

#[derive(Deserialize)]
struct MemberUpdate {
    actor: String,
    name: String,
    role: Option<String>,
    prompt: Option<String>,
    model: Option<String>,
    runtime: Option<String>,
    worktree: Option<bool>,
    icon: Option<String>,
    effort: Option<String>,
    fast: Option<bool>,
    context_limit: Option<u32>,
    mcp_allow: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct MemberRemove {
    actor: String,
    name: String,
    #[serde(default = "yes")]
    removed: bool,
}

#[derive(Deserialize)]
struct SetOwner {
    task: String,
    actor: String,
    /// null or absent unassigns.
    owner: Option<String>,
}

#[derive(Deserialize)]
struct Comment {
    task: String,
    actor: String,
    text: String,
    #[serde(default)]
    attachments: Vec<rteams_core::Attachment>,
}

#[derive(Deserialize)]
struct ReviewRequest {
    task: String,
    actor: String,
    reviewer: String,
}

#[derive(Deserialize)]
struct ReviewApprove {
    task: String,
    actor: String,
    note: Option<String>,
}

#[derive(Deserialize)]
struct ReviewChanges {
    task: String,
    actor: String,
    comment: String,
}

#[derive(Deserialize)]
struct MessageSend {
    actor: String,
    /// Absent or null broadcasts to everyone.
    to: Option<String>,
    text: String,
    /// Optional intent: do, ask or delegate.
    mode: Option<String>,
    /// Sender's team for a cross-team message; the actor then need not be a member here.
    from_team: Option<String>,
    /// Group chat id; the message goes to that chat (then `to` must be absent).
    #[serde(default)]
    chat: Option<String>,
    #[serde(default)]
    attachments: Vec<rteams_core::Attachment>,
}

#[derive(Deserialize)]
struct Clarify {
    task: String,
    actor: String,
    /// null or absent clears the question.
    question: Option<String>,
}

#[derive(Deserialize)]
struct Attach {
    task: String,
    actor: String,
    name: String,
    path: String,
}

#[derive(Deserialize)]
struct Link {
    task: String,
    actor: String,
    related: String,
}

#[derive(Deserialize)]
struct SetOrder {
    actor: String,
    column: String,
    #[serde(default)]
    order: Vec<String>,
}

#[derive(Deserialize)]
struct TeamDelete {
    actor: String,
    #[serde(default = "yes")]
    deleted: bool,
}

#[derive(Deserialize)]
struct React {
    actor: String,
    message_seq: u64,
    emoji: String,
}

#[derive(Deserialize)]
struct ScheduleSet {
    actor: String,
    id: String,
    member: String,
    prompt: String,
    every_minutes: u32,
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default)]
    cron: Option<String>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    warmup_minutes: Option<u32>,
}

#[derive(Deserialize)]
struct ScheduleRemove {
    actor: String,
    id: String,
}

#[derive(Deserialize)]
struct PermissionDecide {
    actor: String,
    id: String,
    allow: bool,
    note: Option<String>,
}

#[derive(Deserialize)]
struct TaskFromMessage {
    actor: String,
    message_seq: u64,
    owner: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct PermissionRequestArgs {
    pub tool_name: String,
    #[serde(default)]
    pub input: Value,
}

#[derive(Deserialize)]
struct InboxGet {
    member: String,
    /// Exclusive cursor; pass back `next_cursor` from the previous call.
    #[serde(default)]
    after: u64,
}

#[derive(Deserialize)]
struct ProcessRegister {
    actor: String,
    pid: u32,
    label: String,
    port: Option<u16>,
    url: Option<String>,
    command: Option<String>,
}

#[derive(Deserialize)]
struct ProcessUnregister {
    actor: String,
    pid: u32,
}

#[derive(Deserialize)]
struct Block {
    task: String,
    actor: String,
    reason: String,
}

#[derive(Deserialize)]
struct Archive {
    task: String,
    actor: String,
    #[serde(default = "yes")]
    archived: bool,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct TaskList {
    status: Option<Status>,
    owner: Option<String>,
    #[serde(default)]
    include_archived: bool,
}

#[derive(Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum BriefingRole {
    #[default]
    Member,
    Lead,
}

#[derive(Deserialize)]
struct Briefing {
    member: String,
    #[serde(default)]
    role: BriefingRole,
}

#[derive(Deserialize)]
struct MemberBriefingArgs {
    member: String,
}

pub(crate) fn args<T: DeserializeOwned>(v: Value) -> Result<T, ToolError> {
    serde_json::from_value(v).map_err(|e| ToolError::InvalidArguments(e.to_string()))
}

// ---- dispatch ----

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Optional idempotency key on any write tool: a retry with the same key returns the
/// original result instead of appending a second event.
pub(crate) fn command_id(raw: &Value) -> Option<String> {
    raw.get("command_id")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Run one command and return the event it produced. The store locks, catches up with other
/// processes, then decides and appends, so the command is validated against the latest state.
pub(crate) fn exec(
    store: &mut dyn Store,
    cid: Option<&str>,
    actor: &str,
    cmd: Command,
) -> Result<rteams_core::Event, ToolError> {
    Ok(store.execute_with_id(actor, now_ms(), cid, cmd)?.event)
}

/// The task plus the derived dependency views: `blocks` (inverse of blocked_by) and
/// `waiting_on` (dependencies not yet done).
pub(crate) fn task_json(state: &State, id: &str) -> Value {
    let Some(task) = state.task(id) else {
        return Value::Null;
    };
    let mut v = serde_json::to_value(task).unwrap_or(Value::Null);
    v["blocks"] = json!(state.dependents(id));
    v["waiting_on"] = json!(state.unmet_dependencies(task));
    v
}

/// Uniform reply for task writes: the event that happened and the task as it now stands.
pub(crate) fn task_reply(store: &mut dyn Store, seq: u64, id: &str) -> Result<Value, ToolError> {
    Ok(json!({ "event_seq": seq, "task": task_json(store.state()?, id) }))
}

fn transition(
    store: &mut dyn Store,
    cid: Option<&str>,
    a: TaskAct,
    to: Status,
) -> Result<Value, ToolError> {
    let cmd = Command::Transition {
        task: a.task.clone(),
        to,
    };
    owner_or_lead(store, cid, &a.actor, &a.task, cmd)
}

pub(crate) fn is_lead(state: &State, name: &str) -> bool {
    state.team.as_ref().is_some_and(|t| {
        t.members
            .iter()
            .any(|m| m.name == name && !m.removed && m.role == "lead")
    })
}

/// Policy guard: only an active lead may proceed.
pub(crate) fn lead_only(actor: &str) -> impl Fn(&State) -> Result<(), String> + '_ {
    move |s| {
        if is_lead(s, actor) {
            Ok(())
        } else {
            Err(format!("{actor} does not have the lead role"))
        }
    }
}

/// Allow `actor` if they are `expected` (e.g. the task owner) or a lead.
pub(crate) fn authorize(
    state: &State,
    actor: &str,
    expected: Option<&str>,
    who: &str,
) -> Result<(), String> {
    if !state.is_member(actor) {
        Err(format!("{actor} is not an active team member"))
    } else if expected == Some(actor) || is_lead(state, actor) {
        Ok(())
    } else {
        Err(format!("{actor} is not the {who}"))
    }
}

/// Working agreement handed to every member on bootstrap.
pub(crate) const PROTOCOL: [&str; 5] = [
    "Call task_briefing for your work queue: act on `actionable` in order; `awareness` is watch-only.",
    "task_start before working a task; task_add_comment with your results; then task_complete.",
    "Ask for review with review_request (owner or lead only); the reviewer answers with review_approve or review_request_changes.",
    "When blocked, task_block with a reason and message_send to a lead; task_unblock when resolved.",
    "Poll inbox_get with the next_cursor from your last call; there are no work-sync tokens, the briefing is derived on demand.",
];

pub(crate) fn require_member(state: &State, actor: &str) -> Result<(), ToolError> {
    if state.is_member(actor) {
        Ok(())
    } else {
        Err(CoreError::UnknownMember(actor.to_string()).into())
    }
}

/// Run a command with a policy guard. The store runs the guard under its lock, after the
/// state machine has accepted the command and before the event is appended, so check, decide
/// and append are atomic across processes. A retried `command_id` returns the original result
/// without consulting the guard.
pub(crate) fn guarded(
    store: &mut dyn Store,
    cid: Option<&str>,
    actor: &str,
    task: &str,
    cmd: Command,
    guard: &dyn Fn(&State) -> Result<(), String>,
) -> Result<Value, ToolError> {
    let seq = store
        .execute_guarded(actor, now_ms(), cid, cmd, guard)?
        .event
        .seq;
    task_reply(store, seq, task)
}

/// Policy: `actor` must be whoever `expected` names on the task (or a lead).
fn review(
    store: &mut dyn Store,
    cid: Option<&str>,
    actor: &str,
    task: &str,
    cmd: Command,
    who: &str,
    expected: impl Fn(&Task) -> Option<String>,
) -> Result<Value, ToolError> {
    let guard = |s: &State| match s.task(task) {
        Some(t) => authorize(s, actor, expected(t).as_deref(), who),
        None => Ok(()),
    };
    guarded(store, cid, actor, task, cmd, &guard)
}

/// Task lifecycle writes: only the task owner or a lead may mutate a task.
pub(crate) fn owner_or_lead(
    store: &mut dyn Store,
    cid: Option<&str>,
    actor: &str,
    task: &str,
    cmd: Command,
) -> Result<Value, ToolError> {
    review(store, cid, actor, task, cmd, "task owner", |t| {
        t.owner.clone()
    })
}

/// Tasks are never deleted, so count + 1 is unique and human-friendly. Another process
/// can take the same id between this read and the locked append; the store then
/// rejects it with TaskExists and has caught up, so we simply pick the next id.
/// Each retry means someone else made progress, so this converges.
fn create_task(
    store: &mut dyn Store,
    cid: Option<&str>,
    actor: &str,
    subject: String,
    description: String,
    owner: Option<String>,
    blocked_by: Vec<String>,
) -> Result<Value, ToolError> {
    let mut attempts = 0;
    let (seq, id) = loop {
        let id = (store.state()?.tasks.len() + 1).to_string();
        let cmd = Command::CreateTask {
            id: id.clone(),
            subject: subject.clone(),
            description: description.clone(),
            owner: owner.clone(),
            blocked_by: blocked_by.clone(),
        };
        match exec(store, cid, actor, cmd) {
            Err(ToolError::Core(CoreError::TaskExists(_))) if attempts < 64 => attempts += 1,
            Err(e) => return Err(e),
            Ok(ev) => {
                // A replayed command returns the original event, with its original id.
                let id = match ev.kind {
                    EventKind::TaskCreated { task, .. } => task,
                    _ => id,
                };
                break (ev.seq, id);
            }
        }
    };
    task_reply(store, seq, &id)
}

/// Append a `PermissionRequested` for `member` and return its id. Called by the server's
/// blocking `permission_request` handler, which must not hold the store lock while waiting.
pub(crate) fn permission_open(
    store: &mut dyn Store,
    member: &str,
    a: PermissionRequestArgs,
) -> Result<String, ToolError> {
    let base = format!("{member}-{}", now_ms());
    for n in 0..64 {
        let id = if n == 0 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        let cmd = Command::RequestPermission {
            id: id.clone(),
            member: member.to_string(),
            tool: a.tool_name.clone(),
            input: a.input.clone(),
        };
        match exec(store, None, member, cmd) {
            Err(ToolError::Core(CoreError::PermissionExists(_))) => continue,
            Err(e) => return Err(e),
            Ok(_) => return Ok(id),
        }
    }
    Err(ToolError::Store(
        "could not allocate a permission id".into(),
    ))
}

/// `Some((allow, note))` once the request has been decided.
pub(crate) fn permission_poll(
    store: &mut dyn Store,
    id: &str,
) -> Result<Option<(bool, Option<String>)>, ToolError> {
    Ok(store
        .state()?
        .permissions
        .iter()
        .find(|p| p.id == id)
        .and_then(|p| p.decided.map(|d| (d, p.note.clone()))))
}

/// Deny a request nobody answered. A decision that raced in wins (the error is ignored and the
/// caller polls again).
pub(crate) fn permission_expire(store: &mut dyn Store, member: &str, id: &str) {
    let cmd = Command::DecidePermission {
        id: id.to_string(),
        allow: false,
        note: Some("Permission request timed out".into()),
    };
    let _ = exec(store, None, member, cmd);
}

pub fn call(store: &mut dyn Store, name: &str, raw: Value) -> Result<Value, ToolError> {
    let cid = command_id(&raw);
    let cid = cid.as_deref();
    match name {
        "team_create" => {
            let a: TeamCreate = args(raw)?;
            let mut seen = std::collections::HashSet::from([a.actor.as_str()]);
            if let Some(dup) = a.members.iter().find(|m| !seen.insert(m.name.as_str())) {
                return Err(ToolError::InvalidArguments(format!(
                    "duplicate member {}",
                    dup.name
                )));
            }
            // One logical call, several events: derive a stable key per step so a retried
            // team_create replays each step instead of failing halfway with TeamExists.
            let step = |n: &str| cid.map(|c| format!("{c}/{n}"));
            let actor = a.actor;
            exec(
                store,
                step("team").as_deref(),
                &actor,
                Command::CreateTeam { name: a.name },
            )?;
            // The creator is the lead; without this they could not own, assign or message.
            let lead = Command::AddMember {
                name: actor.clone(),
                role: "lead".into(),
                runtime: None,
            };
            exec(store, step("lead").as_deref(), &actor, lead)?;
            for (i, m) in a.members.into_iter().enumerate() {
                let cmd = Command::AddMember {
                    name: m.name,
                    role: m.role,
                    runtime: m.runtime,
                };
                exec(store, step(&format!("member/{i}")).as_deref(), &actor, cmd)?;
            }
            Ok(json!({ "team": store.state()?.team }))
        }
        "team_get" => {
            let team = store
                .state()?
                .team
                .as_ref()
                .ok_or(CoreError::TeamNotCreated)?;
            Ok(json!({ "team": team }))
        }
        "task_create" => {
            let a: TaskCreate = args(raw)?;
            create_task(
                store,
                cid,
                &a.actor,
                a.subject,
                a.description,
                a.owner,
                a.blocked_by,
            )
        }
        "task_create_from_message" => {
            let a: TaskFromMessage = args(raw)?;
            let state = store.state()?;
            let m = state
                .messages
                .iter()
                .find(|m| m.seq == a.message_seq)
                .ok_or(CoreError::MessageNotFound(a.message_seq))?;
            let first = m.text.lines().next().unwrap_or("").trim();
            let mut subject: String = first.chars().take(80).collect();
            if subject.is_empty() {
                subject = format!("Message #{}", m.seq);
            }
            let description = format!("{}\n\n(from message #{} by {})", m.text, m.seq, m.from);
            create_task(store, cid, &a.actor, subject, description, a.owner, vec![])
        }
        "task_get" => {
            let a: TaskRef = args(raw)?;
            let state = store.state()?;
            if state.task(&a.task).is_none() {
                return Err(CoreError::TaskNotFound(a.task).into());
            }
            let comments: Vec<_> = state
                .messages
                .iter()
                .filter(|m| matches!(&m.scope, Scope::Task { task } if *task == a.task))
                .collect();
            Ok(json!({ "task": task_json(state, &a.task), "comments": comments }))
        }
        "task_list" => {
            let a: TaskList = args(raw)?;
            let tasks: Vec<&Task> = store
                .state()?
                .tasks
                .iter()
                .filter(|t| a.include_archived || !t.archived)
                .filter(|t| a.status.is_none_or(|s| t.status == s))
                .filter(|t| a.owner.as_ref().is_none_or(|o| t.owner.as_ref() == Some(o)))
                .collect();
            Ok(json!({ "tasks": tasks }))
        }
        "kanban_get" => Ok(json!({ "board": kanban(store.state()?) })),
        "task_briefing" => {
            let a: Briefing = args(raw)?;
            let state = store.state()?;
            require_member(state, &a.member)?;
            let mine = briefing(state, &a.member);
            match a.role {
                BriefingRole::Member => Ok(json!({ "briefing": mine })),
                BriefingRole::Lead => {
                    if !is_lead(state, &a.member) {
                        return Err(ToolError::Forbidden(format!(
                            "{} does not have the lead role",
                            a.member
                        )));
                    }
                    Ok(json!({ "briefing": mine, "lead": lead_briefing(state) }))
                }
            }
        }
        "member_briefing" => {
            let a: MemberBriefingArgs = args(raw)?;
            let state = store.state()?;
            match member_briefing(state, &a.member) {
                Some(b) => Ok(json!({ "member_briefing": b, "protocol": PROTOCOL })),
                None if state.team.is_none() => Err(CoreError::TeamNotCreated.into()),
                None => Err(CoreError::UnknownMember(a.member).into()),
            }
        }
        "task_start" => transition(store, cid, args(raw)?, Status::InProgress),
        "task_complete" => transition(store, cid, args(raw)?, Status::Done),
        "task_set_dependencies" => {
            let a: SetDeps = args(raw)?;
            let cmd = Command::SetDependencies {
                task: a.task.clone(),
                blocked_by: a.blocked_by,
            };
            owner_or_lead(store, cid, &a.actor, &a.task, cmd)
        }
        "task_update" => {
            let a: TaskUpdate = args(raw)?;
            let cmd = Command::UpdateTask {
                task: a.task.clone(),
                subject: a.subject,
                description: a.description,
            };
            owner_or_lead(store, cid, &a.actor, &a.task, cmd)
        }
        "member_add" => {
            let a: MemberAdd = args(raw)?;
            let cmd = Command::AddMember {
                name: a.name,
                role: a.role,
                runtime: a.runtime,
            };
            let actor = a.actor.as_str();
            store.execute_guarded(actor, now_ms(), cid, cmd, &lead_only(actor))?;
            Ok(json!({ "team": store.state()?.team }))
        }
        "team_configure" => {
            let a: TeamConfigure = args(raw)?;
            let cmd = Command::ConfigureTeam {
                project_path: a.project_path,
                lead_prompt: a.lead_prompt,
            };
            let actor = a.actor.as_str();
            store.execute_guarded(actor, now_ms(), cid, cmd, &lead_only(actor))?;
            Ok(json!({ "team": store.state()?.team }))
        }
        "member_update" => {
            let a: MemberUpdate = args(raw)?;
            let (actor, name) = (a.actor.as_str(), a.name.as_str());
            let sets_role = a.role.is_some();
            let sets_runtime = a.runtime.is_some()
                || a.worktree.is_some()
                || a.effort.is_some()
                || a.fast.is_some()
                || a.context_limit.is_some()
                || a.mcp_allow.is_some();
            let cmd = Command::UpdateMember {
                name: a.name.clone(),
                role: a.role.clone(),
                prompt: a.prompt.clone(),
                model: a.model.clone(),
                runtime: a.runtime.clone(),
                worktree: a.worktree,
                icon: a.icon.clone(),
                effort: a.effort.clone(),
                fast: a.fast,
                context_limit: a.context_limit,
                mcp_allow: a.mcp_allow.clone(),
            };
            // A lead edits anyone; a member may edit only their own prompt, model and icon.
            let guard = |s: &State| {
                if is_lead(s, actor) {
                    Ok(())
                } else if !s.is_member(actor) {
                    Err(format!("{actor} is not an active team member"))
                } else if actor != name {
                    Err(format!(
                        "{actor} may only update their own prompt, model and icon"
                    ))
                } else if sets_role {
                    Err(format!("{actor} cannot change their own role; ask a lead"))
                } else if sets_runtime {
                    Err(format!(
                        "{actor} cannot change their own runtime or worktree; ask a lead"
                    ))
                } else {
                    Ok(())
                }
            };
            store.execute_guarded(actor, now_ms(), cid, cmd, &guard)?;
            Ok(json!({ "team": store.state()?.team }))
        }
        "member_remove" => {
            let a: MemberRemove = args(raw)?;
            let cmd = Command::RemoveMember {
                name: a.name,
                removed: a.removed,
            };
            let actor = a.actor.as_str();
            store.execute_guarded(actor, now_ms(), cid, cmd, &lead_only(actor))?;
            Ok(json!({ "team": store.state()?.team }))
        }
        "task_set_owner" => {
            let a: SetOwner = args(raw)?;
            let cmd = Command::AssignTask {
                task: a.task.clone(),
                owner: a.owner.clone(),
            };
            // Owner or lead may reassign; anyone on the team may claim an unowned task for
            // themselves (never steal an owned one).
            let (actor, task, claim) = (a.actor.as_str(), a.task.as_str(), a.owner.as_deref());
            let guard = |s: &State| match s.task(task) {
                Some(t) if t.owner.is_none() && claim == Some(actor) => Ok(()),
                Some(t) => authorize(s, actor, t.owner.as_deref(), "task owner"),
                None => Ok(()),
            };
            guarded(store, cid, actor, task, cmd, &guard)
        }
        "task_add_comment" => {
            let a: Comment = args(raw)?;
            require_member(store.state()?, &a.actor)?;
            let seq = exec(
                store,
                cid,
                &a.actor,
                Command::Comment {
                    task: a.task.clone(),
                    text: a.text,
                    attachments: a.attachments,
                },
            )?
            .seq;
            task_reply(store, seq, &a.task)
        }
        "task_block" => {
            let a: Block = args(raw)?;
            let cmd = Command::Block {
                task: a.task.clone(),
                reason: a.reason,
            };
            owner_or_lead(store, cid, &a.actor, &a.task, cmd)
        }
        "task_unblock" => {
            let a: TaskAct = args(raw)?;
            let cmd = Command::Unblock {
                task: a.task.clone(),
            };
            owner_or_lead(store, cid, &a.actor, &a.task, cmd)
        }
        "task_archive" => {
            let a: Archive = args(raw)?;
            let cmd = Command::ArchiveTask {
                task: a.task.clone(),
                archived: a.archived,
            };
            owner_or_lead(store, cid, &a.actor, &a.task, cmd)
        }
        "review_request" => {
            let a: ReviewRequest = args(raw)?;
            let cmd = Command::RequestReview {
                task: a.task.clone(),
                reviewer: a.reviewer.clone(),
            };
            let (actor, task, reviewer) = (a.actor.as_str(), a.task.as_str(), a.reviewer.as_str());
            let guard = |s: &State| match s.task(task) {
                Some(t) => {
                    authorize(s, actor, t.owner.as_deref(), "task owner")?;
                    if t.owner.as_deref() == Some(reviewer) {
                        return Err("the owner cannot review their own task".into());
                    }
                    Ok(())
                }
                None => Ok(()),
            };
            guarded(store, cid, actor, task, cmd, &guard)
        }
        "review_approve" => {
            let a: ReviewApprove = args(raw)?;
            let cmd = Command::ApproveReview {
                task: a.task.clone(),
                note: a.note,
            };
            review(
                store,
                cid,
                &a.actor,
                &a.task,
                cmd,
                "assigned reviewer",
                |t| t.reviewer.clone(),
            )
        }
        "review_request_changes" => {
            let a: ReviewChanges = args(raw)?;
            let cmd = Command::RequestChanges {
                task: a.task.clone(),
                comment: a.comment,
            };
            review(
                store,
                cid,
                &a.actor,
                &a.task,
                cmd,
                "assigned reviewer",
                |t| t.reviewer.clone(),
            )
        }
        "message_send" => {
            let a: MessageSend = args(raw)?;
            // A cross-team sender lives in another team's roster.
            if a.from_team.is_none() {
                require_member(store.state()?, &a.actor)?;
            }
            let actor = a.actor.as_str();
            let chat = a.chat.clone();
            let cmd = Command::SendMessage {
                to: a.to.clone(),
                text: a.text.clone(),
                mode: a.mode.clone(),
                from_team: a.from_team.clone(),
                chat: a.chat.clone(),
                attachments: a.attachments.clone(),
            };
            // A group chat is for its members (and leads); domain errors still win.
            let guard = |s: &State| match chat.as_deref().and_then(|c| s.group(c)) {
                Some(g) if !g.members.iter().any(|m| m == actor) && !is_lead(s, actor) => {
                    Err(format!("{actor} is not a member of group chat {}", g.id))
                }
                _ => Ok(()),
            };
            let seq = store
                .execute_guarded(actor, now_ms(), cid, cmd, &guard)?
                .event
                .seq;
            Ok(json!({ "event_seq": seq }))
        }
        "inbox_get" => {
            let a: InboxGet = args(raw)?;
            Ok(json!({ "inbox": inbox(store.state()?, &a.member, a.after) }))
        }
        "process_register" => {
            let a: ProcessRegister = args(raw)?;
            require_member(store.state()?, &a.actor)?;
            let cmd = Command::RegisterProcess {
                pid: a.pid,
                label: a.label,
                port: a.port,
                url: a.url,
                command: a.command,
            };
            let seq = exec(store, cid, &a.actor, cmd)?.seq;
            Ok(json!({ "event_seq": seq, "processes": process_list(store.state()?) }))
        }
        "process_list" => Ok(json!({ "processes": process_list(store.state()?) })),
        "process_unregister" => {
            let a: ProcessUnregister = args(raw)?;
            let (actor, pid) = (a.actor.as_str(), a.pid);
            let guard = |s: &State| match s.processes.iter().find(|p| p.pid == pid && p.running) {
                Some(p) => authorize(s, actor, Some(p.owner.as_str()), "process owner"),
                None => Ok(()),
            };
            let seq = store
                .execute_guarded(actor, now_ms(), cid, Command::StopProcess { pid }, &guard)?
                .event
                .seq;
            Ok(json!({ "event_seq": seq, "processes": process_list(store.state()?) }))
        }
        "task_set_clarification" => {
            let a: Clarify = args(raw)?;
            let cmd = Command::SetClarification {
                task: a.task.clone(),
                question: a.question,
            };
            owner_or_lead(store, cid, &a.actor, &a.task, cmd)
        }
        "task_attach_file" => {
            let a: Attach = args(raw)?;
            require_member(store.state()?, &a.actor)?;
            let cmd = Command::AttachFile {
                task: a.task.clone(),
                name: a.name,
                path: a.path,
            };
            let seq = exec(store, cid, &a.actor, cmd)?.seq;
            task_reply(store, seq, &a.task)
        }
        "task_link" | "task_unlink" => {
            let a: Link = args(raw)?;
            let cmd = Command::LinkTasks {
                task: a.task.clone(),
                related: a.related,
                linked: name == "task_link",
            };
            owner_or_lead(store, cid, &a.actor, &a.task, cmd)
        }
        "review_start" => {
            let a: TaskAct = args(raw)?;
            let cmd = Command::StartReview {
                task: a.task.clone(),
            };
            review(
                store,
                cid,
                &a.actor,
                &a.task,
                cmd,
                "assigned reviewer",
                |t| t.reviewer.clone(),
            )
        }
        "kanban_set_order" => {
            let a: SetOrder = args(raw)?;
            let cmd = Command::SetColumnOrder {
                column: a.column,
                order: a.order,
            };
            let actor = a.actor.as_str();
            let seq = store
                .execute_guarded(actor, now_ms(), cid, cmd, &lead_only(actor))?
                .event
                .seq;
            Ok(
                json!({ "event_seq": seq, "column_order": store.state()?.team.as_ref().map(|t| &t.column_order) }),
            )
        }
        "team_delete" => {
            let a: TeamDelete = args(raw)?;
            let cmd = Command::DeleteTeam { deleted: a.deleted };
            let actor = a.actor.as_str();
            store.execute_guarded(actor, now_ms(), cid, cmd, &lead_only(actor))?;
            Ok(json!({ "team": store.state()?.team }))
        }
        "message_react" => {
            let a: React = args(raw)?;
            require_member(store.state()?, &a.actor)?;
            let cmd = Command::React {
                message_seq: a.message_seq,
                emoji: a.emoji,
            };
            let seq = exec(store, cid, &a.actor, cmd)?.seq;
            let reactions = store
                .state()?
                .messages
                .iter()
                .find(|m| m.seq == a.message_seq)
                .map(|m| m.reactions.clone())
                .unwrap_or_default();
            Ok(json!({ "event_seq": seq, "reactions": reactions }))
        }
        "schedule_set" => {
            let a: ScheduleSet = args(raw)?;
            let cmd = Command::SetSchedule {
                schedule: rteams_core::Schedule {
                    id: a.id,
                    member: a.member,
                    prompt: a.prompt,
                    every_minutes: a.every_minutes,
                    enabled: a.enabled,
                    cron: a.cron,
                    timezone: a.timezone,
                    warmup_minutes: a.warmup_minutes,
                },
            };
            let actor = a.actor.as_str();
            store.execute_guarded(actor, now_ms(), cid, cmd, &lead_only(actor))?;
            Ok(json!({ "team": store.state()?.team }))
        }
        "schedule_remove" => {
            let a: ScheduleRemove = args(raw)?;
            let cmd = Command::RemoveSchedule { id: a.id };
            let actor = a.actor.as_str();
            store.execute_guarded(actor, now_ms(), cid, cmd, &lead_only(actor))?;
            Ok(json!({ "team": store.state()?.team }))
        }
        "permission_decide" => {
            let a: PermissionDecide = args(raw)?;
            let cmd = Command::DecidePermission {
                id: a.id.clone(),
                allow: a.allow,
                note: a.note,
            };
            let actor = a.actor.as_str();
            let seq = store
                .execute_guarded(actor, now_ms(), cid, cmd, &lead_only(actor))?
                .event
                .seq;
            let permission = store.state()?.permissions.iter().find(|p| p.id == a.id);
            Ok(json!({ "event_seq": seq, "permission": permission }))
        }
        other => Err(ToolError::UnknownTool(other.to_string())),
    }
}

// ---- tools/list ----

pub(crate) fn tool(name: &str, description: &str, mut props: Value, required: &[&str]) -> Value {
    // Every write tool names its actor; every write tool accepts an idempotency key.
    if props.get("actor").is_some() {
        props["command_id"] = json!({
            "type": "string",
            "description": "Optional idempotency key: retrying with the same key returns the \
                            original result instead of repeating the change",
        });
    }
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": props, "required": required },
    })
}

pub(crate) fn attachments_schema() -> Value {
    json!({ "type": "array", "description": "Files to attach", "items": {
        "type": "object",
        "properties": { "name": { "type": "string" }, "path": { "type": "string" } },
        "required": ["name", "path"],
    }})
}

pub fn definitions() -> Vec<Value> {
    let mut all = base_definitions();
    all.extend(crate::extra::definitions());
    all
}

fn base_definitions() -> Vec<Value> {
    let s = |d: &str| json!({ "type": "string", "description": d });
    let task = s("Task id");
    let actor = s("Your member name");
    let ids = json!({ "type": "array", "items": { "type": "string" }, "description": "Task ids" });
    let statuses = json!({
        "type": "string",
        "enum": ["pending", "in_progress", "review", "approved", "done", "needs_fix", "blocked"],
    });
    vec![
        tool(
            "team_create",
            "Create the team. The actor joins as 'lead'; members are added with their roles.",
            json!({
                "name": s("Team name"),
                "actor": actor,
                "members": { "type": "array", "items": {
                    "type": "object",
                    "properties": { "name": s("Member name"), "role": s("Role, e.g. developer") },
                    "required": ["name", "role"],
                }},
            }),
            &["name", "actor"],
        ),
        tool("team_get", "Get the team and its roster.", json!({}), &[]),
        tool(
            "task_create",
            "Create a task (starts pending). The owner must be a team member.",
            json!({
                "actor": actor,
                "subject": s("Short title"),
                "description": s("Details"),
                "owner": s("Member who owns it"),
                "blocked_by": ids,
            }),
            &["actor", "subject"],
        ),
        tool(
            "task_get",
            "Get a task with its comments.",
            json!({ "task": task }),
            &["task"],
        ),
        tool(
            "task_list",
            "List tasks, optionally filtered. Archived tasks are hidden unless requested.",
            json!({
                "status": statuses,
                "owner": s("Only this owner's tasks"),
                "include_archived": { "type": "boolean" },
            }),
            &[],
        ),
        tool("kanban_get", "Board columns derived from task status.", json!({}), &[]),
        tool(
            "task_briefing",
            "Your work queue: `briefing.actionable` in priority order, plus watch-only `awareness`. \
             With role=lead (members with the lead role only) the reply also carries `lead`: \
             team-wide unassigned/blocked tasks, items in review, board counts and per-member load.",
            json!({
                "member": s("Your member name"),
                "role": { "type": "string", "enum": ["member", "lead"], "description": "Default member" },
            }),
            &["member"],
        ),
        tool(
            "member_briefing",
            "Bootstrap context for a member: team, your role, who leads, the roster, queue sizes, \
             running processes and the working protocol. Not the work queue; use task_briefing.",
            json!({ "member": s("Your member name") }),
            &["member"],
        ),
        tool(
            "task_start",
            "Move a task to in_progress. Also restarts a needs_fix task or reopens a done one. Owner or lead only.",
            json!({ "task": task, "actor": actor }),
            &["task", "actor"],
        ),
        tool(
            "task_complete",
            "Move an in_progress task (or an approved one) to done. Owner or lead only.",
            json!({ "task": task, "actor": actor }),
            &["task", "actor"],
        ),
        tool(
            "task_set_dependencies",
            "Replace the tasks this one waits on (empty list clears). Cycles are rejected. \
             Nothing moves automatically: while a dependency is not done, a pending task \
             leaves your actionable briefing and shows as waiting_on_dependencies. \
             Owner or lead only.",
            json!({ "task": task, "actor": actor, "blocked_by": ids }),
            &["task", "actor"],
        ),
        tool(
            "task_update",
            "Edit a task's subject and/or description (omitted fields stay unchanged; at least \
             one is required and the subject cannot be blank). Owner or lead only.",
            json!({ "task": task, "actor": actor, "subject": s("New title"), "description": s("New details") }),
            &["task", "actor"],
        ),
        tool(
            "member_add",
            "Add a member to the existing team with a role. Lead only.",
            json!({
                "actor": actor,
                "name": s("New member name"),
                "role": s("Role, e.g. developer"),
                "runtime": s("AI runtime id, e.g. claude (default), codex, opencode, cursor, gemini"),
            }),
            &["actor", "name", "role"],
        ),
        tool(
            "team_configure",
            "Set the team's project_path and/or lead_prompt (standing instructions shown in every \
             member_briefing). Omitted fields stay unchanged; at least one is required. Lead only.",
            json!({
                "actor": actor,
                "project_path": s("Project working directory"),
                "lead_prompt": s("Standing instructions for the whole team"),
            }),
            &["actor"],
        ),
        tool(
            "member_update",
            "Update a member's role, prompt, model and/or icon (omitted fields stay unchanged). A \
             lead may update anyone; a member may update only their own prompt, model and icon.",
            json!({
                "actor": actor,
                "name": s("Member to update"),
                "role": s("New role"),
                "prompt": s("Standing instructions for this member"),
                "model": s("Model this member runs on"),
                "runtime": s("AI runtime id (lead only)"),
                "worktree": { "type": "boolean", "description": "Work in its own git worktree (lead only)" },
                "icon": s("Avatar icon, one emoji (empty string resets to the default)"),
                "effort": s("Reasoning effort low|medium|high, empty clears (lead only)"),
                "fast": { "type": "boolean", "description": "Fast mode (lead only)" },
                "context_limit": { "type": "integer", "minimum": 0, "description": "Context window limit in tokens, 0 clears (lead only)" },
                "mcp_allow": { "type": "array", "items": { "type": "string" }, "description": "Extra MCP server names the member may use; replaces the list (lead only)" },
            }),
            &["actor", "name"],
        ),
        tool(
            "member_remove",
            "Remove a member (removed defaults to true; false restores). History is kept but a \
             removed member cannot act or be assigned. The last lead cannot be removed. Lead only.",
            json!({ "actor": actor, "name": s("Member"), "removed": { "type": "boolean" } }),
            &["actor", "name"],
        ),
        tool(
            "task_set_owner",
            "Assign a task to a member, or unassign it by omitting owner. Owner or lead only.",
            json!({ "task": task, "actor": actor, "owner": s("Member name") }),
            &["task", "actor"],
        ),
        tool(
            "task_add_comment",
            "Add a comment to a task. The owner and reviewer see it in their inbox.",
            json!({ "task": task, "actor": actor, "text": s("Comment text"), "attachments": attachments_schema() }),
            &["task", "actor", "text"],
        ),
        tool(
            "task_block",
            "Park a task that cannot proceed. Allowed from pending, in_progress, needs_fix. Owner or lead only.",
            json!({ "task": task, "actor": actor, "reason": s("What it is waiting on") }),
            &["task", "actor", "reason"],
        ),
        tool(
            "task_unblock",
            "Return a blocked task to the status it was blocked from. Owner or lead only.",
            json!({ "task": task, "actor": actor }),
            &["task", "actor"],
        ),
        tool(
            "task_archive",
            "Hide a task from the board and lists (archived defaults to true; false restores). Owner or lead only.",
            json!({ "task": task, "actor": actor, "archived": { "type": "boolean" } }),
            &["task", "actor"],
        ),
        tool(
            "review_request",
            "Ask a member to review your task (in_progress -> review). Owner or lead only; \
             the reviewer cannot be the owner.",
            json!({ "task": task, "actor": actor, "reviewer": s("Member who reviews") }),
            &["task", "actor", "reviewer"],
        ),
        tool(
            "review_approve",
            "Approve a task in review (review -> approved); the owner then completes it. \
             Assigned reviewer or lead only.",
            json!({ "task": task, "actor": actor, "note": s("Optional approval note") }),
            &["task", "actor"],
        ),
        tool(
            "review_request_changes",
            "Send a task in review back to its owner (review -> needs_fix) with feedback. \
             Assigned reviewer or lead only.",
            json!({ "task": task, "actor": actor, "comment": s("What must change") }),
            &["task", "actor", "comment"],
        ),
        tool(
            "message_send",
            "Send a direct message to a member, or broadcast to everyone by omitting `to`.",
            json!({
                "actor": actor,
                "to": s("Recipient member; omit to broadcast"),
                "text": s("Message"),
                "mode": { "type": "string", "enum": ["do", "ask", "delegate"], "description": "Optional intent of the message" },
                "from_team": s("Sender's team for a cross-team message (actor need not be a member here)"),
                "chat": s("Group chat id to post in (omit `to`); members and leads only"),
                "attachments": attachments_schema(),
            }),
            &["actor", "text"],
        ),
        tool(
            "inbox_get",
            "Messages for a member after a cursor: DMs to them, others' broadcasts, and others' \
             comments on tasks they own or review. Pass back next_cursor as `after`.",
            json!({
                "member": s("Member name"),
                "after": { "type": "integer", "minimum": 0, "description": "Exclusive cursor, default 0" },
            }),
            &["member"],
        ),
        tool(
            "process_register",
            "Record a long-running process (dev server, watcher) so teammates can find it.",
            json!({
                "actor": actor,
                "pid": { "type": "integer", "minimum": 1 },
                "label": s("What it is"),
                "port": { "type": "integer", "minimum": 1, "maximum": 65535 },
                "url": s("URL it serves"),
                "command": s("Command line it was started with"),
            }),
            &["actor", "pid", "label"],
        ),
        tool("process_list", "Running registered processes, in registration order.", json!({}), &[]),
        tool(
            "task_create_from_message",
            "Create a task from a message (by seq): subject = its first line (<= 80 chars), \
             description = the full text plus a back-reference.",
            json!({
                "actor": actor,
                "message_seq": { "type": "integer", "minimum": 1 },
                "owner": s("Member who owns it"),
            }),
            &["actor", "message_seq"],
        ),
        tool(
            "task_set_clarification",
            "Ask a question on a task (it then waits for an answer); omit `question` to clear. \
             Owner or lead only.",
            json!({ "task": task, "actor": actor, "question": s("The question; omit to clear") }),
            &["task", "actor"],
        ),
        tool(
            "task_attach_file",
            "Attach a file (name + path) to a task. Any member.",
            json!({ "task": task, "actor": actor, "name": s("Display name"), "path": s("File path") }),
            &["task", "actor", "name", "path"],
        ),
        tool(
            "task_link",
            "Link two tasks as related (symmetric). Owner or lead only.",
            json!({ "task": task, "actor": actor, "related": s("The other task id") }),
            &["task", "actor", "related"],
        ),
        tool(
            "task_unlink",
            "Remove a related link between two tasks. Owner or lead only.",
            json!({ "task": task, "actor": actor, "related": s("The other task id") }),
            &["task", "actor", "related"],
        ),
        tool(
            "review_start",
            "Mark a task in review as picked up by its reviewer. Assigned reviewer or lead only.",
            json!({ "task": task, "actor": actor }),
            &["task", "actor"],
        ),
        tool(
            "kanban_set_order",
            "Set the manual order of one kanban column (status in snake_case) as a list of task ids. Lead only.",
            json!({ "actor": actor, "column": s("Status, e.g. in_progress"), "order": ids }),
            &["actor", "column", "order"],
        ),
        tool(
            "team_delete",
            "Soft-delete the team (deleted defaults to true; false restores). While deleted all \
             other writes are rejected. Lead only.",
            json!({ "actor": actor, "deleted": { "type": "boolean" } }),
            &["actor"],
        ),
        tool(
            "message_react",
            "Toggle your emoji reaction on a message (by seq); reacting twice removes it.",
            json!({ "actor": actor, "message_seq": { "type": "integer", "minimum": 1 }, "emoji": s("Emoji") }),
            &["actor", "message_seq", "emoji"],
        ),
        tool(
            "schedule_set",
            "Create or replace a recurring prompt for a member. Lead only.",
            json!({
                "actor": actor,
                "id": s("Schedule id"),
                "member": s("Member the prompt is for"),
                "prompt": s("Prompt to send"),
                "every_minutes": { "type": "integer", "minimum": 1 },
                "enabled": { "type": "boolean", "description": "Default true" },
                "cron": s("Optional 5-field cron expression; overrides every_minutes"),
                "timezone": s("Optional IANA timezone for cron, e.g. Europe/Berlin"),
                "warmup_minutes": { "type": "integer", "minimum": 0, "description": "Probe the runtime this many minutes before a fire" },
            }),
            &["actor", "id", "member", "prompt", "every_minutes"],
        ),
        tool(
            "schedule_remove",
            "Remove a schedule. Lead only.",
            json!({ "actor": actor, "id": s("Schedule id") }),
            &["actor", "id"],
        ),
        tool(
            "permission_decide",
            "Allow or deny a pending permission request. Lead only.",
            json!({ "actor": actor, "id": s("Request id"), "allow": { "type": "boolean" }, "note": s("Optional reason") }),
            &["actor", "id", "allow"],
        ),
        tool(
            "permission_request",
            "Claude --permission-prompt-tool contract: ask the lead to approve a tool call and \
             block until decided (10 min timeout denies). Identity comes from RTEAMS_MEMBER.",
            json!({ "tool_name": s("Tool being requested"), "input": { "type": "object", "description": "Tool input" } }),
            &["tool_name", "input"],
        ),
        tool(
            "process_unregister",
            "Remove a process from the registry (does not kill it). Process owner or lead only.",
            json!({ "actor": actor, "pid": { "type": "integer", "minimum": 1 } }),
            &["actor", "pid"],
        ),
    ]
}
