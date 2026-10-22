//! Claude-compatible gateways: Z.AI (GLM), MiniMax and OpenRouter (free models) all expose an
//! Anthropic-style endpoint, so they run through the `claude` CLI with `ANTHROPIC_BASE_URL`
//! pointed at the provider and the provider's key as `ANTHROPIC_AUTH_TOKEN`. The key comes from
//! the process env or the rteams key store (`rteams_services::ext::apikeys`).

use super::claude::{self, Claude};
use super::*;

pub struct Compat {
    id: &'static str,
    label: &'static str,
    /// Default Anthropic-compatible base URL.
    base_url: &'static str,
    /// Env var that can override the base URL (custom gateway).
    url_env: &'static str,
    key_envs: &'static [&'static str],
    models: &'static [(&'static str, &'static str)],
    install: &'static str,
    /// Local OpenAI/Anthropic-compatible server (Ollama, LM Studio): models come from a live scan.
    local: bool,
}

impl Compat {
    pub fn zai() -> Self {
        Compat {
            id: "zai",
            label: "Z.AI (GLM)",
            base_url: "https://api.z.ai/api/anthropic",
            url_env: "ZAI_BASE_URL",
            key_envs: &["ZAI_API_KEY", "ZHIPU_API_KEY"],
            models: &[
                ("glm-4.6", "GLM-4.6"),
                ("glm-4.5", "GLM-4.5"),
                ("glm-4.5-air", "GLM-4.5 Air"),
            ],
            local: false,
            install: "Needs the Claude Code CLI plus a Z.AI key (ZAI_API_KEY) in API Keys",
        }
    }
    pub fn minimax() -> Self {
        Compat {
            id: "minimax",
            label: "MiniMax",
            base_url: "https://api.minimax.io/anthropic",
            url_env: "MINIMAX_BASE_URL",
            key_envs: &["MINIMAX_API_KEY"],
            models: &[("MiniMax-M2", "MiniMax M2")],
            local: false,
            install: "Needs the Claude Code CLI plus a MiniMax key (MINIMAX_API_KEY) in API Keys",
        }
    }
    /// OpenRouter / any gateway with an Anthropic-style endpoint: `OPENROUTER_BASE_URL`
    /// overrides the base URL, the member's model is the gateway model id.
    pub fn openrouter() -> Self {
        Compat {
            id: "openrouter",
            label: "OpenRouter (free models)",
            base_url: "https://openrouter.ai/api",
            url_env: "OPENROUTER_BASE_URL",
            key_envs: &["OPENROUTER_API_KEY"],
            models: &[
                ("z-ai/glm-4.5-air:free", "GLM 4.5 Air (free)"),
                ("qwen/qwen3-coder:free", "Qwen3 Coder (free)"),
                ("deepseek/deepseek-chat-v3.1:free", "DeepSeek V3.1 (free)"),
                ("openai/gpt-oss-120b:free", "gpt-oss 120B (free)"),
            ],
            local: false,
            install: "Needs the Claude Code CLI plus OPENROUTER_API_KEY in API Keys",
        }
    }

    /// Ollama (>= 0.14) serves an Anthropic-style `/v1/messages` on its default port.
    pub fn ollama() -> Self {
        Compat {
            id: "ollama",
            label: "Ollama (local)",
            base_url: "http://127.0.0.1:11434",
            url_env: "OLLAMA_BASE_URL",
            key_envs: &[],
            models: &[],
            local: true,
            install: "Needs the Claude Code CLI plus a running Ollama (ollama serve)",
        }
    }
    /// LM Studio's local server (default port 1234) also speaks the Anthropic messages API.
    pub fn lmstudio() -> Self {
        Compat {
            id: "lmstudio",
            label: "LM Studio (local)",
            base_url: "http://127.0.0.1:1234",
            url_env: "LMSTUDIO_BASE_URL",
            key_envs: &[],
            models: &[],
            local: true,
            install: "Needs the Claude Code CLI plus the LM Studio local server running",
        }
    }

    fn base(&self) -> String {
        std::env::var(self.url_env)
            .ok()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| self.base_url.into())
    }
}

impl AgentRuntime for Compat {
    fn id(&self) -> &'static str {
        self.id
    }
    fn label(&self) -> &'static str {
        self.label
    }
    fn detect(&self) -> Detection {
        Claude::default().detect()
    }
    fn auth_status(&self) -> AuthStatus {
        if self.local {
            return if crate::provider_cfg::scan_one(self.id).running {
                AuthStatus::SignedIn(Some("local".into()))
            } else {
                AuthStatus::SignedOut
            };
        }
        match self.key_envs.iter().find(|k| find_key(&[k]).is_some()) {
            Some(k) => AuthStatus::SignedIn(Some((*k).into())),
            None => AuthStatus::SignedOut,
        }
    }
    fn models(&self) -> Vec<ModelInfo> {
        if self.local {
            return crate::provider_cfg::scan_one(self.id)
                .models
                .into_iter()
                .map(|m| ModelInfo {
                    label: m.clone(),
                    id: m,
                })
                .collect();
        }
        self.models
            .iter()
            .map(|(id, label)| ModelInfo {
                id: (*id).into(),
                label: (*label).into(),
            })
            .collect()
    }
    fn api_key_env(&self) -> Option<&'static str> {
        self.key_envs.first().copied()
    }
    fn install_hint(&self) -> &'static str {
        self.install
    }
    fn build_command(&self, ctx: &LaunchCtx) -> Command {
        let mut ctx = ctx.clone();
        let model = ctx
            .model
            .clone()
            .filter(|m| !m.is_empty() && m != "default")
            .or_else(|| self.models.first().map(|m| m.0.to_string()))
            .or_else(|| {
                self.local
                    .then(|| {
                        crate::provider_cfg::scan_one(self.id)
                            .models
                            .into_iter()
                            .next()
                    })
                    .flatten()
            })
            .unwrap_or_else(|| "default".to_string());
        ctx.model = Some(model.clone());
        let mut c = Claude::default().build_command(&ctx);
        c.env("ANTHROPIC_BASE_URL", self.base());
        if self.local {
            // Local servers ignore the token but the CLI wants one.
            c.env("ANTHROPIC_AUTH_TOKEN", self.id);
        } else if let Some(k) = find_key(self.key_envs) {
            c.env("ANTHROPIC_AUTH_TOKEN", k);
        }
        // Never fall through to an Anthropic key and keep every tier on the gateway model.
        c.env("ANTHROPIC_API_KEY", "");
        for tier in ["OPUS", "SONNET", "HAIKU"] {
            c.env(format!("ANTHROPIC_DEFAULT_{tier}_MODEL"), &model);
        }
        c
    }
    fn summarize_line(&self, line: &str) -> Vec<String> {
        claude::summarize_line(line)
    }
    fn session_id(&self, line: &str) -> Option<String> {
        Claude::default().session_id(line)
    }
    fn supports_resume(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtimes::testutil::ctx;

    fn env_of(c: &Command, k: &str) -> Option<String> {
        c.get_envs()
            .find(|(n, _)| *n == k)
            .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
    }

    #[test]
    fn zai_points_claude_at_the_gateway() {
        let mut x = ctx("do", "edits");
        x.model = Some("glm-4.5-air".into());
        let c = Compat::zai().build_command(&x);
        let a = args_of(&c);
        assert!(a.windows(2).any(|w| w == ["--model", "glm-4.5-air"]));
        assert_eq!(
            env_of(&c, "ANTHROPIC_BASE_URL").as_deref(),
            Some("https://api.z.ai/api/anthropic")
        );
        assert_eq!(
            env_of(&c, "ANTHROPIC_DEFAULT_SONNET_MODEL").as_deref(),
            Some("glm-4.5-air")
        );
        assert_eq!(env_of(&c, "ANTHROPIC_API_KEY").as_deref(), Some(""));
    }

    #[test]
    fn default_model_when_unset() {
        let c = Compat::openrouter().build_command(&ctx("do", "edits"));
        assert!(args_of(&c).contains(&"z-ai/glm-4.5-air:free".to_string()));
        assert_eq!(Compat::minimax().id(), "minimax");
    }
}
