//! Runs tab: most recent runs and the most expensive ones.

use eframe::egui::{self, RichText};
use rteams_services::usage::summary::{day_label, RunRow, Summary};

use super::charts::{card, empty, fmt_cost, fmt_tokens};
use crate::theme;
use crate::ui::{FeatureCtx, ShellRequest};

fn list(ui: &mut egui::Ui, id: &str, runs: &[RunRow], c: &mut FeatureCtx) {
    let t = |k: &str| c.tr.t(&format!("dashboard.tokenUsage.{k}"));
    if runs.is_empty() {
        empty(ui, &t("empty.noRuns"));
        return;
    }
    let mut open = None;
    egui::Grid::new(id)
        .striped(true)
        .spacing([14.0, 4.0])
        .show(ui, |ui| {
            for r in runs {
                ui.label(
                    RichText::new(day_label(r.ts_ms))
                        .small()
                        .color(theme::MUTED),
                );
                let who = if r.team.is_empty() {
                    t("labels.unassigned")
                } else {
                    format!("{}/{}", r.team, r.member)
                };
                if !r.team.is_empty() {
                    if ui.link(&who).clicked() {
                        open = Some(r.team.clone());
                    }
                } else {
                    ui.label(who);
                }
                ui.label(RichText::new(&r.model).small().color(theme::TEXT_2));
                ui.label(fmt_tokens(r.tokens));
                ui.label(fmt_cost(r.cost));
                ui.end_row();
            }
        });
    if let Some(team) = open {
        c.req.push(ShellRequest::OpenTeam(team));
    }
}

pub fn show(ui: &mut egui::Ui, c: &mut FeatureCtx, s: &Summary) {
    let t = |k: &str| c.tr.t(&format!("dashboard.tokenUsage.{k}"));
    let (recent, expensive) = (t("panels.recentRuns"), t("panels.expensiveRuns"));
    card(ui, &recent, |ui| list(ui, "runs_recent", &s.recent_runs, c));
    ui.add_space(8.0);
    card(ui, &expensive, |ui| {
        list(ui, "runs_expensive", &s.expensive_runs, c)
    });
}
