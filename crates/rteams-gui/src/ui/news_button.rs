//! news_button: bell button with an unread count; owns the announcements model, refreshes the feed
//! in a worker thread, auto-opens the newest unseen item once, and hosts the news dialog.
//! Hidden when no feed URL is configured.

use std::sync::mpsc::{channel, Receiver};

use eframe::egui::{self, RichText};

use super::close_guard::tx;
use super::news_dialog::NewsDialog;
use super::update_dialog::open_link;
use super::FeatureCtx;
use crate::theme;
use rteams_services::announcements::{self, Model};
use rteams_services::config::AppConfig;

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Default)]
pub struct NewsButton {
    pub(crate) model: Model,
    dialog: NewsDialog,
    started: bool,
    loading: bool,
    auto_opened: bool,
    rx: Option<Receiver<Model>>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl NewsButton {
    pub fn unread(&self) -> usize {
        self.model.unread(now_ms(), APP_VERSION)
    }

    /// Call every frame: first call loads the cache and starts a refresh; later calls poll it.
    pub fn tick(&mut self, ctx: &egui::Context, home: &std::path::Path) {
        if !self.started {
            self.started = true;
            let url = announcements::feed_url_from_config(&AppConfig::load(home));
            self.model = Model::load(home, &url, now_ms());
            if self.model.enabled {
                self.loading = true;
                let (s, r) = channel();
                let (ctx, home) = (ctx.clone(), home.to_path_buf());
                self.rx = Some(r);
                std::thread::spawn(move || {
                    let _ = s.send(Model::refresh(&home, &url, now_ms()));
                    ctx.request_repaint();
                });
            }
        }
        if let Some(r) = &self.rx {
            if let Ok(m) = r.try_recv() {
                self.rx = None;
                self.loading = false;
                self.model = m;
                if !self.auto_opened {
                    self.auto_opened = true;
                    if let Some(id) = self.model.claim_auto(home, now_ms(), APP_VERSION) {
                        self.dialog.open_article(&id);
                    }
                }
            }
        }
    }

    /// Draw the bell (+ unread badge) in a header row, and the dialog.
    pub fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        self.tick(ui.ctx(), c.home);
        if self.model.enabled {
            let n = self.unread();
            ui.horizontal(|ui| {
                let btn = ui
                    .add(egui::Button::new(RichText::new("\u{1F514}")).frame(false))
                    .on_hover_text(tx(c.tr, "common.announcements.title", "News"));
                if n > 0 {
                    theme::count_badge(ui, n, true);
                }
                if btn.clicked() {
                    self.dialog.open();
                }
            });
        }
        if let Some(url) = self.dialog.show(
            ui.ctx(),
            c.tr,
            &mut self.model,
            c.home,
            now_ms(),
            APP_VERSION,
            self.loading,
        ) {
            open_link(c.tr, &url, c.req);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_feed_is_hidden_and_quiet() {
        let home = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut b = NewsButton::default();
        b.tick(&ctx, home.path());
        assert!(!b.model.enabled && b.unread() == 0 && !b.loading);
    }

    #[test]
    fn enabled_feed_loads_cache_then_refreshes_headless() {
        let home = tempfile::tempdir().unwrap();
        let src = home.path().join("feed.json");
        std::fs::write(
            &src,
            r#"{"items":[{"id":"a","title":"A","published_at":1}]}"#,
        )
        .unwrap();
        let mut cfg = AppConfig::default();
        cfg.general.extra.insert(
            "announcements_feed_url".into(),
            format!("file://{}", src.display()).into(),
        );
        cfg.save(home.path()).unwrap();
        let ctx = egui::Context::default();
        let mut b = NewsButton::default();
        b.tick(&ctx, home.path());
        for _ in 0..100 {
            b.tick(&ctx, home.path());
            if !b.loading {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!b.loading);
        assert_eq!(b.unread(), 1);
        assert!(b.dialog.is_open(), "newest unseen item auto-opens");
    }
}
