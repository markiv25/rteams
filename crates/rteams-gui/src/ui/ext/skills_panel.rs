//! Extensions > Skills: list of discovered skills (personal, project, plugin) with search,
//! filters and sorting, plus create / import / detail / edit / review flows.

use std::path::PathBuf;

use eframe::egui::{self, RichText};
use rteams_services::ext_skills::roots::{self, Scope, SkillRoot};
use rteams_services::ext_skills::watch::SkillWatcher;
use rteams_services::ext_skills::{self, Catalog, Skill};
use rteams_services::i18n::I18n;

use super::skill_detail::{DetailAction, SkillDetail};
use super::skill_editor::{EditorAction, SkillEditor};
use super::skill_import::{ImportAction, SkillImportDialog};
use super::skill_review::{ReviewAction, ReviewDialog};
use crate::theme;
use crate::ui::{FeatureCtx, ShellRequest};

/// Translate with an English fallback (new keys may be missing from a locale).
pub(crate) fn t(tr: &I18n, key: &str, fallback: &str) -> String {
    if tr.has(key) {
        tr.t(key)
    } else {
        fallback.to_string()
    }
}

/// `t` with `{{name}}` placeholders.
pub(crate) fn ta(tr: &I18n, key: &str, fallback: &str, args: &[(&str, &str)]) -> String {
    if tr.has(key) {
        tr.t_args(key, args)
    } else {
        let mut s = fallback.to_string();
        for (k, v) in args {
            s = s.replace(&format!("{{{{{k}}}}}"), v);
        }
        s
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Personal,
    Project,
    Shared,
    CodexOnly,
    HasScripts,
    NeedsAttention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    Name,
    Recent,
}

/// Does `s` pass the filter and the search text?
pub fn matches(s: &Skill, filter: Filter, query: &str) -> bool {
    let ok = match filter {
        Filter::All => true,
        Filter::Personal => s.scope == Scope::User,
        Filter::Project => s.scope == Scope::Project,
        Filter::Shared => s.kind.is_shared(),
        Filter::CodexOnly => s.kind.is_codex_only(),
        Filter::HasScripts => s.has_scripts,
        Filter::NeedsAttention => s.needs_attention(),
    };
    if !ok {
        return false;
    }
    let q = query.trim().to_lowercase();
    q.is_empty()
        || s.display_name().to_lowercase().contains(&q)
        || s.folder_name.to_lowercase().contains(&q)
        || s.meta
            .description
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains(&q)
        || s.root_label.to_lowercase().contains(&q)
}

enum Stash {
    Editor(Box<SkillEditor>),
    Import(Box<SkillImportDialog>),
}

enum Modal {
    None,
    Detail(Box<SkillDetail>),
    Editor(Box<SkillEditor>),
    Import(Box<SkillImportDialog>),
    Review {
        dlg: Box<ReviewDialog>,
        origin: Stash,
        success: &'static str,
    },
}

pub struct SkillsPanel {
    pub query: String,
    pub filter: Filter,
    pub sort: Sort,
    pub catalog: Catalog,
    /// Roots are fixed (tests / embedding) instead of derived from `$HOME` and the open team.
    fixed: bool,
    pub user_home: Option<PathBuf>,
    pub project: Option<PathBuf>,
    loaded: bool,
    modal: Modal,
    watcher: Option<SkillWatcher>,
    watched_for: Option<(Option<PathBuf>, Option<PathBuf>)>,
    notice: Option<String>,
}

impl Default for SkillsPanel {
    fn default() -> Self {
        Self {
            query: String::new(),
            filter: Filter::All,
            sort: Sort::Name,
            catalog: Catalog::default(),
            fixed: false,
            user_home: ext_skills::default_user_home(),
            project: None,
            loaded: false,
            modal: Modal::None,
            watcher: None,
            watched_for: None,
            notice: None,
        }
    }
}

impl SkillsPanel {
    /// Panel with explicit roots; never reads the real home directory.
    #[cfg(test)]
    pub fn with_roots(user_home: Option<PathBuf>, project: Option<PathBuf>) -> Self {
        Self {
            fixed: true,
            user_home,
            project,
            ..Default::default()
        }
    }

    pub fn roots(&self) -> Vec<SkillRoot> {
        roots::skill_roots(self.user_home.as_deref(), self.project.as_deref())
    }

    pub fn refresh(&mut self) {
        self.catalog = ext_skills::scan_all(self.user_home.as_deref(), self.project.as_deref());
        self.loaded = true;
        let key = (self.user_home.clone(), self.project.clone());
        if self.watched_for.as_ref() != Some(&key) {
            let dirs: Vec<PathBuf> = self.roots().into_iter().map(|r| r.dir).collect();
            self.watcher = SkillWatcher::start(&dirs).ok();
            self.watched_for = Some(key);
        }
    }

    /// Visible skills after filter, search and sort.
    pub fn visible(&self) -> Vec<&Skill> {
        let mut v: Vec<&Skill> = self
            .catalog
            .skills
            .iter()
            .filter(|s| matches(s, self.filter, &self.query))
            .collect();
        if self.sort == Sort::Recent {
            v.sort_by_key(|s| std::cmp::Reverse(s.modified_ms));
        }
        v
    }

    fn storage_choices(&self, tr: &I18n) -> Vec<(String, SkillRoot)> {
        let project = self
            .project
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        self.roots()
            .into_iter()
            .filter(|r| !r.read_only)
            .map(|r| {
                let scope = match (r.scope, &project) {
                    (Scope::User, _) => t(tr, "extensions.skillEditor.scope.user", "User"),
                    (Scope::Project, Some(p)) => ta(
                        tr,
                        "extensions.skillEditor.scope.project",
                        "Project: {{project}}",
                        &[("project", p)],
                    ),
                    (Scope::Project, None) => t(
                        tr,
                        "extensions.skillEditor.scope.projectUnavailable",
                        "Project unavailable",
                    ),
                };
                let suffix = if r.kind.is_codex_only() {
                    t(tr, "extensions.skillEditor.root.codexOnly", " - Codex only")
                } else if r.kind.is_shared() && r.kind != ext_skills::RootKind::Claude {
                    t(tr, "extensions.skillEditor.root.shared", " - Shared")
                } else {
                    String::new()
                };
                (format!("{scope} ({}){suffix}", r.label), r)
            })
            .collect()
    }

    pub fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        if !self.fixed {
            let p = c
                .view
                .and_then(|v| v.team())
                .and_then(|t| t.project_path.as_deref())
                .filter(|p| !p.trim().is_empty())
                .map(PathBuf::from);
            if p != self.project {
                self.project = p;
                self.loaded = false;
            }
        }
        if !self.loaded {
            self.refresh();
        }
        if let Some(w) = &mut self.watcher {
            if w.poll() {
                self.refresh();
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(500));
        }

        self.toolbar(ui, c);
        if let Some(n) = &self.notice {
            ui.colored_label(theme::SUCCESS, n);
        }
        ui.add_space(4.0);
        self.list(ui, c);
        self.dialogs(ui.ctx(), c);
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        let tr = c.tr;
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text(t(
                        tr,
                        "extensions.skillsPanel.searchPlaceholder",
                        "Search by skill name or what it helps with...",
                    ))
                    .desired_width(320.0),
            );
            egui::ComboBox::from_id_salt("skills_sort")
                .selected_text(match self.sort {
                    Sort::Name => t(tr, "extensions.skillsPanel.sort.name", "Name"),
                    Sort::Recent => t(tr, "extensions.skillsPanel.sort.recent", "Recent"),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.sort,
                        Sort::Name,
                        t(tr, "extensions.skillsPanel.sort.name", "Name"),
                    );
                    ui.selectable_value(
                        &mut self.sort,
                        Sort::Recent,
                        t(tr, "extensions.skillsPanel.sort.recent", "Recent"),
                    );
                });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::solid(
                    ui,
                    &t(
                        tr,
                        "extensions.skillsPanel.actions.createSkill",
                        "Create Skill",
                    ),
                    theme::ACCENT,
                )
                .clicked()
                {
                    let choices = self.storage_choices(tr);
                    self.modal = Modal::Editor(Box::new(SkillEditor::new_create(choices)));
                }
                if ui
                    .button(t(tr, "extensions.skillsPanel.actions.import", "Import"))
                    .clicked()
                {
                    let choices = self.storage_choices(tr);
                    self.modal = Modal::Import(Box::new(SkillImportDialog::new(choices)));
                }
            });
        });
        ui.horizontal_wrapped(|ui| {
            for (f, key, fb) in [
                (Filter::All, "all", "All skills"),
                (Filter::Personal, "personal", "Personal"),
                (Filter::Project, "project", "Project"),
                (Filter::Shared, "shared", "Shared"),
                (Filter::CodexOnly, "codexOnly", "Codex only"),
                (Filter::HasScripts, "hasScripts", "Has scripts"),
                (Filter::NeedsAttention, "needsAttention", "Needs attention"),
            ] {
                ui.selectable_value(
                    &mut self.filter,
                    f,
                    t(tr, &format!("extensions.skillsPanel.filters.{key}"), fb),
                );
            }
        });
    }

    fn list(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        let tr = c.tr;
        let visible: Vec<Skill> = self.visible().into_iter().cloned().collect();
        let mut open: Option<Skill> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if self.catalog.skills.is_empty() {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(t(tr, "extensions.skillsPanel.empty.noSkills", "No skills yet")).size(16.0).strong());
                    ui.label(RichText::new(t(tr, "extensions.skillsPanel.empty.noSkillsDescription", "Create your first skill to teach a repeatable workflow, or import one you already use.")).color(theme::MUTED));
                });
            } else if visible.is_empty() {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(t(tr, "extensions.skillsPanel.empty.noMatches", "No skills match your search")).size(16.0).strong());
                    ui.label(RichText::new(t(tr, "extensions.skillsPanel.empty.noMatchesDescription", "Try a different search term or switch filters.")).color(theme::MUTED));
                });
            }
            for (scope, key, fb_title, fb_desc) in [
                (Scope::User, "personal", "Personal skills", "Habits and instructions you want available everywhere."),
                (Scope::Project, "project", "Project skills", "Workflows that only make sense for this codebase."),
            ] {
                let items: Vec<&Skill> = visible.iter().filter(|s| s.scope == scope).collect();
                if items.is_empty() {
                    continue;
                }
                theme::section(
                    ui,
                    &format!("skills_{key}"),
                    "📁",
                    &t(tr, &format!("extensions.skillsPanel.sections.{key}.title"), fb_title),
                    Some(items.len()),
                    true,
                    |ui| {
                        ui.label(RichText::new(t(tr, &format!("extensions.skillsPanel.sections.{key}.description"), fb_desc)).small().color(theme::MUTED));
                    },
                    |ui| {
                        for s in items {
                            if skill_row(ui, s, tr).clicked() {
                                open = Some(s.clone());
                            }
                        }
                    },
                );
            }
            if !self.catalog.agents.is_empty() && self.filter == Filter::All && self.query.trim().is_empty() {
                theme::section(ui, "skills_agents", "📄", "Agents", Some(self.catalog.agents.len()), false, |_| {}, |ui| {
                    for a in &self.catalog.agents {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&a.name).strong());
                            if let Some(m) = &a.model {
                                theme::pill(ui, m.clone(), theme::INFO);
                            }
                            ui.label(RichText::new(&a.description).small().color(theme::MUTED));
                        });
                    }
                });
            }
        });
        if let Some(s) = open {
            self.modal = Modal::Detail(Box::new(SkillDetail::new(s)));
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context, c: &mut FeatureCtx) {
        let tr = c.tr;
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        self.modal = match modal {
            Modal::None => Modal::None,
            Modal::Detail(mut d) => match d.show(ctx, tr) {
                DetailAction::None => Modal::Detail(d),
                DetailAction::Close => Modal::None,
                DetailAction::Edit => Modal::Editor(Box::new(SkillEditor::new_edit(&d.skill))),
                DetailAction::OpenFolder => {
                    let _ = open::that(&d.skill.dir);
                    Modal::Detail(d)
                }
                DetailAction::OpenFile => {
                    c.req.push(ShellRequest::OpenEditor {
                        root: d.skill.dir.clone(),
                        file: Some(d.skill.skill_file.clone()),
                    });
                    Modal::None
                }
                DetailAction::Delete => {
                    match ext_skills::delete_skill(&self.roots(), &d.skill.dir) {
                        Ok(()) => {
                            self.notice = None;
                            self.refresh();
                        }
                        Err(e) => c.req.push(ShellRequest::Toast(format!(
                            "{}: {e}",
                            t(
                                tr,
                                "extensions.skillDetail.errors.deleteFailed",
                                "Failed to delete skill"
                            )
                        ))),
                    }
                    Modal::None
                }
            },
            Modal::Editor(mut e) => match e.show(ctx, tr) {
                EditorAction::None => Modal::Editor(e),
                EditorAction::Cancel => Modal::None,
                EditorAction::Review(plan, label) => {
                    let (confirm, success) = if e.mode == super::skill_editor::Mode::Edit {
                        (
                            t(tr, "extensions.skillEditor.actions.saveSkill", "Save Skill"),
                            "extensions.skillsPanel.success.saved",
                        )
                    } else {
                        (
                            t(
                                tr,
                                "extensions.skillEditor.actions.createSkill",
                                "Create Skill",
                            ),
                            "extensions.skillsPanel.success.created",
                        )
                    };
                    Modal::Review {
                        dlg: Box::new(ReviewDialog::new(plan, label, confirm)),
                        origin: Stash::Editor(e),
                        success,
                    }
                }
            },
            Modal::Import(mut d) => match d.show(ctx, tr) {
                ImportAction::None => Modal::Import(d),
                ImportAction::Cancel => Modal::None,
                ImportAction::Review(plan, label) => Modal::Review {
                    dlg: Box::new(ReviewDialog::new(
                        plan,
                        label,
                        t(
                            tr,
                            "extensions.skillImport.actions.importSkill",
                            "Import Skill",
                        ),
                    )),
                    origin: Stash::Import(d),
                    success: "extensions.skillsPanel.success.imported",
                },
            },
            Modal::Review {
                mut dlg,
                origin,
                success,
            } => match dlg.show(ctx, tr) {
                ReviewAction::None => Modal::Review {
                    dlg,
                    origin,
                    success,
                },
                ReviewAction::Cancel => Modal::None,
                ReviewAction::Back => match origin {
                    Stash::Editor(e) => Modal::Editor(e),
                    Stash::Import(i) => Modal::Import(i),
                },
                ReviewAction::Apply => match dlg.plan.apply() {
                    Ok(()) => {
                        let fb = match success {
                            "extensions.skillsPanel.success.saved" => "Skill saved successfully.",
                            "extensions.skillsPanel.success.imported" => {
                                "Skill imported successfully."
                            }
                            _ => "Skill created successfully.",
                        };
                        let msg = t(tr, success, fb);
                        c.req.push(ShellRequest::Toast(msg.clone()));
                        self.notice = Some(msg);
                        self.refresh();
                        Modal::None
                    }
                    Err(e) => {
                        dlg.error = Some(e);
                        Modal::Review {
                            dlg,
                            origin,
                            success,
                        }
                    }
                },
            },
        };
    }
}

fn skill_row(ui: &mut egui::Ui, s: &Skill, tr: &I18n) -> egui::Response {
    let r = egui::Frame::new()
        .fill(theme::CARD)
        .stroke(egui::Stroke::new(1.0, theme::hairline(0.07)))
        .corner_radius(8)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(s.display_name()).strong().size(14.0));
                theme::pill(
                    ui,
                    ta(
                        tr,
                        "extensions.skillsPanel.badges.storedIn",
                        "Stored in {{root}}",
                        &[("root", &s.root_label)],
                    ),
                    theme::ACCENT,
                );
                if s.has_scripts {
                    theme::pill(
                        ui,
                        t(
                            tr,
                            "extensions.skillsPanel.badges.hasScripts",
                            "Has scripts",
                        ),
                        theme::WARNING,
                    );
                }
                if s.has_references {
                    theme::pill(
                        ui,
                        t(tr, "extensions.skillsPanel.badges.references", "References"),
                        theme::INFO,
                    );
                }
                if s.has_assets {
                    theme::pill(
                        ui,
                        t(tr, "extensions.skillsPanel.badges.assets", "Assets"),
                        theme::INFO,
                    );
                }
                if s.needs_attention() {
                    theme::pill(
                        ui,
                        t(
                            tr,
                            "extensions.skillsPanel.badges.needsAttention",
                            "Needs attention",
                        ),
                        theme::ERROR,
                    );
                }
            });
            if let Some(d) = s.meta.description.as_deref().filter(|d| !d.is_empty()) {
                ui.label(
                    RichText::new(d.chars().take(240).collect::<String>()).color(theme::TEXT_2),
                );
            }
            ui.label(
                RichText::new(if s.manual_only() {
                    t(
                        tr,
                        "extensions.skillsPanel.invocation.manualOnly",
                        "Only runs when you explicitly ask for it",
                    )
                } else {
                    t(
                        tr,
                        "extensions.skillsPanel.invocation.auto",
                        "Runs automatically when it fits",
                    )
                })
                .small()
                .color(theme::MUTED),
            );
        })
        .response;
    let r = r.interact(egui::Sense::click());
    ui.add_space(4.0);
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::Intents;

    fn write_skill(root: &std::path::Path, folder: &str, name: &str, desc: &str, scripts: bool) {
        let d = root.join(folder);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {desc}\n---\n# Body\n\nHello **world**\n"),
        )
        .unwrap();
        if scripts {
            std::fs::create_dir_all(d.join("scripts")).unwrap();
            std::fs::write(d.join("scripts/a.sh"), "echo").unwrap();
        }
    }

    fn fixture() -> (tempfile::TempDir, tempfile::TempDir) {
        let h = tempfile::tempdir().unwrap();
        let p = tempfile::tempdir().unwrap();
        write_skill(
            &h.path().join(".claude/skills"),
            "alpha",
            "alpha",
            "Reviews code",
            true,
        );
        write_skill(
            &h.path().join(".codex/skills"),
            "cdx",
            "cdx",
            "Codex helper",
            false,
        );
        write_skill(
            &p.path().join(".claude/skills"),
            "proj",
            "Bad Name",
            "Project thing",
            false,
        );
        (h, p)
    }

    #[test]
    fn filters_and_search() {
        let (h, p) = fixture();
        let mut panel = SkillsPanel::with_roots(Some(h.path().into()), Some(p.path().into()));
        panel.refresh();
        assert_eq!(panel.catalog.skills.len(), 3);
        let names = |pn: &SkillsPanel| {
            pn.visible()
                .iter()
                .map(|s| s.folder_name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&panel), vec!["alpha", "proj", "cdx"]);
        panel.filter = Filter::Project;
        assert_eq!(names(&panel), vec!["proj"]);
        panel.filter = Filter::CodexOnly;
        assert_eq!(names(&panel), vec!["cdx"]);
        panel.filter = Filter::HasScripts;
        assert_eq!(names(&panel), vec!["alpha"]);
        panel.filter = Filter::NeedsAttention;
        assert_eq!(names(&panel), vec!["proj"]);
        panel.filter = Filter::All;
        panel.query = "REVIEWS".into();
        assert_eq!(names(&panel), vec!["alpha"]);
        panel.query = "zzz".into();
        assert!(names(&panel).is_empty());
    }

    fn render_panel(panel: &mut SkillsPanel) {
        let home = std::path::PathBuf::from("/nonexistent-rteams-home");
        let tr = I18n::english();
        crate::view::testing::render(|ui| {
            let mut out = Intents::default();
            let mut req = vec![];
            let mut c = FeatureCtx {
                home: &home,
                team: None,
                view: None,
                out: &mut out,
                tr: &tr,
                req: &mut req,
            };
            panel.show(ui, &mut c);
        });
    }

    #[test]
    fn renders_headless_with_all_dialogs() {
        let (h, p) = fixture();
        let mut panel = SkillsPanel::with_roots(Some(h.path().into()), Some(p.path().into()));
        render_panel(&mut panel);
        let tr = I18n::english();
        // detail
        let s = panel.catalog.skills[0].clone();
        panel.modal = Modal::Detail(Box::new(SkillDetail::new(s.clone())));
        render_panel(&mut panel);
        // editor (edit + create)
        panel.modal = Modal::Editor(Box::new(SkillEditor::new_edit(&s)));
        render_panel(&mut panel);
        let choices = panel.storage_choices(&tr);
        assert!(choices.len() >= 8 && choices.iter().all(|(_, r)| !r.read_only));
        let mut ed = SkillEditor::new_create(choices.clone());
        ed.draft.name = "My New Skill".into();
        ed.draft.description = "Does things".into();
        panel.modal = Modal::Editor(Box::new(ed));
        render_panel(&mut panel);
        // import
        panel.modal = Modal::Import(Box::new(SkillImportDialog::new(choices)));
        render_panel(&mut panel);
        // review
        let plan = ext_skills::scaffold::plan_create(
            &h.path().join(".claude/skills"),
            "new-one",
            &ext_skills::scaffold::Draft {
                name: "new-one".into(),
                description: "d".into(),
                extra_dirs: vec!["scripts".into()],
                ..Default::default()
            },
        )
        .unwrap();
        let dlg = ReviewDialog::new(plan, "Creating a skill", "Create Skill");
        assert!(!dlg.findings.is_empty());
        panel.modal = Modal::Review {
            dlg: Box::new(dlg),
            origin: Stash::Editor(Box::new(SkillEditor::new_create(vec![]))),
            success: "extensions.skillsPanel.success.created",
        };
        render_panel(&mut panel);
    }

    #[test]
    fn editor_create_flow_validates_and_plans() {
        let (h, p) = fixture();
        let panel = SkillsPanel::with_roots(Some(h.path().into()), Some(p.path().into()));
        let choices = panel.storage_choices(&I18n::english());
        let mut ed = SkillEditor::new_create(choices);
        assert!(!ed.can_review());
        ed.draft.name = "fresh-skill".into();
        ed.draft.description = "Fresh".into();
        ed.folder = "fresh-skill".into();
        assert!(ed.can_review());
        let plan = ed.build_plan().unwrap();
        plan.apply().unwrap();
        assert!(h
            .path()
            .join(".claude/skills/fresh-skill/SKILL.md")
            .exists());
        // duplicate folder is refused
        assert!(ed.build_plan().is_err());
    }

    #[test]
    fn editor_edit_flow_diffs_only_skill_file() {
        let (h, p) = fixture();
        let mut panel = SkillsPanel::with_roots(Some(h.path().into()), Some(p.path().into()));
        panel.refresh();
        let s = panel
            .catalog
            .skills
            .iter()
            .find(|s| s.folder_name == "alpha")
            .unwrap()
            .clone();
        let mut ed = SkillEditor::new_edit(&s);
        assert!(ed.can_review());
        ed.draft.description = "Changed description".into();
        ed.draft.manual_only = true;
        ed.apply_fields_to_raw();
        let plan = ed.build_plan().unwrap();
        assert_eq!(plan.change_count(), 1);
        assert!(plan.changes[0]
            .diff
            .contains("+description: Changed description"));
        assert!(plan.changes[0]
            .diff
            .contains("+disable-model-invocation: true"));
        plan.apply().unwrap();
        panel.refresh();
        let s = panel
            .catalog
            .skills
            .iter()
            .find(|s| s.folder_name == "alpha")
            .unwrap();
        assert!(s.manual_only() && s.has_scripts);
    }
}
