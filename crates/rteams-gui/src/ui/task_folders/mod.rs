//! task_folders: side panel grouping kanban tasks into user folders (nested, drag-and-drop).
//! Folders only organize; they never change a task's status, owner or team. The board asks
//! `folder_filter()` which task ids to show.

use std::collections::HashSet;

use eframe::egui::{self, RichText};
use rteams_services::task_folders::{FolderError, FolderStore};

use super::close_guard::tx;
use super::FeatureCtx;
use crate::theme;

pub mod dialogs;
pub mod dnd;
pub mod tree;

use dialogs::{Dialog, Outcome};
use tree::{MenuAction, Sel, TaskRow, TreeCtx, TreeEvent};

#[allow(unused_imports)]
pub use dnd::{make_task_draggable, DragItem};

#[derive(Default)]
pub struct TaskFoldersPanel {
    team: Option<String>,
    store: Option<FolderStore>,
    sel: Option<Sel>,
    dialog: Option<Dialog>,
    error: Option<String>,
    show_archived: bool,
    unfiled: HashSet<String>,
}

impl TaskFoldersPanel {
    /// Task ids in the selected folder (and its subfolders), or the unfiled ones; `None` when
    /// no folder is selected (the board shows everything).
    pub fn folder_filter(&self) -> Option<HashSet<String>> {
        match (&self.sel, &self.store) {
            (Some(Sel::Folder(id)), Some(s)) if s.data.folder(id).is_some() => {
                Some(s.data.tasks_in(id, true))
            }
            (Some(Sel::Unfiled), Some(_)) => Some(self.unfiled.clone()),
            _ => None,
        }
    }

    fn sync_team(&mut self, team: Option<&str>, home: &std::path::Path) {
        if self.team.as_deref() != team || self.store.is_none() {
            self.team = team.map(String::from);
            self.store = team.map(|t| FolderStore::open(home, t));
            self.sel = None;
            self.dialog = None;
            self.error = None;
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        self.sync_team(c.team, c.home);
        let tr = c.tr;
        ui.label(RichText::new(tx(tr, "common.taskFolders.title", "Task folders")).strong());
        let Some(store) = self.store.as_mut() else {
            return;
        };
        let tasks: Vec<TaskRow> = c
            .view
            .map(|v| {
                v.state
                    .tasks
                    .iter()
                    .map(|t| TaskRow {
                        id: t.id.clone(),
                        subject: t.subject.clone(),
                        archived: t.archived,
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.unfiled = tasks
            .iter()
            .filter(|t| {
                store
                    .data
                    .folder_of(&t.id)
                    .is_none_or(|f| store.data.folder(f).is_none())
            })
            .map(|t| t.id.clone())
            .collect();
        ui.label(
            RichText::new(tx(
                tr,
                "common.taskFolders.organizationDescription",
                "Organize tasks without changing their team, project or status.",
            ))
            .small()
            .color(theme::MUTED),
        );
        if store.read_only {
            ui.label(
                RichText::new(tx(
                    tr,
                    "common.taskFolders.readOnly",
                    "Folder storage is unavailable or invalid. Organization is read only.",
                ))
                .color(theme::WARNING),
            );
            if ui
                .button(tx(tr, "common.taskFolders.retry", "Retry"))
                .clicked()
            {
                store.reload();
            }
        }
        if let Some(e) = &self.error {
            ui.label(RichText::new(e).small().color(theme::ERROR_FG));
        }
        let ro = store.read_only;
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!ro, |ui| {
                if ui
                    .button(tx(tr, "common.taskFolders.newFolder", "New folder"))
                    .clicked()
                {
                    self.dialog = Some(Dialog::NewFolder {
                        parent: None,
                        name: String::new(),
                    });
                }
                if ui
                    .button(tx(
                        tr,
                        "common.taskFolders.reset",
                        "Reset folder organization",
                    ))
                    .clicked()
                {
                    self.dialog = Some(Dialog::Reset);
                }
            });
        });
        ui.checkbox(
            &mut self.show_archived,
            tx(tr, "common.taskFolders.showArchived", "Show archived tasks"),
        );
        ui.separator();
        let events = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let cx = TreeCtx {
                    data: &store.data,
                    tasks: &tasks,
                    sel: &self.sel,
                    show_archived: self.show_archived,
                    read_only: ro,
                    tr,
                };
                tree::show(ui, &cx, &self.unfiled)
            })
            .inner;
        for e in events {
            match e {
                TreeEvent::Select(s) => self.sel = s,
                TreeEvent::Drop { item, target } => {
                    let r = dnd::apply_drop(store, &item, target.as_deref());
                    self.error = r.err().map(|e| e.to_string());
                }
                TreeEvent::Menu(m) => {
                    self.dialog = Some(match m {
                        MenuAction::NewFolder(parent) => Dialog::NewFolder {
                            parent,
                            name: String::new(),
                        },
                        MenuAction::Rename(id) => {
                            let name = store
                                .data
                                .folder(&id)
                                .map(|f| f.name.clone())
                                .unwrap_or_default();
                            Dialog::Rename { id, name }
                        }
                        MenuAction::Delete(id) => Dialog::Delete { id },
                        MenuAction::Move(item) => {
                            let target = match &item {
                                DragItem::Task(t) => store.data.folder_of(t).map(String::from),
                                DragItem::Folder(f) => {
                                    store.data.folder(f).and_then(|f| f.parent.clone())
                                }
                            };
                            Dialog::Move { item, target }
                        }
                    });
                }
            }
        }
        if let Some(d) = self.dialog.as_mut() {
            match dialogs::show(ui.ctx(), d, &store.data, tr, self.error.as_deref()) {
                Outcome::Open => {}
                Outcome::Cancel => self.dialog = None,
                Outcome::Submit => {
                    let d = self.dialog.take().unwrap();
                    match submit(store, &d) {
                        Ok(()) => self.error = None,
                        Err(e) => {
                            // Keep the dialog open so the user can fix the input; a failed save
                            // leaves the last good file untouched.
                            self.error = Some(e.to_string());
                            self.dialog = Some(d);
                        }
                    }
                    if let Some(Sel::Folder(id)) = &self.sel {
                        if store.data.folder(id).is_none() {
                            self.sel = None;
                        }
                    }
                }
            }
        }
    }
}

/// Apply a confirmed dialog to the store.
pub fn submit(store: &mut FolderStore, d: &Dialog) -> Result<(), FolderError> {
    match d {
        Dialog::NewFolder { parent, name } => store.create(name, parent.as_deref()).map(|_| ()),
        Dialog::Rename { id, name } => store.rename(id, name),
        Dialog::Delete { id } => store.delete(id),
        Dialog::Reset => store.reset(),
        Dialog::Move { item, target } => dnd::apply_drop(store, item, target.as_deref()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submit_dialogs_and_filter() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = TaskFoldersPanel::default();
        p.sync_team(Some("t"), dir.path());
        let s = p.store.as_mut().unwrap();
        submit(
            s,
            &Dialog::NewFolder {
                parent: None,
                name: "A".into(),
            },
        )
        .unwrap();
        let a = s.data.folders[0].id.clone();
        submit(
            s,
            &Dialog::NewFolder {
                parent: Some(a.clone()),
                name: "B".into(),
            },
        )
        .unwrap();
        let b = s.data.folders[1].id.clone();
        s.assign("t1", Some(&b)).unwrap();
        assert!(p.folder_filter().is_none());
        p.sel = Some(Sel::Folder(a.clone()));
        assert!(p.folder_filter().unwrap().contains("t1"));
        p.unfiled = HashSet::from(["u".to_string()]);
        p.sel = Some(Sel::Unfiled);
        assert_eq!(p.folder_filter().unwrap().len(), 1);
        let s = p.store.as_mut().unwrap();
        assert!(submit(
            s,
            &Dialog::Rename {
                id: a,
                name: "".into()
            }
        )
        .is_err());
        submit(s, &Dialog::Reset).unwrap();
        assert!(s.data.folders.is_empty());
    }

    #[test]
    fn headless_tree_render() {
        use rteams_services::i18n::I18n;
        let dir = tempfile::tempdir().unwrap();
        let mut store = FolderStore::open(dir.path(), "t");
        let a = store.create("A", None).unwrap();
        let b = store.create("B", Some(&a)).unwrap();
        store.assign("t1", Some(&b)).unwrap();
        let tasks = vec![
            TaskRow {
                id: "t1".into(),
                subject: "one".into(),
                archived: false,
            },
            TaskRow {
                id: "t2".into(),
                subject: "two".into(),
                archived: true,
            },
        ];
        let tr = I18n::english();
        let ctx = egui::Context::default();
        let sel = Some(Sel::Folder(a));
        let unfiled: HashSet<String> = ["t2".to_string()].into();
        crate::ui::close_guard::headless(&ctx, |ui| {
            let cx = TreeCtx {
                data: &store.data,
                tasks: &tasks,
                sel: &sel,
                show_archived: true,
                read_only: false,
                tr: &tr,
            };
            let ev = tree::show(ui, &cx, &unfiled);
            assert!(ev.is_empty());
        });
    }
}
