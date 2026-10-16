//! The exact conversation Claude Code's MCP client has with a stdio server, run against the
//! real `rteams` binary: initialize -> notifications/initialized -> tools/list -> tools/call
//! (read + write + a tool error), with the requirement that stdout carries nothing but
//! JSON-RPC (a stray banner or log line there breaks the handshake).
//!
//! Verified by hand against Claude Code 2.1.301: `claude -p --mcp-config <.mcp.json>
//! --strict-mcp-config` reported the server `connected` with all tools exposed as
//! `mcp__rteams__<tool>`.

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

/// Spawn like Claude Code does for `"command": "rteams", "args": ["serve", ...]`, send every
/// request line, close stdin (what Claude Code does on shutdown), and return
/// (replies by id, raw stdout, stderr).
fn session(home: &std::path::Path, requests: &[Value]) -> (HashMap<u64, Value>, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rteams"))
        .args(["serve", "--home"])
        .arg(home)
        .args(["--team", "demo"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rteams");
    let mut stdin = child.stdin.take().unwrap();
    for r in requests {
        writeln!(stdin, "{r}").unwrap();
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "server exits cleanly on stdin EOF");
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();

    let mut replies = HashMap::new();
    for line in stdout.lines() {
        let v: Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("stdout must be JSON-RPC only, got {line:?}: {e}"));
        assert_eq!(v["jsonrpc"], "2.0", "{line}");
        assert!(
            v.get("result").is_some() ^ v.get("error").is_some(),
            "exactly one of result/error: {line}"
        );
        let id = v["id"].as_u64().expect("replies carry the request id");
        assert!(replies.insert(id, v).is_none(), "one reply per request");
    }
    (replies, stdout, stderr)
}

fn req(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn initialize(id: u64, version: &str) -> Value {
    req(
        id,
        "initialize",
        json!({
            "protocolVersion": version,
            "capabilities": { "roots": {}, "elicitation": {} },
            "clientInfo": { "name": "claude-code", "version": "2.1.301" },
        }),
    )
}

fn initialized() -> Value {
    json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
}

/// Claude Code attaches a progress token in `_meta`; the server must tolerate it.
fn call(id: u64, name: &str, args: Value) -> Value {
    req(
        id,
        "tools/call",
        json!({ "name": name, "arguments": args, "_meta": { "progressToken": id } }),
    )
}

fn body(reply: &Value) -> (bool, Value) {
    let r = &reply["result"];
    assert_eq!(r["content"][0]["type"], "text");
    let text = r["content"][0]["text"].as_str().expect("text content");
    let parsed: Value = serde_json::from_str(text).unwrap();
    let is_error = r["isError"].as_bool().expect("isError is a bool");
    if !is_error {
        assert_eq!(
            r["structuredContent"], parsed,
            "structuredContent is an object mirror"
        );
        assert!(r["structuredContent"].is_object());
    }
    (is_error, parsed)
}

#[test]
fn claude_code_handshake_list_and_call_sequence() {
    let home = tempfile::tempdir().unwrap();
    let (r, stdout, stderr) = session(
        home.path(),
        &[
            initialize(1, "2025-06-18"),
            initialized(),
            req(2, "tools/list", json!({})),
            call(
                3,
                "team_create",
                json!({ "name": "demo", "actor": "lead", "members": [
                    { "name": "echo", "role": "developer" } ] }),
            ),
            call(
                4,
                "task_create",
                json!({ "actor": "lead", "subject": "hello", "owner": "echo" }),
            ),
            call(5, "task_briefing", json!({ "member": "echo" })),
            call(6, "task_start", json!({ "task": "1", "actor": "echo" })),
            call(
                7,
                "task_complete",
                json!({ "task": "999", "actor": "echo" }),
            ),
            req(8, "ping", json!({})),
        ],
    );

    // initialize: echoes a version we both speak, advertises tools, names itself.
    let init = &r[&1]["result"];
    assert_eq!(init["protocolVersion"], "2025-06-18");
    assert!(init["capabilities"]["tools"].is_object());
    assert_eq!(init["serverInfo"]["name"], "rteams");
    assert!(init["serverInfo"]["version"]
        .as_str()
        .is_some_and(|v| !v.is_empty()));

    // tools/list: names Claude Code can expose as `mcp__rteams__<name>`, object schemas.
    let tools = r[&2]["result"]["tools"].as_array().unwrap();
    assert!(tools.len() >= 24, "all tools listed, got {}", tools.len());
    for t in tools {
        let name = t["name"].as_str().unwrap();
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "{name}"
        );
        assert!(
            format!("mcp__rteams__{name}").len() <= 64,
            "{name} too long once Claude Code prefixes it"
        );
        assert!(
            t["description"].as_str().is_some_and(|d| !d.is_empty()),
            "{name}"
        );
        assert_eq!(t["inputSchema"]["type"], "object", "{name}");
        assert!(t["inputSchema"]["properties"].is_object(), "{name}");
    }
    for n in [
        "team_create",
        "task_create",
        "task_briefing",
        "member_briefing",
        "task_start",
    ] {
        assert!(tools.iter().any(|t| t["name"] == n), "missing {n}");
    }

    // write, then read it back, then a state-changing write
    let (e, team) = body(&r[&3]);
    assert!(!e, "{team}");
    let (e, task) = body(&r[&4]);
    assert!(!e && task["task"]["id"] == "1", "{task}");
    let (e, briefing) = body(&r[&5]);
    assert!(!e);
    assert_eq!(briefing["briefing"]["actionable"][0]["reason"], "start");
    let (e, started) = body(&r[&6]);
    assert!(
        !e && started["task"]["status"] == "in_progress",
        "{started}"
    );

    // a domain failure is a tool error the model can read, not a protocol error
    assert!(r[&7].get("error").is_none());
    let (e, err) = body(&r[&7]);
    assert!(e);
    assert_eq!(err["code"], "task_not_found");

    assert_eq!(r[&8]["result"], json!({}));

    // nothing but the replies above on stdout; the banner goes to stderr
    assert_eq!(
        stdout.lines().count(),
        8,
        "one line per request, none for the notification"
    );
    assert!(stderr.contains("rteams: state in"), "{stderr}");
}

#[test]
fn newer_client_protocol_version_gets_a_version_we_support() {
    let home = tempfile::tempdir().unwrap();
    let (r, _, _) = session(home.path(), &[initialize(1, "2099-01-01")]);
    let v = r[&1]["result"]["protocolVersion"].as_str().unwrap();
    assert!(
        ["2025-06-18", "2025-03-26", "2024-11-05"].contains(&v),
        "{v}"
    );
}

#[test]
fn older_protocol_versions_are_accepted_as_is() {
    for v in ["2025-03-26", "2024-11-05"] {
        let home = tempfile::tempdir().unwrap();
        let (r, _, _) = session(home.path(), &[initialize(1, v)]);
        assert_eq!(r[&1]["result"]["protocolVersion"], v);
    }
}

#[test]
fn unknown_methods_are_json_rpc_errors_not_silence() {
    // A client that probes optional capabilities (resources/prompts) must get a reply.
    let home = tempfile::tempdir().unwrap();
    let (r, _, _) = session(
        home.path(),
        &[
            initialize(1, "2025-06-18"),
            initialized(),
            req(2, "resources/list", json!({})),
            req(3, "prompts/list", json!({})),
        ],
    );
    for id in [2, 3] {
        assert_eq!(r[&id]["error"]["code"], -32601);
    }
}

#[test]
fn two_claude_code_sessions_share_state_through_rteams_home() {
    // Each agent's Claude Code spawns its own `rteams serve` with the same RTEAMS_HOME/team.
    let home = tempfile::tempdir().unwrap();
    let (r, _, _) = session(
        home.path(),
        &[
            initialize(1, "2025-06-18"),
            initialized(),
            call(
                2,
                "team_create",
                json!({ "name": "demo", "actor": "lead", "members": [
                    { "name": "echo", "role": "developer" } ] }),
            ),
            call(
                3,
                "task_create",
                json!({ "actor": "lead", "subject": "shared", "owner": "echo" }),
            ),
        ],
    );
    assert!(!body(&r[&3]).0);

    let (r, _, _) = session(
        home.path(),
        &[
            initialize(1, "2025-06-18"),
            initialized(),
            call(2, "task_briefing", json!({ "member": "echo" })),
        ],
    );
    let (e, b) = body(&r[&2]);
    assert!(!e);
    assert_eq!(b["briefing"]["actionable"][0]["subject"], "shared");
}
