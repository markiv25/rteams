//! Grok CLI (`grok -p <prompt> --directory <dir>`); MCP via `<workdir>/.grok/settings.json`.
//! Flags follow the grok-cli docs (not verified against an installed binary).

use super::*;

#[derive(Default)]
pub struct Grok {
    pub extra_candidates: Vec<PathBuf>,
}

impl AgentRuntime for Grok {
    fn id(&self) -> &'static str {
        "grok"
    }
    fn label(&self) -> &'static str {
        "Grok CLI"
    }
    fn detect(&self) -> Detection {
        detect(&["grok"], &self.extra_candidates, &["--version"])
    }
    fn auth_status(&self) -> AuthStatus {
        for e in ["GROK_API_KEY", "XAI_API_KEY"] {
            if find_key(&[e]).is_some() {
                return AuthStatus::SignedIn(Some(e.into()));
            }
        }
        match home_dir() {
            Some(h) if h.join(".grok/user-settings.json").exists() => AuthStatus::SignedIn(None),
            _ => AuthStatus::Unknown,
        }
    }
    fn models(&self) -> Vec<ModelInfo> {
        [
            ("grok-code-fast-1", "Grok Code Fast 1"),
            ("grok-4", "Grok 4"),
        ]
        .into_iter()
        .map(|(id, label)| ModelInfo {
            id: id.into(),
            label: label.into(),
        })
        .collect()
    }
    fn install_hint(&self) -> &'static str {
        "npm install -g @vibe-kit/grok-cli"
    }
    fn build_command(&self, ctx: &LaunchCtx) -> Command {
        let _ = write_mcp_servers_file_with(
            &ctx.workdir,
            ".grok/settings.json",
            mcp_server_entry(ctx, false),
            &ctx.extra_mcp,
        );
        let mut c = base_command(ctx);
        c.arg("-p")
            .arg(prompt_with_system(ctx))
            .arg("--directory")
            .arg(&ctx.workdir);
        if let Some(m) = ctx
            .model
            .as_deref()
            .filter(|m| !m.is_empty() && *m != "default")
        {
            c.arg("--model").arg(m);
        }
        for e in ["GROK_API_KEY", "XAI_API_KEY"] {
            if let Some(k) = find_key(&[e]) {
                c.env("GROK_API_KEY", k);
                break;
            }
        }
        c
    }
    fn summarize_line(&self, line: &str) -> Vec<String> {
        generic_summarize(line)
    }
    fn session_id(&self, _line: &str) -> Option<String> {
        None
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
    fn writes_mcp_and_passes_model() {
        let d = tmpdir("grok");
        let mut x = ctx("do", "skip");
        x.workdir = d.clone();
        x.model = Some("grok-4".into());
        x.extra_mcp = vec![("fs".into(), serde_json::json!({"command":"npx"}))];
        let a = args_of(&Grok::default().build_command(&x));
        assert!(a.windows(2).any(|w| w == ["--model", "grok-4"]));
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(d.join(".grok/settings.json")).unwrap())
                .unwrap();
        assert_eq!(v["mcpServers"]["fs"]["command"], "npx");
        assert!(v["mcpServers"]["rteams"].is_object());
    }
}
