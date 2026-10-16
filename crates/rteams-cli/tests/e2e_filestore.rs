//! Black-box E2E against the real `rteams` binary over stdio, backed by the shared on-disk
//! FileStore (`RTEAMS_HOME`). This layers on top of `multiprocess.rs` (write-visibility,
//! restart, concurrent id-allocation, command_id idempotency) and `claude_code.rs`
//! (handshake, tools/list, protocol versions) — it does NOT re-cover those. The gaps covered
//! here, which no existing binary-level test exercises:
//!   - the full review lifecycle over the wire (request -> changes -> fix -> request -> approve
//!     -> complete) with derived task JSON (blocks / waiting_on),
//!   - projection correctness end to end (kanban columns by status, briefing actionable vs
//!     awareness incl. waiting_on_dependencies, inbox cursor advancing across two processes),
//!   - authorization over the wire (non-owner forbidden, lead override, domain error winning
//!     over policy),
//!   - a deep restart equality check (the whole board, not just one subject),
//!   - crash-tail repair driven through a fresh binary (store-level repair is covered; this
//!     proves it end to end through `rteams`).
//!
//! Tests assert tool behaviour, not transport internals, so they survive a future rmcp swap.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

/// A live `rteams serve` process you can drive request-by-request over stdio.
struct Proc {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Proc {
    fn spawn(home: &Path, team: &str) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_rteams"))
            .arg("serve")
            .env("RTEAMS_HOME", home)
            .env("RTEAMS_TEAM", team)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn rteams");
        let mut child = child;
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Proc {
            child,
            stdin,
            stdout,
            next_id: 0,
        }
    }

    fn send(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let req =
            json!({ "jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params });
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{req}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let v: Value =
            serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad reply {line:?}: {e}"));
        assert_eq!(v["jsonrpc"], "2.0", "{line}");
        assert_eq!(v["id"], self.next_id, "{line}");
        v
    }

    /// tools/call; returns (is_error, decoded body). The body mirrors structuredContent.
    fn call(&mut self, name: &str, args: Value) -> (bool, Value) {
        let v = self.send("tools/call", json!({ "name": name, "arguments": args }));
        let r = &v["result"];
        assert!(r.is_object(), "expected a tool result, got {v}");
        let body: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
        (r["isError"].as_bool().unwrap(), body)
    }

    fn ok(&mut self, name: &str, args: Value) -> Value {
        let (is_err, body) = self.call(name, args);
        assert!(!is_err, "{name} unexpectedly failed: {body}");
        body
    }

    fn err(&mut self, name: &str, args: Value) -> Value {
        let (is_err, body) = self.call(name, args);
        assert!(is_err, "{name} unexpectedly succeeded: {body}");
        body
    }

    fn close(mut self) {
        drop(self.stdin.take());
        assert!(self.child.wait().unwrap().success(), "server exits cleanly");
    }
}

/// team-lead + echo (developer) + pixel (reviewer): the common cast for these tests.
fn make_team(p: &mut Proc) {
    p.ok(
        "team_create",
        json!({ "name": "t", "actor": "team-lead", "members": [
            { "name": "echo", "role": "developer" },
            { "name": "pixel", "role": "reviewer" } ] }),
    );
}

fn ids(list: &Value) -> Vec<String> {
    list["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect()
}

fn col_ids(board: &Value, col: &str) -> Vec<String> {
    board["board"][col]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect()
}

/// (task id, reason) pairs.
type Pairs = Vec<(String, String)>;

/// (task id, reason) pairs for a member's actionable and awareness buckets.
fn buckets(briefing: &Value) -> (Pairs, Pairs) {
    let pick = |bucket: &str| {
        briefing["briefing"][bucket]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                (
                    i["task"].as_str().unwrap().to_string(),
                    i["reason"].as_str().unwrap().to_string(),
                )
            })
            .collect::<Vec<_>>()
    };
    (pick("actionable"), pick("awareness"))
}

/// The whole review lifecycle over the wire, plus the dependency relation and its briefing
/// projection — all against the real binary and the on-disk store, then re-read after restart.
#[test]
fn full_review_lifecycle_and_dependency_projection_over_the_wire() {
    let home = tempfile::tempdir().unwrap();
    let mut p = Proc::spawn(home.path(), "t");
    make_team(&mut p);

    // A dependency relation: task 2 is blocked_by task 1. Stored one way, derived the other.
    let core = p.ok(
        "task_create",
        json!({ "actor": "team-lead", "subject": "core", "owner": "echo" }),
    );
    assert_eq!(core["task"]["id"], "1");
    let server = p.ok(
        "task_create",
        json!({ "actor": "team-lead", "subject": "server", "owner": "echo", "blocked_by": ["1"] }),
    );
    assert_eq!(server["task"]["blocked_by"], json!(["1"]));
    assert_eq!(server["task"]["waiting_on"], json!(["1"]));
    assert_eq!(
        p.ok("task_get", json!({ "task": "1" }))["task"]["blocks"],
        json!(["2"])
    );

    // echo may start task 1 now, but task 2 is awareness-only until its dependency is done.
    // Task 2 stays pending (in the `todo` column) throughout this test.
    let (act, aware) = buckets(&p.ok("task_briefing", json!({ "member": "echo" })));
    assert_eq!(act, [("1".into(), "start".into())]);
    assert_eq!(aware, [("2".into(), "waiting_on_dependencies".into())]);

    // Walk task 1 through the full review loop: start -> comment -> request -> changes ->
    // fix -> request -> approve -> complete, checking the kanban column at each stop.
    assert_eq!(
        p.ok("task_start", json!({ "task": "1", "actor": "echo" }))["task"]["status"],
        "in_progress"
    );
    assert_eq!(
        col_ids(&p.ok("kanban_get", json!({})), "in_progress"),
        ["1"]
    );

    p.ok(
        "task_add_comment",
        json!({ "task": "1", "actor": "pixel", "text": "early look" }),
    );

    assert_eq!(
        p.ok(
            "review_request",
            json!({ "task": "1", "actor": "echo", "reviewer": "pixel" })
        )["task"]["status"],
        "review"
    );
    assert_eq!(col_ids(&p.ok("kanban_get", json!({})), "review"), ["1"]);
    // pixel, as the reviewer, sees it as actionable "review".
    let (act, _) = buckets(&p.ok("task_briefing", json!({ "member": "pixel" })));
    assert_eq!(act, [("1".into(), "review".into())]);

    assert_eq!(
        p.ok(
            "review_request_changes",
            json!({ "task": "1", "actor": "pixel", "comment": "add tests" })
        )["task"]["status"],
        "needs_fix"
    );
    // needs_fix lives in the todo column (alongside still-pending task 2); echo sees it as
    // actionable "fix" and gets the review note.
    assert_eq!(col_ids(&p.ok("kanban_get", json!({})), "todo"), ["1", "2"]);
    let (act, _) = buckets(&p.ok("task_briefing", json!({ "member": "echo" })));
    assert!(
        act.contains(&("1".to_string(), "fix".to_string())),
        "{act:?}"
    );
    // echo owns task 1, so its inbox carries pixel's task comments in order: the "early look"
    // note, then the review's "add tests".
    let inbox_texts: Vec<_> = p.ok("inbox_get", json!({ "member": "echo" }))["inbox"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["text"].as_str().unwrap().to_string())
        .collect();
    assert!(
        inbox_texts.contains(&"add tests".to_string()),
        "{inbox_texts:?}"
    );

    p.ok("task_start", json!({ "task": "1", "actor": "echo" }));
    p.ok(
        "review_request",
        json!({ "task": "1", "actor": "echo", "reviewer": "pixel" }),
    );
    assert_eq!(
        p.ok(
            "review_approve",
            json!({ "task": "1", "actor": "pixel", "note": "lgtm" })
        )["task"]["status"],
        "approved"
    );
    assert_eq!(col_ids(&p.ok("kanban_get", json!({})), "approved"), ["1"]);
    // approved -> owner's actionable reason is "finish".
    let (act, _) = buckets(&p.ok("task_briefing", json!({ "member": "echo" })));
    assert!(
        act.contains(&("1".to_string(), "finish".to_string())),
        "{act:?}"
    );

    assert_eq!(
        p.ok("task_complete", json!({ "task": "1", "actor": "echo" }))["task"]["status"],
        "done"
    );
    assert_eq!(col_ids(&p.ok("kanban_get", json!({})), "done"), ["1"]);

    // Finishing the dependency clears task 2's wait: waiting_on empties and the relation is gone.
    assert_eq!(
        p.ok("task_get", json!({ "task": "2" }))["task"]["waiting_on"],
        json!([])
    );

    let board_before = p.ok("kanban_get", json!({}));
    p.close();

    // Restart against the same home: the board rebuilds byte-for-byte from the log.
    let mut again = Proc::spawn(home.path(), "t");
    assert_eq!(
        again.ok("kanban_get", json!({})),
        board_before,
        "board survives restart"
    );
    assert_eq!(
        again.ok("task_get", json!({ "task": "1" }))["task"]["status"],
        "done"
    );
    again.close();
}

/// The dependency relation only shapes the briefing — the core does not gate transitions on it.
/// Prove the real semantics over the wire: a member CAN start a task whose dependency is not
/// done; the relation merely keeps it out of the actionable bucket beforehand.
#[test]
fn dependencies_do_not_gate_transitions_over_the_wire() {
    let home = tempfile::tempdir().unwrap();
    let mut p = Proc::spawn(home.path(), "t");
    make_team(&mut p);
    p.ok(
        "task_create",
        json!({ "actor": "team-lead", "subject": "dep", "owner": "echo" }),
    );
    p.ok(
        "task_create",
        json!({ "actor": "team-lead", "subject": "dependent", "owner": "echo", "blocked_by": ["1"] }),
    );

    // Task 2 is awareness-only (waiting_on_dependencies), not actionable, while task 1 is open.
    let (act, aware) = buckets(&p.ok("task_briefing", json!({ "member": "echo" })));
    assert_eq!(act, [("1".into(), "start".into())]);
    assert_eq!(aware, [("2".into(), "waiting_on_dependencies".into())]);

    // Yet there is no hard gate: echo may start task 2 even though task 1 is not done.
    assert_eq!(
        p.ok("task_start", json!({ "task": "2", "actor": "echo" }))["task"]["status"],
        "in_progress"
    );
    // Once started, it is actionable to resume (the dependency no longer hides it).
    let (act, _) = buckets(&p.ok("task_briefing", json!({ "member": "echo" })));
    assert!(
        act.contains(&("2".to_string(), "resume".to_string())),
        "{act:?}"
    );
    p.close();
}

/// Two separate processes on one RTEAMS_HOME see each other's writes, and the inbox cursor
/// advances correctly across the process boundary.
#[test]
fn cross_process_visibility_and_inbox_cursor_advances() {
    let home = tempfile::tempdir().unwrap();
    let mut a = Proc::spawn(home.path(), "t");
    let mut b = Proc::spawn(home.path(), "t");
    make_team(&mut a);

    // Create in A, read in B.
    let made = a.ok(
        "task_create",
        json!({ "actor": "team-lead", "subject": "shared", "owner": "echo" }),
    );
    let id = made["task"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        ids(&b.ok("task_list", json!({}))),
        *std::slice::from_ref(&id)
    );

    // Transition in B, observe in A's kanban.
    b.ok("task_start", json!({ "task": id, "actor": "echo" }));
    assert_eq!(
        col_ids(&a.ok("kanban_get", json!({})), "in_progress"),
        *std::slice::from_ref(&id)
    );

    // Direct message to echo written in A; echo reads it in B and captures the cursor.
    a.ok(
        "message_send",
        json!({ "actor": "pixel", "to": "echo", "text": "ping" }),
    );
    let inbox = b.ok("inbox_get", json!({ "member": "echo" }));
    let texts: Vec<_> = inbox["inbox"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["text"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(texts, ["ping"]);
    let cursor = inbox["inbox"]["next_cursor"].clone();

    // Nothing new: same cursor comes back, no items.
    let repeat = b.ok("inbox_get", json!({ "member": "echo", "after": cursor }));
    assert_eq!(repeat["inbox"]["items"], json!([]));
    assert_eq!(repeat["inbox"]["next_cursor"], cursor);

    // A new message in A shows up past the cursor in B, and the cursor moves forward.
    a.ok(
        "message_send",
        json!({ "actor": "pixel", "to": "echo", "text": "again" }),
    );
    let newer = b.ok("inbox_get", json!({ "member": "echo", "after": cursor }));
    assert_eq!(newer["inbox"]["items"][0]["text"], "again");
    assert!(
        newer["inbox"]["next_cursor"].as_u64().unwrap() > cursor.as_u64().unwrap(),
        "cursor advances"
    );

    a.close();
    b.close();
}

/// Authorization is enforced over the wire: a non-owner member is refused lifecycle writes, a
/// lead may override, and a domain (state) error beats a policy error.
#[test]
fn authorization_and_error_precedence_over_the_wire() {
    let home = tempfile::tempdir().unwrap();
    let mut p = Proc::spawn(home.path(), "t");
    make_team(&mut p);
    p.ok(
        "task_create",
        json!({ "actor": "team-lead", "subject": "owned", "owner": "echo" }),
    );
    p.ok(
        "task_create",
        json!({ "actor": "team-lead", "subject": "pending-owned", "owner": "echo" }),
    );

    // pixel does not own task 1: forbidden, and the task is untouched.
    assert_eq!(
        p.err("task_start", json!({ "task": "1", "actor": "pixel" }))["code"],
        "forbidden"
    );
    assert_eq!(
        p.ok("task_get", json!({ "task": "1" }))["task"]["status"],
        "pending"
    );

    // The lead may start any task (override).
    assert_eq!(
        p.ok("task_start", json!({ "task": "1", "actor": "team-lead" }))["task"]["status"],
        "in_progress"
    );

    // Non-owner completing an in_progress task is a policy error...
    assert_eq!(
        p.err("task_complete", json!({ "task": "1", "actor": "pixel" }))["code"],
        "forbidden"
    );
    // ...but a non-owner completing a *pending* task gets the domain error: state beats policy.
    assert_eq!(
        p.err("task_complete", json!({ "task": "2", "actor": "pixel" }))["code"],
        "illegal_transition"
    );

    // Comments stay open to any roster member (that is how results are reported) but reject ghosts.
    p.ok(
        "task_add_comment",
        json!({ "task": "1", "actor": "pixel", "text": "fyi" }),
    );
    assert_eq!(
        p.err(
            "task_add_comment",
            json!({ "task": "1", "actor": "ghost", "text": "x" })
        )["code"],
        "unknown_member"
    );
    p.close();
}

/// A whole scripted board (mixed statuses: done, review, blocked, pending-with-dep) rebuilds
/// identically after a cold restart — a stronger guarantee than one surviving subject.
#[test]
fn full_board_rebuilds_identically_after_restart() {
    let home = tempfile::tempdir().unwrap();
    let mut a = Proc::spawn(home.path(), "t");
    make_team(&mut a);
    for subj in ["done-one", "in-review", "blocked-one", "pending-one"] {
        a.ok(
            "task_create",
            json!({ "actor": "team-lead", "subject": subj, "owner": "echo" }),
        );
    }
    // task 4 depends on task 1.
    a.ok(
        "task_set_dependencies",
        json!({ "task": "4", "actor": "echo", "blocked_by": ["1"] }),
    );
    // task 1 -> done
    a.ok("task_start", json!({ "task": "1", "actor": "echo" }));
    a.ok("task_complete", json!({ "task": "1", "actor": "echo" }));
    // task 2 -> review
    a.ok("task_start", json!({ "task": "2", "actor": "echo" }));
    a.ok(
        "review_request",
        json!({ "task": "2", "actor": "echo", "reviewer": "pixel" }),
    );
    // task 3 -> blocked
    a.ok("task_start", json!({ "task": "3", "actor": "echo" }));
    a.ok(
        "task_block",
        json!({ "task": "3", "actor": "echo", "reason": "waiting" }),
    );

    let board = a.ok("kanban_get", json!({}));
    let briefing_echo = a.ok("task_briefing", json!({ "member": "echo" }));
    a.close();

    let mut b = Proc::spawn(home.path(), "t");
    assert_eq!(
        b.ok("kanban_get", json!({})),
        board,
        "kanban identical after restart"
    );
    assert_eq!(
        b.ok("task_briefing", json!({ "member": "echo" })),
        briefing_echo,
        "briefing identical after restart"
    );
    // spot-check the derived columns reflect the scripted statuses. A blocked task stays in the
    // column of the status it was blocked from: task 3 was blocked while in_progress, so it sits
    // in `in_progress` (badged blocked), not `todo`. Only the still-pending task 4 is in `todo`.
    assert_eq!(col_ids(&board, "done"), ["1"]);
    assert_eq!(col_ids(&board, "review"), ["2"]);
    assert_eq!(col_ids(&board, "in_progress"), ["3"]);
    assert_eq!(col_ids(&board, "todo"), ["4"]);
    assert_eq!(
        board["board"]["in_progress"][0]["status"], "blocked",
        "blocked badge preserved"
    );
    b.close();
}

/// A crash mid-append leaves a torn final line. A fresh `rteams` process must ignore the torn
/// tail, still read all complete events, and keep appending with a gapless sequence. (The store
/// unit test covers the mechanism; this proves it end to end through the binary.)
#[test]
fn torn_tail_is_repaired_through_a_fresh_process() {
    let home = tempfile::tempdir().unwrap();
    {
        let mut a = Proc::spawn(home.path(), "t");
        make_team(&mut a);
        a.ok(
            "task_create",
            json!({ "actor": "team-lead", "subject": "kept", "owner": "echo" }),
        );
        a.close();
    }
    let log = home.path().join("t/events.jsonl");
    let complete = std::fs::read_to_string(&log).unwrap();
    let complete_lines = complete.lines().count();
    // Simulate a crash in the middle of writing the next event.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&log)
        .unwrap()
        .write_all(br#"{"seq":99,"ts":1,"actor":"echo","even"#)
        .unwrap();

    // A fresh process ignores the torn tail and still sees the committed state.
    let mut b = Proc::spawn(home.path(), "t");
    assert_eq!(ids(&b.ok("task_list", json!({}))), ["1"]);
    assert_eq!(
        b.ok("task_get", json!({ "task": "1" }))["task"]["subject"],
        "kept"
    );
    // It can keep working: a new append overwrites the torn tail and lands as the next event.
    b.ok("task_start", json!({ "task": "1", "actor": "echo" }));
    b.close();

    // The log is well-formed and gapless after the repair: seq 1..=n, one JSON event per line.
    let repaired = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<_> = repaired.lines().collect();
    assert_eq!(
        lines.len(),
        complete_lines + 1,
        "one clean event replaced the torn tail"
    );
    for (i, line) in lines.iter().enumerate() {
        let v: Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("line {i} not JSON: {e}"));
        assert_eq!(v["seq"], i as u64 + 1, "gapless seq");
    }
}
