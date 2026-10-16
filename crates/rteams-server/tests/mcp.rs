use rteams_server::Server;
use serde_json::{json, Value};
use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt, BufReader};

fn rpc(server: &Server, id: u64, method: &str, params: Value) -> Value {
    let req = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
    let reply = server
        .handle_line(&req.to_string())
        .expect("requests get replies");
    let v: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["id"], id);
    v
}

/// tools/call; returns (is_error, structured-or-error body).
fn call(server: &Server, name: &str, args: Value) -> (bool, Value) {
    let v = rpc(
        server,
        1,
        "tools/call",
        json!({ "name": name, "arguments": args }),
    );
    let r = &v["result"];
    let body: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
    if r["isError"] == false {
        assert_eq!(
            r["structuredContent"], body,
            "structuredContent mirrors text"
        );
    }
    (r["isError"].as_bool().unwrap(), body)
}

fn ok(server: &Server, name: &str, args: Value) -> Value {
    let (is_err, body) = call(server, name, args);
    assert!(!is_err, "{name} failed: {body}");
    body
}

fn team(server: &Server) {
    ok(
        server,
        "team_create",
        json!({ "name": "t", "actor": "team-lead", "members": [
            { "name": "echo", "role": "developer" }, { "name": "pixel", "role": "reviewer" } ] }),
    );
}

#[test]
fn handshake_and_tool_listing() {
    let s = Server::new();
    let v = rpc(
        &s,
        1,
        "initialize",
        json!({
        "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "c", "version": "1" } }),
    );
    assert_eq!(v["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(v["result"]["serverInfo"]["name"], "rteams");
    assert!(v["result"]["capabilities"]["tools"].is_object());

    // unknown version -> server proposes its latest
    let v = rpc(
        &s,
        2,
        "initialize",
        json!({ "protocolVersion": "1999-01-01" }),
    );
    assert_eq!(v["result"]["protocolVersion"], "2025-06-18");

    // notifications get no reply
    assert!(s
        .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .is_none());
    assert_eq!(rpc(&s, 3, "ping", json!({}))["result"], json!({}));

    let v = rpc(&s, 4, "tools/list", json!({}));
    let tools = v["result"]["tools"].as_array().unwrap();
    let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for n in [
        "team_create",
        "task_create",
        "task_start",
        "task_complete",
        "task_add_comment",
        "task_briefing",
    ] {
        assert!(names.contains(&n), "missing {n}");
    }
    for t in tools {
        assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
        assert!(t["description"].as_str().is_some_and(|d| !d.is_empty()));
    }
    // no work-sync token tools
    assert!(!names.iter().any(|n| n.contains("work_sync")));
}

#[test]
fn protocol_errors() {
    let s = Server::new();
    let v: Value = serde_json::from_str(&s.handle_line("{not json").unwrap()).unwrap();
    assert_eq!(v["error"]["code"], -32700);
    assert_eq!(v["id"], Value::Null);

    assert_eq!(
        rpc(&s, 1, "nope/method", json!({}))["error"]["code"],
        -32601
    );
    assert_eq!(
        rpc(&s, 2, "tools/call", json!({ "name": "no_such_tool" }))["error"]["code"],
        -32602
    );
    assert_eq!(rpc(&s, 3, "tools/call", json!({}))["error"]["code"], -32602);
    // missing required argument
    let v = rpc(
        &s,
        4,
        "tools/call",
        json!({ "name": "task_start", "arguments": { "task": "1" } }),
    );
    assert_eq!(v["error"]["code"], -32602);
    assert!(v["error"]["message"].as_str().unwrap().contains("actor"));
}

#[test]
fn create_start_comment_complete_end_to_end() {
    let s = Server::new();
    team(&s);
    let team_body = ok(&s, "team_get", json!({}));
    let roster: Vec<_> = team_body["team"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(roster, ["team-lead", "echo", "pixel"]);

    let t = ok(
        &s,
        "task_create",
        json!({ "actor": "team-lead", "subject": "build core", "owner": "echo" }),
    );
    assert_eq!(t["task"]["id"], "1");
    assert_eq!(t["task"]["status"], "pending");
    assert_eq!(t["task"]["owner"], "echo");

    let b = ok(&s, "task_briefing", json!({ "member": "echo" }));
    assert_eq!(b["briefing"]["actionable"][0]["reason"], "start");

    let t = ok(&s, "task_start", json!({ "task": "1", "actor": "echo" }));
    assert_eq!(t["task"]["status"], "in_progress");
    let b = ok(&s, "task_briefing", json!({ "member": "echo" }));
    assert_eq!(b["briefing"]["actionable"][0]["reason"], "resume");

    ok(
        &s,
        "task_add_comment",
        json!({ "task": "1", "actor": "pixel", "text": "looks good so far" }),
    );
    let got = ok(&s, "task_get", json!({ "task": "1" }));
    assert_eq!(got["comments"][0]["text"], "looks good so far");
    assert_eq!(got["comments"][0]["from"], "pixel");
    assert_eq!(got["comments"][0]["scope"], "task");

    let board = ok(&s, "kanban_get", json!({}));
    assert_eq!(board["board"]["in_progress"][0]["id"], "1");

    let t = ok(&s, "task_complete", json!({ "task": "1", "actor": "echo" }));
    assert_eq!(t["task"]["status"], "done");
    assert_eq!(
        ok(&s, "kanban_get", json!({}))["board"]["done"][0]["id"],
        "1"
    );
    assert!(
        ok(&s, "task_briefing", json!({ "member": "echo" }))["briefing"]["actionable"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // reopen goes through task_start
    assert_eq!(
        ok(&s, "task_start", json!({ "task": "1", "actor": "echo" }))["task"]["status"],
        "in_progress"
    );
}

#[test]
fn domain_errors_are_typed_tool_errors() {
    let s = Server::new();
    let (e, b) = call(&s, "team_get", json!({}));
    assert!(e);
    assert_eq!(b["code"], "team_not_created");

    team(&s);
    let (e, b) = call(&s, "team_create", json!({ "name": "again", "actor": "x" }));
    assert!((e, b["code"].as_str()) == (true, Some("team_exists")));

    ok(
        &s,
        "task_create",
        json!({ "actor": "team-lead", "subject": "a", "owner": "echo" }),
    );
    let (e, b) = call(&s, "task_complete", json!({ "task": "1", "actor": "echo" }));
    assert!(e);
    assert_eq!(b["code"], "illegal_transition");
    assert!(b["message"].as_str().unwrap().contains("Pending"));

    let (_, b) = call(&s, "task_start", json!({ "task": "99", "actor": "echo" }));
    assert_eq!(b["code"], "task_not_found");
    let (_, b) = call(
        &s,
        "task_create",
        json!({ "actor": "echo", "subject": "x", "owner": "ghost" }),
    );
    assert_eq!(b["code"], "unknown_member");
    // failed creates don't burn ids
    assert_eq!(
        ok(
            &s,
            "task_create",
            json!({ "actor": "echo", "subject": "b" })
        )["task"]["id"],
        "2"
    );
}

#[test]
fn owner_block_archive_and_list_filters() {
    let s = Server::new();
    team(&s);
    for subject in ["a", "b", "c"] {
        ok(
            &s,
            "task_create",
            json!({ "actor": "team-lead", "subject": subject }),
        );
    }
    ok(
        &s,
        "task_set_owner",
        json!({ "task": "1", "actor": "team-lead", "owner": "echo" }),
    );
    ok(
        &s,
        "task_set_owner",
        json!({ "task": "2", "actor": "team-lead", "owner": "echo" }),
    );
    ok(&s, "task_start", json!({ "task": "2", "actor": "echo" }));
    ok(
        &s,
        "task_block",
        json!({ "task": "2", "actor": "echo", "reason": "waiting" }),
    );
    assert_eq!(
        ok(&s, "task_get", json!({ "task": "2" }))["task"]["status"],
        "blocked"
    );
    assert_eq!(
        ok(&s, "task_unblock", json!({ "task": "2", "actor": "echo" }))["task"]["status"],
        "in_progress"
    );
    let (_, b) = call(&s, "task_unblock", json!({ "task": "2", "actor": "echo" }));
    assert_eq!(b["code"], "not_blocked");

    let ids = |v: Value| -> Vec<String> {
        v["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        ids(ok(&s, "task_list", json!({ "owner": "echo" }))),
        ["1", "2"]
    );
    assert_eq!(
        ids(ok(&s, "task_list", json!({ "status": "in_progress" }))),
        ["2"]
    );

    ok(
        &s,
        "task_archive",
        json!({ "task": "3", "actor": "team-lead" }),
    );
    assert_eq!(ids(ok(&s, "task_list", json!({}))), ["1", "2"]);
    assert_eq!(
        ids(ok(&s, "task_list", json!({ "include_archived": true }))),
        ["1", "2", "3"]
    );

    // unassign by omitting owner
    ok(
        &s,
        "task_set_owner",
        json!({ "task": "1", "actor": "team-lead" }),
    );
    assert_eq!(
        ok(&s, "task_get", json!({ "task": "1" }))["task"]["owner"],
        Value::Null
    );
}

#[tokio::test]
async fn stdio_loop_round_trip() {
    let server = Server::new();
    let (mut client, server_end) = duplex(64 * 1024);
    let (srv_read, srv_write) = tokio::io::split(server_end);
    let handle =
        tokio::spawn(async move { server.serve(BufReader::new(srv_read), srv_write).await });

    let (c_read, mut c_write) = tokio::io::split(&mut client);
    let mut lines = BufReader::new(c_read).lines();
    let reqs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"team_create","arguments":{"name":"t","actor":"lead"}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"task_create","arguments":{"actor":"lead","subject":"x","owner":"lead"}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"task_start","arguments":{"task":"1","actor":"lead"}}}),
    ];
    for r in &reqs {
        c_write
            .write_all(format!("{r}\n").as_bytes())
            .await
            .unwrap();
    }
    // 4 replies: the notification produces none, so ids arrive in order 1,2,3,4
    for want in 1..=4 {
        let line = lines.next_line().await.unwrap().unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], want, "{line}");
        assert!(v.get("error").is_none(), "{line}");
    }
    c_write.shutdown().await.unwrap();
    handle.await.unwrap().unwrap();
}

#[test]
fn dependencies_over_mcp() {
    let s = Server::new();
    team(&s);
    ok(
        &s,
        "task_create",
        json!({ "actor": "team-lead", "subject": "core", "owner": "echo" }),
    );
    let t = ok(
        &s,
        "task_create",
        json!({ "actor": "team-lead", "subject": "server", "owner": "echo", "blocked_by": ["1"] }),
    );
    assert_eq!(t["task"]["blocked_by"], json!(["1"]));
    assert_eq!(t["task"]["waiting_on"], json!(["1"]));
    let first = ok(&s, "task_get", json!({ "task": "1" }));
    assert_eq!(first["task"]["blocks"], json!(["2"]));

    // task 2 is out of echo's actionable list until task 1 is done
    let b = ok(&s, "task_briefing", json!({ "member": "echo" }));
    let act: Vec<_> = b["briefing"]["actionable"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["task"].clone())
        .collect();
    assert_eq!(act, [json!("1")]);
    assert_eq!(
        b["briefing"]["awareness"][0]["reason"],
        "waiting_on_dependencies"
    );

    ok(&s, "task_start", json!({ "task": "1", "actor": "echo" }));
    ok(&s, "task_complete", json!({ "task": "1", "actor": "echo" }));
    let b = ok(&s, "task_briefing", json!({ "member": "echo" }));
    assert_eq!(b["briefing"]["actionable"][0]["reason"], "start");
    assert_eq!(
        ok(&s, "task_get", json!({ "task": "2" }))["task"]["waiting_on"],
        json!([])
    );

    // cycles and unknown ids are typed errors; an empty list clears
    let (e, b) = call(
        &s,
        "task_set_dependencies",
        json!({ "task": "1", "actor": "echo", "blocked_by": ["2"] }),
    );
    assert!(e);
    assert_eq!(b["code"], "dependency_cycle");
    let (_, b) = call(
        &s,
        "task_set_dependencies",
        json!({ "task": "2", "actor": "echo", "blocked_by": ["9"] }),
    );
    assert_eq!(b["code"], "task_not_found");
    let t = ok(
        &s,
        "task_set_dependencies",
        json!({ "task": "2", "actor": "echo", "blocked_by": [] }),
    );
    assert_eq!(t["task"]["blocked_by"], json!([]));
}
