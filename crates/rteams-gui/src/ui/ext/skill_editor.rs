//! Create / edit dialog: a structured form (name, description, how it is used...) that generates
//! SKILL.md, plus an advanced raw editor built on `CodeView`. Saving goes through the review dialog.

use std::collections::BTreeMap;
use std::path::PathBuf;

use eframe::egui::{self, RichText};
use rteams_services::ext_skills::plan::Plan;
use rteams_services::ext_skills::roots::SkillRoot;
use rteams_services::ext_skills::scaffold::{self, Draft};
use rteams_services::ext_skills::{metadata, validator, Skill};
use rteams_services::i18n::I18n;

use crate::theme;
use crate::ui::editor::code_view::{CodeView, CodeViewOptions};

use super::skills_panel::t;

pub enum EditorAction {
    None,
    Cancel,
    Review(Plan, String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Create,
    Edit,
}

pub struct SkillEditor {
    pub mode: Mode,
    pub draft: Draft,
    pub folder: String,
    folder_touched: bool,
    pub root_idx: usize,
    pub roots: Vec<(String, SkillRoot)>,
    /// Raw SKILL.md text when the advanced editor owns the file (always in edit mode).
    pub raw: String,
    pub raw_mode: bool,
    show_advanced: bool,
    code: CodeView,
    /// Edit mode: the existing skill file and folder.
    edit_file: Option<PathBuf>,
    edit_dir: Option<PathBuf>,
    pub error: Option<String>,
}

impl SkillEditor {
    pub fn new_create(roots: Vec<(String, SkillRoot)>) -> Self {
        Self {
            mode: Mode::Create,
            draft: Draft::default(),
            folder: String::new(),
            folder_touched: false,
            root_idx: 0,
            roots,
            raw: String::new(),
            raw_mode: false,
            show_advanced: false,
            code: CodeView::default(),
            edit_file: None,
            edit_dir: None,
            error: None,
        }
    }

    pub fn new_edit(skill: &Skill) -> Self {
        let raw = std::fs::read_to_string(&skill.skill_file).unwrap_or_default();
        let m = &skill.meta;
        Self {
            mode: Mode::Edit,
            draft: Draft {
                name: m.name.clone().unwrap_or_default(),
                description: m.description.clone().unwrap_or_default(),
                license: m.license.clone().unwrap_or_default(),
                compatibility: m.compatibility.clone().unwrap_or_default(),
                manual_only: m.disable_model_invocation,
                ..Default::default()
            },
            folder: skill.folder_name.clone(),
            folder_touched: true,
            root_idx: 0,
            roots: vec![],
            raw,
            raw_mode: true,
            show_advanced: true,
            code: CodeView::default(),
            edit_file: Some(skill.skill_file.clone()),
            edit_dir: Some(skill.dir.clone()),
            error: None,
        }
    }

    /// The SKILL.md text that would be written.
    pub fn skill_md(&self) -> String {
        if self.raw_mode {
            self.raw.clone()
        } else {
            scaffold::render_skill_md(&self.draft)
        }
    }

    pub fn issues(&self) -> Vec<validator::Issue> {
        let text = self.skill_md();
        let (meta, body, fm) = metadata::parse(&text);
        validator::validate(&meta, &fm, &body, Some(&self.folder), false)
    }

    pub fn can_review(&self) -> bool {
        !validator::has_errors(&self.issues())
            && (self.mode == Mode::Edit
                || (!self.roots.is_empty() && validator::is_valid_folder_name(&self.folder)))
    }

    /// Build the plan for the current state.
    pub fn build_plan(&self) -> Result<Plan, String> {
        match self.mode {
            Mode::Create => {
                let (_, root) = self
                    .roots
                    .get(self.root_idx)
                    .ok_or("No writable skill location")?;
                let mut files = scaffold::files_for(&self.draft);
                if self.raw_mode {
                    files.insert("SKILL.md".into(), self.raw.clone().into_bytes());
                }
                scaffold::plan_create_with(&root.dir, &self.folder, &files)
            }
            Mode::Edit => {
                let (dir, file) = self
                    .edit_dir
                    .clone()
                    .zip(self.edit_file.clone())
                    .ok_or("Nothing to edit")?;
                let name = file
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "SKILL.md".into());
                let mut files = BTreeMap::new();
                files.insert(name, self.raw.clone().into_bytes());
                Ok(Plan::build(&dir, &files, false))
            }
        }
    }

    /// Edit mode: copy the structured fields into the raw frontmatter, keeping other keys.
    pub fn apply_fields_to_raw(&mut self) {
        let (mut meta, body, _) = metadata::parse(&self.raw);
        let d = &self.draft;
        meta.name = Some(d.name.trim().to_string());
        meta.description = Some(d.description.trim().to_string());
        meta.license = Some(d.license.trim().to_string()).filter(|s| !s.is_empty());
        meta.compatibility = Some(d.compatibility.trim().to_string()).filter(|s| !s.is_empty());
        meta.disable_model_invocation = d.manual_only;
        self.raw = metadata::render(&meta, &body);
    }

    pub fn show(&mut self, ctx: &egui::Context, tr: &I18n) -> EditorAction {
        let mut action = EditorAction::None;
        let editing = self.mode == Mode::Edit;
        let title = if editing {
            t(tr, "extensions.skillEditor.title.edit", "Edit skill")
        } else {
            t(tr, "extensions.skillEditor.title.create", "Create skill")
        };
        egui::Window::new(title)
            .collapsible(false)
            .default_width(700.0)
            .default_height(600.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(if editing {
                        t(tr, "extensions.skillEditor.description.edit", "Update this skill, review the resulting file changes, then save it.")
                    } else {
                        t(tr, "extensions.skillEditor.description.create", "Describe the workflow in plain language, review the files that will be created, then save it.")
                    })
                    .color(theme::TEXT_2),
                );
                let issues = self.issues();
                egui::ScrollArea::vertical().max_height((ui.available_height() - 50.0).max(60.0)).auto_shrink([false, false]).show(ui, |ui| {
                    ui.add_space(6.0);
                    ui.label(RichText::new(t(tr, "extensions.skillEditor.basics.title", "1. Basics")).strong());
                    let before = self.draft.name.clone();
                    field(ui, &t(tr, "extensions.skillEditor.fields.name", "Skill name"), &t(tr, "extensions.skillEditor.placeholders.name", "Write concise skill name"), &mut self.draft.name, false);
                    let mut changed = before != self.draft.name;
                    if !editing {
                        if changed && !self.folder_touched {
                            self.folder = scaffold::slug(&self.draft.name);
                        }
                        ui.label(RichText::new(t(tr, "extensions.skillEditor.fields.folderName", "Folder name")).small().color(theme::MUTED));
                        if ui.add(egui::TextEdit::singleline(&mut self.folder).desired_width(f32::INFINITY)).changed() {
                            self.folder_touched = true;
                        }
                        ui.label(RichText::new(t(tr, "extensions.skillEditor.fields.folderNameHint", "We suggest this automatically from the skill name so review works right away.")).small().color(theme::MUTED));
                        ui.label(RichText::new(t(tr, "extensions.skillEditor.fields.root", "Where to store it")).small().color(theme::MUTED));
                        let sel = self.roots.get(self.root_idx).map_or(String::new(), |r| r.0.clone());
                        egui::ComboBox::from_id_salt("skill_editor_root").selected_text(sel).width(360.0).show_ui(ui, |ui| {
                            for (i, (label, _)) in self.roots.iter().enumerate() {
                                ui.selectable_value(&mut self.root_idx, i, label);
                            }
                        });
                    } else {
                        ui.label(RichText::new(t(tr, "extensions.skillEditor.extraFiles.lockedForEdits", "Root and folder are locked for edits")).small().color(theme::MUTED));
                    }
                    let d0 = (self.draft.description.clone(), self.draft.license.clone(), self.draft.compatibility.clone(), self.draft.manual_only);
                    field(ui, &t(tr, "extensions.skillEditor.fields.description", "Description"), &t(tr, "extensions.skillEditor.placeholders.description", "What this skill helps with"), &mut self.draft.description, true);
                    ui.label(RichText::new(t(tr, "extensions.skillEditor.fields.invocation", "How it should be used")).small().color(theme::MUTED));
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.draft.manual_only, false, t(tr, "extensions.skillEditor.invocation.auto", "Can be used automatically"));
                        ui.selectable_value(&mut self.draft.manual_only, true, t(tr, "extensions.skillEditor.invocation.manualOnly", "Only when you ask for it"));
                    });
                    field(ui, &t(tr, "extensions.skillEditor.fields.license", "License"), &t(tr, "extensions.skillEditor.placeholders.license", "MIT"), &mut self.draft.license, false);
                    field(ui, &t(tr, "extensions.skillEditor.fields.compatibility", "Compatibility"), &t(tr, "extensions.skillEditor.placeholders.compatibility", "claude-code, cursor"), &mut self.draft.compatibility, false);
                    changed |= d0 != (self.draft.description.clone(), self.draft.license.clone(), self.draft.compatibility.clone(), self.draft.manual_only);
                    if editing && changed {
                        self.apply_fields_to_raw();
                    }

                    if !editing {
                        ui.add_space(8.0);
                        ui.label(RichText::new(t(tr, "extensions.skillEditor.instructions.title", "2. Instructions")).strong());
                        if self.raw_mode {
                            ui.label(RichText::new(t(tr, "extensions.skillEditor.instructions.locked", "Structured fields are locked because you switched to manual `SKILL.md` editing below.")).small().color(theme::WARNING));
                        } else {
                            field(ui, &t(tr, "extensions.skillEditor.fields.whenToUse", "When to reach for this"), &t(tr, "extensions.skillEditor.placeholders.whenToUse", "Example: Use this when the task is a code review or bug triage request."), &mut self.draft.when_to_use, true);
                            field(ui, &t(tr, "extensions.skillEditor.fields.steps", "Main steps to follow"), &t(tr, "extensions.skillEditor.placeholders.steps", "1. Inspect the relevant files."), &mut self.draft.steps, true);
                            field(ui, &t(tr, "extensions.skillEditor.fields.notes", "Extra notes or guardrails"), &t(tr, "extensions.skillEditor.placeholders.notes", "Example: Call out missing tests, regressions, and risky assumptions."), &mut self.draft.notes, true);
                        }
                        ui.add_space(8.0);
                        ui.label(RichText::new(t(tr, "extensions.skillEditor.extraFiles.title", "3. Extra files")).strong());
                        ui.label(RichText::new(t(tr, "extensions.skillEditor.extraFiles.optionalDescription", "Add starter files that will be included in the review and written together with `SKILL.md`.")).small().color(theme::MUTED));
                        ui.horizontal(|ui| {
                            for (dir, key, fb) in [("scripts", "scripts", "Scripts"), ("references", "references", "References"), ("assets", "assets", "Assets")] {
                                let mut on = self.draft.extra_dirs.iter().any(|d| d == dir);
                                if ui.checkbox(&mut on, t(tr, &format!("extensions.skillEditor.extraFiles.{key}"), fb)).changed() {
                                    self.draft.extra_dirs.retain(|d| d != dir);
                                    if on {
                                        self.draft.extra_dirs.push(dir.to_string());
                                    }
                                }
                            }
                        });
                    }

                    ui.add_space(8.0);
                    if !editing {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(t(tr, "extensions.skillEditor.advanced.title", "4. Advanced SKILL.md editor")).strong());
                            let lbl = if self.show_advanced { t(tr, "extensions.skillEditor.advanced.hide", "Hide Advanced Editor") } else { t(tr, "extensions.skillEditor.advanced.show", "Show Advanced Editor") };
                            if theme::ghost(ui, &lbl).clicked() {
                                self.show_advanced = !self.show_advanced;
                                if self.show_advanced && !self.raw_mode {
                                    self.raw = scaffold::render_skill_md(&self.draft);
                                    self.raw_mode = true;
                                }
                            }
                            if self.raw_mode && theme::ghost(ui, &t(tr, "extensions.skillEditor.advanced.resetFromStructuredFields", "Reset From Structured Fields")).clicked() {
                                self.raw_mode = false;
                                self.show_advanced = false;
                            }
                        });
                    } else {
                        ui.label(RichText::new("SKILL.md").strong());
                    }
                    if self.show_advanced {
                        let before = self.raw.clone();
                        let h = 260.0;
                        ui.allocate_ui(egui::vec2(ui.available_width(), h), |ui| {
                            self.code.show(ui, &mut self.raw, "md", &CodeViewOptions { word_wrap: true, ..Default::default() });
                        });
                        if editing && before != self.raw {
                            // keep the form in sync with hand edits
                            let (m, _, _) = metadata::parse(&self.raw);
                            self.draft.name = m.name.unwrap_or_default();
                            self.draft.description = m.description.unwrap_or_default();
                            self.draft.license = m.license.unwrap_or_default();
                            self.draft.compatibility = m.compatibility.unwrap_or_default();
                            self.draft.manual_only = m.disable_model_invocation;
                        }
                    }
                    ui.add_space(6.0);
                    for i in &issues {
                        let col = if i.severity == validator::Severity::Error { theme::ERROR } else { theme::WARNING };
                        ui.colored_label(col, &i.message);
                    }
                    if let Some(e) = &self.error {
                        ui.colored_label(theme::ERROR, e);
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if theme::ghost(ui, &t(tr, "extensions.skillEditor.actions.cancel", "Cancel")).clicked() {
                        action = EditorAction::Cancel;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if editing { t(tr, "extensions.skillEditor.actions.reviewAndSave", "Review And Save") } else { t(tr, "extensions.skillEditor.actions.reviewAndCreate", "Review And Create") };
                        if ui.add_enabled(self.can_review(), egui::Button::new(RichText::new(label).strong().color(egui::Color32::WHITE)).fill(theme::ACCENT)).clicked() {
                            match self.build_plan() {
                                Ok(p) => {
                                    self.error = None;
                                    let lbl = if editing { t(tr, "extensions.skillEditor.review.saving", "Saving this skill") } else { t(tr, "extensions.skillEditor.review.creating", "Creating a skill") };
                                    action = EditorAction::Review(p, lbl);
                                }
                                Err(e) => self.error = Some(e),
                            }
                        }
                    });
                });
            });
        action
    }
}

fn field(ui: &mut egui::Ui, label: &str, hint: &str, text: &mut String, multiline: bool) {
    ui.label(RichText::new(label).small().color(theme::MUTED));
    if multiline {
        ui.add(
            egui::TextEdit::multiline(text)
                .hint_text(hint)
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
    } else {
        ui.add(
            egui::TextEdit::singleline(text)
                .hint_text(hint)
                .desired_width(f32::INFINITY),
        );
    }
}
