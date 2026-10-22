//! Per-task log stream: every agent's stream-json lines, attributed to the task the agent was
//! working on. A tool call naming a task (`task_start`, `task_add_comment`, `task_complete`,
//! the heartbeat's `task`, ...) switches the member's current task; the lines after it (text,
//! tool calls, tool results) belong to that task until another one is named or it completes.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Mutex;

use serde_json::Value;

/// Entries kept per task.
const MAX_PER_TASK: usize = 400;
/// Characters kept of an entry's detail.
const MAX_DETAIL: usize = 4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Tool,
    ToolResult,
    Result,
    Error,
}

/// One log entry of a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub member: String,
    /// Unix ms (live) or run-file time plus line index (history); orders entries.
    pub ts: i64,
    pub kind: Kind,
    pub summary: String,
    pub detail: String,
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}\n... (truncated)")
    }
}

fn one_line(s: &str, max: usize) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        one
    } else {
        let head: String = one.chars().take(max).collect();
        format!("{head}...")
    }
}

/// The task a tool call names: `input.task` (or `input.task_id`), only for rteams task tools
/// and the heartbeat.
fn task_of_call(name: &str, input: &Value) -> Option<String> {
    let short = name.strip_prefix("mcp__rteams__").unwrap_or(name);
    if !(short.starts_with("task_") || short.contains("heartbeat")) {
        return None;
    }
    ["task", "task_id", "id"]
        .iter()
        .find_map(|k| input.get(k).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn tool_result_text(part: &Value) -> String {
    match &part["content"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}

/// Per-member attribution state.
#[derive(Debug, Default, Clone)]
pub struct Attributor {
    current: Option<String>,
}

impl Attributor {
    /// Feed one raw stream-json line of `member`; returns `(task, entry)` pairs it produced.
    pub fn feed(&mut self, member: &str, ts: i64, line: &str) -> Vec<(String, Entry)> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return vec![];
        };
        let mut out = Vec::new();
        let mut add = |task: &Option<String>, kind: Kind, summary: String, detail: String| {
            if let Some(t) = task {
                out.push((
                    t.clone(),
                    Entry {
                        member: member.to_string(),
                        ts,
                        kind,
                        summary,
                        detail: clip(&detail, MAX_DETAIL),
                    },
                ));
            }
        };
        match v["type"].as_str() {
            Some("assistant") => {
                for part in v["message"]["content"].as_array().into_iter().flatten() {
                    match part["type"].as_str() {
                        Some("text") => {
                            let t = part["text"].as_str().unwrap_or("");
                            if !t.trim().is_empty() {
                                add(&self.current, Kind::Text, one_line(t, 160), t.to_string());
                            }
                        }
                        Some("tool_use") => {
                            let name = part["name"].as_str().unwrap_or("?");
                            if let Some(t) = task_of_call(name, &part["input"]) {
                                self.current = Some(t);
                            }
                            let short = name.strip_prefix("mcp__rteams__").unwrap_or(name);
                            let input = serde_json::to_string_pretty(&part["input"])
                                .unwrap_or_else(|_| part["input"].to_string());
                            add(
                                &self.current,
                                Kind::Tool,
                                format!(
                                    "tool: {short} {}",
                                    one_line(&part["input"].to_string(), 100)
                                ),
                                input,
                            );
                            if short == "task_complete" {
                                self.current = None;
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("user") => {
                for part in v["message"]["content"].as_array().into_iter().flatten() {
                    if part["type"].as_str() == Some("tool_result") {
                        let text = tool_result_text(part);
                        if !text.trim().is_empty() {
                            let kind = if part["is_error"].as_bool().unwrap_or(false) {
                                Kind::Error
                            } else {
                                Kind::ToolResult
                            };
                            add(&self.current, kind, one_line(&text, 160), text);
                        }
                    }
                }
            }
            Some("result") => {
                let r = v["result"].as_str().unwrap_or("");
                let err = v["is_error"].as_bool().unwrap_or(false);
                let label = if err { "error" } else { "result" };
                add(
                    &self.current,
                    if err { Kind::Error } else { Kind::Result },
                    format!("{label}: {}", one_line(r, 160)),
                    r.to_string(),
                );
                self.current = None;
            }
            _ => {}
        }
        out
    }
}

/// Attribute a whole raw run (history). `base_ts` + line index order the entries.
pub fn attribute_run(member: &str, raw: &str, base_ts: i64) -> Vec<(String, Entry)> {
    let mut a = Attributor::default();
    raw.lines()
        .enumerate()
        .flat_map(|(i, l)| a.feed(member, base_ts + i as i64, l))
        .collect()
}

#[derive(Default)]
struct TeamLogs {
    attr: HashMap<String, Attributor>,
    tasks: BTreeMap<String, Vec<Entry>>,
}

impl TeamLogs {
    fn push(&mut self, task: String, e: Entry) {
        let v = self.tasks.entry(task).or_default();
        if v.len() >= MAX_PER_TASK {
            v.remove(0);
        }
        v.push(e);
    }
}

static STORE: Mutex<BTreeMap<String, TeamLogs>> = Mutex::new(BTreeMap::new());

fn with_team<R>(team: &str, f: impl FnOnce(&mut TeamLogs) -> R) -> R {
    let mut g = STORE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.entry(team.to_string()).or_default())
}

/// Live: one raw stdout line of `member`'s agent.
pub fn ingest(team: &str, member: &str, ts: i64, line: &str) {
    with_team(team, |t| {
        let a = t.attr.entry(member.to_string()).or_default();
        let found = a.feed(member, ts, line);
        for (task, e) in found {
            t.push(task, e);
        }
    });
}

/// A new run of `member` starts: attribution restarts (no current task).
pub fn reset_member(team: &str, member: &str) {
    with_team(team, |t| {
        t.attr.remove(member);
    });
}

/// Entries of `task`, oldest first.
pub fn entries(team: &str, task: &str) -> Vec<Entry> {
    with_team(team, |t| t.tasks.get(task).cloned().unwrap_or_default())
}

/// Load recorded runs under `<home>/<team>/runs/<member>/<ms>.jsonl` older than `before_ms`
/// (runs started since belong to the live feed), oldest first.
pub fn load_history(home: &Path, team: &str, before_ms: i64) {
    let Ok(dirs) = std::fs::read_dir(home.join(team).join("runs")) else {
        return;
    };
    let mut runs: Vec<(i64, String, std::path::PathBuf)> = Vec::new();
    for d in dirs.flatten() {
        let Some(member) = d.file_name().to_str().map(str::to_string) else {
            continue;
        };
        for f in std::fs::read_dir(d.path()).into_iter().flatten().flatten() {
            let p = f.path();
            let ms = p
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse::<i64>().ok());
            if let (Some(ms), true) = (ms, p.extension().is_some_and(|e| e == "jsonl")) {
                if ms < before_ms {
                    runs.push((ms, member.clone(), p));
                }
            }
        }
    }
    runs.sort();
    for (ms, member, path) in runs {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let found = attribute_run(&member, &raw, ms);
        with_team(team, |t| {
            for (task, e) in found {
                t.push(task, e);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant(parts: Value) -> String {
        serde_json::json!({"type":"assistant","message":{"content":parts}}).to_string()
    }

    fn fixture() -> Vec<String> {
        vec![
            r#"{"type":"system","subtype":"init","session_id":"s"}"#.into(),
            assistant(serde_json::json!([{"type":"text","text":"before any task"}])),
            assistant(serde_json::json!([{"type":"tool_use","name":"mcp__rteams__task_start","input":{"actor":"ada","task":"T1"}}])),
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"started T1"}]}}"#.into(),
            assistant(serde_json::json!([{"type":"text","text":"working on T1 now"},{"type":"tool_use","name":"Bash","input":{"command":"ls"}}])),
            assistant(serde_json::json!([{"type":"tool_use","name":"mcp__rteams__task_add_comment","input":{"task":"T2","text":"hi"}}])),
            assistant(serde_json::json!([{"type":"tool_use","name":"mcp__rteams__task_complete","input":{"task":"T2"}}])),
            assistant(serde_json::json!([{"type":"text","text":"after complete"}])),
            assistant(serde_json::json!([{"type":"tool_use","name":"mcp__rteams__member_heartbeat","input":{"status":"working","task":"T3"}}])),
            r#"{"type":"result","result":"all done","is_error":false}"#.into(),
        ]
    }

    #[test]
    fn attributes_lines_to_the_task_in_progress() {
        let raw = fixture().join("\n");
        let got = attribute_run("ada", &raw, 1000);
        let of = |t: &str| -> Vec<&str> {
            got.iter()
                .filter(|(k, _)| k == t)
                .map(|(_, e)| e.summary.as_str())
                .collect()
        };
        let t1 = of("T1");
        assert_eq!(t1.len(), 4, "{t1:?}");
        assert!(t1[0].starts_with("tool: task_start"));
        assert_eq!(t1[1], "started T1");
        assert_eq!(t1[2], "working on T1 now");
        assert!(t1[3].starts_with("tool: Bash"));
        let t2 = of("T2");
        assert_eq!(t2.len(), 2);
        assert!(t2[1].starts_with("tool: task_complete"));
        // Nothing before a task is named, nothing after completion until the next one.
        assert!(!got.iter().any(|(_, e)| e.summary == "before any task"));
        assert!(!got.iter().any(|(_, e)| e.summary == "after complete"));
        let t3 = of("T3");
        assert_eq!(t3.len(), 2);
        assert_eq!(t3[1], "result: all done");
        assert_eq!(got[0].1.member, "ada");
        assert!(got[0].1.detail.contains("\"task\": \"T1\""));
    }

    #[test]
    fn live_store_and_history() {
        let team = format!("tl-{}", std::process::id());
        for (i, l) in fixture().iter().enumerate() {
            ingest(&team, "ada", i as i64, l);
        }
        assert_eq!(entries(&team, "T1").len(), 4);
        assert!(entries(&team, "nope").is_empty());
        reset_member(&team, "ada");
        ingest(
            &team,
            "ada",
            99,
            &assistant(serde_json::json!([{"type":"text","text":"x"}])),
        );
        assert_eq!(entries(&team, "T1").len(), 4);

        let home = std::env::temp_dir().join(format!("rteams-tl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let runs = home.join("hist").join("runs").join("bob");
        std::fs::create_dir_all(&runs).unwrap();
        std::fs::write(runs.join("5000.jsonl"), fixture().join("\n")).unwrap();
        std::fs::write(runs.join("9000000000000.jsonl"), fixture().join("\n")).unwrap();
        load_history(&home, "hist", 6000);
        assert_eq!(entries("hist", "T1").len(), 4);
        assert_eq!(entries("hist", "T1")[0].member, "bob");
        let _ = std::fs::remove_dir_all(&home);
    }
}
