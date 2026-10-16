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
        CoreError::TeamExists => "team_exists",
        CoreError::TeamNotCreated => "team_not_created",
        CoreError::MemberExists(_) => "member_exists",
        CoreError::UnknownMember(_) => "unknown_member",
        CoreError::ProcessRunning(_) => "process_running",
        CoreError::ProcessNotFound(_) => "process_not_found",
    }
}

// ---- argument shapes ----

#[derive(Deserialize)]
struct MemberArg {
    name: String,
    role: String,
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

fn args<T: DeserializeOwned>(v: Value) -> Result<T, ToolError> {
    serde_json::from_value(v).map_err(|e| ToolError::InvalidArguments(e.to_string()))
}

// ---- dispatch ----

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Optional idempotency key on any write tool: a retry with the same key returns the
/// original result instead of appending a second event.
fn command_id(raw: &Value) -> Option<String> {
    raw.get("command_id")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Run one command and return the event it produced. The store locks, catches up with other
/// processes, then decides and appends, so the command is validated against the latest state.
fn exec(
    store: &mut dyn Store,
    cid: Option<&str>,
    actor: &str,
    cmd: Command,
) -> Result<rteams_core::Event, ToolError> {
    Ok(store.execute_with_id(actor, now_ms(), cid, cmd)?.event)
}

/// The task plus the derived dependency views: `blocks` (inverse of blocked_by) and
/// `waiting_on` (dependencies not yet done).
fn task_json(state: &State, id: &str) -> Value {
    let Some(task) = state.task(id) else {
        return Value::Null;
    };
    let mut v = serde_json::to_value(task).unwrap_or(Value::Null);
    v["blocks"] = json!(state.dependents(id));
    v["waiting_on"] = json!(state.unmet_dependencies(task));
    v
}

/// Uniform reply for task writes: the event that happened and the task as it now stands.
fn task_reply(store: &mut dyn Store, seq: u64, id: &str) -> Result<Value, ToolError> {
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

fn is_lead(state: &State, name: &str) -> bool {
    state
        .team
        .as_ref()
        .is_some_and(|t| t.members.iter().any(|m| m.name == name && m.role == "lead"))
}

/// Allow `actor` if they are `expected` (e.g. the task owner) or a lead.
fn authorize(state: &State, actor: &str, expected: Option<&str>, who: &str) -> Result<(), String> {
    if expected == Some(actor) || is_lead(state, actor) {
        Ok(())
    } else {
        Err(format!("{actor} is not the {who}"))
    }
}

/// Working agreement handed to every member on bootstrap.
const PROTOCOL: [&str; 5] = [
    "Call task_briefing for your work queue: act on `actionable` in order; `awareness` is watch-only.",
    "task_start before working a task; task_add_comment with your results; then task_complete.",
    "Ask for review with review_request (owner or lead only); the reviewer answers with review_approve or review_request_changes.",
    "When blocked, task_block with a reason and message_send to a lead; task_unblock when resolved.",
    "Poll inbox_get with the next_cursor from your last call; there are no work-sync tokens, the briefing is derived on demand.",
];

fn require_member(state: &State, actor: &str) -> Result<(), ToolError> {
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
fn guarded(
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
fn owner_or_lead(
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
            };
            exec(store, step("lead").as_deref(), &actor, lead)?;
            for (i, m) in a.members.into_iter().enumerate() {
                let cmd = Command::AddMember {
                    name: m.name,
                    role: m.role,
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
            // Tasks are never deleted, so count + 1 is unique and human-friendly. Another process
            // can take the same id between this read and the locked append; the store then
            // rejects it with TaskExists and has caught up, so we simply pick the next id.
            // Each retry means someone else made progress, so this converges.
            let mut attempts = 0;
            let (seq, id) = loop {
                let id = (store.state()?.tasks.len() + 1).to_string();
                let cmd = Command::CreateTask {
                    id: id.clone(),
                    subject: a.subject.clone(),
                    description: a.description.clone(),
                    owner: a.owner.clone(),
                    blocked_by: a.blocked_by.clone(),
                };
                match exec(store, cid, &a.actor, cmd) {
                    Err(ToolError::Core(CoreError::TaskExists(_))) if attempts < 64 => {
                        attempts += 1
                    }
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
            require_member(store.state()?, &a.actor)?;
            let seq = exec(
                store,
                cid,
                &a.actor,
                Command::SendMessage {
                    to: a.to,
                    text: a.text,
                },
            )?
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
        other => Err(ToolError::UnknownTool(other.to_string())),
    }
}

// ---- tools/list ----

fn tool(name: &str, description: &str, mut props: Value, required: &[&str]) -> Value {
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

pub fn definitions() -> Vec<Value> {
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
            "task_set_owner",
            "Assign a task to a member, or unassign it by omitting owner. Owner or lead only.",
            json!({ "task": task, "actor": actor, "owner": s("Member name") }),
            &["task", "actor"],
        ),
        tool(
            "task_add_comment",
            "Add a comment to a task. The owner and reviewer see it in their inbox.",
            json!({ "task": task, "actor": actor, "text": s("Comment text") }),
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
            json!({ "actor": actor, "to": s("Recipient member; omit to broadcast"), "text": s("Message") }),
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
            "process_unregister",
            "Remove a process from the registry (does not kill it). Process owner or lead only.",
            json!({ "actor": actor, "pid": { "type": "integer", "minimum": 1 } }),
            &["actor", "pid"],
        ),
    ]
}
