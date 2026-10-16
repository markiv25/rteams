//! news_dialog: "What's new" list of announcements, and an article view rendered as markdown.

use std::path::Path;

use eframe::egui::{self, RichText};

use super::close_guard::tx;
use super::editor::md_preview;
use crate::theme;
use rteams_services::announcements::{policy, Model};
use rteams_services::i18n::I18n;

#[derive(Default)]
pub struct NewsDialog {
    open: bool,
    selected: Option<String>,
}

fn date(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    // civil-from-days (Howard Hinnant), UTC.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

impl NewsDialog {
    pub fn open(&mut self) {
        self.open = true;
    }

    /// Open straight onto one article (auto-open of the newest unseen item).
    pub fn open_article(&mut self, id: &str) {
        self.open = true;
        self.selected = Some(id.to_string());
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Draw the dialog. Returns an https URL when the user asked to open an article's link.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        tr: &I18n,
        model: &mut Model,
        home: &Path,
        now_ms: i64,
        app_version: &str,
        loading: bool,
    ) -> Option<String> {
        if !self.open {
            return None;
        }
        let mut close = false;
        let mut open_url = None;
        let mut read_now: Option<String> = None;
        let modal = egui::Modal::new(egui::Id::unique("news_dialog"))
            .backdrop_color(egui::Color32::from_black_alpha(150))
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::hairline(0.12)))
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::same(20)),
            )
            .show(ctx, |ui| {
                ui.set_width(600.0f32.min((ctx.content_rect().width() - 80.0).max(120.0)));
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(tr.t("common.announcements.historyTitle"))
                            .size(18.0)
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::ghost(ui, &tr.t("common.actions.close")).clicked() {
                            close = true;
                        }
                    });
                });
                ui.label(
                    RichText::new(tr.t("common.announcements.description")).color(theme::TEXT_2),
                );
                if model.offline {
                    ui.label(
                        RichText::new(tr.t("common.announcements.status.offline"))
                            .color(theme::WARNING),
                    );
                }
                ui.add_space(8.0);
                let items = model.listed(now_ms, app_version);
                let article = self
                    .selected
                    .as_ref()
                    .and_then(|id| items.iter().find(|a| &a.id == id).copied());
                if let Some(a) = article {
                    read_now = Some(a.id.clone());
                    if ui
                        .link(format!("\u{25C0} {}", tr.t("common.announcements.allNews")))
                        .clicked()
                    {
                        self.selected = None;
                    }
                    ui.label(RichText::new(&a.title).size(16.0).strong());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(date(a.published_at)).color(theme::MUTED));
                        if policy::is_expired(a, now_ms) {
                            theme::pill(ui, tr.t("common.announcements.expired"), theme::WARNING);
                        }
                    });
                    ui.scope(|ui| {
                        ui.set_height(300.0);
                        md_preview::show(
                            ui,
                            if a.body.is_empty() {
                                &a.summary
                            } else {
                                &a.body
                            },
                        );
                    });
                    if let Some(u) = &a.url {
                        if ui.link(u).clicked() {
                            open_url = Some(u.clone());
                        }
                    }
                } else if items.is_empty() {
                    ui.add_space(12.0);
                    if loading {
                        ui.label(tr.t("common.announcements.loading"));
                    } else {
                        ui.label(RichText::new(tr.t("common.announcements.empty")).strong());
                        ui.label(
                            RichText::new(tr.t("common.announcements.emptyDescription"))
                                .color(theme::TEXT_2),
                        );
                    }
                } else {
                    ui.label(
                        RichText::new(tr.t("common.announcements.allNews")).color(theme::TEXT_2),
                    );
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            for a in &items {
                                let unread =
                                    !model.state.is_read(&a.id) && !policy::is_expired(a, now_ms);
                                ui.horizontal(|ui| {
                                    if unread {
                                        ui.label(RichText::new("\u{2022}").color(theme::INFO));
                                    }
                                    if ui.link(RichText::new(&a.title).strong()).clicked() {
                                        self.selected = Some(a.id.clone());
                                    }
                                    ui.label(
                                        RichText::new(date(a.published_at)).color(theme::MUTED),
                                    );
                                    if policy::is_expired(a, now_ms) {
                                        theme::pill(
                                            ui,
                                            tr.t("common.announcements.expired"),
                                            theme::WARNING,
                                        );
                                    }
                                });
                                if !a.summary.is_empty() {
                                    ui.label(RichText::new(&a.summary).color(theme::TEXT_2));
                                }
                                ui.add_space(6.0);
                            }
                        });
                    if theme::ghost(
                        ui,
                        &tx(tr, "common.announcements.markAllRead", "Mark all as read"),
                    )
                    .clicked()
                    {
                        model.mark_all_read(home, now_ms, app_version);
                    }
                }
            });
        if let Some(id) = read_now {
            if !model.state.is_read(&id) {
                model.mark_read(home, &id);
            }
        }
        if close || modal.should_close() {
            self.open = false;
            self.selected = None;
        }
        open_url
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rteams_services::announcements::feed::Announcement;

    #[test]
    fn dates_format() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_767_225_600_000), "2026-01-01");
    }

    fn model() -> Model {
        let mut m = Model {
            enabled: true,
            ..Model::default()
        };
        m.items.push(Announcement {
            id: "a".into(),
            title: "Hello".into(),
            summary: "sum".into(),
            body: "# Body\ntext".into(),
            published_at: 10,
            expires_at: None,
            min_app_version: None,
            max_app_version: None,
            show_to_new_users: true,
            cover_url: None,
            url: Some("https://example.test/a".into()),
        });
        m
    }

    #[test]
    fn renders_list_article_and_empty_headless() {
        let tr = I18n::default();
        let home = tempfile::tempdir().unwrap();
        let mut m = model();
        let mut d = NewsDialog::default();
        d.open();
        crate::view::testing::render(|ui| {
            d.show(ui.ctx(), &tr, &mut m, home.path(), 1000, "1.0.0", false);
        });
        d.open_article("a");
        crate::view::testing::render(|ui| {
            d.show(ui.ctx(), &tr, &mut m, home.path(), 1000, "1.0.0", false);
        });
        assert!(m.state.is_read("a"), "viewing an article marks it read");
        let mut empty = Model::default();
        d.open();
        crate::view::testing::render(|ui| {
            d.show(ui.ctx(), &tr, &mut empty, home.path(), 1000, "1.0.0", true);
        });
    }
}
