//! OpenAI Codex CLI (`codex exec --json`).
//!
//! `codex exec [resume <id>] --json --skip-git-repo-check -C <dir> [-m model] <sandbox flags>
//! -c mcp_servers.rteams.command=".." -c mcp_servers.rteams.args=[..] -c mcp_servers.rteams.env={..} <prompt>`.
//! Codex has no system-prompt flag, so the system prompt is prepended to the task.

use super::*;
use serde_json::Value;
use std::time::Duration;

#[derive(Default)]
pub struct Codex {
    pub extra_candidates: Vec<PathBuf>,
}

/// The original's static fallback catalog (codexModelCatalogFallback.ts).
pub fn catalog() -> Vec<ModelInfo> {
    [
        ("gpt-5.6-sol", "GPT-5.6 Sol"),
        ("gpt-5.6-terra", "GPT-5.6 Terra"),
        ("gpt-5.6-luna", "GPT-5.6 Luna"),
        ("gpt-5.5", "GPT-5.5"),
        ("gpt-5.3-codex-spark", "GPT-5.3 Codex Spark"),
    ]
    .into_iter()
    .map(|(id, label)| ModelInfo {
        id: id.into(),
        label: label.into(),
    })
    .collect()
}

pub fn parse_login_status(out: &str) -> AuthStatus {
    let l = out.to_lowercase();
    if l.contains("not logged in") {
        AuthStatus::SignedOut
    } else if l.contains("logged in") {
        let how = out
            .trim()
            .rsplit("using ")
            .next()
            .filter(|_| l.contains("using "));
        AuthStatus::SignedIn(how.map(|s| s.trim().to_string()))
    } else {
        AuthStatus::Unknown
    }
}

fn toml_array(items: &[String]) -> String {
    format!(
        "[{}]",
        items
            .iter()
            .map(|s| toml_str(s))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn toml_table(items: &[(String, String)]) -> String {
    format!(
        "{{{}}}",
        items
            .iter()
            .map(|(k, v)| format!("{}={}", toml_str(k), toml_str(v)))
            .collect::<Vec<_>>()
            .join(",")
    )
}

impl AgentRuntime for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }
    fn label(&self) -> &'static str {
        "Codex"
    }
    fn detect(&self) -> Detection {
        let mut extra = self.extra_candidates.clone();
        extra.push(PathBuf::from("/opt/homebrew/bin"));
        detect(&["codex"], &extra, &["--version"])
    }
    fn auth_status(&self) -> AuthStatus {
        if let Some(bin) = detected_bin(&["codex"], &self.extra_candidates) {
            // `codex login status` prints to stdout (or stderr on some versions).
            if let Some(out) = run_timeout(&bin, &["login", "status"], Duration::from_secs(5)) {
                let st = parse_login_status(&out);
                if st != AuthStatus::Unknown {
                    return st;
                }
            }
        }
        if std::env::var_os("OPENAI_API_KEY").is_some()
            || std::env::var_os("CODEX_API_KEY").is_some()
        {
            return AuthStatus::SignedIn(Some("API key".into()));
        }
        AuthStatus::Unknown
    }
    fn models(&self) -> Vec<ModelInfo> {
        catalog()
    }
    fn install_hint(&self) -> &'static str {
        "npm install -g @openai/codex   (or: brew install codex)"
    }
    fn build_command(&self, ctx: &LaunchCtx) -> Command {
        let mut c = base_command(ctx);
        c.arg("exec");
        if let Some(r) = ctx.resume.as_deref().filter(|r| !r.is_empty()) {
            c.arg("resume").arg(r);
        }
        c.arg("--json").arg("--skip-git-repo-check");
        if ctx.resume.is_none() {
            // `exec resume` has no -C/-s flags; it inherits the process cwd (set above).
            c.arg("-C").arg(&ctx.workdir);
        }
        if let Some(m) = ctx
            .model
            .as_deref()
            .filter(|m| !m.is_empty() && *m != "default")
        {
            c.arg("-m").arg(m);
        }
        if ctx.permissions == "skip" && !read_only_mode(ctx) {
            c.arg("--dangerously-bypass-approvals-and-sandbox");
        } else {
            let sandbox = if read_only_mode(ctx) {
                "read-only"
            } else {
                "workspace-write"
            };
            if ctx.resume.is_none() {
                c.arg("-s").arg(sandbox);
            } else {
                c.arg("-c")
                    .arg(format!("sandbox_mode={}", toml_str(sandbox)));
            }
            // Non-interactive: never block on a prompt Codex cannot show.
            let policy = if ctx.permissions == "manual" {
                "on-request"
            } else {
                "never"
            };
            c.arg("-c")
                .arg(format!("approval_policy={}", toml_str(policy)));
        }
        c.arg("-c").arg(format!(
            "mcp_servers.rteams.command={}",
            toml_str(&ctx.mcp_command)
        ));
        c.arg("-c").arg(format!(
            "mcp_servers.rteams.args={}",
            toml_array(&ctx.mcp_args)
        ));
        if !ctx.mcp_env.is_empty() {
            c.arg("-c").arg(format!(
                "mcp_servers.rteams.env={}",
                toml_table(&ctx.mcp_env)
            ));
        }
        for (name, e) in ctx.extra_mcp.iter().filter(|(n, _)| n != "rteams") {
            if !name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
            {
                continue;
            }
            if let Some(url) = e["url"].as_str() {
                c.arg("-c")
                    .arg(format!("mcp_servers.{name}.url={}", toml_str(url)));
                continue;
            }
            let Some(cmd) = e["command"].as_str() else {
                continue;
            };
            c.arg("-c")
                .arg(format!("mcp_servers.{name}.command={}", toml_str(cmd)));
            let args: Vec<String> = e["args"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            c.arg("-c")
                .arg(format!("mcp_servers.{name}.args={}", toml_array(&args)));
            let env: Vec<(String, String)> = e["env"]
                .as_object()
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            if !env.is_empty() {
                c.arg("-c")
                    .arg(format!("mcp_servers.{name}.env={}", toml_table(&env)));
            }
        }
        if let Some(e) = ctx
            .effort
            .as_deref()
            .filter(|e| matches!(*e, "low" | "medium" | "high" | "xhigh"))
        {
            c.arg("-c")
                .arg(format!("model_reasoning_effort={}", toml_str(e)));
        }
        if ctx.fast {
            c.arg("-c")
                .arg(format!("service_tier={}", toml_str("fast")));
        }
        if let Some(n) = ctx.context_limit {
            c.arg("-c").arg(format!("model_context_window={n}"));
        }
        c.arg(prompt_with_system(ctx));
        c
    }
    fn summarize_line(&self, line: &str) -> Vec<String> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return generic_summarize(line);
        };
        // Some versions wrap events as {"msg": {...}}.
        let v = if v.get("msg").is_some() && v.get("type").is_none() {
            v["msg"].clone()
        } else {
            v
        };
        let ty = v["type"].as_str().unwrap_or("");
        match ty {
            "thread.started" => vec!["session started".into()],
            "item.started" | "item.completed" => {
                let item = &v["item"];
                let done = ty == "item.completed";
                match item["type"].as_str() {
                    Some("agent_message") if done => {
                        let t = clip(item["text"].as_str().unwrap_or(""), 300);
                        if t.is_empty() {
                            vec![]
                        } else {
                            vec![t]
                        }
                    }
                    Some("command_execution") if !done => {
                        vec![format!(
                            "tool: shell {}",
                            clip(item["command"].as_str().unwrap_or(""), 120)
                        )]
                    }
                    Some("command_execution") => match item["exit_code"].as_i64() {
                        Some(c) if c != 0 => vec![format!("error: command exited {c}")],
                        _ => vec![],
                    },
                    Some("mcp_tool_call") if !done => vec![format!(
                        "tool: {} {}",
                        short_tool(item["tool"].as_str().unwrap_or("?")),
                        clip(&item["arguments"].to_string(), 120)
                    )],
                    Some("mcp_tool_call") if item["status"].as_str() == Some("failed") => {
                        vec![format!(
                            "error: {}",
                            clip(
                                item["error"]["message"]
                                    .as_str()
                                    .unwrap_or("tool call failed"),
                                200
                            )
                        )]
                    }
                    Some("file_change") if done => {
                        let files: Vec<&str> = item["changes"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|c| c["path"].as_str())
                            .collect();
                        vec![format!("tool: edit {}", clip(&files.join(", "), 120))]
                    }
                    Some("error") => vec![format!(
                        "error: {}",
                        clip(item["message"].as_str().unwrap_or(""), 200)
                    )],
                    _ => vec![],
                }
            }
            "turn.completed" => vec!["result: turn completed".into()],
            "turn.failed" => vec![format!(
                "error: {}",
                clip(v["error"]["message"].as_str().unwrap_or("turn failed"), 300)
            )],
            "error" => vec![format!(
                "error: {}",
                clip(v["message"].as_str().unwrap_or(""), 300)
            )],
            _ => vec![],
        }
    }
    fn session_id(&self, line: &str) -> Option<String> {
        let v: Value = serde_json::from_str(line).ok()?;
        (v["type"].as_str() == Some("thread.started"))
            .then(|| v["thread_id"].as_str().map(str::to_string))
            .flatten()
    }
    fn login_command(&self) -> Option<&'static str> {
        Some("codex login")
    }
    fn supports_resume(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtimes::testutil::ctx;

    #[test]
    fn matrix() {
        let r = Codex::default();
        for mode in ["do", "ask", "delegate"] {
            for perms in ["skip", "edits", "manual"] {
                let a = args_of(&r.build_command(&ctx(mode, perms)));
                assert_eq!(&a[..3], ["exec", "--json", "--skip-git-repo-check"]);
                assert_eq!(a.last().unwrap(), "you are dev\n\n---\n\ndo the thing");
                let bypass = a.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string());
                assert_eq!(bypass, perms == "skip" && mode == "do", "{mode}/{perms}");
                if !bypass {
                    let i = a.iter().position(|s| s == "-s").unwrap();
                    assert_eq!(
                        a[i + 1],
                        if mode == "do" {
                            "workspace-write"
                        } else {
                            "read-only"
                        }
                    );
                    let want = if perms == "manual" {
                        "approval_policy=\"on-request\""
                    } else {
                        "approval_policy=\"never\""
                    };
                    assert!(a.contains(&want.to_string()), "{a:?}");
                }
                assert!(a.contains(&"mcp_servers.rteams.command=\"/usr/bin/rteams\"".to_string()));
                assert!(a.contains(
                    &"mcp_servers.rteams.args=[\"serve\",\"--home\",\"/h\",\"--team\",\"t\"]"
                        .to_string()
                ));
                assert!(
                    a.contains(&"mcp_servers.rteams.env={\"RTEAMS_MEMBER\"=\"dev\"}".to_string())
                );
            }
        }
    }

    #[test]
    fn model_and_resume() {
        let mut x = ctx("do", "edits");
        x.model = Some("gpt-5.5".into());
        x.resume = Some("tid-1".into());
        let a = args_of(&Codex::default().build_command(&x));
        assert_eq!(&a[..3], ["exec", "resume", "tid-1"]);
        assert!(!a.contains(&"-C".to_string()));
        assert!(a.contains(&"sandbox_mode=\"workspace-write\"".to_string()));
        let i = a.iter().position(|s| s == "-m").unwrap();
        assert_eq!(a[i + 1], "gpt-5.5");
    }

    #[test]
    fn events() {
        let r = Codex::default();
        let t = r#"{"type":"thread.started","thread_id":"019-abc"}"#;
        assert_eq!(r.session_id(t).as_deref(), Some("019-abc"));
        assert_eq!(r.summarize_line(t), vec!["session started"]);
        let m = r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"done it"}}"#;
        assert_eq!(r.summarize_line(m), vec!["done it"]);
        let cs = r#"{"type":"item.started","item":{"id":"item_2","type":"command_execution","command":"ls -la","status":"in_progress"}}"#;
        assert_eq!(r.summarize_line(cs), vec!["tool: shell ls -la"]);
        let ce = r#"{"type":"item.completed","item":{"type":"command_execution","command":"x","exit_code":2,"status":"failed"}}"#;
        assert_eq!(r.summarize_line(ce), vec!["error: command exited 2"]);
        let mc = r#"{"type":"item.started","item":{"type":"mcp_tool_call","server":"rteams","tool":"task_create","arguments":{"t":1}}}"#;
        assert_eq!(r.summarize_line(mc), vec![r#"tool: task_create {"t":1}"#]);
        assert_eq!(
            r.summarize_line(r#"{"type":"turn.completed","usage":{}}"#),
            vec!["result: turn completed"]
        );
        assert_eq!(
            r.summarize_line(r#"{"type":"turn.failed","error":{"message":"nope"}}"#),
            vec!["error: nope"]
        );
        assert!(r
            .summarize_line(r#"{"type":"item.completed","item":{"type":"reasoning","text":"hm"}}"#)
            .is_empty());
        assert_eq!(r.summarize_line("warning text"), vec!["warning text"]);
        assert_eq!(r.session_id(m), None);
    }

    #[test]
    fn login_parse() {
        assert_eq!(
            parse_login_status("Logged in using ChatGPT\n"),
            AuthStatus::SignedIn(Some("ChatGPT".into()))
        );
        assert_eq!(parse_login_status("Not logged in"), AuthStatus::SignedOut);
        assert_eq!(parse_login_status("???"), AuthStatus::Unknown);
    }
}
