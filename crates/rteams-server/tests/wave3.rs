//! Wave 3 tools: policy, idempotency, cross-team messages, permissions, runtime.
use std::time::Duration;

use rteams_server::Server;
use rteams_store::{FileStore, Store};
use serde_json::{json, Value};

fn call(server: &Server, name: &str, args: Value) -> (bool, Value) {
    let req = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": args } });
    let reply = server.handle_line(&req.to_string()).expect("reply");
    let v: Value = serde_json::from_str(&reply).unwrap();
    let r = &v["result"];
    let body: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
    (r["isError"].as_bool().unwrap(), body)
}

fn ok(s: &Server, name: &str, args: Value) -> Value {
    let (e, b) = call(s, name, args);
    assert!(!e, "{name} failed: {b}");
    b
}

fn err(s: &Server, name: &str, args: Value) -> Value {
    let (e, b) = call(s, name, args);
    assert!(e, "{name} should fail: {b}");
    b
}

fn seeded() -> Server {
    let s = Server::new();
    seed(&s);
    s
}

fn seed(s: &Server) {
    ok(
        s,
        "team_create",
        json!({ "name": "t", "actor": "lead", "members": [
            { "name": "echo", "role": "developer" }, { "name": "pixel", "role": "reviewer" } ] }),
    );
}

fn mk_task(s: &Server, owner: Option<&str>) -> String {
    let t = ok(
        s,
        "task_create",
        json!({ "actor": "lead", "subject": "s", "owner": owner }),
    );
    t["task"]["id"].as_str().unwrap().to_string()
}

#[test]
fn clarification_owner_or_lead_only_and_idempotent() {
    let s = seeded();
    let t = mk_task(&s, Some("echo"));
    let args = json!({ "actor": "echo", "task": t, "question": "which?", "command_id": "c1" });
    let r = ok(&s, "task_set_clarification", args.clone());
    assert_eq!(r["task"]["clarification"], "which?");
    let again = ok(&s, "task_set_clarification", args);
    assert_eq!(r["event_seq"], again["event_seq"]);
    assert_eq!(
        err(
            &s,
            "task_set_clarification",
            json!({ "actor": "pixel", "task": t, "question": "x" })
        )["code"],
        "forbidden"
    );
    let cleared = ok(
        &s,
        "task_set_clarification",
        json!({ "actor": "lead", "task": t }),
    );
    assert!(cleared["task"]["clarification"].is_null());
}

#[test]
fn attach_any_member_but_not_outsiders() {
    let s = seeded();
    let t = mk_task(&s, Some("echo"));
    let r = ok(
        &s,
        "task_attach_file",
        json!({ "actor": "pixel", "task": t, "name": "a.png", "path": "/x/a.png" }),
    );
    assert_eq!(r["task"]["attachments"][0]["name"], "a.png");
    assert_eq!(
        err(
            &s,
            "task_attach_file",
            json!({ "actor": "ghost", "task": t, "name": "n", "path": "/p" })
        )["code"],
        "unknown_member"
    );
    assert_eq!(
        err(
            &s,
            "task_attach_file",
            json!({ "actor": "pixel", "task": t, "name": "", "path": "/p" })
        )["code"],
        "empty_field"
    );
}

#[test]
fn link_and_unlink_policy_and_symmetry() {
    let s = seeded();
    let a = mk_task(&s, Some("echo"));
    let b = mk_task(&s, None);
    assert_eq!(
        err(
            &s,
            "task_link",
            json!({ "actor": "pixel", "task": a, "related": b })
        )["code"],
        "forbidden"
    );
    let r = ok(
        &s,
        "task_link",
        json!({ "actor": "echo", "task": a, "related": b }),
    );
    assert_eq!(r["task"]["related"], json!([b]));
    let other = ok(&s, "task_get", json!({ "task": b }));
    assert_eq!(other["task"]["related"], json!([a]));
    assert_eq!(
        err(
            &s,
            "task_link",
            json!({ "actor": "echo", "task": a, "related": a })
        )["code"],
        "invalid_link"
    );
    let r = ok(
        &s,
        "task_unlink",
        json!({ "actor": "lead", "task": a, "related": b }),
    );
    assert_eq!(r["task"]["related"], json!([]));
}

#[test]
fn review_start_is_for_the_reviewer_or_lead() {
    let s = seeded();
    let t = mk_task(&s, Some("echo"));
    assert_eq!(
        err(&s, "review_start", json!({ "actor": "pixel", "task": t }))["code"],
        "not_in_review"
    );
    ok(&s, "task_start", json!({ "actor": "echo", "task": t }));
    ok(
        &s,
        "review_request",
        json!({ "actor": "echo", "task": t, "reviewer": "pixel" }),
    );
    assert_eq!(
        err(&s, "review_start", json!({ "actor": "echo", "task": t }))["code"],
        "forbidden"
    );
    let r = ok(
        &s,
        "review_start",
        json!({ "actor": "pixel", "task": t, "command_id": "r1" }),
    );
    assert_eq!(r["task"]["review_started"], true);
    let again = ok(
        &s,
        "review_start",
        json!({ "actor": "pixel", "task": t, "command_id": "r1" }),
    );
    assert_eq!(r["event_seq"], again["event_seq"]);
}

#[test]
fn kanban_set_order_is_lead_only_and_validates() {
    let s = seeded();
    let a = mk_task(&s, None);
    let b = mk_task(&s, None);
    assert_eq!(
        err(
            &s,
            "kanban_set_order",
            json!({ "actor": "echo", "column": "pending", "order": [a] })
        )["code"],
        "forbidden"
    );
    assert_eq!(
        err(
            &s,
            "kanban_set_order",
            json!({ "actor": "lead", "column": "pending", "order": ["99"] })
        )["code"],
        "task_not_found"
    );
    assert_eq!(
        err(
            &s,
            "kanban_set_order",
            json!({ "actor": "lead", "column": "nope", "order": [] })
        )["code"],
        "unknown_column"
    );
    let r = ok(
        &s,
        "kanban_set_order",
        json!({ "actor": "lead", "column": "pending", "order": [b, a] }),
    );
    assert_eq!(r["column_order"]["pending"], json!([b, a]));
    let team = ok(&s, "team_get", json!({}));
    assert_eq!(team["team"]["column_order"]["pending"], json!([b, a]));
}

#[test]
fn team_delete_blocks_writes_until_restored() {
    let s = seeded();
    let t = mk_task(&s, Some("echo"));
    assert_eq!(
        err(&s, "team_delete", json!({ "actor": "echo" }))["code"],
        "forbidden"
    );
    let r = ok(&s, "team_delete", json!({ "actor": "lead" }));
    assert_eq!(r["team"]["deleted"], true);
    assert_eq!(
        err(&s, "task_start", json!({ "actor": "echo", "task": t }))["code"],
        "team_deleted"
    );
    assert_eq!(
        err(&s, "message_send", json!({ "actor": "echo", "text": "x" }))["code"],
        "team_deleted"
    );
    // Reads still work.
    ok(&s, "task_list", json!({}));
    let r = ok(
        &s,
        "team_delete",
        json!({ "actor": "lead", "deleted": false }),
    );
    assert_eq!(r["team"]["deleted"], false);
    ok(&s, "task_start", json!({ "actor": "echo", "task": t }));
}

#[test]
fn message_react_toggles_and_is_member_only() {
    let s = seeded();
    let m = ok(&s, "message_send", json!({ "actor": "echo", "text": "hi" }));
    let seq = m["event_seq"].clone();
    let r = ok(
        &s,
        "message_react",
        json!({ "actor": "pixel", "message_seq": seq, "emoji": "+1" }),
    );
    assert_eq!(r["reactions"], json!([{ "from": "pixel", "emoji": "+1" }]));
    let r = ok(
        &s,
        "message_react",
        json!({ "actor": "pixel", "message_seq": seq, "emoji": "+1" }),
    );
    assert_eq!(r["reactions"], json!([]));
    assert_eq!(
        err(
            &s,
            "message_react",
            json!({ "actor": "ghost", "message_seq": seq, "emoji": "+1" })
        )["code"],
        "unknown_member"
    );
    assert_eq!(
        err(
            &s,
            "message_react",
            json!({ "actor": "pixel", "message_seq": 999, "emoji": "+1" })
        )["code"],
        "message_not_found"
    );
}

#[test]
fn cross_team_message_needs_no_membership_but_a_member_recipient() {
    let s = seeded();
    assert_eq!(
        err(
            &s,
            "message_send",
            json!({ "actor": "stranger", "to": "echo", "text": "hi" })
        )["code"],
        "unknown_member"
    );
    ok(
        &s,
        "message_send",
        json!({ "actor": "stranger", "to": "echo", "text": "hi", "from_team": "other" }),
    );
    assert_eq!(
        err(
            &s,
            "message_send",
            json!({ "actor": "stranger", "to": "ghost", "text": "hi", "from_team": "other" })
        )["code"],
        "unknown_member"
    );
    let inbox = ok(&s, "inbox_get", json!({ "member": "echo" }));
    let msgs = inbox["inbox"]["items"].as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["from_team"], "other");
}

#[test]
fn schedules_lead_only_with_validation() {
    let s = seeded();
    let sched = |actor: &str, mins: u32| json!({ "actor": actor, "id": "s1", "member": "echo", "prompt": "ping", "every_minutes": mins });
    assert_eq!(
        err(&s, "schedule_set", sched("echo", 5))["code"],
        "forbidden"
    );
    assert_eq!(
        err(&s, "schedule_set", sched("lead", 0))["code"],
        "invalid_interval"
    );
    let r = ok(&s, "schedule_set", sched("lead", 5));
    assert_eq!(r["team"]["schedules"][0]["enabled"], true);
    assert_eq!(
        err(
            &s,
            "schedule_set",
            json!({ "actor": "lead", "id": "s2", "member": "ghost", "prompt": "p", "every_minutes": 1 })
        )["code"],
        "unknown_member"
    );
    assert_eq!(
        err(
            &s,
            "schedule_remove",
            json!({ "actor": "echo", "id": "s1" })
        )["code"],
        "forbidden"
    );
    let r = ok(
        &s,
        "schedule_remove",
        json!({ "actor": "lead", "id": "s1" }),
    );
    assert_eq!(r["team"]["schedules"], json!([]));
    assert_eq!(
        err(
            &s,
            "schedule_remove",
            json!({ "actor": "lead", "id": "s1" })
        )["code"],
        "schedule_not_found"
    );
}

#[test]
fn task_create_from_message_builds_subject_and_description() {
    let s = seeded();
    let long = "x".repeat(100);
    let m = ok(
        &s,
        "message_send",
        json!({ "actor": "echo", "text": format!("{long}\nsecond line") }),
    );
    let r = ok(
        &s,
        "task_create_from_message",
        json!({ "actor": "lead", "message_seq": m["event_seq"], "owner": "pixel", "command_id": "k" }),
    );
    assert_eq!(r["task"]["subject"].as_str().unwrap().chars().count(), 80);
    assert_eq!(r["task"]["owner"], "pixel");
    let seq = m["event_seq"].as_u64().unwrap();
    assert_eq!(
        r["task"]["description"],
        format!("{long}\nsecond line\n\n(from message #{seq} by echo)")
    );
    let again = ok(
        &s,
        "task_create_from_message",
        json!({ "actor": "lead", "message_seq": seq, "owner": "pixel", "command_id": "k" }),
    );
    assert_eq!(again["task"]["id"], r["task"]["id"]);
    assert_eq!(
        ok(&s, "task_list", json!({}))["tasks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        err(
            &s,
            "task_create_from_message",
            json!({ "actor": "lead", "message_seq": 999 })
        )["code"],
        "message_not_found"
    );
}

#[test]
fn runtime_and_worktree_via_member_tools() {
    let s = seeded();
    let r = ok(
        &s,
        "member_add",
        json!({ "actor": "lead", "name": "nova", "role": "dev", "runtime": "codex" }),
    );
    let nova = r["team"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "nova")
        .unwrap()
        .clone();
    assert_eq!(nova["runtime"], "codex");
    assert_eq!(
        err(
            &s,
            "member_add",
            json!({ "actor": "lead", "name": "x", "role": "dev", "runtime": " " })
        )["code"],
        "empty_runtime"
    );
    let r = ok(
        &s,
        "member_update",
        json!({ "actor": "lead", "name": "nova", "runtime": "gemini", "worktree": true }),
    );
    let nova = r["team"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "nova")
        .unwrap()
        .clone();
    assert_eq!(
        (nova["runtime"].clone(), nova["worktree"].clone()),
        (json!("gemini"), json!(true))
    );
    // A member may not switch their own runtime, but may still edit their prompt.
    assert_eq!(
        err(
            &s,
            "member_update",
            json!({ "actor": "echo", "name": "echo", "runtime": "codex" })
        )["code"],
        "forbidden"
    );
    ok(
        &s,
        "member_update",
        json!({ "actor": "echo", "name": "echo", "prompt": "p" }),
    );
    assert_eq!(
        err(
            &s,
            "member_update",
            json!({ "actor": "lead", "name": "echo", "runtime": "" })
        )["code"],
        "empty_runtime"
    );
}

#[test]
fn tools_are_listed() {
    let s = Server::new();
    let v = s
        .handle(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
        .unwrap();
    let names: Vec<&str> = v["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for n in [
        "task_set_clarification",
        "task_attach_file",
        "task_link",
        "task_unlink",
        "review_start",
        "kanban_set_order",
        "team_delete",
        "message_react",
        "schedule_set",
        "schedule_remove",
        "permission_decide",
        "permission_request",
        "task_create_from_message",
    ] {
        assert!(names.contains(&n), "{n} missing");
    }
}

// ---- permissions ----

fn file_servers(member: Option<&str>) -> (tempfile::TempDir, Server, Server) {
    let dir = tempfile::tempdir().unwrap();
    let agent = Server::with_identity(
        FileStore::open(dir.path(), "t").unwrap(),
        member.map(String::from),
    );
    let lead = Server::with_identity(FileStore::open(dir.path(), "t").unwrap(), None);
    seed(&lead);
    (dir, agent, lead)
}

/// Wait (via an independent store handle) for the first pending request and return its id.
fn wait_for_request(dir: &std::path::Path) -> String {
    let mut store = FileStore::open(dir, "t").unwrap();
    for _ in 0..200 {
        if let Some(p) = store.state().unwrap().permissions.first() {
            return p.id.clone();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("no permission request appeared");
}

fn decide_from_other_thread(dir: &std::path::Path, lead: &Server, allow: bool, note: Option<&str>) {
    let id = wait_for_request(dir);
    ok(
        lead,
        "permission_decide",
        json!({ "actor": "lead", "id": id, "allow": allow, "note": note }),
    );
}

#[test]
fn permission_request_allow_returns_updated_input() {
    let (dir, agent, lead) = file_servers(Some("echo"));
    let input = json!({ "command": "ls -la" });
    let reply = std::thread::scope(|sc| {
        let h = sc.spawn(|| {
            agent.call_tool(
                "permission_request",
                json!({ "tool_name": "Bash", "input": input }),
            )
        });
        decide_from_other_thread(dir.path(), &lead, true, None);
        h.join().unwrap().unwrap()
    });
    assert_eq!(
        reply,
        json!({ "behavior": "allow", "updatedInput": { "command": "ls -la" } })
    );
    let mut store = FileStore::open(dir.path(), "t").unwrap();
    let p = &store.state().unwrap().permissions[0];
    assert_eq!(
        (p.member.as_str(), p.tool.as_str(), p.decided),
        ("echo", "Bash", Some(true))
    );
}

#[test]
fn permission_request_deny_carries_note_or_default_message() {
    for (note, expected) in [
        (Some("too risky"), "too risky"),
        (None, "Denied by the lead"),
    ] {
        let (dir, agent, lead) = file_servers(Some("echo"));
        let reply = std::thread::scope(|sc| {
            let h = sc.spawn(|| {
                agent.call_tool(
                    "permission_request",
                    json!({ "tool_name": "Bash", "input": {} }),
                )
            });
            decide_from_other_thread(dir.path(), &lead, false, note);
            h.join().unwrap().unwrap()
        });
        assert_eq!(reply, json!({ "behavior": "deny", "message": expected }));
    }
}

#[test]
fn permission_request_over_jsonrpc_is_a_text_block_of_json() {
    let (dir, agent, lead) = file_servers(Some("echo"));
    let reply = std::thread::scope(|sc| {
        let h = sc.spawn(|| {
            call(
                &agent,
                "permission_request",
                json!({ "tool_name": "Edit", "input": { "file_path": "/a" } }),
            )
        });
        decide_from_other_thread(dir.path(), &lead, true, None);
        h.join().unwrap()
    });
    assert!(!reply.0);
    assert_eq!(reply.1["behavior"], "allow");
    assert_eq!(reply.1["updatedInput"]["file_path"], "/a");
}

#[test]
fn permission_request_times_out_as_deny() {
    let (dir, agent, _lead) = file_servers(Some("echo"));
    let agent = agent.with_permission_timeout(Duration::from_millis(700));
    let started = std::time::Instant::now();
    let reply = agent
        .call_tool(
            "permission_request",
            json!({ "tool_name": "Bash", "input": {} }),
        )
        .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(700));
    assert_eq!(reply["behavior"], "deny");
    assert_eq!(reply["message"], "Permission request timed out");
    let mut store = FileStore::open(dir.path(), "t").unwrap();
    assert_eq!(store.state().unwrap().permissions[0].decided, Some(false));
}

#[test]
fn permission_request_needs_identity_and_a_real_member() {
    let (_d, agent, _l) = file_servers(None);
    let e = agent
        .call_tool(
            "permission_request",
            json!({ "tool_name": "Bash", "input": {} }),
        )
        .unwrap_err();
    assert!(e.to_string().contains("RTEAMS_MEMBER"), "{e}");
    let (_d, ghost, _l) = file_servers(Some("ghost"));
    let e = ghost
        .call_tool(
            "permission_request",
            json!({ "tool_name": "Bash", "input": {} }),
        )
        .unwrap_err();
    assert!(e.to_string().contains("unknown member"), "{e}");
}

#[test]
fn permission_decide_is_lead_only_and_once() {
    let (dir, agent, lead) = file_servers(Some("echo"));
    let agent = agent.with_permission_timeout(Duration::from_secs(30));
    std::thread::scope(|sc| {
        let h = sc.spawn(|| {
            agent.call_tool(
                "permission_request",
                json!({ "tool_name": "Bash", "input": {} }),
            )
        });
        let id = wait_for_request(dir.path());
        assert_eq!(
            err(
                &lead,
                "permission_decide",
                json!({ "actor": "echo", "id": id, "allow": true })
            )["code"],
            "forbidden"
        );
        ok(
            &lead,
            "permission_decide",
            json!({ "actor": "lead", "id": id, "allow": true }),
        );
        assert_eq!(
            err(
                &lead,
                "permission_decide",
                json!({ "actor": "lead", "id": id, "allow": false })
            )["code"],
            "permission_decided"
        );
        assert_eq!(h.join().unwrap().unwrap()["behavior"], "allow");
    });
    assert_eq!(
        err(
            &lead,
            "permission_decide",
            json!({ "actor": "lead", "id": "nope", "allow": true })
        )["code"],
        "permission_not_found"
    );
}
