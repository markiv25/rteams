//! Gemini CLI (`gemini -p <prompt> --output-format stream-json`).
//!
//! MCP: merged into `<workdir>/.gemini/settings.json` (`mcpServers.rteams`, `trust: true`).
//! Shared with Qwen Code (a Gemini CLI fork) through [`GeminiFamily`].

use super::*;
use serde_json::Value;

/// Parameters that differ between Gemini CLI and its forks.
pub struct GeminiFamily {
    pub id: &'static str,
    pub label: &'static str,
    pub bins: &'static [&'static str],
    pub settings: &'static str,
    pub install: &'static str,
    pub models: &'static [(&'static str, &'static str)],
    pub oauth_file: &'static str,
    pub api_env: &'static [&'static str],
}

pub const GEMINI: GeminiFamily = GeminiFamily {
    id: "gemini",
    label: "Gemini CLI",
    bins: &["gemini"],
    settings: ".gemini/settings.json",
    install: "npm install -g @google/gemini-cli   (or: brew install gemini-cli)",
    models: &[
        ("gemini-2.5-pro", "Gemini 2.5 Pro"),
        ("gemini-2.5-flash", "Gemini 2.5 Flash"),
        ("gemini-2.5-flash-lite", "Gemini 2.5 Flash-Lite"),
    ],
    oauth_file: ".gemini/oauth_creds.json",
    api_env: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
};

impl GeminiFamily {
    pub fn detect(&self, extra: &[PathBuf]) -> Detection {
        detect(self.bins, extra, &["--version"])
    }
    pub fn auth_status(&self) -> AuthStatus {
        for e in self.api_env {
            if std::env::var_os(e).is_some() {
                return AuthStatus::SignedIn(Some((*e).into()));
            }
        }
        match home_dir() {
            Some(h) if h.join(self.oauth_file).exists() => {
                AuthStatus::SignedIn(Some("OAuth".into()))
            }
            _ => AuthStatus::Unknown,
        }
    }
    pub fn models(&self) -> Vec<ModelInfo> {
        self.models
            .iter()
            .map(|(id, label)| ModelInfo {
                id: (*id).into(),
                label: (*label).into(),
            })
            .collect()
    }
    pub fn build_command(&self, ctx: &LaunchCtx) -> Command {
        let _ = write_mcp_servers_file_with(
            &ctx.workdir,
            self.settings,
            mcp_server_entry(ctx, true),
            &ctx.extra_mcp,
        );
        let mut c = base_command(ctx);
        c.arg("-p")
            .arg(prompt_with_system(ctx))
            .arg("--output-format")
            .arg("stream-json");
        if let Some(m) = ctx
            .model
            .as_deref()
            .filter(|m| !m.is_empty() && *m != "default")
        {
            c.arg("--model").arg(m);
        }
        if read_only_mode(ctx) {
            c.arg("--approval-mode").arg("plan");
        } else {
            match ctx.permissions.as_str() {
                "skip" => c.arg("--yolo"),
                "manual" => c.arg("--approval-mode").arg("default"),
                _ => c.arg("--approval-mode").arg("auto_edit"),
            };
        }
        c.arg("--allowed-mcp-server-names").arg("rteams");
        for (name, _) in &ctx.extra_mcp {
            c.arg("--allowed-mcp-server-names").arg(name);
        }
        c
    }
    pub fn summarize_line(&self, line: &str) -> Vec<String> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return generic_summarize(line);
        };
        match v["type"].as_str() {
            Some("init") => vec!["session started".into()],
            Some("message") if v["role"].as_str() == Some("assistant") => {
                let t = clip(v["content"].as_str().unwrap_or(""), 300);
                if t.is_empty() {
                    vec![]
                } else {
                    vec![t]
                }
            }
            Some("tool_use") => vec![format!(
                "tool: {} {}",
                short_tool(v["tool_name"].as_str().unwrap_or("?")),
                clip(&v["parameters"].to_string(), 120)
            )],
            Some("tool_result") if v["status"].as_str() == Some("error") => vec![format!(
                "error: {}",
                clip(
                    v["error"]["message"]
                        .as_str()
                        .or(v["output"].as_str())
                        .unwrap_or("tool failed"),
                    200
                )
            )],
            Some("result") => {
                if v["status"].as_str() == Some("error") {
                    vec![format!(
                        "error: {}",
                        clip(v["error"]["message"].as_str().unwrap_or("failed"), 300)
                    )]
                } else {
                    vec!["result: done".into()]
                }
            }
            Some("error") => vec![format!(
                "error: {}",
                clip(v["message"].as_str().unwrap_or(""), 300)
            )],
            // Claude-style stream-json (Qwen Code).
            _ => super::claude::summarize_line(line),
        }
    }
    pub fn session_id(&self, line: &str) -> Option<String> {
        let v: Value = serde_json::from_str(line).ok()?;
        match v["type"].as_str() {
            Some("init") | Some("system") | Some("result") => session_field(&v),
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct Gemini {
    pub extra_candidates: Vec<PathBuf>,
}

impl AgentRuntime for Gemini {
    fn id(&self) -> &'static str {
        GEMINI.id
    }
    fn label(&self) -> &'static str {
        GEMINI.label
    }
    fn detect(&self) -> Detection {
        GEMINI.detect(&self.extra_candidates)
    }
    fn auth_status(&self) -> AuthStatus {
        GEMINI.auth_status()
    }
    fn models(&self) -> Vec<ModelInfo> {
        GEMINI.models()
    }
    fn install_hint(&self) -> &'static str {
        GEMINI.install
    }
    fn build_command(&self, ctx: &LaunchCtx) -> Command {
        GEMINI.build_command(ctx)
    }
    fn summarize_line(&self, line: &str) -> Vec<String> {
        GEMINI.summarize_line(line)
    }
    fn session_id(&self, line: &str) -> Option<String> {
        GEMINI.session_id(line)
    }
    /// `gemini --resume` takes an index/`latest`, not a stable id we can pass safely.
    fn login_command(&self) -> Option<&'static str> {
        Some("gemini")
    }
    fn supports_resume(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtimes::testutil::{ctx, tmpdir};

    #[test]
    fn matrix_and_file() {
        let r = Gemini::default();
        for mode in ["do", "ask", "delegate"] {
            for perms in ["skip", "edits", "manual"] {
                let d = tmpdir(&format!("gem-{mode}-{perms}"));
                let mut x = ctx(mode, perms);
                x.workdir = d.clone();
                let a = args_of(&r.build_command(&x));
                assert_eq!(a[0], "-p");
                assert_eq!(a[1], "you are dev\n\n---\n\ndo the thing");
                assert_eq!(&a[2..4], ["--output-format", "stream-json"]);
                let want: &[&str] = match (mode, perms) {
                    ("do", "skip") => &["--yolo"],
                    ("do", "edits") => &["--approval-mode", "auto_edit"],
                    ("do", _) => &["--approval-mode", "default"],
                    _ => &["--approval-mode", "plan"],
                };
                let tail = &a[4..4 + want.len()];
                assert_eq!(tail, want, "{mode}/{perms}");
                let f: Value = serde_json::from_str(
                    &std::fs::read_to_string(d.join(".gemini/settings.json")).unwrap(),
                )
                .unwrap();
                assert_eq!(f["mcpServers"]["rteams"]["trust"], true);
                assert_eq!(f["mcpServers"]["rteams"]["args"][0], "serve");
            }
        }
    }

    #[test]
    fn events() {
        let r = Gemini::default();
        let init = r#"{"type":"init","timestamp":"t","session_id":"g1","model":"gemini-2.5-pro"}"#;
        assert_eq!(r.session_id(init).as_deref(), Some("g1"));
        assert_eq!(r.summarize_line(init), vec!["session started"]);
        assert_eq!(
            r.summarize_line(r#"{"type":"message","role":"user","content":"x"}"#)
                .len(),
            0
        );
        assert_eq!(
            r.summarize_line(
                r#"{"type":"message","role":"assistant","content":"hey","delta":true}"#
            ),
            vec!["hey"]
        );
        let t = r#"{"type":"tool_use","tool_name":"mcp_rteams_task_create","tool_id":"1","parameters":{"a":1}}"#;
        assert_eq!(
            r.summarize_line(t),
            vec![r#"tool: mcp_rteams_task_create {"a":1}"#]
        );
        assert_eq!(
            r.summarize_line(r#"{"type":"result","status":"success","stats":{}}"#),
            vec!["result: done"]
        );
        assert_eq!(
            r.summarize_line(r#"{"type":"result","status":"error","error":{"message":"quota"}}"#),
            vec!["error: quota"]
        );
        assert!(!r.supports_resume());
    }
}
