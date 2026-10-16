//! Cursor Agent CLI (`cursor-agent -p --output-format stream-json`).
//!
//! MCP: merged into `<workdir>/.cursor/mcp.json` (written by `build_command`); `--approve-mcps`
//! auto-approves it. NOTE: the file is shared by all members running in one workdir, so
//! simultaneous launches race on `RTEAMS_MEMBER` (the env is also set on the process itself).

use super::*;
use serde_json::Value;
use std::time::Duration;

#[derive(Default)]
pub struct Cursor {
    pub extra_candidates: Vec<PathBuf>,
}

pub fn parse_status(out: &str) -> AuthStatus {
    let s = strip_ansi(out);
    let l = s.to_lowercase();
    if l.contains("not logged in") || l.contains("not authenticated") {
        AuthStatus::SignedOut
    } else if l.contains("logged in") {
        let who = s
            .split("Logged in as")
            .nth(1)
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty());
        AuthStatus::SignedIn(who)
    } else {
        AuthStatus::Unknown
    }
}

impl AgentRuntime for Cursor {
    fn id(&self) -> &'static str {
        "cursor"
    }
    fn label(&self) -> &'static str {
        "Cursor Agent"
    }
    fn detect(&self) -> Detection {
        detect(
            &["cursor-agent", "agent"],
            &self.extra_candidates,
            &["--version"],
        )
    }
    fn auth_status(&self) -> AuthStatus {
        if std::env::var_os("CURSOR_API_KEY").is_some() {
            return AuthStatus::SignedIn(Some("CURSOR_API_KEY".into()));
        }
        match detected_bin(&["cursor-agent", "agent"], &self.extra_candidates) {
            Some(b) => run_timeout(&b, &["status"], Duration::from_secs(5))
                .map(|o| parse_status(&o))
                .unwrap_or(AuthStatus::Unknown),
            None => AuthStatus::Unknown,
        }
    }
    fn models(&self) -> Vec<ModelInfo> {
        [
            ("auto", "Auto"),
            ("sonnet-4.5", "Claude Sonnet 4.5"),
            ("gpt-5", "GPT-5"),
            ("grok", "Grok"),
        ]
        .into_iter()
        .map(|(id, label)| ModelInfo {
            id: id.into(),
            label: label.into(),
        })
        .collect()
    }
    fn install_hint(&self) -> &'static str {
        "curl https://cursor.com/install -fsS | bash"
    }
    fn build_command(&self, ctx: &LaunchCtx) -> Command {
        // Best effort: a failure here surfaces as the agent lacking the rteams tools.
        let _ = write_mcp_servers_file(
            &ctx.workdir,
            ".cursor/mcp.json",
            mcp_server_entry(ctx, false),
        );
        let mut c = base_command(ctx);
        c.arg("-p")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--approve-mcps")
            .arg("--workspace")
            .arg(&ctx.workdir);
        if let Some(m) = ctx
            .model
            .as_deref()
            .filter(|m| !m.is_empty() && *m != "default")
        {
            c.arg("--model").arg(m);
        }
        if let Some(r) = ctx.resume.as_deref().filter(|r| !r.is_empty()) {
            c.arg("--resume").arg(r);
        }
        if read_only_mode(ctx) {
            c.arg("--mode").arg("ask");
        } else if ctx.permissions != "manual" {
            // Without --force headless runs only propose changes.
            c.arg("--force");
        }
        c.arg(prompt_with_system(ctx));
        c
    }
    fn summarize_line(&self, line: &str) -> Vec<String> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return generic_summarize(line);
        };
        if v["type"].as_str() == Some("tool_call") {
            if v["subtype"].as_str() != Some("started") {
                return vec![];
            }
            let Some((k, body)) = v["tool_call"].as_object().and_then(|o| o.iter().next()) else {
                return vec![];
            };
            let (name, args) = if k == "mcpToolCall" {
                (
                    short_tool(body["args"]["toolName"].as_str().unwrap_or("mcp")).to_string(),
                    body["args"]["args"].to_string(),
                )
            } else {
                (
                    k.strip_suffix("ToolCall").unwrap_or(k).to_string(),
                    body["args"].to_string(),
                )
            };
            return vec![format!("tool: {name} {}", clip(&args, 120))];
        }
        super::claude::summarize_line(line)
    }
    fn session_id(&self, line: &str) -> Option<String> {
        let v: Value = serde_json::from_str(line).ok()?;
        match v["type"].as_str() {
            Some("system") | Some("result") => session_field(&v),
            _ => None,
        }
    }
    fn supports_resume(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtimes::testutil::{ctx, tmpdir};

    #[test]
    fn matrix_and_file() {
        let r = Cursor::default();
        for mode in ["do", "ask", "delegate"] {
            for perms in ["skip", "edits", "manual"] {
                let d = tmpdir(&format!("cur-{mode}-{perms}"));
                let mut x = ctx(mode, perms);
                x.workdir = d.clone();
                let a = args_of(&r.build_command(&x));
                assert_eq!(&a[..3], ["-p", "--output-format", "stream-json"]);
                assert!(a.contains(&"--approve-mcps".to_string()));
                assert_eq!(
                    a.contains(&"--force".to_string()),
                    mode == "do" && perms != "manual"
                );
                assert_eq!(a.contains(&"ask".to_string()), mode != "do");
                assert_eq!(a.last().unwrap(), "you are dev\n\n---\n\ndo the thing");
                let f: Value = serde_json::from_str(
                    &std::fs::read_to_string(d.join(".cursor/mcp.json")).unwrap(),
                )
                .unwrap();
                assert_eq!(f["mcpServers"]["rteams"]["command"], "/usr/bin/rteams");
                assert_eq!(f["mcpServers"]["rteams"]["env"]["RTEAMS_MEMBER"], "dev");
            }
        }
    }

    #[test]
    fn model_resume_events() {
        let d = tmpdir("cur-ev");
        let mut x = ctx("do", "skip");
        x.workdir = d;
        x.model = Some("gpt-5".into());
        x.resume = Some("chat-1".into());
        let a = args_of(&Cursor::default().build_command(&x));
        let i = a.iter().position(|s| s == "--resume").unwrap();
        assert_eq!(a[i + 1], "chat-1");
        assert!(a.contains(&"gpt-5".to_string()));
        let r = Cursor::default();
        let init = r#"{"type":"system","subtype":"init","session_id":"c1","model":"x"}"#;
        assert_eq!(r.session_id(init).as_deref(), Some("c1"));
        assert_eq!(r.summarize_line(init), vec!["session started"]);
        let t = r#"{"type":"tool_call","subtype":"started","call_id":"1","tool_call":{"readToolCall":{"args":{"path":"a.rs"}}}}"#;
        assert_eq!(r.summarize_line(t), vec![r#"tool: read {"path":"a.rs"}"#]);
        let m = r#"{"type":"tool_call","subtype":"started","tool_call":{"mcpToolCall":{"args":{"toolName":"task_create","args":{"x":1}}}}}"#;
        assert_eq!(r.summarize_line(m), vec![r#"tool: task_create {"x":1}"#]);
        assert!(r
            .summarize_line(
                r#"{"type":"tool_call","subtype":"completed","tool_call":{"readToolCall":{}}}"#
            )
            .is_empty());
        let res = r#"{"type":"result","subtype":"success","is_error":false,"result":"all done","session_id":"c1"}"#;
        assert_eq!(r.summarize_line(res), vec!["result: all done"]);
        assert_eq!(
            parse_status("Logged in as a@b.c"),
            AuthStatus::SignedIn(Some("a@b.c".into()))
        );
        assert_eq!(parse_status("Not logged in"), AuthStatus::SignedOut);
    }
}
