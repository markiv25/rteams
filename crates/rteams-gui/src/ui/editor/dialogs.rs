//! Modal dialogs: new file/folder, rename, move to trash, unsaved changes, save conflict.

use eframe::egui;
use rteams_services::editor::conflict::{self, DiffKind, DiffLine};
use rteams_services::editor::files::{self, NameError};
use rteams_services::i18n::I18n;

use crate::theme;

#[derive(Debug, Clone)]
pub enum Dialog {
    None,
    New {
        parent: String,
        is_dir: bool,
        name: String,
    },
    Rename {
        rel: String,
        name: String,
    },
    Delete {
        rel: String,
    },
    /// Closing one tab (index) or, with `None`, the whole editor.
    Unsaved {
        tab: Option<usize>,
        names: Vec<String>,
    },
    Conflict {
        rel: String,
        diff: Vec<DiffLine>,
    },
}

impl Dialog {
    pub fn is_open(&self) -> bool {
        !matches!(self, Dialog::None)
    }

    pub fn new_entry(parent: &str, is_dir: bool) -> Dialog {
        Dialog::New {
            parent: parent.to_string(),
            is_dir,
            name: String::new(),
        }
    }

    pub fn rename(rel: &str) -> Dialog {
        Dialog::Rename {
            rel: rel.to_string(),
            name: rel.rsplit('/').next().unwrap_or(rel).to_string(),
        }
    }

    pub fn conflict(rel: &str, mine: &str, disk: &str) -> Dialog {
        Dialog::Conflict {
            rel: rel.to_string(),
            diff: conflict::diff(mine, disk),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogResult {
    Pending,
    Cancel,
    Create {
        parent: String,
        is_dir: bool,
        name: String,
    },
    Rename {
        rel: String,
        name: String,
    },
    Trash {
        rel: String,
    },
    SaveAllAndClose {
        tab: Option<usize>,
    },
    DiscardAndClose {
        tab: Option<usize>,
    },
    Overwrite {
        rel: String,
    },
    Reload {
        rel: String,
    },
    KeepMine {
        rel: String,
    },
}

fn name_error_text(e: NameError, tr: &I18n) -> String {
    tr.t(e.key())
}

fn window(title: String) -> egui::Window<'static> {
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
}

fn name_form(ui: &mut egui::Ui, name: &mut String, hint: String, tr: &I18n) -> (bool, bool) {
    let r = ui.add(
        egui::TextEdit::singleline(name)
            .hint_text(hint)
            .desired_width(260.0),
    );
    r.request_focus();
    let err = if name.is_empty() {
        None
    } else {
        files::validate_name(name).err()
    };
    if let Some(e) = err {
        ui.colored_label(theme::ERROR, name_error_text(e, tr));
    }
    let valid = files::validate_name(name).is_ok();
    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    (valid, enter)
}

/// Draw the open dialog (if any). The caller resets `dialog` to `None` on any non-`Pending` result.
pub fn show(ctx: &egui::Context, dialog: &mut Dialog, tr: &I18n) -> DialogResult {
    let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    let mut res = DialogResult::Pending;
    match dialog {
        Dialog::None => return res,
        Dialog::New {
            parent,
            is_dir,
            name,
        } => {
            let hint = tr.t(if *is_dir {
                "team.editor.newFile.placeholders.folderName"
            } else {
                "team.editor.newFile.placeholders.fileName"
            });
            window(hint.trim_end_matches("...").to_string()).show(ctx, |ui| {
                let (valid, enter) = name_form(ui, name, hint.clone(), tr);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(valid, egui::Button::new(tr.t("team.editor.actions.save")))
                        .clicked()
                        || (enter && valid)
                    {
                        res = DialogResult::Create {
                            parent: parent.clone(),
                            is_dir: *is_dir,
                            name: name.trim().to_string(),
                        };
                    }
                    if ui.button(tr.t("team.editor.actions.cancel")).clicked() {
                        res = DialogResult::Cancel;
                    }
                });
            });
        }
        Dialog::Rename { rel, name } => {
            window("Rename".into()).show(ctx, |ui| {
                let (valid, enter) = name_form(ui, name, String::new(), tr);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(valid, egui::Button::new(tr.t("team.editor.actions.save")))
                        .clicked()
                        || (enter && valid)
                    {
                        res = DialogResult::Rename {
                            rel: rel.clone(),
                            name: name.trim().to_string(),
                        };
                    }
                    if ui.button(tr.t("team.editor.actions.cancel")).clicked() {
                        res = DialogResult::Cancel;
                    }
                });
            });
        }
        Dialog::Delete { rel, .. } => {
            let n = rel.rsplit('/').next().unwrap_or(rel).to_string();
            window(tr.t("team.editor.fileTree.moveToTrash")).show(ctx, |ui| {
                ui.label(tr.t_args("team.editor.fileTree.moveToTrashConfirm", &[("name", &n)]));
                ui.horizontal(|ui| {
                    if ui
                        .button(tr.t("team.editor.fileTree.moveToTrash"))
                        .clicked()
                    {
                        res = DialogResult::Trash { rel: rel.clone() };
                    }
                    if ui.button(tr.t("team.editor.fileTree.cancel")).clicked() {
                        res = DialogResult::Cancel;
                    }
                });
            });
        }
        Dialog::Unsaved { tab, names } => {
            let single = tab.is_some();
            window(tr.t("team.editor.dialogs.unsavedTitle")).show(ctx, |ui| {
                ui.label(tr.t(if single {
                    "team.editor.dialogs.unsavedFileDescription"
                } else {
                    "team.editor.dialogs.unsavedDescription"
                }));
                for n in names.iter().take(8) {
                    ui.label(egui::RichText::new(format!("• {n}")).color(theme::WARNING));
                }
                ui.horizontal(|ui| {
                    if ui
                        .button(tr.t("team.editor.actions.saveAllAndClose"))
                        .clicked()
                    {
                        res = DialogResult::SaveAllAndClose { tab: *tab };
                    }
                    if ui
                        .button(tr.t("team.editor.actions.discardAndClose"))
                        .clicked()
                    {
                        res = DialogResult::DiscardAndClose { tab: *tab };
                    }
                    if ui.button(tr.t("team.editor.actions.cancel")).clicked() {
                        res = DialogResult::Cancel;
                    }
                });
            });
        }
        Dialog::Conflict { rel, diff } => {
            window(tr.t("team.editor.dialogs.conflictTitle")).show(ctx, |ui| {
                ui.label(tr.t("team.editor.dialogs.conflictDescription"));
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for l in diff.iter() {
                            let (p, c) = match l.kind {
                                DiffKind::Same => (" ", theme::MUTED),
                                DiffKind::Mine => ("-", theme::ERROR_FG),
                                DiffKind::Disk => ("+", theme::RESULT_FG),
                            };
                            ui.label(
                                egui::RichText::new(format!("{p} {}", l.text))
                                    .monospace()
                                    .small()
                                    .color(c),
                            );
                        }
                    });
                ui.horizontal(|ui| {
                    if ui.button(tr.t("team.editor.actions.overwrite")).clicked() {
                        res = DialogResult::Overwrite { rel: rel.clone() };
                    }
                    if ui.button(tr.t("team.editor.actions.reload")).clicked() {
                        res = DialogResult::Reload { rel: rel.clone() };
                    }
                    if ui.button(tr.t("team.editor.actions.keepMine")).clicked() {
                        res = DialogResult::KeepMine { rel: rel.clone() };
                    }
                });
            });
        }
    }
    if esc && res == DialogResult::Pending {
        res = DialogResult::Cancel;
    }
    res
}
