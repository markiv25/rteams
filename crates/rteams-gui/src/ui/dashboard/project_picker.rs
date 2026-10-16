//! Project path field: type a path, pick from recent projects, or Browse with the native
//! folder dialog. Shows whether the folder exists or will be created.

use eframe::egui::{self, RichText};
use rteams_services::i18n::I18n;
use rteams_services::projects::{folder, RecentProject};

use crate::theme;

/// Native folder dialog (blocking).
pub fn pick_folder(tr: &I18n) -> Option<String> {
    rfd::FileDialog::new()
        .set_title(tr.t("dashboard.recentProjects.selectFolderTitle"))
        .pick_folder()
        .map(|p| p.to_string_lossy().into_owned())
}

#[derive(Debug, Default, Clone)]
pub struct ProjectPicker {
    pub path: String,
    filter: String,
}

/// Recent projects matching `query` by label or path (case-insensitive).
pub fn matching<'a>(recent: &'a [RecentProject], query: &str) -> Vec<&'a RecentProject> {
    let q = query.trim().to_lowercase();
    recent
        .iter()
        .filter(|p| {
            q.is_empty()
                || p.label.to_lowercase().contains(&q)
                || p.path.to_lowercase().contains(&q)
        })
        .collect()
}

impl ProjectPicker {
    pub fn new(path: impl Into<String>) -> Self {
        ProjectPicker {
            path: path.into(),
            filter: String::new(),
        }
    }

    /// The trimmed path, `None` when empty.
    pub fn value(&self) -> Option<String> {
        let p = self.path.trim();
        (!p.is_empty()).then(|| p.to_string())
    }

    /// Draw the picker; returns true when the path changed this frame.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        tr: &I18n,
        salt: &str,
        recent: &[RecentProject],
    ) -> bool {
        let before = self.path.clone();
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.path)
                    .hint_text(tr.t("team.projectPath.customWorkingDirectory"))
                    .desired_width((ui.available_width() - 190.0).max(80.0)),
            );
            egui::ComboBox::from_id_salt(("project-picker", salt))
                .selected_text(tr.t("team.projectPath.selectProject"))
                .width(150.0)
                .show_ui(ui, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.filter)
                            .hint_text(tr.t("team.projectPath.searchPlaceholder")),
                    );
                    let found = matching(recent, &self.filter);
                    if found.is_empty() {
                        ui.label(RichText::new(tr.t("team.projectPath.empty")).color(theme::MUTED));
                    }
                    egui::ScrollArea::vertical()
                        .max_height(240.0)
                        .show(ui, |ui| {
                            for p in found.into_iter().take(50) {
                                let r = ui
                                    .selectable_label(self.path == p.path, &p.label)
                                    .on_hover_text(&p.path);
                                if r.clicked() {
                                    self.path = p.path.clone();
                                }
                            }
                        });
                });
            if ui.button(tr.t("team.projectPath.browse")).clicked() {
                if let Some(p) = pick_folder(tr) {
                    self.path = p;
                }
            }
        });
        if let Some(note) = self.note(tr) {
            ui.label(RichText::new(note.0).size(11.5).color(note.1));
        }
        self.path != before
    }

    fn note(&self, tr: &I18n) -> Option<(String, egui::Color32)> {
        let v = self.value()?;
        let st = folder::state(&v);
        if st.exists && !st.is_dir {
            return Some(("Project path is not a directory".into(), theme::ERROR));
        }
        if st.will_create {
            return Some((
                tr.t("team.projectPath.folder.missingCreatedOnSubmit"),
                theme::WARNING,
            ));
        }
        if let Some(b) = st.git_branch {
            return Some((format!("git: {b}"), theme::MUTED));
        }
        if !st.exists {
            return Some((tr.t("team.projectPath.createAutomatically"), theme::MUTED));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_recent_by_label_or_path() {
        let mk = |l: &str, p: &str| RecentProject {
            identity: p.into(),
            path: p.into(),
            label: l.into(),
            last_activity_ms: 1,
            providers: vec![],
            exists: true,
            team: None,
        };
        let r = vec![mk("Alpha", "/w/a"), mk("beta", "/x/b")];
        assert_eq!(matching(&r, "ALP").len(), 1);
        assert_eq!(matching(&r, "/x").len(), 1);
        assert_eq!(matching(&r, "").len(), 2);
    }

    #[test]
    fn renders_headless() {
        let tr = I18n::default();
        let mut p = ProjectPicker::new("/no/such/new/dir");
        crate::view::testing::render(|ui| {
            p.show(ui, &tr, "t", &[]);
        });
        assert_eq!(p.value().as_deref(), Some("/no/such/new/dir"));
    }
}
