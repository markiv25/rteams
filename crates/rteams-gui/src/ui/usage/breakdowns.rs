//! Breakdowns tab: teams, agents and models as share tables.

use eframe::egui::{self, RichText};
use rteams_services::usage::summary::{Row, Summary};

use super::charts::{card, empty, fmt_cost, fmt_tokens};
use crate::theme;
use crate::ui::FeatureCtx;

fn table(
    ui: &mut egui::Ui,
    id: &str,
    rows: &[Row],
    total: u64,
    name: &dyn Fn(&str) -> String,
    none: &str,
    c: &FeatureCtx,
) {
    if rows.is_empty() {
        empty(ui, none);
        return;
    }
    let t = |s: &str| c.tr.t(&format!("dashboard.tokenUsage.{s}"));
    egui::Grid::new(id)
        .striped(true)
        .num_columns(5)
        .spacing([14.0, 4.0])
        .show(ui, |ui| {
            for h in [
                t("labels.total"),
                t("labels.tokens"),
                t("labels.cost"),
                t("metrics.requests"),
                t("labels.share"),
            ] {
                ui.label(RichText::new(h).small().color(theme::MUTED));
            }
            ui.end_row();
            for r in rows.iter().take(50) {
                ui.label(RichText::new(name(&r.key)).color(theme::TEXT));
                ui.label(fmt_tokens(r.tokens));
                ui.label(fmt_cost(r.cost));
                ui.label(r.requests.to_string());
                let share = if total == 0 {
                    0.0
                } else {
                    r.tokens as f64 / total as f64 * 100.0
                };
                ui.label(format!("{share:.0}%"));
                ui.end_row();
            }
        });
}

pub fn show(ui: &mut egui::Ui, c: &FeatureCtx, s: &Summary) {
    let t = |k: &str| c.tr.t(&format!("dashboard.tokenUsage.{k}"));
    let total = s.totals.tokens;
    let anon = t("labels.unassigned");
    let team_name = |k: &str| {
        if k.is_empty() {
            anon.clone()
        } else {
            k.to_string()
        }
    };
    let member_name = |k: &str| {
        let (team, m) = k.split_once('/').unwrap_or(("", k));
        match (team.is_empty(), m.is_empty()) {
            (_, true) => t("labels.unknownAgent"),
            (true, false) => m.to_string(),
            _ => format!("{team}/{m}"),
        }
    };
    let ident = |k: &str| k.to_string();
    card(ui, &t("panels.teams"), |ui| {
        table(
            ui,
            "bd_teams",
            &s.by_team,
            total,
            &team_name,
            &t("empty.noTeams"),
            c,
        )
    });
    ui.add_space(8.0);
    card(ui, &t("panels.agents"), |ui| {
        table(
            ui,
            "bd_agents",
            &s.by_member,
            total,
            &member_name,
            &t("empty.noAgents"),
            c,
        )
    });
    ui.add_space(8.0);
    card(ui, &t("panels.models"), |ui| {
        table(
            ui,
            "bd_models",
            &s.by_model,
            total,
            &ident,
            &t("empty.noModelData"),
            c,
        )
    });
}
