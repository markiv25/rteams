//! GitHub Copilot CLI (`copilot -p <prompt> --allow-all-tools --additional-mcp-config <json>`).
//! Flags follow the Copilot CLI docs (not verified against an installed binary).

use super::*;

#[derive(Default)]
pub struct Copilot {
    pub extra_candidates: Vec<PathBuf>,
}

impl AgentRuntime for Copilot {
    fn id(&self) -> &'static str {
        "copilot"
    }
    fn label(&self) -> &'static str {
        "GitHub Copilot CLI"
    }
    fn detect(&self) -> Detection {
        detect(&["copilot"], &self.extra_candidates, &["--version"])
    }
    fn auth_status(&self) -> AuthStatus {
        for e in ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] {
            if find_key(&[e]).is_some() {
                return AuthStatus::SignedIn(Some(e.into()));
            }
        }
        match home_dir() {
            Some(h) if h.join(".copilot/config.json").exists() => AuthStatus::SignedIn(None),
            _ => AuthStatus::Unknown,
        }
    }
    fn models(&self) -> Vec<ModelInfo> {
        [
            ("claude-sonnet-4.5", "Claude Sonnet 4.5"),
            ("claude-sonnet-4", "Claude Sonnet 4"),
            ("gpt-5", "GPT-5"),
        ]
        .into_iter()
        .map(|(id, label)| ModelInfo {
            id: id.into(),
            label: label.into(),
        })
        .collect()
    }
    fn install_hint(&self) -> &'static str {
        "npm install -g @github/copilot"
    }
    fn login_command(&self) -> Option<&'static str> {
        Some("copilot login")
    }
    fn build_command(&self, ctx: &LaunchCtx) -> Command {
        let mut c = base_command(ctx);
        c.arg("-p")
            .arg(prompt_with_system(ctx))
            .arg("--additional-mcp-config")
            .arg(mcp_doc(ctx, false).to_string());
        if let Some(m) = ctx
            .model
            .as_deref()
            .filter(|m| !m.is_empty() && *m != "default")
        {
            c.arg("--model").arg(m);
        }
        if read_only_mode(ctx) {
            c.arg("--allow-tool").arg("rteams");
            c.arg("--deny-tool").arg("write");
            c.arg("--deny-tool").arg("shell");
        } else if ctx.permissions == "skip" {
            c.arg("--allow-all-tools");
        } else {
            c.arg("--allow-tool").arg("rteams");
            c.arg("--allow-tool").arg("write");
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
    use crate::runtimes::testutil::ctx;

    #[test]
    fn args_by_permission() {
        let a = args_of(&Copilot::default().build_command(&ctx("do", "skip")));
        assert!(a.contains(&"--allow-all-tools".to_string()));
        let a = args_of(&Copilot::default().build_command(&ctx("ask", "skip")));
        assert!(!a.contains(&"--allow-all-tools".to_string()));
        assert!(a.windows(2).any(|w| w == ["--deny-tool", "shell"]));
        let a = args_of(&Copilot::default().build_command(&ctx("do", "edits")));
        assert!(a.windows(2).any(|w| w == ["--allow-tool", "write"]));
        assert!(a.iter().any(|x| x.contains("\"rteams\"")));
    }
}
