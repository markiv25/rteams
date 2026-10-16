//! Tool executions: a `tool_use` block paired with its `tool_result`, a one-line summary for
//! the collapsed card, and per-tool statistics.

use std::collections::BTreeMap;

use serde_json::Value;

/// One tool call and (when found) its result.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolExec {
    pub id: String,
    pub name: String,
    pub input: Value,
    pub result: Option<String>,
    pub is_error: bool,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    /// Subagent transcript id reported by the result of a `Task` / `Agent` call.
    pub agent_id: Option<String>,
}

impl ToolExec {
    pub fn duration_ms(&self) -> Option<i64> {
        match (self.start_ms, self.end_ms) {
            (Some(a), Some(b)) if b >= a => Some(b - a),
            _ => None,
        }
    }

    /// Whether this call spawns a subagent.
    pub fn is_task(&self) -> bool {
        matches!(self.name.as_str(), "Task" | "Agent")
    }

    pub fn summary(&self) -> String {
        summarize(&self.name, &self.input)
    }
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

fn clip(t: &str, n: usize) -> String {
    let t = t.trim().replace('\n', " ");
    if t.chars().count() <= n {
        t
    } else {
        let mut o: String = t.chars().take(n).collect();
        o.push('\u{2026}');
        o
    }
}

fn base(p: &str) -> &str {
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

/// One-line description of a tool call, like the original's `ToolSummaryFormatter`.
pub fn summarize(name: &str, input: &Value) -> String {
    match name {
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => s(input, "file_path")
            .or_else(|| s(input, "notebook_path"))
            .map(|p| base(p).to_string())
            .unwrap_or_default(),
        "Bash" | "shell" | "exec_command" => s(input, "command")
            .map(|c| clip(c, 80))
            .or_else(|| {
                input.get("command").and_then(Value::as_array).map(|a| {
                    clip(
                        &a.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(" "),
                        80,
                    )
                })
            })
            .unwrap_or_default(),
        "Grep" | "Glob" => s(input, "pattern").map(|p| clip(p, 60)).unwrap_or_default(),
        "Task" | "Agent" => s(input, "description")
            .or_else(|| s(input, "subagent_type"))
            .map(|d| clip(d, 60))
            .unwrap_or_default(),
        "WebFetch" => s(input, "url").map(|u| clip(u, 70)).unwrap_or_default(),
        "WebSearch" => s(input, "query").map(|q| clip(q, 70)).unwrap_or_default(),
        "TodoWrite" => input
            .get("todos")
            .and_then(Value::as_array)
            .map(|a| format!("{} todos", a.len()))
            .unwrap_or_default(),
        _ => {
            // MCP tools and anything else: first short string field.
            input
                .as_object()
                .and_then(|o| o.values().find_map(Value::as_str))
                .map(|v| clip(v, 60))
                .unwrap_or_default()
        }
    }
}

/// The shell command of a `Bash` / Codex `shell` call (string or argv array).
pub fn command_of(input: &Value) -> Option<String> {
    match input.get("command")? {
        Value::String(c) => Some(c.clone()),
        Value::Array(a) => Some(
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => None,
    }
}

/// Files a tool call changes (`Edit`, `Write`, ...), if any.
pub fn written_file(name: &str, input: &Value) -> Option<String> {
    match name {
        "Write" | "Edit" | "MultiEdit" => s(input, "file_path").map(str::to_string),
        "NotebookEdit" => s(input, "notebook_path").map(str::to_string),
        _ => None,
    }
}

/// Files a tool call only reads.
pub fn read_file(name: &str, input: &Value) -> Option<String> {
    (name == "Read").then(|| s(input, "file_path").map(str::to_string))?
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolStat {
    pub calls: u64,
    pub errors: u64,
}

/// Calls and errors per tool name, sorted by name.
pub fn stats<'a>(it: impl IntoIterator<Item = &'a ToolExec>) -> BTreeMap<String, ToolStat> {
    let mut m: BTreeMap<String, ToolStat> = BTreeMap::new();
    for t in it {
        let e = m.entry(t.name.clone()).or_default();
        e.calls += 1;
        if t.is_error {
            e.errors += 1;
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn summaries() {
        assert_eq!(summarize("Read", &json!({"file_path":"/a/b/c.rs"})), "c.rs");
        assert_eq!(
            summarize("Bash", &json!({"command":"git status\n-s"})),
            "git status -s"
        );
        assert_eq!(summarize("TodoWrite", &json!({"todos":[1,2]})), "2 todos");
        assert_eq!(summarize("mcp__x__y", &json!({"q":"hello"})), "hello");
    }

    #[test]
    fn stats_count_errors() {
        let t = |n: &str, e: bool| ToolExec {
            id: "i".into(),
            name: n.into(),
            input: Value::Null,
            result: None,
            is_error: e,
            start_ms: Some(10),
            end_ms: Some(25),
            agent_id: None,
        };
        let v = vec![t("Bash", false), t("Bash", true), t("Read", false)];
        let m = stats(&v);
        assert_eq!(
            m["Bash"],
            ToolStat {
                calls: 2,
                errors: 1
            }
        );
        assert_eq!(v[0].duration_ms(), Some(15));
    }
}
