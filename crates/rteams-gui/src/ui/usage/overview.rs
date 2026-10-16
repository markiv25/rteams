//! Overview tab: metric tiles, usage trend, burn rate, billing split, runtime mix, model usage,
//! budget alerts.

use eframe::egui::{self, Color32, RichText};
use rteams_services::usage::budget::{BudgetStatus, Level};
use rteams_services::usage::summary::Summary;

use super::charts::{card, empty, fmt_cost, fmt_tokens, segment_bar, trend};
use crate::theme;
use crate::ui::dashboard::kit;
use crate::ui::FeatureCtx;

/// What the tab asks the screen to do.
pub enum OverviewAction {
    /// Open the budget dialog for a scope (`None` = all teams).
    ConfigureBudget(Option<String>),
}

const SEG_COLORS: [Color32; 4] = [
    Color32::from_rgb(0x81, 0x8c, 0xf8),
    Color32::from_rgb(0x38, 0xbd, 0xf8),
    Color32::from_rgb(0x14, 0xb8, 0xa6),
    Color32::from_rgb(0xf5, 0x9e, 0x0b),
];

pub fn show(
    ui: &mut egui::Ui,
    c: &FeatureCtx,
    s: &Summary,
    statuses: &[BudgetStatus],
) -> Option<OverviewAction> {
    let tr = c.tr;
    let k = |s: &str| format!("dashboard.tokenUsage.{s}");
    let t = |s: &str| tr.t(&k(s));
    let mut action = None;
    let tot = &s.totals;

    if tot.requests == 0 {
        empty(ui, &t("empty.noUsageYet"));
        ui.label(
            RichText::new(t("empty.noUsageDetail"))
                .small()
                .color(theme::MUTED),
        );
    }

    let mut tiles: Vec<(String, String, String)> = vec![
        (
            t("metrics.totalTokens"),
            fmt_tokens(tot.tokens),
            tr.t_args(
                &k("metrics.totalTokensDetail"),
                &[
                    ("input", &fmt_tokens(tot.input)),
                    ("output", &fmt_tokens(tot.output)),
                ],
            ),
        ),
        (
            t("metrics.requests"),
            tot.requests.to_string(),
            tr.t_args(
                &k("metrics.estimatedRequests"),
                &[
                    ("pricedCount", &tot.priced.to_string()),
                    ("totalCount", &tot.requests.to_string()),
                ],
            ),
        ),
        (t("metrics.appRuns"), tot.runs.to_string(), String::new()),
        (
            t("metrics.apiEquivalent"),
            fmt_cost(tot.cost),
            t("billingSplit.apiEquivalentHelp"),
        ),
    ];
    if tot.reasoning > 0 {
        tiles.push((
            t("segments.reasoning"),
            fmt_tokens(tot.reasoning),
            tr.t_args(
                &k("metrics.reasoningTokens"),
                &[("count", &tot.reasoning.to_string())],
            ),
        ));
    }
    // Equal-width tiles that always span the full row (no dead gap at the right).
    let n = tiles.len() as f32;
    let min_w = ((ui.available_width() - 14.0 * (n - 1.0)) / n).clamp(170.0, 400.0);
    kit::grid(ui, min_w, &tiles, |ui, w, (label, value, detail)| {
        kit::tile(ui, label, w, 108.0, false, |ui| {
            ui.label(RichText::new(label).size(13.0).color(theme::TEXT_2));
            ui.label(RichText::new(value).size(24.0).strong().color(theme::TEXT));
            if !detail.is_empty() {
                ui.add(
                    egui::Label::new(RichText::new(detail).size(12.5).color(theme::MUTED))
                        .truncate(),
                );
            }
        });
    });
    ui.add_space(8.0);

    card(ui, &t("panels.usageTrend"), |ui| {
        if s.by_day.is_empty() {
            empty(ui, &t("empty.noTrendData"));
        } else {
            trend(ui, "usage_trend", &s.by_day, 180.0);
        }
    });
    ui.add_space(8.0);

    ui.columns(2, |cols| {
        card(&mut cols[0], &t("panels.burnRate"), |ui| {
            if s.burn.days == 0 {
                empty(ui, &t("empty.noChartData"));
                return;
            }
            egui::Grid::new("burn").num_columns(2).show(ui, |ui| {
                for (label, value) in [
                    (t("burnRate.dailyCost"), fmt_cost(s.burn.daily_cost)),
                    (
                        t("burnRate.dailyTokens"),
                        fmt_tokens(s.burn.daily_tokens as u64),
                    ),
                    (t("burnRate.forecastWeek"), fmt_cost(s.burn.forecast_week)),
                    (t("burnRate.forecastMonth"), fmt_cost(s.burn.forecast_month)),
                ] {
                    ui.label(RichText::new(label).color(theme::TEXT_2));
                    ui.label(RichText::new(value).strong());
                    ui.end_row();
                }
            });
            ui.label(
                RichText::new(
                    tr.t_args(&k("burnRate.basis"), &[("days", &s.burn.days.to_string())]),
                )
                .small()
                .color(theme::MUTED),
            );
        });
        card(&mut cols[1], &t("panels.billingSplit"), |ui| {
            if s.by_billing.is_empty() {
                empty(ui, &t("empty.noChartData"));
            }
            for r in &s.by_billing {
                let label = match r.key.as_str() {
                    "api" => t("billingSplit.apiBillable"),
                    "subscription" => t("metrics.subscriptionUsage"),
                    "free" => t("billingSplit.freeUsage"),
                    _ => t("metrics.legacyUnclassified"),
                };
                ui.label(
                    RichText::new(format!(
                        "{label}: {} / {}",
                        tr.t_args(&k("labels.reqCount"), &[("count", &r.requests.to_string())]),
                        fmt_tokens(r.tokens)
                    ))
                    .color(theme::TEXT_2),
                );
            }
        });
    });
    ui.add_space(8.0);

    card(ui, &t("panels.runtimeMix"), |ui| {
        if s.by_runtime.is_empty() {
            empty(ui, &t("empty.noChartData"));
        } else {
            let segs: Vec<_> = s
                .by_runtime
                .iter()
                .enumerate()
                .map(|(i, r)| (r.key.clone(), r.tokens, SEG_COLORS[i % SEG_COLORS.len()]))
                .collect();
            segment_bar(ui, &segs);
        }
    });
    ui.add_space(8.0);

    card(ui, &t("panels.modelUsage"), |ui| {
        if s.by_model.is_empty() {
            empty(ui, &t("empty.noModelData"));
            return;
        }
        let segs = [
            (t("segments.input"), tot.input, SEG_COLORS[0]),
            (t("segments.output"), tot.output, SEG_COLORS[1]),
            (
                t("segments.cache"),
                tot.cache_creation + tot.cache_read,
                SEG_COLORS[2],
            ),
            (t("segments.reasoning"), tot.reasoning, SEG_COLORS[3]),
        ];
        segment_bar(ui, &segs);
        ui.add_space(4.0);
        for r in s.by_model.iter().take(5) {
            ui.label(
                RichText::new(tr.t_args(
                    &k("tooltips.tokensCost"),
                    &[
                        ("label", &r.key),
                        ("tokens", &fmt_tokens(r.tokens)),
                        ("cost", &fmt_cost(r.cost)),
                    ],
                ))
                .color(theme::TEXT_2),
            );
        }
        if s.by_model.len() > 5 {
            let rest: u64 = s.by_model[5..].iter().map(|r| r.tokens).sum();
            ui.label(
                RichText::new(format!("{}: {}", t("labels.otherModels"), fmt_tokens(rest)))
                    .color(theme::MUTED),
            );
        }
    });
    ui.add_space(8.0);

    card(ui, &t("panels.budgetAlerts"), |ui| {
        if statuses.is_empty() {
            empty(ui, &t("budgets.noBudgets"));
        }
        for st in statuses {
            ui.horizontal_wrapped(|ui| {
                let (label, color) = match st.level {
                    Level::Ok => (t("budgets.ok"), theme::SUCCESS),
                    Level::Warning => (t("budgets.warning"), theme::WARNING),
                    Level::Critical => (t("budgets.critical"), theme::ERROR),
                };
                theme::pill(ui, label, color);
                ui.label(
                    RichText::new(st.team.clone().unwrap_or_else(|| t("budgets.allTeams")))
                        .strong(),
                );
                if let Some(l) = st.token_limit {
                    ui.label(tr.t_args(
                        &k("budgets.tokenLimitDetail"),
                        &[
                            ("tokens", &fmt_tokens(st.tokens_used)),
                            ("limit", &fmt_tokens(l)),
                        ],
                    ));
                }
                if let Some(l) = st.cost_limit {
                    ui.label(tr.t_args(
                        &k("budgets.costLimitDetail"),
                        &[("cost", &fmt_cost(st.cost_used)), ("limit", &fmt_cost(l))],
                    ));
                }
                if let Some(n) = st.next_threshold {
                    ui.label(
                        RichText::new(
                            tr.t_args(&k("budgets.card.next"), &[("threshold", &n.to_string())]),
                        )
                        .small()
                        .color(theme::MUTED),
                    );
                }
                if ui.small_button(t("budgets.editor.title")).clicked() {
                    action = Some(OverviewAction::ConfigureBudget(st.team.clone()));
                }
            });
        }
        if ui.button(t("budgets.editor.title")).clicked() {
            action = Some(OverviewAction::ConfigureBudget(None));
        }
    });
    action
}
