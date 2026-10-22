//! Tools mirroring the original's wider surface: group chats, comment lookup, extra reviewers,
//! column moves, team management, cross-team messaging and runtime check-ins. Same rules as
//! `tools`: each write is one core `Command` (a few compose two, with derived command ids),
//! policy lives here, domain errors win over policy errors.

use std::path::Path;

use rteams_core::{briefing, lead_briefing, member_briefing, Command, CoreError, Scope, State};
use rteams_store::{FileStore, Store};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::tools::{
    args, authorize, command_id, exec, guarded, is_lead, lead_only, now_ms, owner_or_lead,
    require_member, task_json, task_reply, tool, ToolError, PROTOCOL,
};

#[derive(Deserialize)]
struct ChatCreate {
    actor: String,
    #[serde(default)]
    id: Option<String>,
    name: String,
    #[serde(default)]
    members: Vec<String>,
}

#[derive(Deserialize)]
struct ChatUpdate {
    actor: String,
    id: String,
    name: Option<String>,
    members: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct ChatDelete {
    actor: String,
    id: String,
}

#[derive(Deserialize)]
struct CommentRef {
    task: String,
    seq: u64,
}

#[derive(Deserialize)]
struct CommentFile {
    task: String,
    seq: u64,
    actor: String,
    name: String,
    path: String,
}

#[derive(Deserialize)]
struct ReviewerArgs {
    task: String,
    actor: String,
    reviewer: String,
}

#[derive(Deserialize)]
struct SetColumnArgs {
    task: String,
    actor: String,
    column: String,
}

#[derive(Deserialize)]
struct ClearArgs {
    actor: String,
    column: String,
}

#[derive(Deserialize)]
struct TaskRestore {
    task: String,
    actor: String,
}

#[derive(Deserialize)]
struct MemberRef {
    member: String,
}

#[derive(Deserialize)]
struct TeamUpdate {
    actor: String,
    name: Option<String>,
    project_path: Option<String>,
    lead_prompt: Option<String>,
}

#[derive(Deserialize)]
struct Launch {
    actor: String,
    #[serde(default)]
    members: Vec<String>,
}

#[derive(Deserialize)]
struct Trash {
    actor: String,
    #[serde(default = "yes")]
    deleted: bool,
}

#[derive(Deserialize)]
struct Actor {
    actor: String,
}

#[derive(Deserialize)]
struct CrossSend {
    actor: String,
    team: String,
    to: Option<String>,
    text: String,
    mode: Option<String>,
}

#[derive(Deserialize)]
struct Heartbeat {
    actor: String,
    status: String,
    task: Option<String>,
    note: Option<String>,
}

#[derive(Deserialize)]
struct Deliver {
    actor: String,
    text: String,
}

#[derive(Deserialize)]
struct TaskEvent {
    actor: String,
    task: String,
    event: String,
    note: Option<String>,
}

#[derive(Deserialize)]
struct SetStatus {
    #[serde(alias = "taskId")]
    task: String,
    status: String,
    actor: String,
}

#[derive(Deserialize)]
struct SyncReport {
    #[serde(alias = "memberName", alias = "from")]
    member: String,
    state: String,
    #[serde(default, alias = "taskIds")]
    task_ids: Vec<String>,
    note: Option<String>,
}

#[derive(Deserialize)]
struct DeliverMessage {
    #[serde(alias = "fromMemberName")]
    actor: String,
    #[serde(default)]
    to: Option<Value>,
    text: String,
}

fn yes() -> bool {
    true
}

const NAMES: [&str; 34] = [
    "app_get_connection_info",
    "kanban_list_reviewers",
    "member_work_sync_report",
    "process_stop",
    "runtime_deliver_message",
    "task_set_status",
    "group_chat_create",
    "group_chat_update",
    "group_chat_delete",
    "group_chat_list",
    "task_get_comment",
    "task_attach_comment_file",
    "kanban_add_reviewer",
    "kanban_remove_reviewer",
    "kanban_set_column",
    "kanban_clear",
    "task_restore",
    "lead_briefing",
    "team_list",
    "team_update",
    "team_launch",
    "team_stop",
    "team_trash",
    "cross_team_list_targets",
    "cross_team_send",
    "cross_team_get_outbox",
    "runtime_heartbeat",
    "runtime_bootstrap_checkin",
    "runtime_deliver",
    "runtime_task_event",
    "member_work_sync_status",
    "member_work_sync_ack",
    "task_comment_files",
    "team_get_launch_state",
];

/// `None` when `name` is not one of ours (the caller falls through to the base tools).
pub(crate) fn call(
    store: &mut dyn Store,
    home: Option<&Path>,
    name: &str,
    raw: &Value,
) -> Option<Result<Value, ToolError>> {
    if !NAMES.contains(&name) {
        return None;
    }
    if name == "process_stop" {
        return Some(process_stop(store, raw.clone()));
    }
    Some(run(store, home, name, raw.clone()))
}

/// The original's `process_stop` takes only a pid; default the actor to the process owner.
fn process_stop(store: &mut dyn Store, mut raw: Value) -> Result<Value, ToolError> {
    if raw.get("actor").is_none() {
        let pid = raw.get("pid").and_then(Value::as_u64).unwrap_or(0);
        let owner = store
            .state()?
            .processes
            .iter()
            .find(|p| u64::from(p.pid) == pid)
            .map(|p| p.owner.clone());
        if let (Some(o), Some(m)) = (owner, raw.as_object_mut()) {
            m.insert("actor".into(), json!(o));
        }
    }
    crate::tools::call(store, "process_unregister", raw)
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Creator or lead may change a group chat.
fn chat_guard<'a>(actor: &'a str, id: &'a str) -> impl Fn(&State) -> Result<(), String> + 'a {
    move |s| match s.group(id) {
        Some(g) if g.created_by == actor || is_lead(s, actor) => Ok(()),
        Some(_) => Err(format!("{actor} is neither the creator of {id} nor a lead")),
        None => Ok(()),
    }
}

fn team_of(store: &mut dyn Store) -> Result<rteams_core::Team, ToolError> {
    Ok(store
        .state()?
        .team
        .clone()
        .ok_or(CoreError::TeamNotCreated)?)
}

/// This team's directory name: its stable id (the display name may differ after a rename).
fn own_dir(store: &mut dyn Store) -> Result<String, ToolError> {
    match store.team_dir() {
        Some(d) => Ok(d),
        None => Ok(team_of(store)?.name),
    }
}

fn need_home(home: Option<&Path>) -> Result<&Path, ToolError> {
    home.ok_or_else(|| {
        ToolError::InvalidArguments("this server has no team home directory (in-memory)".into())
    })
}

/// Sibling team directories (those with a log), sorted by name.
fn sibling_dirs(home: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("events.jsonl").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

fn open_ro(home: &Path, dir: &str) -> Option<FileStore> {
    FileStore::open_read_only(home, dir).ok()
}

fn lead_name(state: &State) -> Option<String> {
    state.team.as_ref().and_then(|t| {
        t.members
            .iter()
            .find(|m| m.role == "lead" && !m.removed)
            .map(|m| m.name.clone())
    })
}

fn beat(
    store: &mut dyn Store,
    cid: Option<&str>,
    actor: &str,
    status: &str,
    task: Option<String>,
    note: Option<String>,
) -> Result<u64, ToolError> {
    let cmd = Command::Heartbeat {
        member: actor.to_string(),
        status: status.to_string(),
        task,
        note,
    };
    Ok(exec(store, cid, actor, cmd)?.seq)
}

fn run(
    store: &mut dyn Store,
    home: Option<&Path>,
    name: &str,
    raw: Value,
) -> Result<Value, ToolError> {
    let cid = command_id(&raw);
    let cid = cid.as_deref();
    match name {
        "group_chat_create" => {
            let a: ChatCreate = args(raw)?;
            require_member(store.state()?, &a.actor)?;
            let id = a.id.clone().unwrap_or_else(|| slug(&a.name));
            let mut members = a.members.clone();
            if !members.contains(&a.actor) {
                members.insert(0, a.actor.clone());
            }
            for m in &members {
                require_member(store.state()?, m)?;
            }
            let cmd = Command::CreateGroupChat {
                id: id.clone(),
                name: a.name,
                members,
            };
            let seq = exec(store, cid, &a.actor, cmd)?.seq;
            Ok(json!({ "event_seq": seq, "group_chat": store.state()?.group(&id) }))
        }
        "group_chat_update" => {
            let a: ChatUpdate = args(raw)?;
            if let Some(ms) = &a.members {
                for m in ms {
                    require_member(store.state()?, m)?;
                }
            }
            let cmd = Command::UpdateGroupChat {
                id: a.id.clone(),
                name: a.name,
                members: a.members,
            };
            let guard = chat_guard(&a.actor, &a.id);
            let seq = store
                .execute_guarded(&a.actor, now_ms(), cid, cmd, &guard)?
                .event
                .seq;
            Ok(json!({ "event_seq": seq, "group_chat": store.state()?.group(&a.id) }))
        }
        "group_chat_delete" => {
            let a: ChatDelete = args(raw)?;
            let cmd = Command::DeleteGroupChat { id: a.id.clone() };
            let guard = chat_guard(&a.actor, &a.id);
            let seq = store
                .execute_guarded(&a.actor, now_ms(), cid, cmd, &guard)?
                .event
                .seq;
            Ok(json!({ "event_seq": seq, "deleted": a.id }))
        }
        "group_chat_list" => {
            let chats = store
                .state()?
                .team
                .as_ref()
                .map(|t| t.group_chats.clone())
                .unwrap_or_default();
            Ok(json!({ "group_chats": chats }))
        }
        "task_get_comment" => {
            let a: CommentRef = args(raw)?;
            let state = store.state()?;
            if state.task(&a.task).is_none() {
                return Err(CoreError::TaskNotFound(a.task).into());
            }
            let c = state
                .messages
                .iter()
                .find(|m| {
                    m.seq == a.seq && matches!(&m.scope, Scope::Task { task } if *task == a.task)
                })
                .ok_or(CoreError::MessageNotFound(a.seq))?;
            Ok(json!({ "comment": c }))
        }
        "task_attach_comment_file" => {
            let a: CommentFile = args(raw)?;
            let state = store.state()?;
            require_member(state, &a.actor)?;
            if state.task(&a.task).is_none() {
                return Err(CoreError::TaskNotFound(a.task).into());
            }
            let author = state
                .messages
                .iter()
                .find(|m| {
                    m.seq == a.seq && matches!(&m.scope, Scope::Task { task } if *task == a.task)
                })
                .ok_or(CoreError::MessageNotFound(a.seq))?
                .from
                .clone();
            let cmd = Command::AttachFile {
                task: a.task.clone(),
                name: a.name,
                path: a.path,
            };
            let (actor, task) = (a.actor.as_str(), a.task.as_str());
            // The comment's author, the task owner or a lead may attach to it.
            let guard = |s: &State| {
                let owner = s.task(task).and_then(|t| t.owner.as_deref());
                if actor == author || owner == Some(actor) {
                    Ok(())
                } else {
                    authorize(s, actor, None, "comment author")
                }
            };
            guarded(store, cid, actor, task, cmd, &guard)
        }
        "kanban_add_reviewer" | "kanban_remove_reviewer" => {
            let a: ReviewerArgs = args(raw)?;
            let cmd = if name == "kanban_add_reviewer" {
                Command::AddReviewer {
                    task: a.task.clone(),
                    reviewer: a.reviewer,
                }
            } else {
                Command::RemoveReviewer {
                    task: a.task.clone(),
                    reviewer: a.reviewer,
                }
            };
            owner_or_lead(store, cid, &a.actor, &a.task, cmd)
        }
        "kanban_set_column" => {
            let a: SetColumnArgs = args(raw)?;
            let cmd = Command::SetColumn {
                task: a.task.clone(),
                column: a.column,
            };
            owner_or_lead(store, cid, &a.actor, &a.task, cmd)
        }
        "kanban_clear" => {
            let a: ClearArgs = args(raw)?;
            let cmd = Command::ClearColumn { column: a.column };
            let seq = store
                .execute_guarded(&a.actor, now_ms(), cid, cmd, &lead_only(&a.actor))?
                .event
                .seq;
            Ok(json!({ "event_seq": seq, "board": rteams_core::kanban(store.state()?) }))
        }
        "task_restore" => {
            let a: TaskRestore = args(raw)?;
            let cmd = Command::RestoreTask {
                task: a.task.clone(),
            };
            owner_or_lead(store, cid, &a.actor, &a.task, cmd)
        }
        "lead_briefing" => {
            let a: MemberRef = args(raw)?;
            let state = store.state()?;
            require_member(state, &a.member)?;
            if !is_lead(state, &a.member) {
                return Err(ToolError::Forbidden(format!(
                    "{} does not have the lead role",
                    a.member
                )));
            }
            Ok(json!({ "briefing": briefing(state, &a.member), "lead": lead_briefing(state) }))
        }
        "team_list" => {
            let home = need_home(home)?;
            let teams: Vec<Value> = sibling_dirs(home)
                .into_iter()
                .filter_map(|dir| {
                    let mut s = open_ro(home, &dir)?;
                    let st = s.state().ok()?;
                    let t = st.team.as_ref()?;
                    Some(json!({
                        "dir": dir,
                        "name": t.name,
                        "members": t.members.iter().filter(|m| !m.removed).count(),
                        "tasks": st.tasks.iter().filter(|t| !t.archived).count(),
                        "project_path": t.project_path,
                        "deleted": t.deleted,
                    }))
                })
                .collect();
            Ok(json!({ "teams": teams }))
        }
        "team_update" => {
            let a: TeamUpdate = args(raw)?;
            let guard = lead_only(&a.actor);
            if let Some(n) = &a.name {
                if *n != team_of(store)?.name {
                    let rename_id = cid.map(|c| format!("{c}:rename"));
                    store.execute_guarded(
                        &a.actor,
                        now_ms(),
                        rename_id.as_deref(),
                        Command::RenameTeam { name: n.clone() },
                        &guard,
                    )?;
                }
            }
            if a.project_path.is_some() || a.lead_prompt.is_some() || a.name.is_none() {
                let cmd = Command::ConfigureTeam {
                    project_path: a.project_path,
                    lead_prompt: a.lead_prompt,
                };
                store.execute_guarded(&a.actor, now_ms(), cid, cmd, &guard)?;
            }
            Ok(json!({ "team": store.state()?.team }))
        }
        "team_launch" | "team_stop" => {
            let a: Launch = args(raw)?;
            let cmd = if name == "team_launch" {
                Command::LaunchRequest { members: a.members }
            } else {
                Command::StopRequest { members: a.members }
            };
            let seq = store
                .execute_guarded(&a.actor, now_ms(), cid, cmd, &lead_only(&a.actor))?
                .event
                .seq;
            Ok(json!({ "event_seq": seq, "requested": name == "team_launch" }))
        }
        "team_get_launch_state" => {
            let state = store.state()?;
            Ok(json!({ "heartbeats": state.heartbeats }))
        }
        "team_trash" => {
            let a: Trash = args(raw)?;
            let cmd = Command::DeleteTeam { deleted: a.deleted };
            store.execute_guarded(&a.actor, now_ms(), cid, cmd, &lead_only(&a.actor))?;
            Ok(json!({ "team": store.state()?.team }))
        }
        "cross_team_list_targets" => {
            let home = need_home(home)?;
            let own = own_dir(store)?;
            let targets: Vec<Value> = sibling_dirs(home)
                .into_iter()
                .filter(|d| *d != own)
                .filter_map(|dir| {
                    let mut s = open_ro(home, &dir)?;
                    let t = s.state().ok()?.team.clone()?;
                    if t.deleted {
                        return None;
                    }
                    let lead = t.members.iter().find(|m| m.role == "lead" && !m.removed);
                    Some(json!({
                        "team": dir,
                        "name": t.name,
                        "lead": lead.map(|m| &m.name),
                        "members": t.members.iter().filter(|m| !m.removed).map(|m| &m.name).collect::<Vec<_>>(),
                    }))
                })
                .collect();
            Ok(json!({ "targets": targets }))
        }
        "cross_team_send" => {
            let a: CrossSend = args(raw)?;
            let home = need_home(home)?;
            let own = own_dir(store)?;
            require_member(store.state()?, &a.actor)?;
            if a.team == own {
                return Err(ToolError::InvalidArguments(
                    "cross_team_send targets another team; use message_send for your own".into(),
                ));
            }
            if !sibling_dirs(home).contains(&a.team) {
                return Err(ToolError::InvalidArguments(format!(
                    "unknown team {:?}; see cross_team_list_targets",
                    a.team
                )));
            }
            let mut target = FileStore::open(home, &a.team)?;
            let t = target
                .state()?
                .team
                .clone()
                .ok_or(CoreError::TeamNotCreated)?;
            if t.deleted {
                return Err(CoreError::TeamDeleted.into());
            }
            let to = a.to.clone().or_else(|| lead_name(target.state().ok()?));
            let cmd = Command::SendMessage {
                to: to.clone(),
                text: a.text,
                mode: a.mode,
                from_team: Some(own),
                chat: None,
                attachments: vec![],
            };
            let seq = exec(&mut target, cid, &a.actor, cmd)?.seq;
            Ok(json!({ "event_seq": seq, "team": a.team, "to": to }))
        }
        "cross_team_get_outbox" => {
            let a: Actor = args(raw)?;
            let home = need_home(home)?;
            let own = own_dir(store)?;
            require_member(store.state()?, &a.actor)?;
            let mut outbox = Vec::new();
            for dir in sibling_dirs(home).into_iter().filter(|d| *d != own) {
                let Some(mut s) = open_ro(home, &dir) else {
                    continue;
                };
                let Ok(st) = s.state() else { continue };
                for m in &st.messages {
                    if m.from_team.as_deref() == Some(own.as_str()) && m.from == a.actor {
                        let to = match &m.scope {
                            Scope::Direct { to } => Some(to.clone()),
                            _ => None,
                        };
                        outbox.push(json!({
                            "team": dir, "seq": m.seq, "ts": m.ts, "to": to, "text": m.text,
                        }));
                    }
                }
            }
            Ok(json!({ "outbox": outbox }))
        }
        "runtime_heartbeat" => {
            let a: Heartbeat = args(raw)?;
            let seq = beat(store, cid, &a.actor, &a.status, a.task, a.note)?;
            Ok(json!({ "event_seq": seq, "heartbeat": store.state()?.heartbeats.get(&a.actor) }))
        }
        "runtime_bootstrap_checkin" => {
            let a: Actor = args(raw)?;
            let seq = beat(
                store,
                cid,
                &a.actor,
                "ready",
                None,
                Some("bootstrap".into()),
            )?;
            let state = store.state()?;
            Ok(json!({
                "event_seq": seq,
                "member_briefing": member_briefing(state, &a.actor),
                "briefing": briefing(state, &a.actor),
                "protocol": PROTOCOL,
            }))
        }
        "runtime_deliver" => {
            let a: Deliver = args(raw)?;
            let state = store.state()?;
            require_member(state, &a.actor)?;
            let to = lead_name(state).filter(|l| *l != a.actor);
            let cmd = Command::SendMessage {
                to: to.clone(),
                text: a.text,
                mode: None,
                from_team: None,
                chat: None,
                attachments: vec![],
            };
            let seq = exec(store, cid, &a.actor, cmd)?.seq;
            Ok(json!({ "event_seq": seq, "to": to }))
        }
        "runtime_task_event" => {
            let a: TaskEvent = args(raw)?;
            require_member(store.state()?, &a.actor)?;
            let step = |n: &str| cid.map(|c| format!("{c}/{n}"));
            let text = match &a.note {
                Some(n) if !n.trim().is_empty() => format!("[{}] {n}", a.event),
                _ => format!("[{}]", a.event),
            };
            let comment = Command::Comment {
                task: a.task.clone(),
                text,
                attachments: vec![],
            };
            let seq = exec(store, step("comment").as_deref(), &a.actor, comment)?.seq;
            let status = match a.event.as_str() {
                "completed" | "done" | "finished" => "idle",
                "blocked" | "stuck" | "failed" => "stuck",
                _ => "working",
            };
            beat(
                store,
                step("beat").as_deref(),
                &a.actor,
                status,
                Some(a.task.clone()),
                a.note,
            )?;
            task_reply(store, seq, &a.task)
        }
        "member_work_sync_status" => {
            let a: MemberRef = args(raw)?;
            let state = store.state()?;
            require_member(state, &a.member)?;
            let b = briefing(state, &a.member);
            let hb = state.heartbeats.get(&a.member);
            let in_sync = b.actionable.is_empty()
                || hb.is_some_and(|h| h.status == "working" || h.status == "stuck");
            Ok(json!({
                "member": a.member,
                "state": if in_sync { "in_sync" } else { "needs_attention" },
                "actionable": b.actionable.len(),
                "awareness": b.awareness.len(),
                "heartbeat": hb,
                "briefing": b,
            }))
        }
        "member_work_sync_ack" => {
            let a: MemberRef = args(raw)?;
            let state = store.state()?;
            require_member(state, &a.member)?;
            let b = briefing(state, &a.member);
            let (status, task) = match b.actionable.first() {
                Some(i) => ("working", Some(i.task.clone())),
                None => ("idle", None),
            };
            let seq = beat(
                store,
                cid,
                &a.member,
                status,
                task,
                Some("work-sync ack".into()),
            )?;
            Ok(json!({ "event_seq": seq, "heartbeat": store.state()?.heartbeats.get(&a.member) }))
        }
        "app_get_connection_info" => {
            let team = store.state()?.team.as_ref().map(|t| t.name.clone());
            Ok(json!({
                "app": "rteams",
                "version": env!("CARGO_PKG_VERSION"),
                "transport": "stdio",
                "team": team,
                "home": home.map(|h| h.display().to_string()),
            }))
        }
        "kanban_list_reviewers" => {
            let state = store.state()?;
            let mut by_task = serde_json::Map::new();
            for t in state.tasks.iter().filter(|t| !t.archived) {
                let mut rs: Vec<&String> = t.reviewer.iter().chain(t.reviewers.iter()).collect();
                rs.sort();
                rs.dedup();
                if !rs.is_empty() {
                    by_task.insert(t.id.clone(), json!(rs));
                }
            }
            let reviewers: Vec<&str> = state
                .team
                .iter()
                .flat_map(|t| t.members.iter())
                .filter(|m| !m.removed && (m.role.contains("review") || is_lead(state, &m.name)))
                .map(|m| m.name.as_str())
                .collect();
            Ok(json!({ "reviewers": reviewers, "by_task": by_task }))
        }
        "member_work_sync_report" => {
            let a: SyncReport = args(raw)?;
            require_member(store.state()?, &a.member)?;
            let status = match a.state.as_str() {
                "still_working" | "working" => "working",
                "blocked" => "stuck",
                _ => "idle",
            };
            let note = Some(match &a.note {
                Some(n) if !n.trim().is_empty() => format!("work-sync {}: {n}", a.state),
                _ => format!("work-sync {}", a.state),
            });
            let seq = beat(
                store,
                cid,
                &a.member,
                status,
                a.task_ids.into_iter().next(),
                note,
            )?;
            Ok(json!({ "event_seq": seq, "heartbeat": store.state()?.heartbeats.get(&a.member) }))
        }
        "runtime_deliver_message" => {
            let a: DeliverMessage = args(raw)?;
            let state = store.state()?;
            require_member(state, &a.actor)?;
            let to = match &a.to {
                Some(Value::String(s)) if s != "user" => Some(s.clone()),
                Some(Value::Object(o)) => o
                    .get("memberName")
                    .or_else(|| o.get("name"))
                    .and_then(Value::as_str)
                    .map(String::from),
                _ => None,
            }
            .or_else(|| lead_name(state).filter(|l| *l != a.actor));
            let cmd = Command::SendMessage {
                to: to.clone(),
                text: a.text,
                mode: None,
                from_team: None,
                chat: None,
                attachments: vec![],
            };
            let seq = exec(store, cid, &a.actor, cmd)?.seq;
            Ok(json!({ "event_seq": seq, "to": to }))
        }
        "task_set_status" => {
            let a: SetStatus = args(raw)?;
            let cmd = match a.status.as_str() {
                "deleted" => Command::ArchiveTask {
                    task: a.task.clone(),
                    archived: true,
                },
                s => Command::SetColumn {
                    task: a.task.clone(),
                    column: if s == "completed" {
                        "done".into()
                    } else {
                        s.into()
                    },
                },
            };
            owner_or_lead(store, cid, &a.actor, &a.task, cmd)
        }
        "task_comment_files" => {
            let a: CommentRef = args(raw)?;
            let state = store.state()?;
            let t = state
                .task(&a.task)
                .ok_or_else(|| CoreError::TaskNotFound(a.task.clone()))?;
            let c = state
                .messages
                .iter()
                .find(|m| {
                    m.seq == a.seq && matches!(&m.scope, Scope::Task { task } if *task == a.task)
                })
                .ok_or(CoreError::MessageNotFound(a.seq))?;
            Ok(
                json!({ "comment_attachments": c.attachments, "task_attachments": t.attachments, "task": task_json(state, &a.task) }),
            )
        }
        _ => unreachable!("NAMES guards dispatch"),
    }
}

pub(crate) fn definitions() -> Vec<Value> {
    let s = |d: &str| json!({ "type": "string", "description": d });
    let task = s("Task id");
    let actor = s("Your member name");
    let seq =
        json!({ "type": "integer", "minimum": 1, "description": "Message seq of the comment" });
    let names =
        json!({ "type": "array", "items": { "type": "string" }, "description": "Member names" });
    vec![
        tool(
            "group_chat_create",
            "Create a subset group chat; you are added as a member. Post with message_send {chat}.",
            json!({ "actor": actor, "id": s("Chat id (default: slug of the name)"), "name": s("Chat name"), "members": names }),
            &["actor", "name"],
        ),
        tool(
            "group_chat_update",
            "Rename a group chat and/or replace its members. Creator or lead only.",
            json!({ "actor": actor, "id": s("Chat id"), "name": s("New name"), "members": names }),
            &["actor", "id"],
        ),
        tool(
            "group_chat_delete",
            "Delete a group chat (its messages stay in the log). Creator or lead only.",
            json!({ "actor": actor, "id": s("Chat id") }),
            &["actor", "id"],
        ),
        tool("group_chat_list", "List the team's group chats.", json!({}), &[]),
        tool(
            "app_get_connection_info",
            "The app, version, team and home this MCP server is connected to.",
            json!({}),
            &[],
        ),
        tool(
            "kanban_list_reviewers",
            "Members who can review (reviewer roles and leads) and each open task's reviewers.",
            json!({}),
            &[],
        ),
        tool(
            "member_work_sync_report",
            "Report your work-sync state: still_working, blocked or caught_up. Never completes tasks.",
            json!({ "member": s("Your member name"), "state": { "type": "string", "enum": ["still_working", "blocked", "caught_up"] }, "task_ids": names, "note": s("Short note") }),
            &["member", "state"],
        ),
        tool(
            "process_stop",
            "Mark a registered background service as stopped (history kept). Not for teammates.",
            json!({ "pid": { "type": "integer", "minimum": 1 }, "actor": actor }),
            &["pid"],
        ),
        tool(
            "runtime_deliver_message",
            "Deliver a message to a member (default: the lead).",
            json!({ "actor": actor, "to": s("Recipient member; default the lead"), "text": s("Message") }),
            &["actor", "text"],
        ),
        tool(
            "task_set_status",
            "Set a task's status: pending, in_progress, completed or deleted (archives). Owner or lead only.",
            json!({ "task": task, "actor": actor, "status": { "type": "string", "enum": ["pending", "in_progress", "completed", "deleted"] } }),
            &["task", "actor", "status"],
        ),
        tool(
            "task_get_comment",
            "Get one task comment by message seq.",
            json!({ "task": task, "seq": seq }),
            &["task", "seq"],
        ),
        tool(
            "task_comment_files",
            "Attachments of one task comment plus the task's own attachments.",
            json!({ "task": task, "seq": seq }),
            &["task", "seq"],
        ),
        tool(
            "task_attach_comment_file",
            "Attach a file (name + path) to the task, referencing a comment. Comment author, task owner or lead only.",
            json!({ "task": task, "seq": seq, "actor": actor, "name": s("Display name"), "path": s("File path") }),
            &["task", "seq", "actor", "name", "path"],
        ),
        tool(
            "kanban_add_reviewer",
            "Add an extra reviewer to a task. Owner or lead only.",
            json!({ "task": task, "actor": actor, "reviewer": s("Member who also reviews") }),
            &["task", "actor", "reviewer"],
        ),
        tool(
            "kanban_remove_reviewer",
            "Remove an extra reviewer from a task. Owner or lead only.",
            json!({ "task": task, "actor": actor, "reviewer": s("Reviewer to remove") }),
            &["task", "actor", "reviewer"],
        ),
        tool(
            "kanban_set_column",
            "Move a task to a kanban column (status in snake_case) along legal transitions. Owner or lead only.",
            json!({ "task": task, "actor": actor, "column": s("Column, e.g. in_progress") }),
            &["task", "actor", "column"],
        ),
        tool(
            "kanban_clear",
            "Archive every task in the approved or done column. Lead only.",
            json!({ "actor": actor, "column": s("approved or done") }),
            &["actor", "column"],
        ),
        tool(
            "task_restore",
            "Restore an archived task to the board. Owner or lead only.",
            json!({ "task": task, "actor": actor }),
            &["task", "actor"],
        ),
        tool(
            "lead_briefing",
            "Team-wide queue for a lead: unassigned and blocked tasks, items in review, board counts, load. Lead only.",
            json!({ "member": s("Your member name (must be a lead)") }),
            &["member"],
        ),
        tool("team_list", "List the teams under the same home directory.", json!({}), &[]),
        tool(
            "team_update",
            "Update the team's display name, project_path and/or lead_prompt. A rename changes only the display name; the team directory (id used by cross_team_* tools) stays the same. Lead only.",
            json!({ "actor": actor, "name": s("New display name"), "project_path": s("Project directory"), "lead_prompt": s("Standing instructions") }),
            &["actor"],
        ),
        tool(
            "team_launch",
            "Ask the app to launch teammates (none given = everyone). Lead only.",
            json!({ "actor": actor, "members": names }),
            &["actor"],
        ),
        tool(
            "team_stop",
            "Ask the app to stop teammates (none given = everyone). Lead only.",
            json!({ "actor": actor, "members": names }),
            &["actor"],
        ),
        tool(
            "team_get_launch_state",
            "Last heartbeat of every member that reported one.",
            json!({}),
            &[],
        ),
        tool(
            "team_trash",
            "Move the team to the trash (deleted defaults to true; false restores). Lead only.",
            json!({ "actor": actor, "deleted": { "type": "boolean" } }),
            &["actor"],
        ),
        tool(
            "cross_team_list_targets",
            "Other teams under the same home that you can message.",
            json!({}),
            &[],
        ),
        tool(
            "cross_team_send",
            "Send a message to another team (to a named member, default its lead). The receiver sees your team as the sender's team.",
            json!({ "actor": actor, "team": s("Target team directory name"), "to": s("Member there; default the lead"), "text": s("Message"), "mode": { "type": "string", "enum": ["do", "ask", "delegate"] } }),
            &["actor", "team", "text"],
        ),
        tool(
            "cross_team_get_outbox",
            "Messages you sent to other teams.",
            json!({ "actor": actor }),
            &["actor"],
        ),
        tool(
            "runtime_heartbeat",
            "Report your state: starting, ready, working, idle or stuck, optionally with a task and note.",
            json!({ "actor": actor, "status": { "type": "string", "enum": ["starting", "ready", "working", "idle", "stuck"] }, "task": task, "note": s("Short note") }),
            &["actor", "status"],
        ),
        tool(
            "runtime_bootstrap_checkin",
            "Check in on startup: records a ready heartbeat and returns your briefing and the protocol.",
            json!({ "actor": actor }),
            &["actor"],
        ),
        tool(
            "runtime_deliver",
            "Deliver a message to the team lead.",
            json!({ "actor": actor, "text": s("Message") }),
            &["actor", "text"],
        ),
        tool(
            "runtime_task_event",
            "Record a task event (e.g. started, blocked, completed) as a comment and update your heartbeat.",
            json!({ "actor": actor, "task": task, "event": s("Event name"), "note": s("Optional detail") }),
            &["actor", "task", "event"],
        ),
        tool(
            "member_work_sync_status",
            "Whether a member's reported state matches their queue (in_sync or needs_attention).",
            json!({ "member": s("Member name") }),
            &["member"],
        ),
        {
            let mut t = tool(
                "member_work_sync_ack",
                "Acknowledge your queue: records a working or idle heartbeat from your briefing.",
                json!({ "member": s("Member name"), "actor": s("Same as member") }),
                &["member"],
            );
            t["inputSchema"]["properties"]["actor"] = json!({ "type": "string", "description": "Ignored; accepted for symmetry" });
            t
        },
    ]
}
