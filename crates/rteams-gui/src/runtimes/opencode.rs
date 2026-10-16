//! OpenCode (`opencode run --format json`).
//!
//! The rteams MCP server and the permission policy are injected through the
//! `OPENCODE_CONFIG_CONTENT` env var (inline JSON config), no file is written.
//! Models are `provider/model` ids from `opencode models`.

use super::*;
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Default)]
pub struct OpenCode {
    pub extra_candidates: Vec<PathBuf>,
}

fn fallback_models() -> Vec<ModelInfo> {
    [
        ("opencode/big-pickle", "OpenCode Zen: Big Pickle"),
        (
            "anthropic/claude-sonnet-4-5",
            "Anthropic: Claude Sonnet 4.5",
        ),
        ("openai/gpt-5", "OpenAI: GPT-5"),
        ("google/gemini-2.5-pro", "Google: Gemini 2.5 Pro"),
    ]
    .into_iter()
    .map(|(id, label)| ModelInfo {
        id: id.into(),
        label: label.into(),
    })
    .collect()
}

pub fn parse_models(out: &str) -> Vec<ModelInfo> {
    strip_ansi(out)
        .lines()
        .map(str::trim)
        .filter(|l| l.contains('/') && !l.contains(' '))
        .map(|l| ModelInfo {
            id: l.to_string(),
            label: l.to_string(),
        })
        .collect()
}

pub fn parse_auth_list(out: &str) -> AuthStatus {
    let s = strip_ansi(out);
    for line in s.lines() {
        let line = line
            .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != ' ')
            .trim();
        if let Some(n) = line
            .strip_suffix(" credentials")
            .or_else(|| line.strip_suffix(" credential"))
        {
            if let Ok(n) = n.trim().parse::<u32>() {
                return if n == 0 {
                    AuthStatus::SignedOut
                } else {
                    AuthStatus::SignedIn(Some(format!("{n} provider(s)")))
                };
            }
        }
    }
    AuthStatus::Unknown
}

/// Inline config: MCP server + permission policy.
pub fn config_json(ctx: &LaunchCtx) -> Value {
    let env: serde_json::Map<String, Value> = ctx
        .mcp_env
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    let mut command = vec![ctx.mcp_command.clone()];
    command.extend(ctx.mcp_args.iter().cloned());
    let mut cfg = json!({
        "$schema": "https://opencode.ai/config.json",
        "mcp": { "rteams": { "type": "local", "command": command, "environment": env, "enabled": true } },
    });
    if read_only_mode(ctx) {
        let mut p = json!({ "edit": "deny", "bash": "deny" });
        if ctx.mode == "delegate" {
            p["webfetch"] = json!("deny");
        }
        cfg["permission"] = p;
    } else {
        match ctx.permissions.as_str() {
            "skip" => {}
            "manual" => cfg["permission"] = json!({ "edit": "ask", "bash": "ask" }),
            _ => cfg["permission"] = json!({ "edit": "allow", "bash": "ask" }),
        }
    }
    cfg
}

impl AgentRuntime for OpenCode {
    fn id(&self) -> &'static str {
        "opencode"
    }
    fn label(&self) -> &'static str {
        "OpenCode"
    }
    fn detect(&self) -> Detection {
        detect(&["opencode"], &self.extra_candidates, &["--version"])
    }
    fn auth_status(&self) -> AuthStatus {
        match detected_bin(&["opencode"], &self.extra_candidates) {
            Some(bin) => run_timeout(&bin, &["auth", "list"], Duration::from_secs(5))
                .map(|o| parse_auth_list(&o))
                .unwrap_or(AuthStatus::Unknown),
            None => AuthStatus::Unknown,
        }
    }
    fn models(&self) -> Vec<ModelInfo> {
        let live = detected_bin(&["opencode"], &self.extra_candidates)
            .and_then(|b| run_timeout(&b, &["models"], Duration::from_secs(5)))
            .map(|o| parse_models(&o))
            .unwrap_or_default();
        if live.is_empty() {
            fallback_models()
        } else {
            live
        }
    }
    fn install_hint(&self) -> &'static str {
        "curl -fsSL https://opencode.ai/install | bash   (or: npm install -g opencode-ai)"
    }
    fn build_command(&self, ctx: &LaunchCtx) -> Command {
        let mut c = base_command(ctx);
        c.arg("run")
            .arg("--format")
            .arg("json")
            .arg("--dir")
            .arg(&ctx.workdir);
        if let Some(m) = ctx.model.as_deref().filter(|m| m.contains('/')) {
            c.arg("--model").arg(m);
        }
        if let Some(s) = ctx.resume.as_deref().filter(|s| !s.is_empty()) {
            c.arg("--session").arg(s);
        }
        if ctx.permissions == "skip" && !read_only_mode(ctx) {
            c.arg("--auto");
        }
        c.arg(prompt_with_system(ctx));
        c.env("OPENCODE_CONFIG_CONTENT", config_json(ctx).to_string());
        c
    }
    fn summarize_line(&self, line: &str) -> Vec<String> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return generic_summarize(line);
        };
        let part = &v["part"];
        match v["type"].as_str() {
            Some("text") => {
                let t = clip(part["text"].as_str().unwrap_or(""), 300);
                if t.is_empty() {
                    vec![]
                } else {
                    vec![t]
                }
            }
            Some("tool_use") => {
                let st = &part["state"];
                let name = short_tool(part["tool"].as_str().unwrap_or("?")).replace("rteams_", "");
                match st["status"].as_str() {
                    Some("completed") => vec![format!(
                        "tool: {name} {}",
                        clip(&st["input"].to_string(), 120)
                    )],
                    Some("error") => vec![
                        format!("tool: {name} {}", clip(&st["input"].to_string(), 120)),
                        format!(
                            "error: {}",
                            clip(st["error"].as_str().unwrap_or("tool failed"), 200)
                        ),
                    ],
                    _ => vec![],
                }
            }
            Some("step_finish") if part["reason"].as_str() == Some("stop") => {
                vec!["result: finished".into()]
            }
            Some("error") => {
                let e = &v["error"];
                let m = e["data"]["message"]
                    .as_str()
                    .or_else(|| e["message"].as_str())
                    .or_else(|| e["name"].as_str());
                vec![format!(
                    "error: {}",
                    clip(m.unwrap_or("unknown error"), 300)
                )]
            }
            _ => vec![],
        }
    }
    fn session_id(&self, line: &str) -> Option<String> {
        session_field(&serde_json::from_str::<Value>(line).ok()?)
    }
    fn supports_resume(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtimes::testutil::ctx;

    fn cfg_of(c: &Command) -> Value {
        let (_, v) = c
            .get_envs()
            .find(|(k, _)| *k == "OPENCODE_CONFIG_CONTENT")
            .unwrap();
        serde_json::from_str(&v.unwrap().to_string_lossy()).unwrap()
    }

    #[test]
    fn matrix() {
        let r = OpenCode::default();
        for mode in ["do", "ask", "delegate"] {
            for perms in ["skip", "edits", "manual"] {
                let c = r.build_command(&ctx(mode, perms));
                let a = args_of(&c);
                assert_eq!(&a[..3], ["run", "--format", "json"]);
                assert_eq!(a.last().unwrap(), "you are dev\n\n---\n\ndo the thing");
                assert_eq!(
                    a.contains(&"--auto".to_string()),
                    perms == "skip" && mode == "do"
                );
                let cfg = cfg_of(&c);
                assert_eq!(cfg["mcp"]["rteams"]["command"][0], "/usr/bin/rteams");
                assert_eq!(cfg["mcp"]["rteams"]["command"][1], "serve");
                assert_eq!(cfg["mcp"]["rteams"]["environment"]["RTEAMS_MEMBER"], "dev");
                match (mode, perms) {
                    ("do", "skip") => assert!(cfg.get("permission").is_none()),
                    ("do", "edits") => assert_eq!(cfg["permission"]["edit"], "allow"),
                    ("do", _) => assert_eq!(cfg["permission"]["edit"], "ask"),
                    (m, _) => {
                        assert_eq!(cfg["permission"]["edit"], "deny");
                        assert_eq!(cfg["permission"]["bash"], "deny");
                        assert_eq!(cfg["permission"].get("webfetch").is_some(), m == "delegate");
                    }
                }
            }
        }
    }

    #[test]
    fn model_resume() {
        let mut x = ctx("do", "edits");
        x.model = Some("anthropic/claude-sonnet-4-5".into());
        x.resume = Some("ses_1".into());
        let a = args_of(&OpenCode::default().build_command(&x));
        let i = a.iter().position(|s| s == "--model").unwrap();
        assert_eq!(a[i + 1], "anthropic/claude-sonnet-4-5");
        let i = a.iter().position(|s| s == "--session").unwrap();
        assert_eq!(a[i + 1], "ses_1");
        x.model = Some("opus".into()); // not provider/model: ignored
        assert!(!args_of(&OpenCode::default().build_command(&x)).contains(&"--model".to_string()));
    }

    #[test]
    fn events() {
        let r = OpenCode::default();
        let t = r#"{"type":"text","timestamp":1,"sessionID":"ses_9","part":{"type":"text","text":"hi there"}}"#;
        assert_eq!(r.summarize_line(t), vec!["hi there"]);
        assert_eq!(r.session_id(t).as_deref(), Some("ses_9"));
        let tool = r#"{"type":"tool_use","sessionID":"ses_9","part":{"type":"tool","tool":"rteams_task_create","state":{"status":"completed","input":{"a":1},"output":"ok"}}}"#;
        assert_eq!(r.summarize_line(tool), vec![r#"tool: task_create {"a":1}"#]);
        let running =
            r#"{"type":"tool_use","part":{"tool":"bash","state":{"status":"running","input":{}}}}"#;
        assert!(r.summarize_line(running).is_empty());
        let fin = r#"{"type":"step_finish","part":{"reason":"stop"}}"#;
        assert_eq!(r.summarize_line(fin), vec!["result: finished"]);
        let err = r#"{"type":"error","error":{"name":"APIError","data":{"message":"bad key"}}}"#;
        assert_eq!(r.summarize_line(err), vec!["error: bad key"]);
        assert_eq!(r.summarize_line("plain"), vec!["plain"]);
    }

    #[test]
    fn parsers() {
        let ms = parse_models("opencode/big-pickle\nanthropic/claude-x\nnoise line\n");
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[0].id, "opencode/big-pickle");
        assert_eq!(
            parse_auth_list("\u{1b}[0m\n┌  Credentials ~/x\n│\n└  0 credentials\n"),
            AuthStatus::SignedOut
        );
        assert!(matches!(
            parse_auth_list("└  2 credentials"),
            AuthStatus::SignedIn(_)
        ));
        assert_eq!(parse_auth_list("junk"), AuthStatus::Unknown);
    }
}
