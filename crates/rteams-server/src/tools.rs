//! Tool surface. Each write tool maps to exactly one core `Command`, so the state machine
//! in rteams-core is the only place lifecycle rules live. Writes name their `actor`.
//!
//! Folded on purpose: `task_start` also restarts needs_fix and reopens done tasks (all are
//! a plain transition to in_progress), so there is no separate set_status/transition tool.
//! Review, direct messages and the process registry arrive in Phase 3.

use std::time::{SystemTime, UNIX_EPOCH};

use rteams_core::{briefing, kanban, Command, CoreError, MemStore, Scope, State, Status, Task};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug)]
pub enum ToolError {
    /// A domain rule rejected the call (reported as an MCP tool error).
    Core(CoreError),
    UnknownTool(String),
    InvalidArguments(String),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::Core(e) => write!(f, "{e}"),
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

#[derive(Deserialize)]
struct Briefing {
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

/// Run one command and return the seq of the event it produced.
fn exec(store: &mut MemStore, actor: &str, cmd: Command) -> Result<u64, ToolError> {
    Ok(store.execute(actor, now_ms(), cmd)?.seq)
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
fn task_reply(store: &MemStore, seq: u64, id: &str) -> Value {
    json!({ "event_seq": seq, "task": task_json(store.state(), id) })
}

fn transition(store: &mut MemStore, a: TaskAct, to: Status) -> Result<Value, ToolError> {
    let seq = exec(
        store,
        &a.actor,
        Command::Transition {
            task: a.task.clone(),
            to,
        },
    )?;
    Ok(task_reply(store, seq, &a.task))
}

pub fn call(store: &mut MemStore, name: &str, raw: Value) -> Result<Value, ToolError> {
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
            exec(store, &a.actor, Command::CreateTeam { name: a.name })?;
            // The creator is the lead; without this they could not own, assign or message.
            let lead = Command::AddMember {
                name: a.actor.clone(),
                role: "lead".into(),
            };
            exec(store, &a.actor, lead)?;
            for m in a.members {
                exec(
                    store,
                    &a.actor,
                    Command::AddMember {
                        name: m.name,
                        role: m.role,
                    },
                )?;
            }
            Ok(json!({ "team": store.state().team }))
        }
        "team_get" => {
            let team = store
                .state()
                .team
                .as_ref()
                .ok_or(CoreError::TeamNotCreated)?;
            Ok(json!({ "team": team }))
        }
        "task_create" => {
            let a: TaskCreate = args(raw)?;
            // Tasks are never deleted, so count + 1 is unique and human-friendly.
            let id = (store.state().tasks.len() + 1).to_string();
            let cmd = Command::CreateTask {
                id: id.clone(),
                subject: a.subject,
                description: a.description,
                owner: a.owner,
                blocked_by: a.blocked_by,
            };
            let seq = exec(store, &a.actor, cmd)?;
            Ok(task_reply(store, seq, &id))
        }
        "task_get" => {
            let a: TaskRef = args(raw)?;
            let state = store.state();
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
                .state()
                .tasks
                .iter()
                .filter(|t| a.include_archived || !t.archived)
                .filter(|t| a.status.is_none_or(|s| t.status == s))
                .filter(|t| a.owner.as_ref().is_none_or(|o| t.owner.as_ref() == Some(o)))
                .collect();
            Ok(json!({ "tasks": tasks }))
        }
        "kanban_get" => Ok(json!({ "board": kanban(store.state()) })),
        "task_briefing" => {
            let a: Briefing = args(raw)?;
            Ok(json!({ "briefing": briefing(store.state(), &a.member) }))
        }
        "task_start" => transition(store, args(raw)?, Status::InProgress),
        "task_complete" => transition(store, args(raw)?, Status::Done),
        "task_set_dependencies" => {
            let a: SetDeps = args(raw)?;
            let cmd = Command::SetDependencies {
                task: a.task.clone(),
                blocked_by: a.blocked_by,
            };
            let seq = exec(store, &a.actor, cmd)?;
            Ok(task_reply(store, seq, &a.task))
        }
        "task_set_owner" => {
            let a: SetOwner = args(raw)?;
            let seq = exec(
                store,
                &a.actor,
                Command::AssignTask {
                    task: a.task.clone(),
                    owner: a.owner,
                },
            )?;
            Ok(task_reply(store, seq, &a.task))
        }
        "task_add_comment" => {
            let a: Comment = args(raw)?;
            let seq = exec(
                store,
                &a.actor,
                Command::Comment {
                    task: a.task.clone(),
                    text: a.text,
                },
            )?;
            Ok(task_reply(store, seq, &a.task))
        }
        "task_block" => {
            let a: Block = args(raw)?;
            let seq = exec(
                store,
                &a.actor,
                Command::Block {
                    task: a.task.clone(),
                    reason: a.reason,
                },
            )?;
            Ok(task_reply(store, seq, &a.task))
        }
        "task_unblock" => {
            let a: TaskAct = args(raw)?;
            let seq = exec(
                store,
                &a.actor,
                Command::Unblock {
                    task: a.task.clone(),
                },
            )?;
            Ok(task_reply(store, seq, &a.task))
        }
        "task_archive" => {
            let a: Archive = args(raw)?;
            let seq = exec(
                store,
                &a.actor,
                Command::ArchiveTask {
                    task: a.task.clone(),
                    archived: a.archived,
                },
            )?;
            Ok(task_reply(store, seq, &a.task))
        }
        other => Err(ToolError::UnknownTool(other.to_string())),
    }
}

// ---- tools/list ----

fn tool(name: &str, description: &str, props: Value, required: &[&str]) -> Value {
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
            "What a member should do now: actionable tasks in priority order, plus awareness items.",
            json!({ "member": s("Member name") }),
            &["member"],
        ),
        tool(
            "task_start",
            "Move a task to in_progress. Also restarts a needs_fix task or reopens a done one.",
            json!({ "task": task, "actor": actor }),
            &["task", "actor"],
        ),
        tool(
            "task_complete",
            "Move an in_progress task (or an approved one) to done.",
            json!({ "task": task, "actor": actor }),
            &["task", "actor"],
        ),
        tool(
            "task_set_dependencies",
            "Replace the tasks this one waits on (empty list clears). Cycles are rejected. \
             Nothing moves automatically: while a dependency is not done, a pending task \
             leaves your actionable briefing and shows as waiting_on_dependencies.",
            json!({ "task": task, "actor": actor, "blocked_by": ids }),
            &["task", "actor"],
        ),
        tool(
            "task_set_owner",
            "Assign a task to a member, or unassign it by omitting owner.",
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
            "Park a task that cannot proceed. Allowed from pending, in_progress, needs_fix.",
            json!({ "task": task, "actor": actor, "reason": s("What it is waiting on") }),
            &["task", "actor", "reason"],
        ),
        tool(
            "task_unblock",
            "Return a blocked task to the status it was blocked from.",
            json!({ "task": task, "actor": actor }),
            &["task", "actor"],
        ),
        tool(
            "task_archive",
            "Hide a task from the board and lists (archived defaults to true; false restores).",
            json!({ "task": task, "actor": actor, "archived": { "type": "boolean" } }),
            &["task", "actor"],
        ),
    ]
}
