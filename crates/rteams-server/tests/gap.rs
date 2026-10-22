//! Gap-wave tools: group chats, comments, reviewers, columns, team admin, cross-team, runtime.

use rteams_server::Server;
use rteams_store::{FileStore, MemoryStore};
use serde_json::{json, Value};

fn call(s: &Server, name: &str, args: Value) -> Result<Value, Value> {
    let req = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                      "params": { "name": name, "arguments": args } });
    let v: Value = serde_json::from_str(&s.handle_line(&req.to_string()).unwrap()).unwrap();
    if v.get("error").is_some() {
        return Err(json!({ "code": "invalid_params", "message": v["error"]["message"] }));
    }
    let r = &v["result"];
    let body: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
    if r["isError"] == true {
        Err(body)
    } else {
        Ok(body)
    }
}

fn team(s: &Server, name: &str) {
    call(
        s,
        "team_create",
        json!({ "name": name, "actor": "lead", "members": [
            { "name": "dev", "role": "developer" }, { "name": "rev", "role": "reviewer" } ] }),
    )
    .unwrap();
}

fn mem() -> Server {
    let s = Server::with_identity(MemoryStore::new(), None);
    team(&s, "t");
    s
}

fn code(e: Value) -> String {
    e["code"].as_str().unwrap().to_string()
}

#[test]
fn group_chat_lifecycle_and_policy() {
    let s = mem();
    let c = call(
        &s,
        "group_chat_create",
        json!({ "actor": "dev", "name": "Back End", "members": ["rev"], "command_id": "g1" }),
    )
    .unwrap();
    assert_eq!(c["group_chat"]["id"], "back-end");
    assert_eq!(c["group_chat"]["members"], json!(["dev", "rev"]));
    // replay: same result, still one chat
    call(
        &s,
        "group_chat_create",
        json!({ "actor": "dev", "name": "Back End", "members": ["rev"], "command_id": "g1" }),
    )
    .unwrap();
    assert_eq!(
        call(&s, "group_chat_list", json!({})).unwrap()["group_chats"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // non-creator non-lead cannot update/delete
    let e = call(
        &s,
        "group_chat_update",
        json!({ "actor": "rev", "id": "back-end", "name": "x" }),
    )
    .unwrap_err();
    assert_eq!(e["code"], "forbidden");
    call(
        &s,
        "group_chat_update",
        json!({ "actor": "lead", "id": "back-end", "name": "BE" }),
    )
    .unwrap();
    // send: member ok, outsider forbidden, unknown chat is a domain error
    call(
        &s,
        "message_send",
        json!({ "actor": "dev", "chat": "back-end", "text": "hi" }),
    )
    .unwrap();
    call(
        &s,
        "group_chat_create",
        json!({ "actor": "lead", "id": "ops", "name": "ops", "members": ["lead"] }),
    )
    .unwrap();
    let e = call(
        &s,
        "message_send",
        json!({ "actor": "dev", "chat": "ops", "text": "hi" }),
    )
    .unwrap_err();
    assert_eq!(e["code"], "forbidden");
    let e = call(
        &s,
        "message_send",
        json!({ "actor": "dev", "chat": "nope", "text": "hi" }),
    )
    .unwrap_err();
    assert_eq!(code(e), "group_chat_not_found");
    call(
        &s,
        "group_chat_delete",
        json!({ "actor": "dev", "id": "back-end" }),
    )
    .unwrap();
    assert_eq!(
        call(&s, "group_chat_list", json!({})).unwrap()["group_chats"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn comments_attachments_and_lookup() {
    let s = mem();
    call(
        &s,
        "task_create",
        json!({ "actor": "lead", "subject": "a", "owner": "dev" }),
    )
    .unwrap();
    let r = call(&s, "task_add_comment", json!({ "task": "1", "actor": "dev", "text": "done", "attachments": [{ "name": "a.png", "path": "/x/a.png" }] })).unwrap();
    let seq = r["event_seq"].as_u64().unwrap();
    let c = call(&s, "task_get_comment", json!({ "task": "1", "seq": seq })).unwrap();
    assert_eq!(c["comment"]["text"], "done");
    assert_eq!(c["comment"]["attachments"][0]["name"], "a.png");
    assert_eq!(
        code(call(&s, "task_get_comment", json!({ "task": "1", "seq": 9999 })).unwrap_err()),
        "message_not_found"
    );
    // attach: author ok, unrelated member forbidden
    let e = call(
        &s,
        "task_attach_comment_file",
        json!({ "task": "1", "seq": seq, "actor": "rev", "name": "n", "path": "/p" }),
    )
    .unwrap_err();
    assert_eq!(e["code"], "forbidden");
    let r = call(&s, "task_attach_comment_file", json!({ "task": "1", "seq": seq, "actor": "dev", "name": "n", "path": "/p", "command_id": "f1" })).unwrap();
    assert_eq!(r["task"]["attachments"].as_array().unwrap().len(), 1);
    call(&s, "task_attach_comment_file", json!({ "task": "1", "seq": seq, "actor": "dev", "name": "n", "path": "/p", "command_id": "f1" })).unwrap();
    let t = call(&s, "task_get", json!({ "task": "1" })).unwrap();
    assert_eq!(t["task"]["attachments"].as_array().unwrap().len(), 1);
    // message attachments
    call(&s, "message_send", json!({ "actor": "dev", "to": "lead", "text": "see", "attachments": [{ "name": "b", "path": "/b" }] })).unwrap();
    let inbox = call(&s, "inbox_get", json!({ "member": "lead" })).unwrap();
    assert_eq!(inbox["inbox"]["items"][0]["attachments"][0]["name"], "b");
}

#[test]
fn reviewers_columns_restore_and_clear() {
    let s = mem();
    call(
        &s,
        "task_create",
        json!({ "actor": "lead", "subject": "a", "owner": "dev" }),
    )
    .unwrap();
    let e = call(
        &s,
        "kanban_add_reviewer",
        json!({ "task": "1", "actor": "rev", "reviewer": "rev" }),
    )
    .unwrap_err();
    assert_eq!(e["code"], "forbidden");
    let r = call(
        &s,
        "kanban_add_reviewer",
        json!({ "task": "1", "actor": "dev", "reviewer": "rev", "command_id": "r1" }),
    )
    .unwrap();
    assert_eq!(r["task"]["reviewers"], json!(["rev"]));
    call(
        &s,
        "kanban_add_reviewer",
        json!({ "task": "1", "actor": "dev", "reviewer": "rev", "command_id": "r1" }),
    )
    .unwrap();
    let e = call(
        &s,
        "kanban_add_reviewer",
        json!({ "task": "1", "actor": "dev", "reviewer": "rev" }),
    )
    .unwrap_err();
    assert_eq!(code(e), "reviewer_exists");
    let r = call(
        &s,
        "kanban_remove_reviewer",
        json!({ "task": "1", "actor": "lead", "reviewer": "rev" }),
    )
    .unwrap();
    assert_eq!(r["task"]["reviewers"], json!([]));

    let e = call(
        &s,
        "kanban_set_column",
        json!({ "task": "1", "actor": "rev", "column": "in_progress" }),
    )
    .unwrap_err();
    assert_eq!(e["code"], "forbidden");
    let r = call(
        &s,
        "kanban_set_column",
        json!({ "task": "1", "actor": "dev", "column": "in_progress" }),
    )
    .unwrap();
    assert_eq!(r["task"]["status"], "in_progress");
    call(
        &s,
        "kanban_set_column",
        json!({ "task": "1", "actor": "dev", "column": "done" }),
    )
    .unwrap();

    let e = call(
        &s,
        "kanban_clear",
        json!({ "actor": "dev", "column": "done" }),
    )
    .unwrap_err();
    assert_eq!(e["code"], "forbidden");
    call(
        &s,
        "kanban_clear",
        json!({ "actor": "lead", "column": "done", "command_id": "k1" }),
    )
    .unwrap();
    call(
        &s,
        "kanban_clear",
        json!({ "actor": "lead", "column": "done", "command_id": "k1" }),
    )
    .unwrap();
    assert!(call(&s, "task_list", json!({})).unwrap()["tasks"]
        .as_array()
        .unwrap()
        .is_empty());
    let e = call(&s, "task_restore", json!({ "task": "1", "actor": "rev" })).unwrap_err();
    assert_eq!(e["code"], "forbidden");
    call(&s, "task_restore", json!({ "task": "1", "actor": "dev" })).unwrap();
    assert_eq!(
        call(&s, "task_list", json!({})).unwrap()["tasks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        code(call(&s, "task_restore", json!({ "task": "1", "actor": "dev" })).unwrap_err()),
        "not_archived"
    );
}

#[test]
fn lead_briefing_and_team_admin_policy() {
    let s = mem();
    let b = call(&s, "lead_briefing", json!({ "member": "lead" })).unwrap();
    assert!(b["lead"].is_object());
    assert_eq!(
        call(&s, "lead_briefing", json!({ "member": "dev" })).unwrap_err()["code"],
        "forbidden"
    );
    assert_eq!(
        code(call(&s, "lead_briefing", json!({ "member": "ghost" })).unwrap_err()),
        "unknown_member"
    );

    for tool in ["team_update", "team_launch", "team_stop", "team_trash"] {
        let e = call(&s, tool, json!({ "actor": "dev", "project_path": "/p" })).unwrap_err();
        assert_eq!(e["code"], "forbidden", "{tool}");
    }
    let t = call(
        &s,
        "team_update",
        json!({ "actor": "lead", "project_path": "/p", "lead_prompt": "go", "name": "t" }),
    )
    .unwrap();
    assert_eq!(t["team"]["project_path"], "/p");
    // rename: lead only, display name only, blank refused
    assert_eq!(
        call(
            &s,
            "team_update",
            json!({ "actor": "dev", "name": "other" })
        )
        .unwrap_err()["code"],
        "forbidden"
    );
    assert!(call(&s, "team_update", json!({ "actor": "lead", "name": "  " })).is_err());
    let t = call(
        &s,
        "team_update",
        json!({ "actor": "lead", "name": "Other Name", "command_id": "rn1" }),
    )
    .unwrap();
    assert_eq!(t["team"]["name"], "Other Name");
    assert_eq!(t["team"]["project_path"], "/p");
    let r = call(
        &s,
        "team_launch",
        json!({ "actor": "lead", "members": ["dev"], "command_id": "l1" }),
    )
    .unwrap();
    let r2 = call(
        &s,
        "team_launch",
        json!({ "actor": "lead", "members": ["dev"], "command_id": "l1" }),
    )
    .unwrap();
    assert_eq!(r["event_seq"], r2["event_seq"]);
    call(&s, "team_stop", json!({ "actor": "lead" })).unwrap();
    assert_eq!(
        code(
            call(
                &s,
                "team_launch",
                json!({ "actor": "lead", "members": ["ghost"] })
            )
            .unwrap_err()
        ),
        "unknown_member"
    );
    let t = call(&s, "team_trash", json!({ "actor": "lead" })).unwrap();
    assert_eq!(t["team"]["deleted"], true);
    call(
        &s,
        "team_trash",
        json!({ "actor": "lead", "deleted": false }),
    )
    .unwrap();
}

#[test]
fn runtime_tools() {
    let s = mem();
    call(
        &s,
        "task_create",
        json!({ "actor": "lead", "subject": "a", "owner": "dev" }),
    )
    .unwrap();
    assert_eq!(
        code(
            call(
                &s,
                "runtime_heartbeat",
                json!({ "actor": "dev", "status": "bogus" })
            )
            .unwrap_err()
        ),
        "invalid_heartbeat"
    );
    assert_eq!(
        code(
            call(
                &s,
                "runtime_heartbeat",
                json!({ "actor": "ghost", "status": "idle" })
            )
            .unwrap_err()
        ),
        "unknown_member"
    );
    let h = call(&s, "runtime_heartbeat", json!({ "actor": "dev", "status": "working", "task": "1", "note": "n", "command_id": "h1" })).unwrap();
    assert_eq!(h["heartbeat"]["status"], "working");
    let h2 = call(&s, "runtime_heartbeat", json!({ "actor": "dev", "status": "working", "task": "1", "note": "n", "command_id": "h1" })).unwrap();
    assert_eq!(h["event_seq"], h2["event_seq"]);

    let c = call(&s, "runtime_bootstrap_checkin", json!({ "actor": "rev" })).unwrap();
    assert!(c["member_briefing"].is_object() && c["protocol"].is_array());
    assert_eq!(
        call(&s, "team_get_launch_state", json!({})).unwrap()["heartbeats"]["rev"]["status"],
        "ready"
    );

    let d = call(
        &s,
        "runtime_deliver",
        json!({ "actor": "dev", "text": "report", "command_id": "d1" }),
    )
    .unwrap();
    assert_eq!(d["to"], "lead");
    let inbox = call(&s, "inbox_get", json!({ "member": "lead" }))
        .unwrap()
        .to_string();
    assert!(inbox.contains("report"));

    let r = call(&s, "runtime_task_event", json!({ "actor": "dev", "task": "1", "event": "blocked", "note": "need key", "command_id": "e1" })).unwrap();
    call(&s, "runtime_task_event", json!({ "actor": "dev", "task": "1", "event": "blocked", "note": "need key", "command_id": "e1" })).unwrap();
    let t = call(&s, "task_get", json!({ "task": "1" })).unwrap();
    assert_eq!(t["comments"].as_array().unwrap().len(), 1);
    assert!(t["comments"][0]["text"]
        .as_str()
        .unwrap()
        .starts_with("[blocked]"));
    assert!(r["task"].is_object());
    assert_eq!(
        call(&s, "team_get_launch_state", json!({})).unwrap()["heartbeats"]["dev"]["status"],
        "stuck"
    );

    // work sync: rev has nothing, dev has actionable work
    let st = call(&s, "member_work_sync_status", json!({ "member": "rev" })).unwrap();
    assert_eq!(st["state"], "in_sync");
    call(
        &s,
        "runtime_heartbeat",
        json!({ "actor": "dev", "status": "idle" }),
    )
    .unwrap();
    let st = call(&s, "member_work_sync_status", json!({ "member": "dev" })).unwrap();
    assert_eq!(st["state"], "needs_attention");
    let a = call(&s, "member_work_sync_ack", json!({ "member": "dev" })).unwrap();
    assert_eq!(a["heartbeat"]["status"], "working");
    assert_eq!(
        call(&s, "member_work_sync_status", json!({ "member": "dev" })).unwrap()["state"],
        "in_sync"
    );
}

#[test]
fn member_update_new_fields_are_lead_only() {
    let s = mem();
    let e = call(
        &s,
        "member_update",
        json!({ "actor": "dev", "name": "dev", "effort": "high" }),
    )
    .unwrap_err();
    assert_eq!(e["code"], "forbidden");
    let t = call(&s, "member_update", json!({ "actor": "lead", "name": "dev", "effort": "high", "fast": true, "context_limit": 50000, "mcp_allow": ["fs"] })).unwrap();
    let m = t["team"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "dev")
        .unwrap()
        .clone();
    assert_eq!(m["effort"], "high");
    assert_eq!(m["fast"], true);
    assert_eq!(m["mcp_allow"], json!(["fs"]));
}

#[test]
fn renamed_team_keeps_its_directory_id_for_cross_team() {
    let dir = tempfile::tempdir().unwrap();
    let a = Server::with_identity(FileStore::open(dir.path(), "alpha").unwrap(), None);
    let b = Server::with_identity(FileStore::open(dir.path(), "beta").unwrap(), None);
    team(&a, "alpha");
    team(&b, "beta");
    call(
        &a,
        "team_update",
        json!({ "actor": "lead", "name": "Beta" }),
    )
    .unwrap();
    call(
        &b,
        "team_update",
        json!({ "actor": "lead", "name": "Alpha Prime" }),
    )
    .unwrap();
    let t = call(&a, "cross_team_list_targets", json!({})).unwrap();
    let targets = t["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0]["team"], "beta");
    assert_eq!(targets[0]["name"], "Alpha Prime");
    // messaging still addresses the directory; the sender is identified by its directory
    call(
        &a,
        "cross_team_send",
        json!({ "actor": "dev", "team": "beta", "text": "hi" }),
    )
    .unwrap();
    assert!(call(
        &a,
        "cross_team_send",
        json!({ "actor": "dev", "team": "alpha", "text": "x" })
    )
    .is_err());
    let out = call(&a, "cross_team_get_outbox", json!({ "actor": "dev" })).unwrap();
    assert_eq!(out["outbox"].as_array().unwrap().len(), 1);
    let inbox = call(&b, "inbox_get", json!({ "member": "lead" })).unwrap();
    assert_eq!(inbox["inbox"]["items"][0]["from_team"], "alpha");
}

#[test]
fn cross_team_between_two_temp_teams() {
    let dir = tempfile::tempdir().unwrap();
    let a = Server::with_identity(FileStore::open(dir.path(), "alpha").unwrap(), None);
    let b = Server::with_identity(FileStore::open(dir.path(), "beta").unwrap(), None);
    team(&a, "alpha");
    team(&b, "beta");

    let l = call(&a, "team_list", json!({})).unwrap();
    assert_eq!(l["teams"].as_array().unwrap().len(), 2);
    let t = call(&a, "cross_team_list_targets", json!({})).unwrap();
    assert_eq!(t["targets"].as_array().unwrap().len(), 1);
    assert_eq!(t["targets"][0]["team"], "beta");
    assert_eq!(t["targets"][0]["lead"], "lead");

    let args = json!({ "actor": "dev", "team": "beta", "text": "need help", "command_id": "x1" });
    let r1 = call(&a, "cross_team_send", args.clone()).unwrap();
    let r2 = call(&a, "cross_team_send", args).unwrap();
    assert_eq!(r1["event_seq"], r2["event_seq"]);
    assert_eq!(r1["to"], "lead");
    let inbox = call(&b, "inbox_get", json!({ "member": "lead" })).unwrap();
    let msgs = inbox["inbox"]["items"].as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["from_team"], "alpha");
    assert_eq!(msgs[0]["from"], "dev");

    call(
        &a,
        "cross_team_send",
        json!({ "actor": "lead", "team": "beta", "to": "rev", "text": "fyi" }),
    )
    .unwrap();
    let out = call(&a, "cross_team_get_outbox", json!({ "actor": "dev" })).unwrap();
    assert_eq!(out["outbox"].as_array().unwrap().len(), 1);
    assert_eq!(out["outbox"][0]["team"], "beta");
    assert_eq!(
        call(&a, "cross_team_get_outbox", json!({ "actor": "lead" })).unwrap()["outbox"][0]["to"],
        "rev"
    );

    // policy and errors
    let e = call(
        &a,
        "cross_team_send",
        json!({ "actor": "ghost", "team": "beta", "text": "x" }),
    )
    .unwrap_err();
    assert_eq!(code(e), "unknown_member");
    assert!(call(
        &a,
        "cross_team_send",
        json!({ "actor": "dev", "team": "alpha", "text": "x" })
    )
    .is_err());
    assert!(call(
        &a,
        "cross_team_send",
        json!({ "actor": "dev", "team": "../x", "text": "x" })
    )
    .is_err());
    assert!(call(
        &a,
        "cross_team_send",
        json!({ "actor": "dev", "team": "gamma", "text": "x" })
    )
    .is_err());
    // a trashed team drops out of targets and refuses messages
    call(&b, "team_trash", json!({ "actor": "lead" })).unwrap();
    assert!(
        call(&a, "cross_team_list_targets", json!({})).unwrap()["targets"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(call(
        &a,
        "cross_team_send",
        json!({ "actor": "dev", "team": "beta", "text": "x" })
    )
    .is_err());
}

#[test]
fn in_memory_server_has_no_home() {
    let s = mem();
    assert!(call_is_invalid(&s, "team_list"));
}

fn call_is_invalid(s: &Server, name: &str) -> bool {
    let req = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": {} } });
    let v: Value = serde_json::from_str(&s.handle_line(&req.to_string()).unwrap()).unwrap();
    v["error"]["code"] == -32602
}

#[test]
fn every_new_tool_is_listed() {
    let s = mem();
    let req = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
    let v: Value = serde_json::from_str(&s.handle_line(&req.to_string()).unwrap()).unwrap();
    let names: Vec<&str> = v["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for n in [
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
    ] {
        assert!(names.contains(&n), "{n}");
    }
}
