//! Kiro CLI (`kiro-cli chat --no-interactive --trust-all-tools <prompt>`); MCP via
//! `<workdir>/.kiro/settings/mcp.json`. Flags follow the Kiro docs (not verified locally).

use super::*;

#[derive(Default)]
pub struct Kiro {
    pub extra_candidates: Vec<PathBuf>,
}

impl AgentRuntime for Kiro {
    fn id(&self) -> &'static str {
        "kiro"
    }
    fn label(&self) -> &'static str {
        "Kiro"
    }
    fn detect(&self) -> Detection {
        detect(
            &["kiro-cli", "kiro"],
            &self.extra_candidates,
            &["--version"],
        )
    }
    fn auth_status(&self) -> AuthStatus {
        if find_key(&["KIRO_API_KEY"]).is_some() {
            return AuthStatus::SignedIn(Some("KIRO_API_KEY".into()));
        }
        AuthStatus::Unknown
    }
    fn models(&self) -> Vec<ModelInfo> {
        [("claude-sonnet-4.5", "Claude Sonnet 4.5"), ("auto", "Auto")]
            .into_iter()
            .map(|(id, label)| ModelInfo {
                id: id.into(),
                label: label.into(),
            })
            .collect()
    }
    fn install_hint(&self) -> &'static str {
        "curl -fsSL https://cli.kiro.dev/install | bash"
    }
    fn login_command(&self) -> Option<&'static str> {
        Some("kiro-cli login")
    }
    fn build_command(&self, ctx: &LaunchCtx) -> Command {
        let _ = write_mcp_servers_file_with(
            &ctx.workdir,
            ".kiro/settings/mcp.json",
            mcp_server_entry(ctx, false),
            &ctx.extra_mcp,
        );
        let mut c = base_command(ctx);
        c.arg("chat").arg("--no-interactive");
        if let Some(m) = ctx
            .model
            .as_deref()
            .filter(|m| !m.is_empty() && *m != "default")
        {
            c.arg("--model").arg(m);
        }
        if read_only_mode(ctx) {
            c.arg("--trust-tools=@rteams");
        } else if ctx.permissions == "skip" {
            c.arg("--trust-all-tools");
        } else {
            c.arg("--trust-tools=@rteams,fs_read,fs_write");
        }
        c.arg(prompt_with_system(ctx));
        c
    }
    fn summarize_line(&self, line: &str) -> Vec<String> {
        generic_summarize(&strip_ansi(line))
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
    fn trust_flags_follow_permissions() {
        let d = tmpdir("kiro");
        let mut x = ctx("do", "skip");
        x.workdir = d.clone();
        let a = args_of(&Kiro::default().build_command(&x));
        assert_eq!(&a[..2], ["chat", "--no-interactive"]);
        assert!(a.contains(&"--trust-all-tools".to_string()));
        assert!(d.join(".kiro/settings/mcp.json").exists());
        x.mode = "ask".into();
        let a = args_of(&Kiro::default().build_command(&x));
        assert!(a.contains(&"--trust-tools=@rteams".to_string()));
    }
}
