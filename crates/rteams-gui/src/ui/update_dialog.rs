//! update_dialog: release notes for an available update with "Later" / "Download". Download only
//! opens the release page in the browser; nothing is downloaded or replaced by the app itself.

use eframe::egui::{self, RichText};

use super::close_guard::tx;
use super::editor::md_preview;
use super::ShellRequest;
use crate::theme;
use rteams_services::i18n::I18n;
use rteams_services::updater::UpdateInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogAct {
    Later,
    Download,
}

#[derive(Default)]
pub struct UpdateDialog {
    open: bool,
}

/// Open `url` in the default browser; on failure toast the address so it can be copied.
pub(crate) fn open_link(tr: &I18n, url: &str, req: &mut Vec<ShellRequest>) {
    if !url.starts_with("https://") {
        return;
    }
    if open::that(url).is_err() {
        req.push(ShellRequest::Toast(format!(
            "{} {url}",
            tx(
                tr,
                "common.updateDialog.openLinkFailed",
                "Could not open your browser. Open this address manually:"
            )
        )));
    }
}

impl UpdateDialog {
    pub fn open(&mut self) {
        self.open = true;
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Draw the dialog; returns the user's choice (the dialog closes itself on either).
    pub fn show(&mut self, ctx: &egui::Context, tr: &I18n, info: &UpdateInfo) -> Option<DialogAct> {
        if !self.open {
            return None;
        }
        let mut act = None;
        let modal = egui::Modal::new(egui::Id::unique("update_dialog"))
            .backdrop_color(egui::Color32::from_black_alpha(150))
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::hairline(0.12)))
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::same(20)),
            )
            .show(ctx, |ui| {
                ui.set_width(520.0f32.min((ctx.content_rect().width() - 80.0).max(120.0)));
                ui.label(
                    RichText::new(format!(
                        "{} v{}",
                        tr.t("common.updateDialog.updateAvailable"),
                        info.latest
                    ))
                    .size(18.0)
                    .strong(),
                );
                ui.label(
                    RichText::new(format!("v{} ➡ v{}", info.current, info.latest))
                        .color(theme::TEXT_2),
                );
                ui.add_space(8.0);
                if info.notes.trim().is_empty() {
                    ui.label(
                        RichText::new(tr.t("common.updateDialog.noReleaseNotes"))
                            .color(theme::MUTED),
                    );
                } else {
                    ui.scope(|ui| {
                        ui.set_height(240.0);
                        md_preview::show(ui, &info.notes);
                    });
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if theme::ghost(ui, &tr.t("common.updateDialog.later")).clicked() {
                        act = Some(DialogAct::Later);
                    }
                    if !info.url.is_empty()
                        && theme::solid(ui, &tr.t("common.updateDialog.download"), theme::INFO)
                            .clicked()
                    {
                        act = Some(DialogAct::Download);
                    }
                });
            });
        if modal.should_close() && act.is_none() {
            act = Some(DialogAct::Later);
        }
        if act.is_some() {
            self.open = false;
        }
        act
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_headless_open_and_closed() {
        let tr = I18n::default();
        let info = UpdateInfo {
            current: "0.1.0".into(),
            latest: "0.2.0".into(),
            available: true,
            notes: "# Notes\n- one\n- two".into(),
            url: "https://example.test/r".into(),
            published_at: String::new(),
        };
        let mut d = UpdateDialog::default();
        crate::view::testing::render(|ui| assert!(d.show(ui.ctx(), &tr, &info).is_none()));
        d.open();
        crate::view::testing::render(|ui| assert!(d.show(ui.ctx(), &tr, &info).is_none()));
        assert!(d.is_open());
    }
}
