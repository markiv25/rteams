//! Bottom status bar: line/col, language, git branch, encoding, watcher and word-wrap toggles.

use eframe::egui;
use rteams_services::i18n::I18n;

use crate::theme;

pub struct StatusInfo<'a> {
    pub cursor: Option<(usize, usize)>,
    pub language: Option<&'a str>,
    pub branch: Option<&'a str>,
    pub encoding: Option<&'a str>,
    pub large: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct StatusResponse {
    pub toggle_watch: bool,
    pub toggle_wrap: bool,
    pub shortcuts: bool,
}

pub fn show(
    ui: &mut egui::Ui,
    info: &StatusInfo,
    watching: bool,
    wrap: bool,
    tr: &I18n,
) -> StatusResponse {
    let mut r = StatusResponse::default();
    ui.horizontal(|ui| {
        let dim = |s: String| egui::RichText::new(s).small().color(theme::TEXT_2);
        if let Some(b) = info.branch {
            ui.label(dim(format!("git: {b}")));
        }
        if let Some((l, c)) = info.cursor {
            ui.label(dim(tr.t_args(
                "team.editor.statusBar.position",
                &[("line", &l.to_string()), ("col", &c.to_string())],
            )));
        }
        if info.large {
            ui.label(
                egui::RichText::new("large file")
                    .small()
                    .color(theme::WARNING),
            )
            .on_hover_text(tr.t("team.editor.largeFileMode"));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("?")
                .on_hover_text(tr.t("team.editor.actions.keyboardShortcuts"))
                .clicked()
            {
                r.shortcuts = true;
            }
            let w = if watching {
                tr.t("team.editor.statusBar.watching")
            } else {
                tr.t("team.editor.statusBar.watch")
            };
            let tip = if watching {
                tr.t("team.editor.statusBar.disableExternalWatcher")
            } else {
                tr.t("team.editor.statusBar.watchExternalChanges")
            };
            if ui
                .add(egui::Button::selectable(
                    watching,
                    egui::RichText::new(w).small(),
                ))
                .on_hover_text(tip)
                .clicked()
            {
                r.toggle_watch = true;
            }
            let wt = if wrap {
                tr.t("team.editor.toolbar.disableWordWrap")
            } else {
                tr.t("team.editor.toolbar.enableWordWrap")
            };
            if ui
                .add(egui::Button::selectable(
                    wrap,
                    egui::RichText::new("↩").small(),
                ))
                .on_hover_text(wt)
                .clicked()
            {
                r.toggle_wrap = true;
            }
            if let Some(e) = info.encoding {
                ui.label(dim(e.to_string()));
            }
            if let Some(l) = info.language {
                ui.label(dim(l.to_string()));
            }
        });
    });
    r
}
