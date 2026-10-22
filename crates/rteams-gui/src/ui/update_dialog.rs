//! update_dialog: release notes for an available update with "Later" / "Download". When the
//! release has an asset for this OS/arch, Download saves it to ~/Downloads (progress, size,
//! SHA-256) and offers Reveal / Open; otherwise it opens the release page. The running binary is
//! never replaced.

use eframe::egui::{self, RichText};

use super::close_guard::tx;
use super::editor::md_preview;
use super::ShellRequest;
use crate::theme;
use rteams_services::i18n::I18n;
use rteams_services::updater::{Downloaded, UpdateInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogAct {
    Later,
    /// Open the release page in the browser.
    Download,
    /// Download the matching asset into ~/Downloads.
    DownloadAsset,
    CancelDownload,
    Reveal,
    OpenFile,
}

/// What the dialog shows for the asset download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DlView {
    /// No matching asset (page link only) or not started yet.
    Idle {
        asset: Option<String>,
    },
    Running {
        name: String,
        done: u64,
        total: u64,
    },
    Done(Downloaded),
    Failed(String),
}

pub fn fmt_bytes(n: u64) -> String {
    const U: [&str; 4] = ["B", "KB", "MB", "GB"];
    let (mut v, mut i) = (n as f64, 0);
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
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
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        tr: &I18n,
        info: &UpdateInfo,
        dl: &DlView,
    ) -> Option<DialogAct> {
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
                match dl {
                    DlView::Running { name, done, total } => {
                        let frac = if *total > 0 {
                            (*done as f32 / *total as f32).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        let mut bar = egui::ProgressBar::new(frac);
                        bar = if *total > 0 {
                            bar.text(format!("{} / {}", fmt_bytes(*done), fmt_bytes(*total)))
                        } else {
                            bar.text(fmt_bytes(*done)).animate(true)
                        };
                        ui.label(RichText::new(name).color(theme::TEXT_2));
                        ui.add(bar);
                    }
                    DlView::Done(d) => {
                        ui.label(
                            RichText::new(tx(tr, "common.updateDialog.downloaded", "Downloaded and verified"))
                                .color(theme::SUCCESS)
                                .strong(),
                        );
                        ui.label(RichText::new(d.path.display().to_string()).monospace().size(11.0));
                        ui.label(
                            RichText::new(format!("{}  SHA-256 {}", fmt_bytes(d.size), d.sha256))
                                .monospace()
                                .size(11.0)
                                .color(theme::MUTED),
                        );
                        ui.label(
                            RichText::new(tx(
                                tr,
                                "common.updateDialog.installManually",
                                "Quit rteams, install the downloaded file, then open the app again. rteams never replaces itself.",
                            ))
                            .color(theme::TEXT_2),
                        );
                    }
                    DlView::Failed(e) => {
                        ui.label(RichText::new(e).color(theme::ERROR_FG));
                    }
                    DlView::Idle { .. } => {}
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if theme::ghost(ui, &tr.t("common.updateDialog.later")).clicked() {
                        act = Some(DialogAct::Later);
                    }
                    match dl {
                        DlView::Running { .. } => {
                            if ui.button(tx(tr, "common.actions.cancel", "Cancel")).clicked() {
                                act = Some(DialogAct::CancelDownload);
                            }
                        }
                        DlView::Done(_) => {
                            if theme::solid(
                                ui,
                                &tx(tr, "common.updateDialog.reveal", "Reveal in Finder"),
                                theme::INFO,
                            )
                            .clicked()
                            {
                                act = Some(DialogAct::Reveal);
                            }
                            if ui.button(tx(tr, "common.updateDialog.openFile", "Open")).clicked() {
                                act = Some(DialogAct::OpenFile);
                            }
                        }
                        DlView::Idle { asset: Some(name) } => {
                            if theme::solid(
                                ui,
                                &format!("{} ({name})", tr.t("common.updateDialog.download")),
                                theme::INFO,
                            )
                            .clicked()
                            {
                                act = Some(DialogAct::DownloadAsset);
                            }
                        }
                        DlView::Failed(_)
                            if !info.assets.is_empty()
                                && ui.button(tr.t("common.updateDialog.retry")).clicked() =>
                        {
                            act = Some(DialogAct::DownloadAsset);
                        }
                        _ => {}
                    }
                    if !info.url.is_empty()
                        && !matches!(dl, DlView::Running { .. })
                        && (matches!(dl, DlView::Idle { asset: None } | DlView::Failed(_)))
                        && theme::solid(
                            ui,
                            &tx(tr, "common.updateDialog.viewOnGitHub", "View release page"),
                            theme::INFO,
                        )
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
            assets: vec![],
        };
        let mut d = UpdateDialog::default();
        let idle = DlView::Idle { asset: None };
        crate::view::testing::render(|ui| assert!(d.show(ui.ctx(), &tr, &info, &idle).is_none()));
        d.open();
        let views = [
            idle,
            DlView::Idle {
                asset: Some("rteams.dmg".into()),
            },
            DlView::Running {
                name: "rteams.dmg".into(),
                done: 1024 * 1024,
                total: 4 * 1024 * 1024,
            },
            DlView::Running {
                name: "rteams.dmg".into(),
                done: 10,
                total: 0,
            },
            DlView::Done(Downloaded {
                path: "/tmp/rteams.dmg".into(),
                size: 4096,
                sha256: "ab".repeat(32),
            }),
            DlView::Failed("size mismatch".into()),
        ];
        for v in &views {
            crate::view::testing::render(|ui| assert!(d.show(ui.ctx(), &tr, &info, v).is_none()));
        }
        assert!(d.is_open());
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(1536), "1.5 KB");
    }
}
