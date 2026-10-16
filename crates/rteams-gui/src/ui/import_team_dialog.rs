//! Import agent team modal: choose a folder (`.claude/agents`, `CLAUDE.md`), a team-definition
//! file, or copy another team's roster; review everything; get a `TeamDraft` back.

use std::path::Path;

use eframe::egui::{self, RichText};
use rteams_services::i18n::I18n;
use rteams_services::projects::{folder, RecentProject};
use rteams_services::team_import::{self, ImportPreview, TeamDraft};

use crate::theme;
use crate::ui::dashboard::project_picker::ProjectPicker;

#[derive(Default)]
pub struct ImportTeamDialog {
    open: bool,
    preview: Option<ImportPreview>,
    name: String,
    project: ProjectPicker,
    error: Option<String>,
}

impl ImportTeamDialog {
    pub fn open(&mut self) {
        *self = ImportTeamDialog {
            open: true,
            ..Default::default()
        };
    }

    /// Load a preview from a folder or definition file path.
    pub fn load_path(&mut self, path: &Path) {
        match team_import::import_path(path) {
            Ok(p) => self.set_preview(p),
            Err(e) => self.fail(e),
        }
    }

    /// Load a preview from another team's roster.
    pub fn load_team(&mut self, home: &Path, team: &str) {
        match team_import::from_team(home, team) {
            Ok(p) => self.set_preview(p),
            Err(e) => self.fail(e),
        }
    }

    fn set_preview(&mut self, p: ImportPreview) {
        self.name = p.suggested_name.clone();
        self.error = None;
        self.preview = Some(p);
    }

    fn fail(&mut self, e: String) {
        self.preview = None;
        self.error = Some(e);
    }

    /// Draw the modal; returns the draft when confirmed.
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
        let mut pick: Option<std::path::PathBuf> = None;
        let mut from_team: Option<String> = None;
        let teams: Vec<String> = crate::teams::scan(home)
            .into_iter()
            .filter(|t| !t.deleted && t.name.is_some())
            .map(|t| t.dir)
            .collect();
        let modal = egui::Modal::new(egui::Id::unique("import_team_dialog"))
            .backdrop_color(egui::Color32::from_black_alpha(150))
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::hairline(0.12)))
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::same(20)),
            )
            .show(ctx, |ui| {
                ui.set_width(600.0f32.min((ctx.content_rect().width() - 80.0).max(120.0)));
                ui.label(
                    RichText::new(tr.t("team.teamImport.title"))
                        .size(18.0)
                        .strong(),
                );
                ui.label(
                    RichText::new(tr.t("team.teamImport.description"))
                        .size(12.0)
                        .color(theme::TEXT_2),
                );
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.button(tr.t("team.teamImport.chooseFolder")).clicked() {
                        pick = rfd::FileDialog::new().pick_folder();
                    }
                    if ui.button("JSON / YAML...").clicked() {
                        pick = rfd::FileDialog::new()
                            .add_filter("team definition", &["json", "yaml", "yml"])
                            .pick_file();
                    }
                    egui::ComboBox::from_id_salt("import-from-team")
                        .selected_text(tr.t("team.list.actions.copyTeam"))
                        .show_ui(ui, |ui| {
                            for t in &teams {
                                if ui.selectable_label(false, t).clicked() {
                                    from_team = Some(t.clone());
                                }
                            }
                        });
                });
                if let Some(e) = &self.error {
                    ui.label(RichText::new(e).color(theme::ERROR));
                }
                let Some(p) = self.preview.as_mut() else {
                    ui.add_space(8.0);
                    if self.error.is_none() {
                        ui.label(
                            RichText::new(tr.t("team.teamImport.selectPrompt")).color(theme::MUTED),
                        );
                    }
                    ui.add_space(8.0);
                    if ui.button(tr.t("common.actions.cancel")).clicked() {
                        cancel = true;
                    }
                    return;
                };
                ui.add_space(8.0);
                ui.label(RichText::new(tr.t("team.teamImport.teamName")).color(theme::TEXT_2));
                ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(f32::INFINITY));
                let name_err = team_import::validate_team_name(&self.name).err();
                if let Some(e) = name_err {
                    ui.label(RichText::new(tr.t(e.key())).size(11.5).color(theme::ERROR));
                }
                let exists = home.join(self.name.trim()).join("events.jsonl").exists();
                if exists {
                    ui.label(
                        RichText::new(tr.t("team.create.errors.nameExists"))
                            .size(11.5)
                            .color(theme::ERROR),
                    );
                }
                ui.add_space(4.0);
                ui.label(RichText::new(tr.t("team.teamImport.projectPath")).color(theme::TEXT_2));
                self.project.show(ui, tr, "import", recent);
                ui.add_space(4.0);
                ui.label(RichText::new(tr.t("team.teamImport.leadPrompt")).color(theme::TEXT_2));
                egui::ScrollArea::vertical()
                    .id_salt("import-lead")
                    .max_height(100.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut p.lead_prompt)
                                .desired_rows(3)
                                .desired_width(f32::INFINITY),
                        );
                    });
                ui.label(
                    RichText::new(tr.t_args(
                        "team.teamImport.members",
                        &[("count", &p.members.len().to_string())],
                    ))
                    .color(theme::TEXT_2),
                );
                egui::ScrollArea::vertical()
                    .id_salt("import-members")
                    .max_height(110.0)
                    .show(ui, |ui| {
                        for m in &p.members {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&m.name).strong());
                                ui.label(RichText::new(&m.role).size(11.5).color(theme::MUTED));
                            });
                        }
                    });
                if !p.skills.is_empty() {
                    ui.label(
                        RichText::new(tr.t_args(
                            "team.teamImport.skills",
                            &[("count", &p.skills.len().to_string())],
                        ))
                        .color(theme::TEXT_2),
                    );
                    ui.label(
                        RichText::new(p.skills.join(", "))
                            .size(11.5)
                            .color(theme::MUTED),
                    );
                }
                for w in &p.warnings {
                    ui.label(RichText::new(w.text(tr)).size(11.5).color(theme::WARNING));
                }
                ui.add_space(8.0);
                ui.label(
                    RichText::new(tr.t_args(
                        "team.teamImport.summary",
                        &[
                            ("teamName", self.name.trim()),
                            ("count", &p.members.len().to_string()),
                        ],
                    ))
                    .size(11.5)
                    .color(theme::TEXT_2),
                );
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ok = name_err.is_none() && !exists;
                    let b = egui::Button::new(
                        RichText::new(tr.t("team.teamImport.createDraft"))
                            .strong()
                            .color(egui::Color32::WHITE),
                    )
                    .fill(theme::ACCENT);
                    if ui.add_enabled(ok, b).clicked() {
                        if let Some(pp) = self.project.value() {
                            if let Err(e) = folder::ensure(&pp) {
                                self.error = Some(e);
                                return;
                            }
                        }
                        done = Some(p.clone().into_draft(&self.name, self.project.value()));
                    }
                    if ui.button(tr.t("common.actions.cancel")).clicked() {
                        cancel = true;
                    }
                });
            });
        if let Some(p) = pick {
            self.load_path(&p);
        }
        if let Some(t) = from_team {
            self.load_team(home, &t);
        }
        if done.is_some() || cancel || modal.should_close() {
            self.open = false;
        }
        done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_json_and_renders() {
        let dir = std::env::temp_dir().join(format!("rteams-import-dlg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("t.json");
        std::fs::write(
            &f,
            r#"{"name":"alpha","members":[{"name":"a","role":"r"}]}"#,
        )
        .unwrap();
        let tr = I18n::default();
        let mut dlg = ImportTeamDialog::default();
        dlg.open();
        crate::view::testing::render(|ui| {
            assert!(dlg.show(ui.ctx(), &tr, &dir, &[]).is_none());
        });
        dlg.load_path(&f);
        assert_eq!(dlg.name, "alpha");
        crate::view::testing::render(|ui| {
            assert!(dlg.show(ui.ctx(), &tr, &dir, &[]).is_none());
        });
        dlg.load_path(&dir.join("missing.json"));
        assert!(dlg.error.is_some() && dlg.preview.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
