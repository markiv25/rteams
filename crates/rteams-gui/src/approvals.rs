//! Tool approvals (the original's ToolApprovalSheet): a modal listing the permission requests
//! agents are waiting on, each with Allow / Deny. Auto-opens when a new request arrives.

use std::collections::{HashMap, HashSet};

use eframe::egui::{self, Color32, RichText};

use crate::actions::GuiCmd;
use crate::theme;
use crate::view::{Intent, Intents, View};

/// Per-team auto-approval rules (the original's tool-approval settings), kept next to the log in
/// `approval_settings.json`. They apply while this window is open on the team.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AutoAllow {
    /// Allow every request.
    pub all: bool,
    /// Allow Edit / Write / MultiEdit / NotebookEdit.
    pub file_edits: bool,
    /// Allow read-only shell commands (`ls`, `git status`, ...), never a compound one.
    pub safe_bash: bool,
}

const SETTINGS_FILE: &str = "approval_settings.json";
const SAFE_COMMANDS: &[&str] = &[
    "ls", "pwd", "cat", "head", "tail", "wc", "grep", "rg", "echo", "date", "whoami", "which",
    "stat", "file", "tree", "du", "df",
];
const SAFE_GIT: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "branch",
    "rev-parse",
    "ls-files",
];

impl AutoAllow {
    pub fn load(dir: &std::path::Path) -> Self {
        std::fs::read_to_string(dir.join(SETTINGS_FILE))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &std::path::Path) {
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(dir.join(SETTINGS_FILE), t);
        }
    }

    /// Should this request be allowed without asking?
    pub fn allows(&self, tool: &str, input: &serde_json::Value) -> bool {
        if self.all {
            return true;
        }
        if self.file_edits && matches!(tool, "Edit" | "Write" | "MultiEdit" | "NotebookEdit") {
            return true;
        }
        self.safe_bash && tool == "Bash" && input["command"].as_str().is_some_and(is_safe_bash)
    }
}

/// A single read-only command: no chaining, pipes, redirects or substitution.
pub fn is_safe_bash(cmd: &str) -> bool {
    let cmd = cmd.trim();
    if cmd.is_empty() || cmd.contains(['\n', ';', '&', '|', '>', '<', '$', '`', '(', ')']) {
        return false;
    }
    let mut words = cmd.split_whitespace();
    match words.next() {
        Some("git") => words.next().is_some_and(|w| SAFE_GIT.contains(&w)),
        Some(w) => SAFE_COMMANDS.contains(&w),
        None => false,
    }
}

#[derive(Default)]
pub struct Approvals {
    auto: AutoAllow,
    auto_dir: Option<std::path::PathBuf>,
    is_open: bool,
    /// Every request id already seen, so only new ones auto-open the sheet.
    seen: HashSet<String>,
    /// Ids answered this session (the log may not have refreshed yet).
    answered: HashSet<String>,
    notes: HashMap<String, String>,
}

/// Ids of the undecided requests, oldest first.
pub fn pending_ids<'a>(v: &View<'a>) -> Vec<&'a str> {
    v.state
        .permissions
        .iter()
        .filter(|p| p.decided.is_none())
        .map(|p| p.id.as_str())
        .collect()
}

/// Pretty-printed tool input for display.
pub fn pretty_input(input: &serde_json::Value) -> String {
    serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string())
}

impl Approvals {
    /// Undecided requests in this view.
    pub fn pending_count(v: &View) -> usize {
        pending_ids(v).len()
    }

    pub fn open(&mut self) {
        self.is_open = true;
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.is_open
    }

    /// Open when a pending request appears that has not been seen before.
    fn auto_open(&mut self, v: &View) {
        let mut fresh = false;
        for id in pending_ids(v) {
            fresh |= self.seen.insert(id.to_string());
        }
        if fresh {
            self.is_open = true;
        }
    }

    /// Answer the requests the team's auto-allow rules cover, once each.
    fn auto_decide(&mut self, v: &View, out: &mut Intents) {
        if self.auto_dir.as_deref() != Some(v.data.dir()) {
            self.auto = AutoAllow::load(v.data.dir());
            self.auto_dir = Some(v.data.dir().to_path_buf());
        }
        if v.actor.is_none() {
            return;
        }
        for p in v.state.permissions.iter().filter(|p| p.decided.is_none()) {
            if !self.answered.contains(&p.id) && self.auto.allows(&p.tool, &p.input) {
                out.push(Intent::Run(GuiCmd::DecidePermission {
                    id: p.id.clone(),
                    allow: true,
                    note: Some("auto-allowed".to_string()),
                }));
                self.answered.insert(p.id.clone());
                self.seen.insert(p.id.clone());
            }
        }
    }

    /// The three auto-allow switches, saved on change.
    fn settings(&mut self, ui: &mut egui::Ui, v: &View) {
        let tx = |k: &str, d: &str| {
            if v.tr.has(k) {
                v.tr.t(k)
            } else {
                d.to_string()
            }
        };
        let before = self.auto;
        egui::CollapsingHeader::new(
            RichText::new(tx("rteams.approvals.auto", "Auto-allow"))
                .size(12.0)
                .color(theme::TEXT_2),
        )
        .id_salt("approval_auto")
        .show(ui, |ui| {
            ui.checkbox(
                &mut self.auto.all,
                tx("rteams.approvals.autoAll", "Allow everything"),
            );
            ui.checkbox(
                &mut self.auto.file_edits,
                tx("rteams.approvals.autoEdits", "Allow file edits"),
            );
            ui.checkbox(
                &mut self.auto.safe_bash,
                tx(
                    "rteams.approvals.autoBash",
                    "Allow read-only shell commands",
                ),
            );
        });
        if self.auto != before {
            self.auto.save(v.data.dir());
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, v: &View, out: &mut Intents) {
        self.auto_decide(v, out);
        self.auto_open(v);
        if !self.is_open {
            return;
        }
        let pending: Vec<_> = v
            .state
            .permissions
            .iter()
            .filter(|p| p.decided.is_none() && !self.answered.contains(&p.id))
            .collect();
        let screen = ctx.content_rect();
        let width = (screen.width() * 0.92).clamp(1.0, 560.0);
        let max_h = (screen.height() * 0.9).max(1.0);
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("tool_approvals"))
            .backdrop_color(Color32::from_black_alpha(150))
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::hairline(0.12)))
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::same(20)),
            )
            .show(ctx, |ui| {
                ui.set_width(width);
                egui::ScrollArea::vertical()
                    .max_height(max_h)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(v.tr.t("rteams.team.toolApprovals"))
                                    .size(18.0)
                                    .strong(),
                            );
                            theme::count_badge(ui, pending.len(), !pending.is_empty());
                        });
                        ui.add_space(8.0);
                        self.settings(ui, v);
                        if pending.is_empty() {
                            ui.label(
                                RichText::new(v.tr.t("rteams.approvals.none")).color(theme::MUTED),
                            );
                        }
                        for p in &pending {
                            self.request(ui, v, p, out);
                            theme::rule(ui);
                        }
                        ui.add_space(6.0);
                        if ui.button(v.tr.t("common.actions.close")).clicked() {
                            close = true;
                        }
                    });
            });
        if close || modal.should_close() {
            self.is_open = false;
        }
    }

    fn request(
        &mut self,
        ui: &mut egui::Ui,
        v: &View,
        p: &rteams_core::PermissionRequest,
        out: &mut Intents,
    ) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&p.member).strong().color(v.color(&p.member)));
            ui.label(RichText::new(v.tr.t("rteams.approvals.wantsToUse")).color(theme::MUTED));
            ui.label(RichText::new(&p.tool).strong().color(theme::TOOL_FG));
        });
        egui::CollapsingHeader::new(
            RichText::new(v.tr.t("common.toolViewer.input"))
                .size(12.0)
                .color(theme::TEXT_2),
        )
        .id_salt(("approval_input", &p.id))
        .show(ui, |ui| {
            let text = pretty_input(&p.input);
            egui::ScrollArea::both()
                .id_salt(("approval_scroll", &p.id))
                .max_height(160.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.label(RichText::new(text).monospace().size(11.5));
                });
        });
        let note = self.notes.entry(p.id.clone()).or_default();
        ui.add(
            egui::TextEdit::singleline(note)
                .hint_text(v.tr.t("rteams.approvals.note"))
                .desired_width(f32::INFINITY),
        );
        let can = v.actor.is_some();
        ui.horizontal(|ui| {
            let allow = ui
                .add_enabled_ui(can, |ui| {
                    theme::solid(ui, &v.tr.t("team.toolApproval.allow"), theme::SUCCESS)
                })
                .inner;
            let deny = ui
                .add_enabled_ui(can, |ui| {
                    theme::solid(ui, &v.tr.t("team.toolApproval.deny"), theme::ERROR)
                })
                .inner;
            let decision = if allow.clicked() {
                Some(true)
            } else if deny.clicked() {
                Some(false)
            } else {
                None
            };
            if let Some(allow) = decision {
                let note = self.notes.remove(&p.id).unwrap_or_default();
                out.push(Intent::Run(GuiCmd::DecidePermission {
                    id: p.id.clone(),
                    allow,
                    note: Some(note.trim().to_string()).filter(|n| !n.is_empty()),
                }));
                self.answered.insert(p.id.clone());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::{fixture, render};
    use rteams_core::PermissionRequest;

    fn req(id: &str, decided: Option<bool>) -> PermissionRequest {
        PermissionRequest {
            id: id.into(),
            member: "ada".into(),
            tool: "Bash".into(),
            input: serde_json::json!({"command": "rm -rf /tmp/x", "nested": {"a": [1, 2]}}),
            decided,
            note: None,
        }
    }

    #[test]
    fn counts_auto_opens_and_renders() {
        let (home, data) = fixture("approvals");
        let mut st = data.state().clone();
        let v0 = View {
            state: &st,
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let mut a = Approvals::default();
        assert_eq!(Approvals::pending_count(&v0), 0);
        render(|ui| a.show(ui.ctx(), &v0, &mut Intents::default()));
        assert!(!a.is_open());

        st.permissions = vec![req("p1", None), req("p2", Some(true))];
        let v = View {
            state: &st,
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        assert_eq!(Approvals::pending_count(&v), 1);
        render(|ui| a.show(ui.ctx(), &v, &mut Intents::default()));
        assert!(a.is_open(), "new request auto-opens");
        a.is_open = false;
        render(|ui| a.show(ui.ctx(), &v, &mut Intents::default()));
        assert!(!a.is_open(), "same request does not reopen");
        let mut asked = Intents::default();
        render(|ui| a.show(ui.ctx(), &v, &mut asked));
        assert!(asked.0.is_empty(), "no rules: nothing is auto-decided");
        a.open();
        assert!(a.is_open());
        assert!(pretty_input(&st.permissions[0].input).contains("\n"));

        // Read-only viewer renders too.
        let ro = View {
            actor: None,
            ..View {
                state: &st,
                data: &data,
                actor: None,
                agents: &[],
                tr: crate::view::testing::tr(),
            }
        };
        render(|ui| a.show(ui.ctx(), &ro, &mut Intents::default()));

        let mut seen = HashSet::new();
        let notes = crate::notify::approval_notifications(&st, &mut seen, 5);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].body.contains("ada") && notes[0].body.contains("Bash"));
        assert!(crate::notify::approval_notifications(&st, &mut seen, 6).is_empty());
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn auto_allow_rules() {
        let none = AutoAllow::default();
        let bash = |c: &str| serde_json::json!({ "command": c });
        assert!(!none.allows("Edit", &serde_json::json!({})));
        let edits = AutoAllow {
            file_edits: true,
            ..none
        };
        assert!(
            edits.allows("Write", &serde_json::json!({})) && !edits.allows("Bash", &bash("ls"))
        );
        let safe = AutoAllow {
            safe_bash: true,
            ..none
        };
        assert!(safe.allows("Bash", &bash("git status -s")));
        assert!(safe.allows("Bash", &bash("ls -la src")));
        for bad in [
            "rm -rf /",
            "ls; rm x",
            "cat a | sh",
            "echo $(id)",
            "ls > f",
            "git push",
            "",
        ] {
            assert!(!safe.allows("Bash", &bash(bad)), "{bad}");
        }
        assert!(AutoAllow { all: true, ..none }.allows("Anything", &serde_json::json!(null)));
    }

    #[test]
    fn auto_allow_decides_matching_requests_once_and_persists() {
        let (home, data) = fixture("approvals-auto");
        let mut st = data.state().clone();
        st.permissions = vec![req("p1", None)]; // Bash rm -rf
        let mut other = req("p2", None);
        other.tool = "Edit".into();
        st.permissions.push(other);
        let v = View {
            state: &st,
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let mut a = Approvals {
            auto: AutoAllow {
                file_edits: true,
                ..Default::default()
            },
            auto_dir: Some(data.dir().to_path_buf()),
            ..Default::default()
        };
        let mut out = Intents::default();
        a.auto_decide(&v, &mut out);
        assert_eq!(out.0.len(), 1, "only the edit, not the rm -rf");
        assert!(matches!(
            &out.0[0],
            Intent::Run(GuiCmd::DecidePermission { id, allow: true, .. }) if id == "p2"
        ));
        a.auto_decide(&v, &mut out);
        assert_eq!(out.0.len(), 1, "not re-sent");
        a.auto.save(data.dir());
        assert!(AutoAllow::load(data.dir()).file_edits);
        let _ = std::fs::remove_dir_all(home);
    }
}
