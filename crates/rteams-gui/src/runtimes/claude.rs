//! Claude Code (`claude`): the default runtime.
//!
//! `claude -p <prompt> --output-format=stream-json --verbose --mcp-config=<json>
//! --strict-mcp-config --append-system-prompt <text> [--model m] [--resume id] <permissions>`.

use super::*;
use serde_json::Value;
use std::time::Duration;

#[derive(Default)]
pub struct Claude {
    pub extra_candidates: Vec<PathBuf>,
}

/// Aliases accepted by `--model` plus pinned ids.
pub fn catalog() -> Vec<ModelInfo> {
    [
        ("default", "Default (recommended)"),
        ("sonnet", "Sonnet (latest)"),
        ("opus", "Opus (latest)"),
        ("haiku", "Haiku (latest)"),
        ("opusplan", "Opus plan / Sonnet execute"),
        ("claude-opus-4-5", "Claude Opus 4.5"),
        ("claude-sonnet-4-5", "Claude Sonnet 4.5"),
        ("claude-haiku-4-5", "Claude Haiku 4.5"),
    ]
    .into_iter()
    .map(|(id, label)| ModelInfo {
        id: id.into(),
        label: label.into(),
    })
    .collect()
}

pub fn parse_auth_status(json: &str) -> AuthStatus {
    match serde_json::from_str::<Value>(json) {
        Ok(v) => match v["loggedIn"].as_bool() {
            Some(true) => AuthStatus::SignedIn(
                v["email"]
                    .as_str()
                    .or_else(|| v["authMethod"].as_str())
                    .map(str::to_string),
            ),
            Some(false) => AuthStatus::SignedOut,
            None => AuthStatus::Unknown,
        },
        Err(_) => AuthStatus::Unknown,
    }
}

impl AgentRuntime for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }
    fn label(&self) -> &'static str {
        "Claude Code"
    }
    fn detect(&self) -> Detection {
        detect(&["claude"], &self.extra_candidates, &["--version"])
    }
    fn auth_status(&self) -> AuthStatus {
        if std::env::var_os("ANTHROPIC_API_KEY").is_some() {
            return AuthStatus::SignedIn(Some("ANTHROPIC_API_KEY".into()));
        }
        if let Some(bin) = detected_bin(&["claude"], &self.extra_candidates) {
            if let Some(out) = run_timeout(&bin, &["auth", "status"], Duration::from_secs(5)) {
                let st = parse_auth_status(&out);
                if st != AuthStatus::Unknown {
                    return st;
                }
            }
        }
        match home_dir() {
            Some(h) if h.join(".claude/.credentials.json").exists() => AuthStatus::SignedIn(None),
            // macOS keeps the login in the keychain, which we do not read.
            _ => AuthStatus::Unknown,
        }
    }
    fn models(&self) -> Vec<ModelInfo> {
        catalog()
    }
    fn install_hint(&self) -> &'static str {
        "npm install -g @anthropic-ai/claude-code   (or: curl -fsSL https://claude.ai/install.sh | bash)"
    }
    fn build_command(&self, ctx: &LaunchCtx) -> Command {
        let mcp = serde_json::json!({ "mcpServers": { "rteams": mcp_server_entry(ctx, false) } });
        let mut c = base_command(ctx);
        c.arg("-p")
            .arg(&ctx.prompt)
            .arg("--output-format=stream-json")
            .arg("--verbose")
            .arg(format!("--mcp-config={mcp}"))
            .arg("--strict-mcp-config");
        if !ctx.system_prompt.trim().is_empty() {
            c.arg("--append-system-prompt").arg(&ctx.system_prompt);
        }
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
        match ctx.permissions.as_str() {
            "skip" => {
                c.arg("--dangerously-skip-permissions");
            }
            "manual" => {
                c.arg("--permission-prompt-tool")
                    .arg("mcp__rteams__permission_request");
            }
            _ => {
                c.arg("--permission-mode").arg("acceptEdits");
            }
        }
        if read_only_mode(ctx) {
            c.arg("--disallowedTools=Edit,Write,NotebookEdit,Bash");
        }
        if ctx.mode == "delegate" {
            c.arg("--allowedTools=mcp__rteams");
        }
        c
    }
    fn summarize_line(&self, line: &str) -> Vec<String> {
        summarize_line(line)
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

/// stream-json line -> display lines (copy of the launcher's summarizer).
pub fn summarize_line(line: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return if line.trim().is_empty() {
            vec![]
        } else {
            vec![line.to_string()]
        };
    };
    let mut out = Vec::new();
    match v["type"].as_str() {
        Some("system") if v["subtype"].as_str() == Some("init") => {
            out.push("session started".to_string());
        }
        Some("assistant") => {
            for part in v["message"]["content"].as_array().into_iter().flatten() {
                match part["type"].as_str() {
                    Some("text") => {
                        let t = clip(part["text"].as_str().unwrap_or(""), 300);
                        if !t.is_empty() {
                            out.push(t);
                        }
                    }
                    Some("tool_use") => {
                        let name = short_tool(part["name"].as_str().unwrap_or("?"));
                        out.push(format!(
                            "tool: {name} {}",
                            clip(&part["input"].to_string(), 120)
                        ));
                    }
                    _ => {}
                }
            }
        }
        Some("result") => {
            let r = clip(v["result"].as_str().unwrap_or(""), 300);
            let err = v["is_error"].as_bool().unwrap_or(false);
            out.push(format!("{}: {r}", if err { "error" } else { "result" }));
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtimes::testutil::ctx;

    fn args(c: &Command) -> Vec<String> {
        args_of(c)
    }

    #[test]
    fn base_args_and_env() {
        let mut x = ctx("do", "edits");
        x.model = Some("opus".into());
        x.resume = Some("sess-1".into());
        let c = Claude::default().build_command(&x);
        let a = args(&c);
        assert_eq!(a[0], "-p");
        assert_eq!(a[1], "do the thing");
        assert!(a.contains(&"--output-format=stream-json".to_string()));
        assert!(a.contains(&"--verbose".to_string()));
        assert!(a.contains(&"--strict-mcp-config".to_string()));
        let mcp = a.iter().find(|s| s.starts_with("--mcp-config=")).unwrap();
        let v: Value = serde_json::from_str(&mcp["--mcp-config=".len()..]).unwrap();
        assert_eq!(v["mcpServers"]["rteams"]["command"], "/usr/bin/rteams");
        assert_eq!(v["mcpServers"]["rteams"]["args"][0], "serve");
        assert_eq!(v["mcpServers"]["rteams"]["env"]["RTEAMS_MEMBER"], "dev");
        let i = a
            .iter()
            .position(|s| s == "--append-system-prompt")
            .unwrap();
        assert_eq!(a[i + 1], "you are dev");
        let i = a.iter().position(|s| s == "--model").unwrap();
        assert_eq!(a[i + 1], "opus");
        let i = a.iter().position(|s| s == "--resume").unwrap();
        assert_eq!(a[i + 1], "sess-1");
        let i = a.iter().position(|s| s == "--permission-mode").unwrap();
        assert_eq!(a[i + 1], "acceptEdits");
        assert!(!a.iter().any(|s| s.starts_with("--disallowedTools")));
        let envs: Vec<_> = c.get_envs().collect();
        assert!(envs
            .iter()
            .any(|(k, v)| *k == "RTEAMS_MEMBER" && v.is_some()));
    }

    #[test]
    fn permission_and_mode_matrix() {
        let r = Claude::default();
        for mode in ["do", "ask", "delegate"] {
            for perms in ["skip", "edits", "manual"] {
                let a = args(&r.build_command(&ctx(mode, perms)));
                match perms {
                    "skip" => assert!(a.contains(&"--dangerously-skip-permissions".to_string())),
                    "manual" => {
                        let i = a
                            .iter()
                            .position(|s| s == "--permission-prompt-tool")
                            .unwrap();
                        assert_eq!(a[i + 1], "mcp__rteams__permission_request");
                    }
                    _ => assert!(a.contains(&"acceptEdits".to_string())),
                }
                assert_eq!(
                    a.contains(&"--disallowedTools=Edit,Write,NotebookEdit,Bash".to_string()),
                    mode != "do",
                    "{mode}/{perms}"
                );
                assert_eq!(
                    a.contains(&"--allowedTools=mcp__rteams".to_string()),
                    mode == "delegate"
                );
            }
        }
    }

    #[test]
    fn default_model_omitted() {
        let mut x = ctx("do", "skip");
        x.model = Some("default".into());
        assert!(!args(&Claude::default().build_command(&x)).contains(&"--model".to_string()));
    }

    #[test]
    fn summarize_and_session() {
        let r = Claude::default();
        let init = r#"{"type":"system","subtype":"init","session_id":"abc"}"#;
        assert_eq!(r.summarize_line(init), vec!["session started"]);
        assert_eq!(r.session_id(init).as_deref(), Some("abc"));
        let asst = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello"},{"type":"tool_use","name":"mcp__rteams__task_create","input":{"a":1}}]}}"#;
        assert_eq!(
            r.summarize_line(asst),
            vec!["hello", r#"tool: task_create {"a":1}"#]
        );
        let res = r#"{"type":"result","is_error":true,"result":"boom","session_id":"s2"}"#;
        assert_eq!(r.summarize_line(res), vec!["error: boom"]);
        assert_eq!(r.session_id(res).as_deref(), Some("s2"));
        assert_eq!(r.session_id(asst), None);
        assert_eq!(r.summarize_line("raw text"), vec!["raw text"]);
        assert!(r.summarize_line("").is_empty());
    }

    #[test]
    fn auth_parse() {
        assert_eq!(
            parse_auth_status(r#"{"loggedIn":true,"authMethod":"claude.ai"}"#),
            AuthStatus::SignedIn(Some("claude.ai".into()))
        );
        assert_eq!(
            parse_auth_status(r#"{"loggedIn":false}"#),
            AuthStatus::SignedOut
        );
        assert_eq!(parse_auth_status("nope"), AuthStatus::Unknown);
    }

    #[test]
    fn catalog_has_aliases() {
        let ids: Vec<_> = catalog().into_iter().map(|m| m.id).collect();
        for a in ["default", "sonnet", "opus", "haiku"] {
            assert!(ids.contains(&a.to_string()));
        }
    }
}
