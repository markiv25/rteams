//! Briefing tools: task_briefing (role member|lead) and member_briefing.
use rteams_server::Server;
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

fn ok(server: &Server, name: &str, args: Value) -> Value {
    let (is_err, body) = call(server, name, args);
    assert!(!is_err, "{name} failed: {body}");
    body
}

fn err(server: &Server, name: &str, args: Value) -> Value {
    let (is_err, body) = call(server, name, args);
    assert!(is_err, "{name} should fail: {body}");
    body
}

fn seeded() -> Server {
    let s = Server::new();
    ok(
        &s,
        "team_create",
        json!({ "name": "t", "actor": "team-lead", "members": [
            { "name": "echo", "role": "developer" }, { "name": "pixel", "role": "reviewer" } ] }),
    );
    s
}

fn task(s: &Server, subject: &str, owner: Option<&str>) {
    ok(
        s,
        "task_create",
        json!({ "actor": "team-lead", "subject": subject, "owner": owner }),
    );
}

#[test]
fn member_role_is_the_default_and_has_no_lead_section() {
    let s = seeded();
    task(&s, "t1", Some("echo"));
    let b = ok(&s, "task_briefing", json!({ "member": "echo" }));
    assert_eq!(b["briefing"]["actionable"][0]["reason"], "start");
    assert!(b.get("lead").is_none());
    let b2 = ok(
        &s,
        "task_briefing",
        json!({ "member": "echo", "role": "member" }),
    );
    assert_eq!(b, b2);
}

#[test]
fn lead_role_adds_team_wide_queue() {
    let s = seeded();
    task(&s, "loose", None);
    task(&s, "mine", Some("echo"));
    let b = ok(
        &s,
        "task_briefing",
        json!({ "member": "team-lead", "role": "lead" }),
    );
    // the lead's own queue is empty; the team-wide view shows the unowned task
    assert_eq!(b["briefing"]["actionable"], json!([]));
    assert_eq!(b["lead"]["actionable"][0]["task"], "1");
    assert_eq!(b["lead"]["actionable"][0]["reason"], "unassigned");
    assert_eq!(b["lead"]["counts"]["todo"], 2);
    let echo = b["lead"]["load"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["member"] == "echo")
        .unwrap();
    assert_eq!(echo["todo"], 1);
}

#[test]
fn lead_role_is_refused_for_non_leads_and_unknown_members() {
    let s = seeded();
    let e = err(
        &s,
        "task_briefing",
        json!({ "member": "echo", "role": "lead" }),
    );
    assert_eq!(e["code"], "forbidden");
    let e = err(&s, "task_briefing", json!({ "member": "ghost" }));
    assert_eq!(e["code"], "unknown_member");
    // unknown role is a protocol-level invalid-params error
    let req = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "task_briefing", "arguments": { "member": "echo", "role": "boss" } } });
    let v: Value = serde_json::from_str(&s.handle_line(&req.to_string()).unwrap()).unwrap();
    assert_eq!(v["error"]["code"], -32602);
}

#[test]
fn member_briefing_is_bootstrap_context_not_the_queue() {
    let s = seeded();
    task(&s, "t1", Some("echo"));
    let b = ok(&s, "member_briefing", json!({ "member": "echo" }));
    let mb = &b["member_briefing"];
    assert_eq!(mb["team"], "t");
    assert_eq!(mb["you"], json!({ "name": "echo", "role": "developer" }));
    assert_eq!(mb["leads"], json!(["team-lead"]));
    assert_eq!(mb["roster"].as_array().unwrap().len(), 3);
    assert_eq!(mb["queue"], json!({ "actionable": 1, "awareness": 0 }));
    assert!(mb.get("actionable").is_none() && mb.get("briefing").is_none());
    assert!(b["protocol"].as_array().is_some_and(|p| !p.is_empty()));
}

#[test]
fn member_briefing_errors_before_team_exists_and_for_unknown_members() {
    let s = Server::new();
    assert_eq!(
        err(&s, "member_briefing", json!({ "member": "echo" }))["code"],
        "team_not_created"
    );
    let s = seeded();
    assert_eq!(
        err(&s, "member_briefing", json!({ "member": "ghost" }))["code"],
        "unknown_member"
    );
}

#[test]
fn work_sync_tools_do_not_exist() {
    let s = seeded();
    for name in [
        "member_work_sync_status",
        "member_work_sync_report",
        "lead_briefing",
    ] {
        let req = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": name, "arguments": {} } });
        let v: Value = serde_json::from_str(&s.handle_line(&req.to_string()).unwrap()).unwrap();
        assert!(v.get("error").is_some(), "{name} must be unknown: {v}");
    }
}
