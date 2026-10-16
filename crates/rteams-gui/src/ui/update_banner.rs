//! update_banner: slim bar "Update available vX -- Release notes / Download / Dismiss". Checks the
//! configured release feed in a worker thread at most once per 24 h (empty feed = never) and on
//! demand. It only reports and opens the release page; it never downloads or replaces binaries.

use std::sync::mpsc::{channel, Receiver};

use eframe::egui::{self, RichText};

use super::close_guard::tx;
use super::update_dialog::{open_link, DialogAct, UpdateDialog};
use super::FeatureCtx;
use crate::theme;
use rteams_services::config::AppConfig;
use rteams_services::i18n::I18n;
use rteams_services::updater::{self, UpdateError, UpdateInfo};

type CheckResult = Result<UpdateInfo, UpdateError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Act {
    Notes,
    Download,
    Dismiss,
}

#[derive(Default)]
pub struct UpdateBanner {
    pub(crate) info: Option<UpdateInfo>,
    pub(crate) dismissed: bool,
    pub(crate) last_error: Option<UpdateError>,
    pub(crate) checking: bool,
    started: bool,
    rx: Option<Receiver<CheckResult>>,
    dialog: UpdateDialog,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl UpdateBanner {
    /// Check now (Settings > Advanced > "Check for Updates"). Ignored while one is running.
    pub fn check_now(&mut self, ctx: &egui::Context, home: &std::path::Path) {
        if self.checking {
            return;
        }
        let url = updater::feed_url_from_config(&AppConfig::load(home));
        let (s, r) = channel();
        let ctx = ctx.clone();
        self.checking = true;
        self.rx = Some(r);
        std::thread::spawn(move || {
            let _ = s.send(updater::check(&url, env!("CARGO_PKG_VERSION")));
            ctx.request_repaint();
        });
    }

    /// Call every frame: the automatic check (once per 24 h) and result polling.
    pub fn tick(&mut self, ctx: &egui::Context, home: &std::path::Path) {
        if !self.started {
            self.started = true;
            let enabled = !updater::feed_url_from_config(&AppConfig::load(home)).is_empty();
            if enabled && updater::should_check(home, now_ms()) {
                self.check_now(ctx, home);
            }
        }
        if let Some(r) = &self.rx {
            if let Ok(res) = r.try_recv() {
                self.rx = None;
                self.checking = false;
                match res {
                    Ok(info) => {
                        updater::record_check(home, now_ms());
                        self.dismissed = updater::is_dismissed(home, &info.latest);
                        self.last_error = None;
                        self.info = Some(info);
                    }
                    Err(e) => self.last_error = Some(e),
                }
            }
        }
    }

    pub fn available(&self) -> Option<&UpdateInfo> {
        self.info.as_ref().filter(|i| i.available)
    }

    /// Status text for Settings > Advanced > Updates.
    #[cfg_attr(not(test), allow(dead_code))] // called by Settings > Advanced
    pub fn status_text(&self, tr: &I18n) -> String {
        if self.checking {
            return tr.t("settings.advanced.updates.checking");
        }
        match (&self.info, &self.last_error) {
            (Some(i), _) if i.available => tr.t_args(
                "settings.advanced.updates.available",
                &[("version", &i.latest)],
            ),
            (Some(_), _) => tr.t("settings.advanced.updates.upToDate"),
            (None, Some(e)) => e.to_string(),
            (None, None) => String::new(),
        }
    }

    /// Draw the banner (nothing when no update, dismissed, or disabled) and the notes dialog.
    pub fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        self.tick(ui.ctx(), c.home);
        let Some(info) = self.available().cloned() else {
            return;
        };
        if !self.dismissed {
            if let Some(act) = Self::render(ui, c.tr, &info) {
                match act {
                    Act::Notes => self.dialog.open(),
                    Act::Download => open_link(c.tr, &info.url, c.req),
                    Act::Dismiss => {
                        self.dismissed = true;
                        updater::dismiss(c.home, &info.latest);
                    }
                }
            }
        }
        match self.dialog.show(ui.ctx(), c.tr, &info) {
            Some(DialogAct::Download) => open_link(c.tr, &info.url, c.req),
            Some(DialogAct::Later) | None => {}
        }
    }

    fn render(ui: &mut egui::Ui, tr: &I18n, info: &UpdateInfo) -> Option<Act> {
        let mut act = None;
        egui::Frame::new()
            .fill(theme::tint(theme::INFO, 0.12))
            .stroke(egui::Stroke::new(1.0, theme::tint(theme::INFO, 0.4)))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(10, 4))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} v{}",
                            tr.t("common.updateDialog.updateAvailable"),
                            info.latest
                        ))
                        .strong(),
                    );
                    if ui
                        .link(tx(tr, "common.updateBanner.releaseNotes", "Release notes"))
                        .clicked()
                    {
                        act = Some(Act::Notes);
                    }
                    if !info.url.is_empty()
                        && ui.link(tr.t("common.updateDialog.download")).clicked()
                    {
                        act = Some(Act::Download);
                    }
                    if ui
                        .link(tx(tr, "common.updateBanner.dismiss", "Dismiss"))
                        .clicked()
                    {
                        act = Some(Act::Dismiss);
                    }
                });
            });
        act
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(avail: bool) -> UpdateInfo {
        UpdateInfo {
            current: "0.1.0".into(),
            latest: "0.2.0".into(),
            available: avail,
            notes: "n".into(),
            url: "https://example.test/r".into(),
            published_at: String::new(),
        }
    }

    #[test]
    fn banner_renders_headless_and_status_text() {
        let tr = I18n::english();
        crate::view::testing::render(|ui| {
            assert!(UpdateBanner::render(ui, &tr, &info(true)).is_none());
        });
        let mut b = UpdateBanner::default();
        assert!(b.available().is_none());
        b.info = Some(info(true));
        assert!(b.available().is_some());
        assert_eq!(b.status_text(&tr), "v0.2.0 available");
        b.info = Some(info(false));
        assert!(b.available().is_none());
        assert_eq!(b.status_text(&tr), "Up to date");
    }

    #[test]
    fn disabled_feed_does_not_check() {
        let home = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut b = UpdateBanner::default();
        b.tick(&ctx, home.path());
        assert!(!b.checking && b.info.is_none());
    }
}
