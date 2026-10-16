//! Two in-process servers over one FileStore root share state through the log.

use rteams_server::Server;
use rteams_store::FileStore;
use serde_json::{json, Value};

fn call(s: &Server, name: &str, args: Value) -> Result<Value, Value> {
    let req = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                      "params": { "name": name, "arguments": args } });
    let v: Value = serde_json::from_str(&s.handle_line(&req.to_string()).unwrap()).unwrap();
    let r = &v["result"];
    let body: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
    if r["isError"] == true {
        Err(body)
    } else {
        Ok(body)
    }
}

#[test]
fn two_servers_share_state_and_replay_command_ids() {
    let dir = tempfile::tempdir().unwrap();
    let a = Server::with_store(FileStore::open(dir.path(), "t").unwrap());
    let b = Server::with_store(FileStore::open(dir.path(), "t").unwrap());

    call(
        &a,
        "team_create",
        json!({ "name": "t", "actor": "team-lead", "members": [{ "name": "echo", "role": "developer" }] }),
    )
    .unwrap();
    let args = json!({ "actor": "team-lead", "subject": "x", "command_id": "c1" });
    let first = call(&a, "task_create", args.clone()).unwrap();
    let retry = call(&b, "task_create", args).unwrap();
    assert_eq!(first["task"]["id"], retry["task"]["id"]);

    let seen = call(&b, "task_list", json!({})).unwrap();
    assert_eq!(seen["tasks"].as_array().unwrap().len(), 1);
    let board_a = call(&a, "kanban_get", json!({})).unwrap();
    let board_b = call(&b, "kanban_get", json!({})).unwrap();
    assert_eq!(board_a, board_b);
}

#[test]
fn corrupt_log_surfaces_as_store_error_tool_result() {
    let dir = tempfile::tempdir().unwrap();
    let s = Server::with_store(FileStore::open(dir.path(), "t").unwrap());
    std::fs::write(dir.path().join("t/events.jsonl"), "not json\n").unwrap();
    let err = call(&s, "task_list", json!({})).unwrap_err();
    assert_eq!(err["code"], "store_error");
}

#[test]
fn retried_command_id_replays_for_policy_checked_tools_across_servers() {
    let dir = tempfile::tempdir().unwrap();
    let a = Server::with_store(FileStore::open(dir.path(), "t").unwrap());
    let b = Server::with_store(FileStore::open(dir.path(), "t").unwrap());
    call(
        &a,
        "team_create",
        json!({ "name": "t", "actor": "team-lead", "members": [{ "name": "echo", "role": "developer" }] }),
    )
    .unwrap();
    let made = call(
        &a,
        "task_create",
        json!({ "actor": "team-lead", "subject": "x", "owner": "echo" }),
    )
    .unwrap();
    let id = made["task"]["id"].as_str().unwrap().to_string();

    let args = json!({ "task": id, "actor": "echo", "command_id": "start-1" });
    let first = call(&a, "task_start", args.clone()).unwrap();
    // The task is already in_progress; a plain repeat is illegal, a retry with the id replays.
    let retry = call(&b, "task_start", args).unwrap();
    assert_eq!(first["event_seq"], retry["event_seq"]);
    let plain = call(&b, "task_start", json!({ "task": id, "actor": "echo" })).unwrap_err();
    assert_eq!(plain["code"], "illegal_transition");
}

fn two_servers() -> (tempfile::TempDir, Server, Server) {
    let dir = tempfile::tempdir().unwrap();
    let a = Server::with_store(FileStore::open(dir.path(), "t").unwrap());
    let b = Server::with_store(FileStore::open(dir.path(), "t").unwrap());
    call(
        &a,
        "team_create",
        json!({ "name": "t", "actor": "team-lead", "members": [
            { "name": "echo", "role": "developer" }, { "name": "pixel", "role": "reviewer" }] }),
    )
    .unwrap();
    call(
        &a,
        "task_create",
        json!({ "actor": "team-lead", "subject": "x", "owner": "echo" }),
    )
    .unwrap();
    (dir, a, b)
}

#[test]
fn policy_is_checked_against_latest_state_under_the_lock() {
    let (_dir, a, b) = two_servers();
    // `a` has seen echo own the task; `b` reassigns it before `a` acts on that belief.
    call(&a, "task_list", json!({})).unwrap();
    call(
        &b,
        "task_set_owner",
        json!({ "task": "1", "actor": "team-lead", "owner": "pixel" }),
    )
    .unwrap();

    let err = call(&a, "task_start", json!({ "task": "1", "actor": "echo" })).unwrap_err();
    assert_eq!(err["code"], "forbidden");
    let ok = call(&a, "task_start", json!({ "task": "1", "actor": "pixel" })).unwrap();
    assert_eq!(ok["task"]["status"], "in_progress");
}

#[test]
fn only_one_of_two_racing_claims_wins() {
    let dir = tempfile::tempdir().unwrap();
    let a = Server::with_store(FileStore::open(dir.path(), "t").unwrap());
    let b = Server::with_store(FileStore::open(dir.path(), "t").unwrap());
    call(
        &a,
        "team_create",
        json!({ "name": "t", "actor": "team-lead", "members": [
            { "name": "echo", "role": "developer" }, { "name": "pixel", "role": "reviewer" }] }),
    )
    .unwrap();
    call(
        &a,
        "task_create",
        json!({ "actor": "team-lead", "subject": "free" }),
    )
    .unwrap();
    call(&b, "task_list", json!({})).unwrap();

    call(
        &a,
        "task_set_owner",
        json!({ "task": "1", "actor": "echo", "owner": "echo" }),
    )
    .unwrap();
    let err = call(
        &b,
        "task_set_owner",
        json!({ "task": "1", "actor": "pixel", "owner": "pixel" }),
    )
    .unwrap_err();
    assert_eq!(err["code"], "forbidden");
    let t = call(&b, "task_get", json!({ "task": "1" })).unwrap();
    assert_eq!(t["task"]["owner"], "echo");
}

#[test]
fn retry_after_losing_ownership_replays_instead_of_forbidden() {
    let (_dir, a, b) = two_servers();
    let args = json!({ "task": "1", "actor": "echo", "command_id": "start-1" });
    let first = call(&a, "task_start", args.clone()).unwrap();
    call(
        &b,
        "task_set_owner",
        json!({ "task": "1", "actor": "team-lead", "owner": "pixel" }),
    )
    .unwrap();
    // echo is no longer the owner and the task already moved on; the retry still returns the original
    let retry = call(&a, "task_start", args).unwrap();
    assert_eq!(first["event_seq"], retry["event_seq"]);
}
