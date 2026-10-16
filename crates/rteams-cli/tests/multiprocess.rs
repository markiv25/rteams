//! Real `rteams` processes over stdio sharing one RTEAMS_HOME: writes in one are visible in
//! the others, across restarts, and concurrent writers never lose or duplicate events.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

struct Proc {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Proc {
    fn spawn(home: &Path, team: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rteams"))
            .arg("serve")
            .env("RTEAMS_HOME", home)
            .env("RTEAMS_TEAM", team)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn rteams");
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Proc {
            child,
            stdin,
            stdout,
            next_id: 0,
        }
    }

    /// tools/call; returns (is_error, body).
    fn call(&mut self, name: &str, args: Value) -> (bool, Value) {
        self.next_id += 1;
        let req = json!({ "jsonrpc": "2.0", "id": self.next_id, "method": "tools/call",
                          "params": { "name": name, "arguments": args } });
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{req}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let v: Value =
            serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad reply {line:?}: {e}"));
        assert_eq!(v["id"], self.next_id);
        let r = &v["result"];
        assert!(r.is_object(), "rpc error for {name}: {v}");
        let body: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
        (r["isError"].as_bool().unwrap(), body)
    }

    fn ok(&mut self, name: &str, args: Value) -> Value {
        let (e, b) = self.call(name, args);
        assert!(!e, "{name} failed: {b}");
        b
    }

    fn close(mut self) {
        drop(self.stdin.take());
        assert!(self.child.wait().unwrap().success());
    }
}

fn create_team(p: &mut Proc) {
    p.ok(
        "team_create",
        json!({ "name": "t", "actor": "team-lead", "members": [
            { "name": "echo", "role": "developer" }, { "name": "vector", "role": "developer" } ] }),
    );
}

#[test]
fn write_in_one_process_is_read_in_another() {
    let home = tempfile::tempdir().unwrap();
    let mut a = Proc::spawn(home.path(), "t");
    let mut b = Proc::spawn(home.path(), "t");

    create_team(&mut a);
    let made = a.ok(
        "task_create",
        json!({ "actor": "team-lead", "subject": "shared", "owner": "echo" }),
    );
    let id = made["task"]["id"].as_str().expect("task id").to_string();

    // B started before the team existed and still sees A's writes.
    let list = b.ok("task_list", json!({}));
    assert_eq!(list["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(list["tasks"][0]["subject"], "shared");

    // B writes, A reads.
    b.ok("task_start", json!({ "task": id, "actor": "echo" }));
    let list = a.ok("task_list", json!({ "status": "in_progress" }));
    assert_eq!(list["tasks"].as_array().unwrap().len(), 1);

    // Domain rules hold across processes: A cannot repeat B's transition.
    let (is_err, body) = a.call("task_start", json!({ "task": id, "actor": "echo" }));
    assert!(is_err, "{body}");
    assert_eq!(body["code"], "illegal_transition");

    a.close();
    b.close();
}

#[test]
fn state_survives_restart_and_teams_are_isolated() {
    let home = tempfile::tempdir().unwrap();
    let mut a = Proc::spawn(home.path(), "one");
    create_team(&mut a);
    a.ok(
        "task_create",
        json!({ "actor": "team-lead", "subject": "kept" }),
    );
    a.close();

    let mut again = Proc::spawn(home.path(), "one");
    let list = again.ok("task_list", json!({}));
    assert_eq!(list["tasks"][0]["subject"], "kept");
    again.close();

    let mut other = Proc::spawn(home.path(), "two");
    let (is_err, _) = other.call("task_list", json!({}));
    let tasks = if is_err {
        0
    } else {
        other.ok("task_list", json!({}))["tasks"]
            .as_array()
            .unwrap()
            .len()
    };
    assert_eq!(tasks, 0);
    other.close();

    assert!(home.path().join("one/events.jsonl").exists());
    assert!(home.path().join("one/tasks").is_dir());
}

#[test]
fn concurrent_writers_in_separate_processes_lose_nothing() {
    const PROCS: usize = 4;
    const PER: usize = 15;
    let home = tempfile::tempdir().unwrap();
    {
        let mut setup = Proc::spawn(home.path(), "t");
        create_team(&mut setup);
        setup.close();
    }

    let threads: Vec<_> = (0..PROCS)
        .map(|n| {
            let home = home.path().to_path_buf();
            std::thread::spawn(move || {
                let mut p = Proc::spawn(&home, "t");
                for i in 0..PER {
                    p.ok(
                        "task_create",
                        json!({ "actor": "team-lead", "subject": format!("p{n}-{i}") }),
                    );
                }
                p.close();
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }

    let mut reader = Proc::spawn(home.path(), "t");
    let tasks = reader.ok("task_list", json!({}))["tasks"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(tasks.len(), PROCS * PER, "no task lost");
    let mut ids: Vec<_> = tasks
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), PROCS * PER, "no duplicate ids");
    reader.close();

    // The log is gapless: seq 1..=n, one event per line.
    let log = std::fs::read_to_string(home.path().join("t/events.jsonl")).unwrap();
    for (i, line) in log.lines().enumerate() {
        let v: Value = serde_json::from_str(line).unwrap();
        assert_eq!(v["seq"], i as u64 + 1);
    }
}

#[test]
fn command_id_makes_retries_idempotent_across_processes() {
    let home = tempfile::tempdir().unwrap();
    let mut a = Proc::spawn(home.path(), "t");
    let mut b = Proc::spawn(home.path(), "t");
    create_team(&mut a);
    let args = json!({ "actor": "team-lead", "subject": "once", "command_id": "cmd-xyz" });
    let first = a.ok("task_create", args.clone());
    let retry = b.ok("task_create", args);
    assert_eq!(first["task"]["id"], retry["task"]["id"]);
    assert_eq!(
        a.ok("task_list", json!({}))["tasks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    a.close();
    b.close();
}
