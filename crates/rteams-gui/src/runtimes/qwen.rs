//! Qwen Code (`qwen -p ... --output-format stream-json`), a Gemini CLI fork: same flags,
//! settings in `.qwen/settings.json`, Claude-style stream-json events (handled by the shared
//! summarizer's fallback).

use super::gemini::GeminiFamily;
use super::*;

pub const QWEN: GeminiFamily = GeminiFamily {
    id: "qwen",
    label: "Qwen Code",
    bins: &["qwen"],
    settings: ".qwen/settings.json",
    install: "npm install -g @qwen-code/qwen-code",
    models: &[
        ("qwen3-coder-plus", "Qwen3 Coder Plus"),
        ("qwen3-coder-flash", "Qwen3 Coder Flash"),
    ],
    oauth_file: ".qwen/oauth_creds.json",
    api_env: &["OPENAI_API_KEY", "DASHSCOPE_API_KEY"],
};

#[derive(Default)]
pub struct Qwen {
    pub extra_candidates: Vec<PathBuf>,
}

impl AgentRuntime for Qwen {
    fn id(&self) -> &'static str {
        QWEN.id
    }
    fn label(&self) -> &'static str {
        QWEN.label
    }
    fn detect(&self) -> Detection {
        QWEN.detect(&self.extra_candidates)
    }
    fn auth_status(&self) -> AuthStatus {
        QWEN.auth_status()
    }
    fn models(&self) -> Vec<ModelInfo> {
        QWEN.models()
    }
    fn install_hint(&self) -> &'static str {
        QWEN.install
    }
    fn build_command(&self, ctx: &LaunchCtx) -> Command {
        QWEN.build_command(ctx)
    }
    fn summarize_line(&self, line: &str) -> Vec<String> {
        QWEN.summarize_line(line)
    }
    fn session_id(&self, line: &str) -> Option<String> {
        QWEN.session_id(line)
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
    fn builds_and_summarizes() {
        let d = tmpdir("qwen");
        let mut x = ctx("do", "skip");
        x.workdir = d.clone();
        let a = args_of(&Qwen::default().build_command(&x));
        assert!(a.contains(&"--yolo".to_string()));
        assert!(d.join(".qwen/settings.json").exists());
        let r = Qwen::default();
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"yo"}]}}"#;
        assert_eq!(r.summarize_line(line), vec!["yo"]);
        assert_eq!(
            r.session_id(r#"{"type":"system","subtype":"init","session_id":"q1"}"#)
                .as_deref(),
            Some("q1")
        );
    }
}
