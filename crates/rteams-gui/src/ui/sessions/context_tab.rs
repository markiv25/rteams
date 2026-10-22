//! "Context" tab: CLAUDE.md / rules / memory files that apply to the session's project, the
//! project's `.claude/agents`, workspace-trust status and `@path` mention validation.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use rteams_services::i18n::I18n;
use rteams_services::sessions::chunks::{Block, Role, Session};
use rteams_services::sessions::context_files::{self as cf, AgentConfig, FileInfo};
use rteams_services::trust;

use super::tt;
use crate::theme;

#[derive(Debug, Clone)]
pub struct ContextData {
    pub session_id: String,
    pub project: Option<PathBuf>,
    /// `None` when the project path is unknown.
    pub trusted: Option<bool>,
    pub files: Vec<(&'static str, FileInfo)>,
    pub agents: Vec<AgentConfig>,
    pub mentions: Vec<(String, bool)>,
}

/// Gather everything for `session`. `claude_base` is the Claude root; `claude_json` the file
/// holding workspace-trust flags.
pub fn build(session: &Session, claude_base: &Path, claude_json: Option<&Path>) -> ContextData {
    let project = session.cwd.as_deref().map(PathBuf::from);
    let mut texts = String::new();
    for m in &session.msgs {
        if m.role == Role::User && !m.synthetic {
            for b in &m.blocks {
                if let Block::Text(t) = b {
                    texts.push_str(t);
                    texts.push('\n');
                }
            }
        }
    }
    let mentions_raw = cf::extract_mentions(&texts);
    let (trusted, files, agents, mentions) = match &project {
        Some(p) => {
            let valid = cf::validate_mentions(p, &mentions_raw);
            (
                claude_json.map(|f| trust::is_claude_trusted_at(f, p)),
                cf::claude_md_files(p, claude_base, &cf::enterprise_path()),
                cf::read_agent_configs(p).into_values().collect(),
                mentions_raw
                    .iter()
                    .map(|m| {
                        (
                            m.clone(),
                            valid.get(&format!("@{m}")).copied().unwrap_or(false),
                        )
                    })
                    .collect(),
            )
        }
        None => (None, Vec::new(), Vec::new(), Vec::new()),
    };
    ContextData {
        session_id: session.id.clone(),
        project,
        trusted,
        files,
        agents,
        mentions,
    }
}

fn label(kind: &str) -> &'static str {
    match kind {
        "enterprise" => "Managed policy",
        "user" => "User memory (~/.claude/CLAUDE.md)",
        "project" => "Project CLAUDE.md",
        "project-alt" => "Project .claude/CLAUDE.md",
        "project-rules" => "Project rules (.claude/rules)",
        "project-local" => "Project CLAUDE.local.md",
        "user-rules" => "User rules (~/.claude/rules)",
        "auto-memory" => "Auto memory",
        _ => "Other",
    }
}

pub fn show(ui: &mut egui::Ui, tr: &I18n, d: &ContextData) {
    egui::ScrollArea::vertical()
        .id_salt("sessions_context")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let Some(p) = &d.project else {
                ui.label(
                    RichText::new(tt(tr, "sessions.context.noProject", "Project path unknown"))
                        .color(theme::MUTED),
                );
                return;
            };
            ui.label(RichText::new(p.display().to_string()).monospace());
            let (txt, col) = match d.trusted {
                Some(true) => (
                    tt(tr, "sessions.context.trusted", "Workspace trusted"),
                    theme::SUCCESS,
                ),
                Some(false) => (
                    tt(
                        tr,
                        "sessions.context.untrusted",
                        "Workspace not trusted yet",
                    ),
                    theme::WARNING,
                ),
                None => (
                    tt(tr, "sessions.context.trustUnknown", "Trust status unknown"),
                    theme::MUTED,
                ),
            };
            ui.label(RichText::new(txt).color(col));
            ui.add_space(8.0);
            ui.label(RichText::new(tt(tr, "sessions.context.files", "Memory files")).strong());
            for (kind, f) in &d.files {
                ui.horizontal(|ui| {
                    let c = if f.exists {
                        theme::SUCCESS
                    } else {
                        theme::MUTED
                    };
                    let i = if f.exists {
                        crate::icons::Icon::Check
                    } else {
                        crate::icons::Icon::X
                    };
                    crate::icons::icon(ui, i, 12.0, c);
                    ui.label(label(kind));
                    if f.exists {
                        ui.label(
                            RichText::new(format!("~{} tokens", f.estimated_tokens))
                                .small()
                                .color(theme::MUTED),
                        )
                        .on_hover_text(f.path.display().to_string());
                    }
                });
            }
            ui.add_space(8.0);
            ui.label(RichText::new(tt(tr, "sessions.context.agents", "Project agents")).strong());
            if d.agents.is_empty() {
                ui.label(RichText::new("\u{2014}").color(theme::MUTED));
            }
            for a in &d.agents {
                let tag = a
                    .color
                    .as_deref()
                    .map(|c| format!(" ({c})"))
                    .unwrap_or_default();
                ui.label(format!("{}{tag}", a.name));
            }
            ui.add_space(8.0);
            ui.label(RichText::new(tt(tr, "sessions.context.mentions", "File mentions")).strong());
            if d.mentions.is_empty() {
                ui.label(RichText::new("\u{2014}").color(theme::MUTED));
            }
            for (m, ok) in &d.mentions {
                let c = if *ok { theme::SUCCESS } else { theme::ERROR };
                ui.horizontal(|ui| {
                    let i = if *ok {
                        crate::icons::Icon::Check
                    } else {
                        crate::icons::Icon::X
                    };
                    crate::icons::icon(ui, i, 12.0, c);
                    ui.label(RichText::new(format!("@{m}")).monospace());
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rteams_services::sessions::chunks::Msg;
    use rteams_services::sessions::paths::Source;

    #[test]
    fn builds_from_session_cwd_and_prompts() {
        let d = tempfile::tempdir().unwrap();
        let proj = d.path().join("p");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("CLAUDE.md"), "abcd").unwrap();
        std::fs::write(proj.join("a.rs"), "x").unwrap();
        let cj = d.path().join("claude.json");
        trust::ensure_claude_trusted_at(&cj, &proj).unwrap();
        let m = Msg {
            role: Role::User,
            uuid: "u1".into(),
            parent: None,
            ts_ms: None,
            sidechain: false,
            synthetic: false,
            model: String::new(),
            msg_id: None,
            usage: None,
            blocks: vec![Block::Text("look at @a.rs and @gone.rs".into())],
            cwd: Some(proj.to_string_lossy().into_owned()),
            branch: None,
            session_id: None,
            agent_id: None,
            reported_cost: None,
            duration_ms: None,
        };
        let s = Session::from_msgs(&d.path().join("s.jsonl"), Source::Claude, vec![m]);
        let ctx = build(&s, &d.path().join("base"), Some(&cj));
        assert_eq!(ctx.trusted, Some(true));
        assert_eq!(
            ctx.mentions,
            vec![("a.rs".to_string(), true), ("gone.rs".to_string(), false)]
        );
        assert!(ctx.files.iter().any(|(k, f)| *k == "project" && f.exists));
        let tr = I18n::default();
        crate::view::testing::render(|ui| show(ui, &tr, &ctx));
    }
}
