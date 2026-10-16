//! Activity tab: day heatmap, streak and tokens per weekday.

use eframe::egui::{self, RichText};
use rteams_services::usage::summary::Summary;

use super::charts::{card, empty, fmt_tokens, heatmap};
use crate::theme;
use crate::ui::FeatureCtx;

pub fn show(ui: &mut egui::Ui, c: &FeatureCtx, s: &Summary) {
    let tr = c.tr;
    let t = |s: &str| tr.t(&format!("dashboard.tokenUsage.{s}"));
    card(ui, &t("panels.activityByDay"), |ui| {
        if s.by_day.is_empty() {
            empty(ui, &t("empty.noActivityData"));
            return;
        }
        egui::ScrollArea::horizontal()
            .id_salt("heat_scroll")
            .show(ui, |ui| {
                heatmap(ui, &s.by_day, &t("labels.less"), &t("labels.more"));
            });
        if s.streak > 0 {
            ui.label(
                RichText::new(tr.t_args(
                    "dashboard.tokenUsage.labels.streakCount",
                    &[("count", &s.streak.to_string())],
                ))
                .color(theme::TEXT_2),
            );
        }
    });
    ui.add_space(8.0);
    card(ui, &t("labels.tokens"), |ui| {
        let days = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
        let max = s.weekday_tokens.iter().copied().max().unwrap_or(0).max(1);
        egui::Grid::new("weekday_grid")
            .num_columns(3)
            .show(ui, |ui| {
                for (i, d) in days.iter().enumerate() {
                    ui.label(RichText::new(t(&format!("weekdays.{d}"))).color(theme::TEXT_2));
                    ui.add(
                        egui::ProgressBar::new(s.weekday_tokens[i] as f32 / max as f32)
                            .desired_width(160.0)
                            .fill(theme::ACCENT),
                    );
                    ui.label(fmt_tokens(s.weekday_tokens[i]));
                    ui.end_row();
                }
            });
    });
}
