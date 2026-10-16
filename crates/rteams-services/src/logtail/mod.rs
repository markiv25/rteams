//! logtail: incremental tail of recorded agent runs
//! (`<home>/<team>/runs/<member>/<unix_ms>.jsonl`, raw Claude stream-json lines), merged across
//! members in time order, with previews and read markers.

pub mod cache;
pub mod merge;
pub mod preview;
pub mod stream;

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LogKind {
    Text,
    Thinking,
    ToolUse,
    ToolResult,
    Error,
    /// End-of-run summary line.
    Result,
}

impl LogKind {
    pub fn label(self) -> &'static str {
        match self {
            LogKind::Text => "Message",
            LogKind::Thinking => "Thinking",
            LogKind::ToolUse => "Tool use",
            LogKind::ToolResult => "Tool result",
            LogKind::Error => "Error",
            LogKind::Result => "Result",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    /// Unix ms (when first observed, or the run start plus line index for history).
    pub ts_ms: i64,
    pub member: String,
    pub kind: LogKind,
    /// Single-line display text.
    pub text: String,
}

pub(crate) fn clip(s: &str, max: usize) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        one
    } else {
        let h: String = one.chars().take(max).collect();
        format!("{h}...")
    }
}

/// Entries of one raw run line (a line may hold several content blocks). Non-JSON lines
/// starting with `stderr:` / `error:` become errors; other junk is ignored.
pub fn parse_line(line: &str, member: &str, ts_ms: i64) -> Vec<LogEntry> {
    let line = line.trim();
    if line.is_empty() {
        return vec![];
    }
    let mut out = Vec::new();
    let mut push = |kind, text: String| {
        out.push(LogEntry {
            ts_ms,
            member: member.to_string(),
            kind,
            text,
        });
    };
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        let lower = line.to_lowercase();
        if lower.starts_with("stderr:") || lower.starts_with("error:") {
            push(LogKind::Error, clip(line, 400));
        }
        return out;
    };
    match v.get("type").and_then(Value::as_str) {
        Some("assistant") => {
            for b in v
                .pointer("/message/content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                match b.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        push(LogKind::Text, clip(b["text"].as_str().unwrap_or(""), 400))
                    }
                    Some("thinking") => push(
                        LogKind::Thinking,
                        clip(b["thinking"].as_str().unwrap_or(""), 400),
                    ),
                    Some("tool_use") => {
                        let name = b["name"].as_str().unwrap_or("?");
                        let input = b.get("input").map(|i| i.to_string()).unwrap_or_default();
                        push(LogKind::ToolUse, format!("{name} {}", clip(&input, 200)));
                    }
                    _ => {}
                }
            }
        }
        Some("user") => {
            for b in v
                .pointer("/message/content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                    let bad = b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                    let text = match b.get("content") {
                        Some(Value::String(s)) => s.clone(),
                        Some(Value::Array(a)) => a
                            .iter()
                            .filter_map(|x| x.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join(" "),
                        _ => String::new(),
                    };
                    push(
                        if bad {
                            LogKind::Error
                        } else {
                            LogKind::ToolResult
                        },
                        clip(&text, 300),
                    );
                }
            }
        }
        Some("result") => {
            let bad = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            let text = clip(v.get("result").and_then(Value::as_str).unwrap_or(""), 300);
            push(if bad { LogKind::Error } else { LogKind::Result }, text);
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks_errors_and_junk() {
        let a = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"},{"type":"tool_use","name":"Bash","input":{"cmd":"ls"}}]}}"#;
        let e = parse_line(a, "bob", 5);
        assert_eq!(e.len(), 2);
        assert_eq!(e[1].kind, LogKind::ToolUse);
        assert!(e[1].text.starts_with("Bash "));
        let u = r#"{"type":"user","message":{"content":[{"type":"tool_result","is_error":true,"content":"nope"}]}}"#;
        assert_eq!(parse_line(u, "bob", 5)[0].kind, LogKind::Error);
        assert_eq!(parse_line("stderr: boom", "bob", 5)[0].kind, LogKind::Error);
        assert!(parse_line("garbage", "bob", 5).is_empty());
        assert!(parse_line("{\"type\":\"system\"}", "bob", 5).is_empty());
    }
}
