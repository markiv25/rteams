//! Analysis of recorded agent runs (`<home>/<team>/runs/<member>/<unix_ms>.jsonl`, raw Claude
//! stream-json lines): task logs, changed files, session history and per-member stats.
//! Task boundaries come from the rteams MCP tool calls `task_start`, `task_complete` and
//! `review_request`, whose input carries the task id.

use std::path::{Path, PathBuf};

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum RunKind {
    Text,
    Thinking,
    ToolUse {
        name: String,
        input: Value,
    },
    ToolResult {
        ok: bool,
    },
    Result {
        tokens_in: u64,
        tokens_out: u64,
        cost_usd: f64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunEvent {
    /// Unix ms of the run start (from the file name; stream-json lines carry no clock).
    pub ts_ms: u64,
    pub kind: RunKind,
    /// Text / thinking body, tool summary or result summary, for display.
    pub text: String,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskSegment {
    pub task: String,
    pub member: String,
    pub events: Vec<RunEvent>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummary {
    pub path: PathBuf,
    pub started_ms: u64,
    pub events: usize,
    pub tool_calls: usize,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Stats {
    pub runs: usize,
    pub tool_calls: usize,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

fn clip(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n).collect();
        t.push('\u{2026}');
        t
    }
}

fn start_ms(path: &Path) -> u64 {
    path.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.split('-').next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Parse one run file. Unreadable files give no events; bad lines are skipped.
pub fn read_run(path: &Path) -> Vec<RunEvent> {
    let Ok(raw) = std::fs::read(path) else {
        return Vec::new();
    };
    parse_run(&String::from_utf8_lossy(&raw), start_ms(path))
}

fn parse_run(raw: &str, ts_ms: u64) -> Vec<RunEvent> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let session_id = v
            .get("session_id")
            .and_then(Value::as_str)
            .map(String::from);
        let mut push = |kind: RunKind, text: String| {
            out.push(RunEvent {
                ts_ms,
                kind,
                text,
                session_id: session_id.clone(),
            });
        };
        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                let blocks = v.pointer("/message/content").and_then(Value::as_array);
                for b in blocks.into_iter().flatten() {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let t = b.get("text").and_then(Value::as_str).unwrap_or("");
                            push(RunKind::Text, t.to_string());
                        }
                        Some("thinking") => {
                            let t = b.get("thinking").and_then(Value::as_str).unwrap_or("");
                            push(RunKind::Thinking, t.to_string());
                        }
                        Some("tool_use") => {
                            let name = b.get("name").and_then(Value::as_str).unwrap_or("?");
                            let input = b.get("input").cloned().unwrap_or(Value::Null);
                            let text = format!("{name} {}", clip(&input.to_string(), 200));
                            push(
                                RunKind::ToolUse {
                                    name: name.to_string(),
                                    input,
                                },
                                text,
                            );
                        }
                        _ => {}
                    }
                }
            }
            Some("user") => {
                let blocks = v.pointer("/message/content").and_then(Value::as_array);
                for b in blocks.into_iter().flatten() {
                    if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                        let ok = !b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                        let text = match b.get("content") {
                            Some(Value::String(s)) => s.clone(),
                            Some(Value::Array(a)) => a
                                .iter()
                                .filter_map(|x| x.get("text").and_then(Value::as_str))
                                .collect::<Vec<_>>()
                                .join(" "),
                            _ => String::new(),
                        };
                        push(RunKind::ToolResult { ok }, clip(&text, 300));
                    }
                }
            }
            Some("result") => {
                let n = |p: &str| v.pointer(p).and_then(Value::as_u64).unwrap_or(0);
                let text = v
                    .get("result")
                    .and_then(Value::as_str)
                    .map(|s| clip(s, 300))
                    .unwrap_or_default();
                push(
                    RunKind::Result {
                        tokens_in: n("/usage/input_tokens"),
                        tokens_out: n("/usage/output_tokens"),
                        cost_usd: v
                            .get("total_cost_usd")
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0),
                    },
                    text,
                );
            }
            _ => {}
        }
    }
    out
}

fn run_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    v.sort_by_key(|p| (start_ms(p), p.clone()));
    v
}

fn members(home: &Path, team: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(home.join(team).join("runs"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    v.sort();
    v
}

enum Boundary {
    Start(String),
    End(String),
}

fn boundary(e: &RunEvent) -> Option<Boundary> {
    let RunKind::ToolUse { name, input } = &e.kind else {
        return None;
    };
    let task = match input.get("task")? {
        Value::String(s) => s.trim_start_matches('#').to_string(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if name.ends_with("task_start") {
        Some(Boundary::Start(task))
    } else if name.ends_with("task_complete") || name.ends_with("review_request") {
        Some(Boundary::End(task))
    } else {
        None
    }
}

/// Split a member's runs (directory `<home>/<team>/runs/<member>`) into per-task segments.
/// A segment opens at `task_start` and closes at `task_complete`/`review_request` of that task,
/// at the next `task_start`, or at the end of the run file.
pub fn task_segments(runs_dir_for_member: &Path) -> Vec<TaskSegment> {
    let member = runs_dir_for_member
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let mut out: Vec<TaskSegment> = Vec::new();
    for f in run_files(runs_dir_for_member) {
        let mut cur: Option<TaskSegment> = None;
        let close = |cur: &mut Option<TaskSegment>, out: &mut Vec<TaskSegment>| {
            if let Some(s) = cur.take() {
                out.push(s);
            }
        };
        for e in read_run(&f) {
            match boundary(&e) {
                Some(Boundary::Start(t)) => {
                    close(&mut cur, &mut out);
                    cur = Some(TaskSegment {
                        task: t,
                        member: member.clone(),
                        events: vec![e],
                    });
                }
                Some(Boundary::End(t)) => {
                    if let Some(mut s) = cur.take() {
                        s.events.push(e);
                        if s.task == t {
                            out.push(s);
                        } else {
                            // Another task's request while this one is still open.
                            cur = Some(s);
                        }
                    }
                }
                None => {
                    if let Some(s) = cur.as_mut() {
                        s.events.push(e);
                    }
                }
            }
        }
        close(&mut cur, &mut out);
    }
    out
}

fn norm(t: &str) -> &str {
    t.trim_start_matches('#')
}

/// Every event attributed to `task`, across members, oldest run first.
pub fn task_log(home: &Path, team: &str, task: &str) -> Vec<(String, RunEvent)> {
    let mut v = Vec::new();
    for m in members(home, team) {
        for s in task_segments(&home.join(team).join("runs").join(&m)) {
            if norm(&s.task) == norm(task) {
                v.extend(s.events.into_iter().map(|e| (m.clone(), e)));
            }
        }
    }
    v.sort_by_key(|(_, e)| e.ts_ms);
    v
}

/// Files written by Edit/Write/MultiEdit/NotebookEdit inside the task's segments, deduplicated
/// in first-touched order.
pub fn changed_files(home: &Path, team: &str, task: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for (_, e) in task_log(home, team, task) {
        let RunKind::ToolUse { name, input } = &e.kind else {
            continue;
        };
        if !matches!(
            name.as_str(),
            "Edit" | "Write" | "MultiEdit" | "NotebookEdit"
        ) {
            continue;
        }
        let p = ["file_path", "notebook_path", "path"]
            .iter()
            .find_map(|k| input.get(*k).and_then(Value::as_str));
        if let Some(p) = p {
            if !v.iter().any(|x| x == p) {
                v.push(p.to_string());
            }
        }
    }
    v
}

fn summarize(path: &Path) -> SessionSummary {
    let mut s = SessionSummary {
        path: path.to_path_buf(),
        started_ms: start_ms(path),
        events: 0,
        tool_calls: 0,
        tokens_in: 0,
        tokens_out: 0,
    };
    for e in read_run(path) {
        s.events += 1;
        match e.kind {
            RunKind::ToolUse { .. } => s.tool_calls += 1,
            RunKind::Result {
                tokens_in,
                tokens_out,
                ..
            } => {
                s.tokens_in += tokens_in;
                s.tokens_out += tokens_out;
            }
            _ => {}
        }
    }
    s
}

/// One summary per recorded run of `member`, newest first.
pub fn sessions(home: &Path, team: &str, member: &str) -> Vec<SessionSummary> {
    let dir = home.join(team).join("runs").join(member);
    let mut v: Vec<_> = run_files(&dir).iter().map(|p| summarize(p)).collect();
    v.reverse();
    v
}

pub fn member_stats(home: &Path, team: &str, member: &str) -> Stats {
    sessions(home, team, member)
        .iter()
        .fold(Stats::default(), |mut a, s| {
            a.runs += 1;
            a.tool_calls += s.tool_calls;
            a.tokens_in += s.tokens_in;
            a.tokens_out += s.tokens_out;
            a
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, input: Value) -> String {
        serde_json::json!({"type":"assistant","session_id":"s1","message":{"content":[
            {"type":"tool_use","name":name,"input":input}]}})
        .to_string()
    }

    fn write_run(home: &Path, member: &str, ms: u64, lines: &[String]) {
        let d = home.join("t").join("runs").join(member);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(format!("{ms}.jsonl")), lines.join("\n")).unwrap();
    }

    fn setup(tag: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!("rteams-runs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let result = serde_json::json!({"type":"result","result":"done","total_cost_usd":0.25,
            "usage":{"input_tokens":10,"output_tokens":5}})
        .to_string();
        write_run(
            &home,
            "ada",
            1000,
            &[
                "not json".into(),
                tool("Read", serde_json::json!({"file_path":"/x/ignored.rs"})),
                tool("mcp__rteams__task_start", serde_json::json!({"task":"1"})),
                serde_json::json!({"type":"assistant","message":{"content":[
                    {"type":"thinking","thinking":"hmm"},{"type":"text","text":"working"}]}})
                .to_string(),
                tool("Edit", serde_json::json!({"file_path":"/x/a.rs"})),
                serde_json::json!({"type":"user","message":{"content":[
                    {"type":"tool_result","content":"ok","is_error":false}]}})
                .to_string(),
                tool("Write", serde_json::json!({"file_path":"/x/b.rs"})),
                tool("Edit", serde_json::json!({"file_path":"/x/a.rs"})),
                tool(
                    "mcp__rteams__review_request",
                    serde_json::json!({"task":"1"}),
                ),
                tool("Edit", serde_json::json!({"file_path":"/x/after.rs"})),
                tool("mcp__rteams__task_start", serde_json::json!({"task":2})),
                tool("Write", serde_json::json!({"file_path":"/x/c.rs"})),
                result,
            ],
        );
        write_run(
            &home,
            "ada",
            2000,
            &[
                tool("mcp__rteams__task_start", serde_json::json!({"task":"1"})),
                tool("Edit", serde_json::json!({"file_path":"/x/d.rs"})),
            ],
        );
        home
    }

    #[test]
    fn read_run_skips_bad_lines_and_parses_kinds() {
        let home = setup("read");
        let ev = read_run(&home.join("t/runs/ada/1000.jsonl"));
        assert!(ev.iter().any(|e| e.kind == RunKind::Thinking));
        assert!(ev
            .iter()
            .any(|e| e.kind == RunKind::Text && e.text == "working"));
        assert!(ev
            .iter()
            .any(|e| e.kind == RunKind::ToolResult { ok: true } && e.text == "ok"));
        assert!(matches!(
            ev.last().unwrap().kind,
            RunKind::Result {
                tokens_in: 10,
                tokens_out: 5,
                ..
            }
        ));
        assert_eq!(ev[0].ts_ms, 1000);
        assert_eq!(ev[0].session_id.as_deref(), Some("s1"));
        assert!(read_run(&home.join("missing.jsonl")).is_empty());
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn segments_and_changed_files() {
        let home = setup("seg");
        let segs = task_segments(&home.join("t/runs/ada"));
        let tasks: Vec<&str> = segs.iter().map(|s| s.task.as_str()).collect();
        assert_eq!(tasks, ["1", "2", "1"]);
        assert_eq!(segs[0].member, "ada");
        assert_eq!(
            changed_files(&home, "t", "1"),
            ["/x/a.rs", "/x/b.rs", "/x/d.rs"]
        );
        assert_eq!(changed_files(&home, "t", "2"), ["/x/c.rs"]);
        assert!(changed_files(&home, "t", "9").is_empty());
        let log = task_log(&home, "t", "#1");
        assert!(log.iter().all(|(m, _)| m == "ada"));
        assert!(log.iter().any(|(_, e)| e.text == "working"));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn sessions_and_stats() {
        let home = setup("stats");
        let s = sessions(&home, "t", "ada");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].started_ms, 2000);
        assert_eq!(s[1].tokens_in, 10);
        let st = member_stats(&home, "t", "ada");
        assert_eq!(st.runs, 2);
        assert_eq!(st.tokens_out, 5);
        assert_eq!(st.tool_calls, 11);
        assert_eq!(member_stats(&home, "t", "nobody"), Stats::default());
        let _ = std::fs::remove_dir_all(home);
    }
}
