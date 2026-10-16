//! Tool approvals (the original's ToolApprovalSheet): a modal listing the permission requests
//! agents are waiting on, each with Allow / Deny. Auto-opens when a new request arrives.

use std::collections::{HashMap, HashSet};

use eframe::egui::{self, Color32, RichText};

use crate::actions::GuiCmd;
use crate::theme;
use crate::view::{Intent, Intents, View};

#[derive(Default)]
pub struct Approvals {
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

    pub fn show(&mut self, ctx: &egui::Context, v: &View, out: &mut Intents) {
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
}
