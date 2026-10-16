//! Import dialog: pick a folder / zip / git URL, choose where it goes, then review the plan.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

use eframe::egui::{self, RichText};
use rteams_services::ext_skills::import::{self, Prepared, Source};
use rteams_services::ext_skills::plan::Plan;
use rteams_services::ext_skills::roots::SkillRoot;
use rteams_services::i18n::I18n;

use crate::theme;

use super::skills_panel::t;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceKind {
    #[default]
    Folder,
    Zip,
    Git,
}

pub enum ImportAction {
    None,
    Cancel,
    /// Review this plan; `Plan` writes only on confirm.
    Review(Plan, String),
}

pub struct SkillImportDialog {
    pub kind: SourceKind,
    pub path: String,
    pub git_url: String,
    pub folder: String,
    pub root_idx: usize,
    pub roots: Vec<(String, SkillRoot)>,
    pub error: Option<String>,
    rx: Option<Receiver<Result<Prepared, String>>>,
    prepared: Option<Prepared>,
}

impl SkillImportDialog {
    pub fn new(roots: Vec<(String, SkillRoot)>) -> Self {
        Self {
            kind: SourceKind::Folder,
            path: String::new(),
            git_url: String::new(),
            folder: String::new(),
            root_idx: 0,
            roots,
            error: None,
            rx: None,
            prepared: None,
        }
    }

    pub fn source(&self) -> Option<Source> {
        match self.kind {
            SourceKind::Folder if !self.path.trim().is_empty() => {
                Some(Source::Folder(PathBuf::from(self.path.trim())))
            }
            SourceKind::Zip if !self.path.trim().is_empty() => {
                Some(Source::Zip(PathBuf::from(self.path.trim())))
            }
            SourceKind::Git if !self.git_url.trim().is_empty() => {
                Some(Source::Git(self.git_url.trim().to_string()))
            }
            _ => None,
        }
    }

    pub fn is_preparing(&self) -> bool {
        self.rx.is_some()
    }

    fn start(&mut self) {
        let Some(src) = self.source() else { return };
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.error = None;
        std::thread::spawn(move || {
            let _ = tx.send(import::prepare(&src).map_err(|e| e.to_string()));
        });
    }

    fn build_plan(&mut self) -> Option<(Plan, String)> {
        let p = self.prepared.as_ref()?;
        let (_, root) = self.roots.get(self.root_idx)?;
        let folder = if self.folder.trim().is_empty() {
            p.default_folder.clone()
        } else {
            self.folder.trim().to_string()
        };
        match import::plan_import(p, &root.dir, &folder) {
            Ok(plan) => Some((plan, folder)),
            Err(e) => {
                self.error = Some(e.to_string());
                None
            }
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, tr: &I18n) -> ImportAction {
        let mut action = ImportAction::None;
        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok(Ok(p)) => {
                    self.rx = None;
                    self.prepared = Some(p);
                    if let Some((plan, _)) = self.build_plan() {
                        action = ImportAction::Review(
                            plan,
                            t(
                                tr,
                                "extensions.skillImport.reviewLabel",
                                "Importing this skill",
                            ),
                        );
                    }
                }
                Ok(Err(e)) => {
                    self.rx = None;
                    self.error = Some(e);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100))
                }
                Err(_) => self.rx = None,
            }
        }
        let preparing = self.is_preparing();
        egui::Window::new(t(tr, "extensions.skillImport.title", "Import skill"))
            .collapsible(false)
            .default_width(560.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(RichText::new(t(tr, "extensions.skillImport.description", "Pick an existing skill folder, review what will be copied, then import it into one of your supported skill locations.")).color(theme::TEXT_2));
                ui.add_space(8.0);
                ui.label(RichText::new(t(tr, "extensions.skillImport.steps.chooseFolder.title", "1. Choose a skill folder")).strong());
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.kind, SourceKind::Folder, "Folder");
                    ui.selectable_value(&mut self.kind, SourceKind::Zip, "Zip file");
                    ui.selectable_value(&mut self.kind, SourceKind::Git, "Git URL");
                });
                ui.label(RichText::new(t(tr, "extensions.skillImport.fields.sourceFolder", "Source folder")).small().color(theme::MUTED));
                if self.kind == SourceKind::Git {
                    ui.add(egui::TextEdit::singleline(&mut self.git_url).hint_text("https://github.com/owner/repo").desired_width(f32::INFINITY));
                } else {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.path).desired_width((ui.available_width() - 90.0).max(80.0)));
                        if ui.button(t(tr, "extensions.skillImport.actions.browse", "Browse")).clicked() {
                            let picked = if self.kind == SourceKind::Zip {
                                rfd::FileDialog::new().add_filter("zip", &["zip"]).pick_file()
                            } else {
                                rfd::FileDialog::new().pick_folder()
                            };
                            if let Some(p) = picked {
                                self.path = p.display().to_string();
                            }
                        }
                    });
                }
                ui.add_space(8.0);
                ui.label(RichText::new(t(tr, "extensions.skillImport.steps.location.title", "2. Decide where it belongs")).strong());
                ui.label(RichText::new(t(tr, "extensions.skillImport.steps.location.description", "Personal skills work everywhere. Project skills only show up for one codebase.")).small().color(theme::MUTED));
                ui.label(RichText::new(t(tr, "extensions.skillImport.fields.storage", "Where to store it")).small().color(theme::MUTED));
                let sel = self.roots.get(self.root_idx).map_or(String::new(), |r| r.0.clone());
                egui::ComboBox::from_id_salt("skill_import_root").selected_text(sel).width(360.0).show_ui(ui, |ui| {
                    for (i, (label, _)) in self.roots.iter().enumerate() {
                        ui.selectable_value(&mut self.root_idx, i, label);
                    }
                });
                ui.label(RichText::new(t(tr, "extensions.skillImport.fields.destinationFolderName", "Destination folder name")).small().color(theme::MUTED));
                ui.add(egui::TextEdit::singleline(&mut self.folder).hint_text(t(tr, "extensions.skillImport.placeholders.defaultFolderName", "Defaults to source folder name")).desired_width(f32::INFINITY));
                ui.label(RichText::new(t(tr, "extensions.skillImport.reviewHint", "Review the copied files first, then confirm the import in the next step.")).small().color(theme::MUTED));
                if let Some(e) = &self.error {
                    ui.colored_label(theme::ERROR, e);
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if theme::ghost(ui, &t(tr, "extensions.skillImport.actions.cancel", "Cancel")).clicked() {
                        action = ImportAction::Cancel;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if preparing {
                            t(tr, "extensions.skillImport.actions.preparing", "Preparing...")
                        } else {
                            t(tr, "extensions.skillImport.actions.reviewAndImport", "Review And Import")
                        };
                        let enabled = !preparing && self.source().is_some() && !self.roots.is_empty();
                        if ui.add_enabled(enabled, egui::Button::new(RichText::new(label).strong().color(egui::Color32::WHITE)).fill(theme::ACCENT)).clicked() {
                            self.start();
                        }
                    });
                });
            });
        action
    }
}
