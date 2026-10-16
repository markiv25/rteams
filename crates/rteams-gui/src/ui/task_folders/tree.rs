//! task_folders::tree: the folder / task tree with drag-and-drop.

use std::collections::HashSet;

use eframe::egui::{self, Id, RichText};
use rteams_services::i18n::I18n;
use rteams_services::task_folders::TaskFolders;

use super::dnd::{drop_target, DragItem};
use crate::theme;
use crate::ui::close_guard::tx;

/// A task shown in the tree.
#[derive(Debug, Clone)]
pub struct TaskRow {
    pub id: String,
    pub subject: String,
    pub archived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sel {
    Folder(String),
    Unfiled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    NewFolder(Option<String>),
    Rename(String),
    Delete(String),
    Move(DragItem),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeEvent {
    Select(Option<Sel>),
    Drop {
        item: DragItem,
        target: Option<String>,
    },
    Menu(MenuAction),
}

pub struct TreeCtx<'a> {
    pub data: &'a TaskFolders,
    pub tasks: &'a [TaskRow],
    pub sel: &'a Option<Sel>,
    pub show_archived: bool,
    pub read_only: bool,
    pub tr: &'a I18n,
}

fn short(s: &str) -> String {
    let t: String = s.chars().take(48).collect();
    if t.len() < s.len() {
        format!("{t}\u{2026}")
    } else {
        t
    }
}

fn task_row(ui: &mut egui::Ui, t: &TaskRow, cx: &TreeCtx, ev: &mut Vec<TreeEvent>) {
    let payload = DragItem::Task(t.id.clone());
    let color = if t.archived {
        theme::MUTED
    } else {
        theme::TEXT_2
    };
    let r = if cx.read_only {
        ui.label(RichText::new(short(&t.subject)).color(color))
    } else {
        ui.dnd_drag_source(Id::unique(("tf_task", &t.id)), payload.clone(), |ui| {
            ui.label(RichText::new(short(&t.subject)).color(color))
        })
        .response
    };
    let r = if t.archived {
        r.on_hover_text(tx(cx.tr, "common.taskFolders.archived", "Archived task"))
    } else {
        r
    };
    if !cx.read_only {
        r.context_menu(|ui| {
            if ui
                .button(tx(cx.tr, "common.taskFolders.move", "Move to\u{2026}"))
                .clicked()
            {
                ev.push(TreeEvent::Menu(MenuAction::Move(payload.clone())));
                ui.close();
            }
        });
    }
}

fn visible<'a>(cx: &'a TreeCtx, ids: &'a HashSet<String>) -> impl Iterator<Item = &'a TaskRow> {
    cx.tasks
        .iter()
        .filter(move |t| ids.contains(&t.id) && (cx.show_archived || !t.archived))
}

fn folder_node(ui: &mut egui::Ui, id: &str, cx: &TreeCtx, ev: &mut Vec<TreeEvent>) {
    let Some(f) = cx.data.folder(id) else { return };
    let own = cx.data.tasks_in(id, false);
    let state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        Id::unique(("tf_folder", id)),
        true,
    );
    let selected = *cx.sel == Some(Sel::Folder(id.to_string()));
    let payload = DragItem::Folder(id.to_string());
    let header = state.show_header(ui, |ui| {
        let label = |ui: &mut egui::Ui| {
            ui.selectable_label(selected, format!("\u{1F4C1} {}", short(&f.name)))
        };
        let r = if cx.read_only {
            label(ui)
        } else {
            ui.dnd_drag_source(Id::unique(("tf_fdrag", id)), payload.clone(), label)
                .inner
        };
        if r.clicked() {
            ev.push(TreeEvent::Select(if selected {
                None
            } else {
                Some(Sel::Folder(id.to_string()))
            }));
        }
        if !cx.read_only {
            if let Some(item) = drop_target(ui, &r) {
                ev.push(TreeEvent::Drop {
                    item,
                    target: Some(id.to_string()),
                });
            }
            r.context_menu(|ui| {
                let items = [
                    (
                        "common.taskFolders.newSubfolder",
                        "New subfolder",
                        MenuAction::NewFolder(Some(id.to_string())),
                    ),
                    (
                        "common.taskFolders.renameFolder",
                        "Rename folder",
                        MenuAction::Rename(id.to_string()),
                    ),
                    (
                        "common.taskFolders.move",
                        "Move to\u{2026}",
                        MenuAction::Move(DragItem::Folder(id.to_string())),
                    ),
                    (
                        "common.taskFolders.deleteFolder",
                        "Delete folder",
                        MenuAction::Delete(id.to_string()),
                    ),
                ];
                for (k, fb, a) in items {
                    if ui.button(tx(cx.tr, k, fb)).clicked() {
                        ev.push(TreeEvent::Menu(a));
                        ui.close();
                    }
                }
            });
        }
    });
    header.body(|ui| {
        for ch in cx.data.children(Some(id)) {
            folder_node(ui, &ch.id, cx, ev);
        }
        for t in visible(cx, &own) {
            task_row(ui, t, cx, ev);
        }
    });
}

/// Draw the tree; returns what happened. `unfiled` = ids of tasks not in any folder.
pub fn show(ui: &mut egui::Ui, cx: &TreeCtx, unfiled: &HashSet<String>) -> Vec<TreeEvent> {
    let mut ev = Vec::new();
    for f in cx.data.children(None) {
        folder_node(ui, &f.id, cx, &mut ev);
    }
    let selected = *cx.sel == Some(Sel::Unfiled);
    let state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        Id::unique("tf_unfiled"),
        true,
    );
    let header = state.show_header(ui, |ui| {
        let r = ui.selectable_label(selected, tx(cx.tr, "common.taskFolders.unfiled", "Unfiled"));
        if r.clicked() {
            ev.push(TreeEvent::Select(if selected {
                None
            } else {
                Some(Sel::Unfiled)
            }));
        }
        if !cx.read_only {
            if let Some(item) = drop_target(ui, &r) {
                ev.push(TreeEvent::Drop { item, target: None });
            }
        }
    });
    header.body(|ui| {
        for t in visible(cx, unfiled) {
            task_row(ui, t, cx, &mut ev);
        }
    });
    ev
}
