//! Create-from-template modal: pick one of the eight team templates (or none), name the team,
//! choose the project folder and runtime, and get back a `TeamDraft` for the shell to create.

use std::path::Path;

use eframe::egui::{self, RichText};
use rteams_services::i18n::I18n;
use rteams_services::projects::{folder, RecentProject};
use rteams_services::team_import::TeamDraft;
use rteams_services::templates::{self, TeamTemplate};

use crate::theme;
use crate::ui::dashboard::project_picker::ProjectPicker;

pub const RUNTIMES: [(&str, &str); 2] = [("claude", "Claude Code"), ("codex", "Codex")];

pub struct TemplatePicker {
    open: bool,
    templates: Vec<TeamTemplate>,
    selected: Option<usize>,
    name: String,
    runtime: usize,
    project: ProjectPicker,
    error: Option<String>,
}

impl Default for TemplatePicker {
    fn default() -> Self {
        TemplatePicker {
            open: false,
            templates: templates::all(),
            selected: None,
            name: String::new(),
            runtime: 0,
            project: ProjectPicker::default(),
            error: None,
        }
    }
}

impl TemplatePicker {
    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open the modal, optionally with the project path and/or team name prefilled.
    pub fn open(&mut self, project: Option<String>, name: Option<String>) {
        self.open = true;
        self.error = None;
        self.project = ProjectPicker::new(project.unwrap_or_default());
        if let Some(n) = name {
            self.name = n;
        }
    }

    /// Build the draft for the current selection (no side effects).
    pub fn draft(&self) -> TeamDraft {
        let runtime = RUNTIMES[self.runtime].0;
        let project = self.project.value();
        match self.selected.and_then(|i| self.templates.get(i)) {
            Some(t) => t.to_draft(&self.name, project, runtime),
            None => TeamDraft {
                team_name: self.name.trim().to_string(),
                project_path: project,
                ..Default::default()
            },
        }
    }

    /// Draw the modal; returns a draft when the user confirmed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        tr: &I18n,
        home: &Path,
        recent: &[RecentProject],
    ) -> Option<TeamDraft> {
        if !self.open {
            return None;
        }
        let mut done = None;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::unique("template_picker"))
            .backdrop_color(egui::Color32::from_black_alpha(150))
            .frame(frame())
            .show(ctx, |ui| {
                ui.set_width(560.0f32.min((ctx.content_rect().width() - 80.0).max(120.0)));
                ui.label(
                    RichText::new(tr.t("team.templates.label"))
                        .size(18.0)
                        .strong(),
                );
                ui.add_space(8.0);
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for (i, t) in self.templates.iter().enumerate() {
                            let on = self.selected == Some(i);
                            let resp = egui::Frame::new()
                                .fill(if on {
                                    theme::tint(theme::ACCENT, 0.15)
                                } else {
                                    theme::RAISED
                                })
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    if on {
                                        theme::ACCENT
                                    } else {
                                        theme::hairline(0.1)
                                    },
                                ))
                                .corner_radius(egui::CornerRadius::same(6))
                                .inner_margin(egui::Margin::same(8))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(RichText::new(tr.t(&t.name_key())).strong());
                                    ui.label(
                                        RichText::new(tr.t(&t.description_key()))
                                            .size(11.5)
                                            .color(theme::TEXT_2),
                                    );
                                    let roles: Vec<&str> =
                                        t.members.iter().map(|m| m.role.as_str()).collect();
                                    ui.label(
                                        RichText::new(roles.join(", "))
                                            .size(11.0)
                                            .color(theme::MUTED),
                                    );
                                })
                                .response
                                .interact(egui::Sense::click());
                            if resp.clicked() {
                                self.selected = if on { None } else { Some(i) };
                            }
                        }
                    });
                ui.add_space(8.0);
                ui.label(RichText::new(tr.t("team.create.fields.teamName")).color(theme::TEXT_2));
                ui.add(
                    egui::TextEdit::singleline(&mut self.name)
                        .hint_text("my-team")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(4.0);
                ui.label(RichText::new(tr.t("team.templates.providerLabel")).color(theme::TEXT_2));
                egui::ComboBox::from_id_salt("template-runtime")
                    .selected_text(RUNTIMES[self.runtime].1)
                    .show_ui(ui, |ui| {
                        for (i, (_, label)) in RUNTIMES.iter().enumerate() {
                            ui.selectable_value(&mut self.runtime, i, *label);
                        }
                    });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(tr.t("team.projectPath.selectProject")).color(theme::TEXT_2),
                );
                self.project.show(ui, tr, "template", recent);
                let name_ok = crate::teams::valid_name(&self.name);
                if !self.name.trim().is_empty() && !name_ok {
                    ui.label(
                        RichText::new(tr.t("team.create.validation.nameMustContainLetterOrDigit"))
                            .size(11.0)
                            .color(theme::ERROR),
                    );
                }
                if name_ok && home.join(self.name.trim()).join("events.jsonl").exists() {
                    self.error = Some(tr.t("team.create.errors.nameExists"));
                } else if self.error.as_deref() == Some(&tr.t("team.create.errors.nameExists")) {
                    self.error = None;
                }
                if let Some(e) = &self.error {
                    ui.label(RichText::new(e).size(11.5).color(theme::ERROR));
                }
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ok = name_ok && self.error.is_none();
                    let b = egui::Button::new(
                        RichText::new(tr.t("team.create.actions.create"))
                            .strong()
                            .color(egui::Color32::WHITE),
                    )
                    .fill(theme::ACCENT);
                    if ui.add_enabled(ok, b).clicked() {
                        if let Some(p) = self.project.value() {
                            if let Err(e) = folder::ensure(&p) {
                                self.error = Some(e);
                                return;
                            }
                        }
                        done = Some(self.draft());
                    }
                    if ui.button(tr.t("common.actions.cancel")).clicked() {
                        cancel = true;
                    }
                });
            });
        if done.is_some() || cancel || modal.should_close() {
            self.open = false;
        }
        done
    }
}

fn frame() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::SURFACE)
        .stroke(egui::Stroke::new(1.0, theme::hairline(0.12)))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(20))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_follows_selection() {
        let mut p = TemplatePicker::default();
        p.open(Some("/p".into()), Some("x".into()));
        assert!(p.draft().members.is_empty());
        p.selected = Some(0);
        p.runtime = 1;
        let d = p.draft();
        assert_eq!(d.members.len(), 3);
        assert_eq!(d.members[0].runtime, "codex");
        assert_eq!(d.project_path.as_deref(), Some("/p"));
    }

    #[test]
    fn renders_headless() {
        let tr = I18n::default();
        let mut p = TemplatePicker::default();
        p.open(None, None);
        crate::view::testing::render(|ui| {
            assert!(p
                .show(ui.ctx(), &tr, Path::new("/nonexistent"), &[])
                .is_none());
        });
        assert!(p.is_open());
    }
}
