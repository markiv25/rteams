//! Detail dialog for one discovered skill: metadata, issues, files and the rendered SKILL.md.

use eframe::egui::{self, RichText};
use rteams_services::ext_skills::roots::Scope;
use rteams_services::ext_skills::{Severity, Skill};
use rteams_services::i18n::I18n;

use crate::theme;
use crate::ui::editor::md_preview;

use super::skills_panel::{t, ta};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailAction {
    None,
    Close,
    Edit,
    Delete,
    OpenFolder,
    OpenFile,
}

pub struct SkillDetail {
    pub skill: Skill,
    confirm_delete: bool,
}

impl SkillDetail {
    pub fn new(skill: Skill) -> Self {
        Self {
            skill,
            confirm_delete: false,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, tr: &I18n) -> DetailAction {
        let mut action = DetailAction::None;
        let s = &self.skill;
        egui::Window::new(s.display_name().to_string())
            .collapsible(false)
            .default_width(680.0)
            .default_height(560.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().max_height((ui.available_height() - 50.0).max(60.0)).auto_shrink([false, false]).show(ui, |ui| {
                    ui.label(
                        RichText::new(
                            s.meta
                                .description
                                .clone()
                                .filter(|d| !d.is_empty())
                                .unwrap_or_else(|| t(tr, "extensions.skillDetail.descriptionFallback", "Inspect discovered skill metadata and raw instructions.")),
                        )
                        .color(theme::TEXT_2),
                    );
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        theme::pill(ui, ta(tr, "extensions.skillDetail.badges.storedIn", "Stored in {{root}}", &[("root", &s.root_label)]), theme::ACCENT);
                        if s.manual_only() {
                            theme::pill(ui, t(tr, "extensions.skillDetail.badges.manualUse", "Manual use"), theme::WARNING);
                        } else {
                            theme::pill(ui, t(tr, "extensions.skillDetail.badges.autoUse", "Auto use"), theme::SUCCESS);
                        }
                        if s.has_scripts {
                            theme::pill(ui, t(tr, "extensions.skillDetail.badges.hasScripts", "Has scripts"), theme::WARNING);
                        }
                        if s.has_references {
                            theme::pill(ui, t(tr, "extensions.skillDetail.badges.references", "References"), theme::INFO);
                        }
                        if s.has_assets {
                            theme::pill(ui, t(tr, "extensions.skillDetail.badges.assets", "Assets"), theme::INFO);
                        }
                    });
                    ui.add_space(8.0);
                    egui::Grid::new("skill_detail_summary").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                        ui.label(RichText::new(t(tr, "extensions.skillDetail.summary.whoCanUse", "Who can use it")).color(theme::MUTED));
                        ui.label(match s.scope {
                            Scope::User => t(tr, "extensions.skillDetail.scope.personal", "Your personal skills"),
                            Scope::Project => t(tr, "extensions.skillDetail.scope.projectOnly", "This project only"),
                        });
                        ui.end_row();
                        ui.label(RichText::new(t(tr, "extensions.skillDetail.summary.howUsed", "How it is used")).color(theme::MUTED));
                        ui.label(if s.manual_only() {
                            t(tr, "extensions.skillDetail.invocation.manualOnly", "Only runs when you explicitly ask for it.")
                        } else {
                            t(tr, "extensions.skillDetail.invocation.auto", "Runs automatically when it matches the task.")
                        });
                        ui.end_row();
                        ui.label(RichText::new(t(tr, "extensions.skillDetail.summary.included", "What comes with it")).color(theme::MUTED));
                        let mut inc = vec![];
                        if s.has_scripts {
                            inc.push(t(tr, "extensions.skillDetail.includes.scripts", "scripts"));
                        }
                        if s.has_references {
                            inc.push(t(tr, "extensions.skillDetail.includes.references", "references"));
                        }
                        if s.has_assets {
                            inc.push(t(tr, "extensions.skillDetail.includes.assets", "assets"));
                        }
                        ui.label(if inc.is_empty() { t(tr, "extensions.skillDetail.includes.instructionsOnly", "Just the skill instructions") } else { inc.join(", ") });
                        ui.end_row();
                        if !s.meta.allowed_tools.is_empty() {
                            ui.label(RichText::new("allowed-tools").color(theme::MUTED));
                            ui.label(s.meta.allowed_tools.join(", "));
                            ui.end_row();
                        }
                        ui.label(RichText::new(t(tr, "extensions.skillDetail.files.storedAt", "Stored at")).color(theme::MUTED));
                        ui.label(RichText::new(s.dir.display().to_string()).monospace().small());
                        ui.end_row();
                    });
                    for i in &s.issues {
                        let col = if i.severity == Severity::Error { theme::ERROR } else { theme::WARNING };
                        ui.colored_label(col, &i.message);
                    }
                    ui.add_space(6.0);
                    egui::CollapsingHeader::new(t(tr, "extensions.skillDetail.files.advancedDetails", "Advanced file details")).id_salt("skill_detail_files").show(ui, |ui| {
                        for f in &s.files {
                            ui.label(RichText::new(f).monospace().small());
                        }
                    });
                    theme::rule(ui);
                    md_preview::show(ui, &s.body);
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if self.confirm_delete {
                        ui.label(ta(tr, "extensions.skillDetail.deleteDialog.descriptionWithName", "Delete \"{{name}}\" and move it to Trash? You can restore it later from Trash if needed.", &[("name", s.display_name())]));
                        if theme::solid(ui, &t(tr, "extensions.skillDetail.actions.delete", "Delete"), theme::ERROR).clicked() {
                            action = DetailAction::Delete;
                        }
                        if theme::ghost(ui, &t(tr, "extensions.skillDetail.actions.cancel", "Cancel")).clicked() {
                            self.confirm_delete = false;
                        }
                        return;
                    }
                    if theme::ghost(ui, "Close").clicked() {
                        action = DetailAction::Close;
                    }
                    if theme::ghost(ui, &t(tr, "extensions.skillDetail.actions.openFolder", "Open Folder")).clicked() {
                        action = DetailAction::OpenFolder;
                    }
                    if theme::ghost(ui, &t(tr, "extensions.skillDetail.actions.openSkillFile", "Open SKILL.md")).clicked() {
                        action = DetailAction::OpenFile;
                    }
                    if !s.read_only {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::solid(ui, &t(tr, "extensions.skillDetail.actions.editSkill", "Edit Skill"), theme::ACCENT).clicked() {
                                action = DetailAction::Edit;
                            }
                            if theme::ghost(ui, &t(tr, "extensions.skillDetail.actions.deleteSkill", "Delete Skill")).clicked() {
                                self.confirm_delete = true;
                            }
                        });
                    }
                });
            });
        action
    }
}
