//! Team routes: list, state, tasks, kanban, messages, raw events, and the whitelisted POST routes
//! that go through `rteams_server::Server::call_tool` (the same policy the agents are held to).

use std::path::Path;

use rteams_server::Server;
use rteams_store::FileStore;
use serde_json::{json, Value};

use super::{Ctx, Req, Resp};

pub fn valid_team(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && !s.starts_with('.')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Team directory names under `home` that have an event log.
pub fn team_names(home: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("events.jsonl").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| valid_team(n))
        .collect();
    v.sort();
    v
}

fn open(ctx: &Ctx, team: &str) -> Result<FileStore, Resp> {
    if !valid_team(team) {
        return Err(Resp::err(400, "invalid team name"));
    }
    FileStore::open_read_only(&ctx.home, team).map_err(|_| Resp::err(404, "team not found"))
}

/// Tools reachable by POST. Everything else (member changes, deletes, permission decisions ...)
/// stays GUI/agent only.
const POST_TOOLS: &[(&str, &str)] = &[
    ("tasks", "task_create"),
    ("messages", "message_send"),
    ("comments", "task_add_comment"),
];

pub fn handle(ctx: &Ctx, r: &Req) -> Option<Resp> {
    let s = r.segs();
    match (r.method.as_str(), s.as_slice()) {
        ("GET", ["api", "teams"]) => Some(list(ctx)),
        ("GET", ["api", "teams", t]) => Some(with_store(ctx, t, |st| {
            let state = st.loaded_state();
            Resp::ok(json!({
                "name": t,
                "team": state.team,
                "tasks": state.tasks.len(),
                "messages": state.messages.len(),
                "last_seq": st.loaded_events().last().map_or(0, |e| e.seq),
            }))
        })),
        ("GET", ["api", "teams", t, "tasks"]) => Some(with_store(ctx, t, |st| {
            let all = r.q("archived") == Some("1");
            let tasks: Vec<_> = st
                .loaded_state()
                .tasks
                .iter()
                .filter(|x| all || !x.archived)
                .collect();
            Resp::ok(json!({ "tasks": tasks }))
        })),
        ("GET", ["api", "teams", t, "kanban"]) => Some(with_store(ctx, t, |st| {
            Resp::ok(json!({ "board": rteams_core::projection::kanban(st.loaded_state()) }))
        })),
        ("GET", ["api", "teams", t, "messages"]) => Some(with_store(ctx, t, |st| {
            let limit = r
                .q("limit")
                .and_then(|v| v.parse().ok())
                .unwrap_or(100usize)
                .min(1000);
            let after = r
                .q("after")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let msgs: Vec<_> = st
                .loaded_state()
                .messages
                .iter()
                .filter(|m| m.seq > after)
                .collect();
            let skip = msgs.len().saturating_sub(limit);
            Resp::ok(json!({ "messages": &msgs[skip..] }))
        })),
        ("GET", ["api", "teams", t, "events"]) => Some(with_store(ctx, t, |st| {
            let limit = r
                .q("limit")
                .and_then(|v| v.parse().ok())
                .unwrap_or(200usize)
                .min(2000);
            let after = r
                .q("after")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let ev: Vec<_> = st
                .loaded_events()
                .iter()
                .filter(|e| e.seq > after)
                .take(limit)
                .collect();
            Resp::ok(json!({ "events": ev }))
        })),
        ("POST", ["api", "teams", t, what]) => {
            let tool = POST_TOOLS.iter().find(|(n, _)| n == what)?.1;
            Some(post_tool(ctx, t, tool, &r.body))
        }
        _ => None,
    }
}

fn with_store(ctx: &Ctx, team: &str, f: impl FnOnce(&FileStore) -> Resp) -> Resp {
    match open(ctx, team) {
        Ok(st) => f(&st),
        Err(r) => r,
    }
}

fn list(ctx: &Ctx) -> Resp {
    let teams: Vec<Value> = team_names(&ctx.home)
        .into_iter()
        .filter_map(|n| {
            let st = FileStore::open_read_only(&ctx.home, &n).ok()?;
            let s = st.loaded_state();
            Some(json!({
                "name": n,
                "members": s.team.as_ref().map_or(0, |t| t.members.len()),
                "tasks": s.tasks.len(),
                "last_seq": st.loaded_events().last().map_or(0, |e| e.seq),
            }))
        })
        .collect();
    Resp::ok(json!({ "teams": teams }))
}

fn post_tool(ctx: &Ctx, team: &str, tool: &str, body: &[u8]) -> Resp {
    if !valid_team(team) || !ctx.home.join(team).join("events.jsonl").is_file() {
        return Resp::err(404, "team not found");
    }
    let args: Value = match serde_json::from_slice(body) {
        Ok(v @ Value::Object(_)) => v,
        _ => return Resp::err(400, "body must be a JSON object"),
    };
    // Writes must be idempotent: the id lands on the event line and dedups retries.
    if args
        .get("command_id")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Resp::err(400, "command_id is required for write routes");
    }
    let store = match FileStore::open(&ctx.home, team) {
        Ok(s) => s,
        Err(e) => return Resp::err(500, &e.to_string()),
    };
    let server = Server::with_identity(store, None);
    match server.call_tool(tool, args) {
        Ok(v) => Resp::ok(v),
        Err(e) => Resp::err(422, &e.to_string()),
    }
}
