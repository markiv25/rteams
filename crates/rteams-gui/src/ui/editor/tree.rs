//! File tree (left sidebar): lazy children, git decorations, context menu, drag-move.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use eframe::egui;
use rteams_services::editor::files::{self, TreeEntry};
use rteams_services::editor::gitstatus::{GitMark, GitStatus};
use rteams_services::i18n::I18n;

use crate::theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeAction {
    Open(String),
    NewFile(String),
    NewFolder(String),
    Rename(String),
    Delete {
        rel: String,
        is_dir: bool,
    },
    Reveal(String),
    /// Move `from` into folder `to_dir` ("" = project root).
    Move {
        from: String,
        to_dir: String,
    },
    /// A folder was expanded (the screen starts watching it).
    Expanded(String),
}

#[derive(Debug, Default)]
pub struct TreeState {
    pub children: HashMap<String, Vec<TreeEntry>>,
    pub expanded: HashSet<String>,
    pub selected: Option<String>,
    pub error: Option<String>,
    pub loaded_root: bool,
}

/// Icon glyph by file type (all glyphs exist in egui's bundled fonts).
pub fn icon_for(name: &str, is_dir: bool, open: bool) -> &'static str {
    if is_dir {
        return if open { "📂" } else { "📁" };
    }
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" => "🖼",
        "md" | "markdown" | "txt" | "rst" => "📋",
        "json" | "toml" | "yaml" | "yml" | "ini" | "lock" | "env" => "🔧",
        "rs" | "js" | "ts" | "tsx" | "jsx" | "py" | "go" | "java" | "c" | "h" | "cpp" | "rb"
        | "sh" | "html" | "css" | "swift" | "kt" => "📜",
        _ => "📄",
    }
}

fn mark_style(m: GitMark) -> (&'static str, egui::Color32) {
    match m {
        GitMark::Modified => ("M", theme::WARNING),
        GitMark::Added => ("A", theme::SUCCESS),
        GitMark::Deleted => ("D", theme::ERROR),
        GitMark::Renamed => ("R", theme::INFO),
        GitMark::Untracked => ("U", theme::SUCCESS),
        GitMark::Conflicted => ("!", theme::ERROR),
    }
}

impl TreeState {
    /// (Re)load one directory's children.
    pub fn load(&mut self, root: &Path, rel: &str) {
        match files::list_dir(root, Path::new(rel)) {
            Ok(v) => {
                self.children.insert(rel.to_string(), v);
                if rel.is_empty() {
                    self.loaded_root = true;
                    self.error = None;
                }
            }
            Err(e) => {
                if rel.is_empty() {
                    self.error = Some(e.to_string());
                } else {
                    self.children.remove(rel);
                }
            }
        }
    }

    /// Reload every loaded directory (after create/rename/delete or watcher events).
    pub fn reload_all(&mut self, root: &Path) {
        let dirs: Vec<String> = self.children.keys().cloned().collect();
        for d in dirs {
            self.load(root, &d);
        }
        // forget expanded folders that vanished
        let known: HashSet<&String> = self.children.keys().collect();
        self.expanded
            .retain(|e| known.contains(e) || self.children.contains_key(parent_of(e)));
    }

    pub fn expand(&mut self, root: &Path, rel: &str) {
        if self.expanded.insert(rel.to_string()) && !self.children.contains_key(rel) {
            self.load(root, rel);
        }
    }

    /// Expand every ancestor folder of `rel` and select it.
    pub fn reveal(&mut self, root: &Path, rel: &str) {
        let mut acc = String::new();
        let parts: Vec<&str> = rel.split('/').collect();
        for p in &parts[..parts.len().saturating_sub(1)] {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(p);
            self.expand(root, &acc);
        }
        self.selected = Some(rel.to_string());
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        root: &Path,
        git: &GitStatus,
        tr: &I18n,
    ) -> Vec<TreeAction> {
        let mut acts = Vec::new();
        if let Some(e) = &self.error {
            ui.colored_label(
                theme::ERROR,
                tr.t_args("team.editor.fileTree.failedToLoadFiles", &[("error", e)]),
            );
            return acts;
        }
        if !self.loaded_root {
            for w in [0.7, 0.5, 0.8, 0.4] {
                crate::ui::dashboard::kit::skeleton(ui, ui.available_width() * w, 14.0);
            }
            ui.label(egui::RichText::new(tr.t("team.editor.fileTree.loading")).color(theme::MUTED));
            return acts;
        }
        if self.children.get("").is_none_or(|v| v.is_empty()) {
            ui.label(egui::RichText::new(tr.t("team.editor.fileTree.empty")).color(theme::MUTED));
        }
        let dragging = egui::DragAndDrop::has_payload_of_type::<String>(ui.ctx());
        if dragging {
            let r = ui.add(
                egui::Label::new(
                    egui::RichText::new(tr.t("team.editor.fileTree.dropForProjectRoot"))
                        .color(theme::ACCENT),
                )
                .sense(egui::Sense::hover()),
            );
            if let Some(p) = r.dnd_release_payload::<String>() {
                acts.push(TreeAction::Move {
                    from: (*p).clone(),
                    to_dir: String::new(),
                });
            }
        }
        self.show_dir(ui, root, "", 0, git, tr, &mut acts);
        // Context menu on empty space: operate on the root.
        let rest = ui.available_rect_before_wrap();
        let bg = ui.interact(rest, ui.scope_id().with("tree-bg"), egui::Sense::click());
        bg.context_menu(|ui| root_menu(ui, tr, &mut acts));
        acts
    }

    #[allow(clippy::too_many_arguments)]
    fn show_dir(
        &mut self,
        ui: &mut egui::Ui,
        root: &Path,
        rel: &str,
        depth: usize,
        git: &GitStatus,
        tr: &I18n,
        acts: &mut Vec<TreeAction>,
    ) {
        let entries = self.children.get(rel).cloned().unwrap_or_default();
        for e in entries {
            let open = e.is_dir && self.expanded.contains(&e.rel);
            let selected = self.selected.as_deref() == Some(e.rel.as_str());
            let g = git.get(&e.rel);
            let dir_dirty = e.is_dir && git.dir_dirty(&e.rel);
            let resp = ui
                .horizontal(|ui| {
                    ui.add_space(depth as f32 * 14.0 + if e.is_dir { 0.0 } else { 14.0 });
                    if e.is_dir {
                        ui.label(
                            egui::RichText::new(if open { "⏷" } else { "⏵" }).color(theme::MUTED),
                        );
                    }
                    let color = match g.map(|g| g.mark) {
                        Some(GitMark::Untracked) => theme::SUCCESS,
                        Some(GitMark::Deleted | GitMark::Conflicted) => theme::ERROR,
                        Some(_) => theme::WARNING,
                        None if dir_dirty => theme::WARNING,
                        None => theme::TEXT,
                    };
                    let text = format!("{} {}", icon_for(&e.name, e.is_dir, open), e.name);
                    let r = ui.add(
                        egui::Button::selectable(selected, egui::RichText::new(text).color(color))
                            .frame_when_inactive(false)
                            .sense(egui::Sense::click_and_drag()),
                    );
                    if let Some(g) = g {
                        let (s, c) = mark_style(g.mark);
                        ui.label(
                            egui::RichText::new(if g.staged {
                                format!("{s}+")
                            } else {
                                s.to_string()
                            })
                            .small()
                            .color(c),
                        );
                    }
                    r
                })
                .inner;
            resp.dnd_set_drag_payload(e.rel.clone());
            if e.is_dir {
                if egui::DragAndDrop::has_payload_of_type::<String>(ui.ctx())
                    && resp.dnd_hover_payload::<String>().is_some()
                {
                    ui.painter().rect_stroke(
                        resp.rect,
                        3.0,
                        (1.0, theme::ACCENT),
                        egui::StrokeKind::Inside,
                    );
                }
                if let Some(p) = resp.dnd_release_payload::<String>() {
                    acts.push(TreeAction::Move {
                        from: (*p).clone(),
                        to_dir: e.rel.clone(),
                    });
                }
            }
            if resp.clicked() {
                self.selected = Some(e.rel.clone());
                if e.is_dir {
                    if open {
                        self.expanded.remove(&e.rel);
                    } else {
                        self.expand(root, &e.rel);
                        acts.push(TreeAction::Expanded(e.rel.clone()));
                    }
                } else {
                    acts.push(TreeAction::Open(e.rel.clone()));
                }
            }
            resp.context_menu(|ui| {
                if e.is_dir {
                    dir_menu(ui, tr, &e.rel, acts);
                }
                if ui.button(rename_label()).clicked() {
                    acts.push(TreeAction::Rename(e.rel.clone()));
                    ui.close();
                }
                if ui
                    .button(tr.t("team.editor.fileTree.moveToTrash"))
                    .clicked()
                {
                    acts.push(TreeAction::Delete {
                        rel: e.rel.clone(),
                        is_dir: e.is_dir,
                    });
                    ui.close();
                }
                if ui.button(tr.t("team.editor.showInFolder")).clicked() {
                    acts.push(TreeAction::Reveal(e.rel.clone()));
                    ui.close();
                }
            });
            if open {
                self.show_dir(ui, root, &e.rel, depth + 1, git, tr, acts);
            }
        }
    }
}

fn rename_label() -> &'static str {
    "Rename"
}

fn dir_menu(ui: &mut egui::Ui, tr: &I18n, rel: &str, acts: &mut Vec<TreeAction>) {
    if ui
        .button(
            tr.t("team.editor.newFile.placeholders.fileName")
                .trim_end_matches("..."),
        )
        .clicked()
    {
        acts.push(TreeAction::NewFile(rel.to_string()));
        ui.close();
    }
    if ui
        .button(
            tr.t("team.editor.newFile.placeholders.folderName")
                .trim_end_matches("..."),
        )
        .clicked()
    {
        acts.push(TreeAction::NewFolder(rel.to_string()));
        ui.close();
    }
    ui.separator();
}

fn root_menu(ui: &mut egui::Ui, tr: &I18n, acts: &mut Vec<TreeAction>) {
    dir_menu(ui, tr, "", acts);
}

pub fn parent_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(p, _)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_and_parents() {
        assert_eq!(icon_for("a.png", false, false), "🖼");
        assert_eq!(icon_for("src", true, true), "📂");
        assert_eq!(parent_of("a/b/c.rs"), "a/b");
        assert_eq!(parent_of("c.rs"), "");
    }

    #[test]
    fn reveal_expands_ancestors() {
        let d = crate::ui::editor::testutil::TmpDir::new();
        std::fs::create_dir_all(d.path().join("a/b")).unwrap();
        std::fs::write(d.path().join("a/b/f.txt"), "x").unwrap();
        let mut t = TreeState::default();
        t.load(d.path(), "");
        t.reveal(d.path(), "a/b/f.txt");
        assert!(t.expanded.contains("a") && t.expanded.contains("a/b"));
        assert_eq!(t.children["a/b"][0].name, "f.txt");
    }
}
