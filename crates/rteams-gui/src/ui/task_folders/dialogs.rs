//! task_folders::dialogs: name / delete / reset / move dialogs.

use eframe::egui::{self, Color32, RichText};
use rteams_services::i18n::I18n;
use rteams_services::task_folders::model::clean_name;
use rteams_services::task_folders::TaskFolders;

use super::dnd::DragItem;
use crate::theme;
use crate::ui::close_guard::tx;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    NewFolder {
        parent: Option<String>,
        name: String,
    },
    Rename {
        id: String,
        name: String,
    },
    Delete {
        id: String,
    },
    Reset,
    Move {
        item: DragItem,
        target: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Open,
    Cancel,
    Submit,
}

fn folder_name<'a>(d: &'a TaskFolders, id: &str) -> &'a str {
    d.folder(id).map_or("?", |f| f.name.as_str())
}

/// Folders as (id, depth, name) in tree order, for the move picker.
fn flat(
    d: &TaskFolders,
    parent: Option<&str>,
    depth: usize,
    skip: &std::collections::HashSet<String>,
    out: &mut Vec<(String, usize, String)>,
) {
    for f in d.children(parent) {
        if skip.contains(&f.id) {
            continue;
        }
        out.push((f.id.clone(), depth, f.name.clone()));
        flat(d, Some(&f.id), depth + 1, skip, out);
    }
}

pub fn show(
    ctx: &egui::Context,
    dlg: &mut Dialog,
    data: &TaskFolders,
    tr: &I18n,
    error: Option<&str>,
) -> Outcome {
    let mut out = Outcome::Open;
    let title = match dlg {
        Dialog::NewFolder { parent: None, .. } => {
            tx(tr, "common.taskFolders.newFolder", "New folder")
        }
        Dialog::NewFolder { .. } => tx(tr, "common.taskFolders.newSubfolder", "New subfolder"),
        Dialog::Rename { .. } => tx(tr, "common.taskFolders.renameFolder", "Rename folder"),
        Dialog::Delete { .. } => tx(tr, "common.taskFolders.deleteFolder", "Delete folder"),
        Dialog::Reset => tx(tr, "common.taskFolders.reset", "Reset folder organization"),
        Dialog::Move { .. } => tx(tr, "common.taskFolders.move", "Move to\u{2026}"),
    };
    let width = (ctx.content_rect().width() * 0.92).clamp(1.0, 380.0);
    let modal = egui::Modal::new(egui::Id::unique("task_folder_dialog"))
        .backdrop_color(Color32::from_black_alpha(150))
        .frame(
            egui::Frame::new()
                .fill(theme::SURFACE)
                .stroke(egui::Stroke::new(1.0, theme::hairline(0.12)))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::same(18)),
        )
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.label(RichText::new(&title).size(16.0).strong());
            ui.add_space(8.0);
            let mut can_submit = true;
            let mut danger = false;
            match dlg {
                Dialog::NewFolder { name, .. } | Dialog::Rename { name, .. } => {
                    ui.label(RichText::new(tx(tr, "common.taskFolders.folderName", "Folder name")).color(theme::TEXT_2));
                    let r = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
                    if !r.has_focus() && !r.lost_focus() {
                        r.request_focus();
                    }
                    if clean_name(name).is_err() {
                        can_submit = false;
                        ui.label(RichText::new(tx(tr, "common.taskFolders.invalidName", "Use a name with 1\u{2013}120 characters.")).small().color(theme::WARNING));
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && can_submit {
                        out = Outcome::Submit;
                    }
                }
                Dialog::Delete { id } => {
                    danger = true;
                    ui.label(RichText::new(folder_name(data, id)).strong());
                    ui.label(RichText::new(tx(tr, "common.taskFolders.deleteDescription", "Tasks and subfolders move to the parent. Tasks in a root folder become unfiled. No tasks are deleted.")).color(theme::TEXT_2));
                }
                Dialog::Reset => {
                    danger = true;
                    ui.label(RichText::new(tx(tr, "common.taskFolders.resetDescription", "Remove all folder organization. Tasks, pins, archives and task names are preserved.")).color(theme::TEXT_2));
                }
                Dialog::Move { item, target } => {
                    ui.label(RichText::new(tx(tr, "common.taskFolders.destination", "Destination folder")).color(theme::TEXT_2));
                    let skip = match item {
                        DragItem::Folder(f) => data.subtree_ids(f),
                        DragItem::Task(_) => Default::default(),
                    };
                    let mut rows = Vec::new();
                    flat(data, None, 0, &skip, &mut rows);
                    let root_label = match item {
                        DragItem::Task(_) => tx(tr, "common.taskFolders.unfiled", "Unfiled"),
                        DragItem::Folder(_) => tx(tr, "common.taskFolders.root", "Root"),
                    };
                    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                        if ui.selectable_label(target.is_none(), root_label).clicked() {
                            *target = None;
                        }
                        for (id, depth, name) in rows {
                            ui.horizontal(|ui| {
                                ui.add_space(14.0 * (depth + 1) as f32);
                                if ui.selectable_label(target.as_deref() == Some(id.as_str()), name).clicked() {
                                    *target = Some(id);
                                }
                            });
                        }
                    });
                }
            }
            if let Some(e) = error {
                ui.add_space(6.0);
                ui.label(RichText::new(e).small().color(theme::ERROR_FG));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let label = match dlg {
                    Dialog::Delete { .. } => tx(tr, "common.taskFolders.deleteFolder", "Delete folder"),
                    Dialog::Reset => tx(tr, "common.actions.reset", "Reset"),
                    Dialog::Move { .. } => tx(tr, "common.taskFolders.move", "Move to\u{2026}"),
                    _ => tx(tr, "common.actions.save", "Save"),
                };
                let fill = if danger { theme::ERROR } else { theme::ACCENT };
                ui.add_enabled_ui(can_submit, |ui| {
                    if theme::solid(ui, &label, fill).clicked() {
                        out = Outcome::Submit;
                    }
                });
                if ui.button(tx(tr, "common.actions.cancel", "Cancel")).clicked() {
                    out = Outcome::Cancel;
                }
            });
        });
    if out == Outcome::Open && modal.should_close() {
        out = Outcome::Cancel;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_render_every_dialog() {
        let mut data = TaskFolders::default();
        let a = data.create("a", None).unwrap();
        data.create("b", Some(&a)).unwrap();
        let tr = I18n::english();
        let ctx = egui::Context::default();
        for mut d in [
            Dialog::NewFolder {
                parent: None,
                name: String::new(),
            },
            Dialog::Rename {
                id: a.clone(),
                name: "a".into(),
            },
            Dialog::Delete { id: a.clone() },
            Dialog::Reset,
            Dialog::Move {
                item: DragItem::Folder(a.clone()),
                target: None,
            },
            Dialog::Move {
                item: DragItem::Task("t".into()),
                target: Some(a.clone()),
            },
        ] {
            let mut o = Outcome::Cancel;
            crate::ui::close_guard::headless(&ctx, |ui| {
                o = show(ui.ctx(), &mut d, &data, &tr, None);
            });
            assert_eq!(o, Outcome::Open);
        }
    }
}
