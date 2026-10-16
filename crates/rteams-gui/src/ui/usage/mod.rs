//! Usage screen (original: dashboard token-usage): tabs Overview | Activity | Breakdowns | Runs,
//! refresh, cache-token switch, team filter, date presets and the budget dialog. Scanning runs off
//! the UI thread in `rteams_services::usage::UsageState`; this screen only reads snapshots.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, RichText};
use rteams_services::usage::budget::{self, BudgetStatus};
use rteams_services::usage::ledger::now_ms;
use rteams_services::usage::summary::{self, Filter, Preset, Summary};
use rteams_services::usage::UsageState;

use super::{FeatureCtx, Screen, ShellRequest, TabId};
use crate::icons::Icon;
use crate::theme;
use crate::ui::dashboard::kit;

pub mod activity;
pub mod breakdowns;
pub mod budget_dialog;
pub mod charts;
pub mod overview;
pub mod runs;

const STALE_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Activity,
    Breakdowns,
    Runs,
}

struct Computed {
    key: (u64, Filter),
    summary: Arc<Summary>,
    statuses: Vec<BudgetStatus>,
    teams: Vec<String>,
}

/// The shell owns one of these, registers it as the `TabId::Usage` screen and calls `tick`
/// every frame (it triggers the stale refresh and turns fresh budget alerts into toasts).
pub struct UsageScreen {
    state: UsageState,
    tab: Tab,
    preset: Preset,
    custom_from: String,
    custom_to: String,
    include_cache: bool,
    teams: BTreeSet<String>,
    computed: Option<Computed>,
    dialog: budget_dialog::BudgetDialog,
    alerted_version: u64,
}

impl UsageScreen {
    pub fn new(home: &std::path::Path) -> Self {
        Self::with_state(UsageState::for_home(home))
    }

    pub fn with_state(state: UsageState) -> Self {
        UsageScreen {
            state,
            tab: Tab::Overview,
            preset: Preset::AllTime,
            custom_from: String::new(),
            custom_to: String::new(),
            include_cache: false,
            teams: BTreeSet::new(),
            computed: None,
            dialog: budget_dialog::BudgetDialog::default(),
            alerted_version: 0,
        }
    }

    fn filter(&self) -> Filter {
        let (mut from_ms, mut to_ms) = self.preset.range(now_ms());
        if self.preset == Preset::Custom {
            from_ms = summary::parse_day(&self.custom_from);
            to_ms = summary::parse_day(&self.custom_to).map(|d| d + 86_400_000);
        }
        Filter {
            from_ms,
            to_ms,
            teams: self.teams.clone(),
            include_cache: self.include_cache,
        }
    }

    fn compute(&mut self, home: &std::path::Path) -> &Computed {
        let key = (self.state.version(), self.filter());
        if self.computed.as_ref().is_none_or(|c| c.key != key) {
            let ledger = self.state.snapshot();
            let now = now_ms();
            let summary = Arc::new(summary::summarize(&ledger.records, &key.1, now));
            let statuses = budget::build_status(&budget::load(home), &ledger.records, now);
            let teams = summary::teams_of(&ledger.records);
            self.computed = Some(Computed {
                key,
                summary,
                statuses,
                teams,
            });
        }
        self.computed.as_ref().expect("just computed")
    }
}

impl Screen for UsageScreen {
    fn id(&self) -> TabId {
        TabId::Usage
    }

    fn title_key(&self) -> &'static str {
        "dashboard.tokenUsage.title"
    }

    fn tick(&mut self, ctx: &egui::Context, c: &mut FeatureCtx) {
        self.state.refresh_if_stale(STALE_MS);
        let v = self.state.version();
        if v != self.alerted_version && self.state.is_loaded() {
            self.alerted_version = v;
            for a in self.state.budget_alerts() {
                c.req
                    .push(ShellRequest::Toast(format!("{}: {}", a.title, a.message)));
            }
        }
        ctx.request_repaint_after(Duration::from_millis(if self.state.is_scanning() {
            250
        } else {
            10_000
        }));
    }

    fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        let tr = c.tr;
        let t = |k: &str| tr.t(&format!("dashboard.tokenUsage.{k}"));
        let home = c.home.to_path_buf();
        let scanning = self.state.is_scanning();
        let errors = self.state.snapshot().errors;

        kit::checkbox_visuals(ui);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.label(RichText::new(t("title")).size(22.0).strong());
            if scanning || !self.state.is_loaded() {
                ui.label(RichText::new(tr.t("common.states.loading")).color(theme::MUTED));
            }
            if errors > 0 {
                theme::pill(ui, t("partialData"), theme::WARNING);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::pill_icon_button(ui, Some(Icon::Refresh), &t("actions.refresh"), false)
                    .clicked()
                {
                    self.state.refresh();
                }
            });
        });
        ui.add_space(8.0);
        // Filters: each control sits on its own, evenly spaced, on a wrapping row.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(12.0, 8.0);
            let all_teams = self.compute(&home).teams.clone();
            let label = match self.teams.len() {
                0 => t("filters.allTeams"),
                n => tr.t_args(
                    "dashboard.tokenUsage.filters.selectedTeams",
                    &[("count", &n.to_string())],
                ),
            };
            ui.menu_button(label, |ui| {
                ui.label(
                    RichText::new(t("filters.filterTeams"))
                        .small()
                        .color(theme::MUTED),
                );
                if ui
                    .selectable_label(self.teams.is_empty(), t("filters.allTeams"))
                    .clicked()
                {
                    self.teams.clear();
                }
                for team in &all_teams {
                    let name = if team.is_empty() {
                        t("labels.unassigned")
                    } else {
                        team.clone()
                    };
                    let mut on = self.teams.contains(team);
                    if ui.checkbox(&mut on, name).changed() {
                        if on {
                            self.teams.insert(team.clone());
                        } else {
                            self.teams.remove(team);
                        }
                    }
                }
            });
            let preset_label = |p: Preset| {
                if p == Preset::Custom {
                    t("dateRange.customRange")
                } else {
                    t(&format!("dateRange.presets.{}", p.key()))
                }
            };
            egui::ComboBox::from_id_salt("usage_preset")
                .selected_text(preset_label(self.preset))
                .show_ui(ui, |ui| {
                    for p in Preset::ALL {
                        ui.selectable_value(&mut self.preset, p, preset_label(p));
                    }
                });
            if self.preset == Preset::Custom {
                ui.add(
                    egui::TextEdit::singleline(&mut self.custom_from)
                        .hint_text("YYYY-MM-DD")
                        .desired_width(100.0),
                );
                ui.label("\u{2013}");
                ui.add(
                    egui::TextEdit::singleline(&mut self.custom_to)
                        .hint_text("YYYY-MM-DD")
                        .desired_width(100.0),
                );
                if summary::parse_day(&self.custom_from).is_none()
                    || summary::parse_day(&self.custom_to).is_none()
                {
                    ui.label(
                        RichText::new(t("dateRange.selectRange"))
                            .small()
                            .color(theme::MUTED),
                    );
                }
            }
            ui.checkbox(&mut self.include_cache, t("controls.includeCacheTokens"));
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            for (tab, key) in [
                (Tab::Overview, "tabs.overview"),
                (Tab::Activity, "tabs.activity"),
                (Tab::Breakdowns, "tabs.breakdowns"),
                (Tab::Runs, "tabs.runs"),
            ] {
                if theme::pill_button(ui, &t(key), self.tab == tab).clicked() {
                    self.tab = tab;
                }
            }
        });
        ui.add_space(8.0);

        let (sum, statuses, teams) = {
            let comp = self.compute(&home);
            (
                comp.summary.clone(),
                comp.statuses.clone(),
                comp.teams.clone(),
            )
        };
        let mut action = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                match self.tab {
                    Tab::Overview => action = overview::show(ui, c, &sum, &statuses),
                    Tab::Activity => activity::show(ui, c, &sum),
                    Tab::Breakdowns => breakdowns::show(ui, c, &sum),
                    Tab::Runs => runs::show(ui, c, &sum),
                }
                ui.add_space(32.0);
            });
        if let Some(overview::OverviewAction::ConfigureBudget(scope)) = action {
            self.dialog.open(&home, scope);
        }
        if self.dialog.show(ui.ctx(), tr, &home, &teams) {
            self.computed = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{testing::render, Intents};
    use rteams_services::i18n::I18n;
    use rteams_services::usage::ledger::Scanner;

    fn home_with_runs(tag: &str) -> std::path::PathBuf {
        let home =
            std::env::temp_dir().join(format!("rteams-usage-ui-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let now = now_ms();
        for (i, member) in ["ada", "bob"].iter().enumerate() {
            let d = home.join("t/runs").join(member);
            std::fs::create_dir_all(&d).unwrap();
            for day in 0..5u64 {
                let ms = now - day * 86_400_000 - i as u64;
                std::fs::write(
                    d.join(format!("{ms}.jsonl")),
                    r#"{"type":"system","subtype":"init","model":"claude-sonnet-4-5"}
{"type":"result","total_cost_usd":0.5,"usage":{"input_tokens":1000,"output_tokens":500,"cache_read_input_tokens":9000}}
"#,
                )
                .unwrap();
            }
        }
        home
    }

    fn wait(st: &UsageState) {
        for _ in 0..500 {
            if st.is_loaded() && !st.is_scanning() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("scan did not finish");
    }

    #[test]
    fn renders_every_tab_with_data_and_empty() {
        for (home, data) in [
            (home_with_runs("render"), true),
            (std::env::temp_dir().join("rteams-usage-empty"), false),
        ] {
            std::fs::create_dir_all(&home).unwrap();
            let st = UsageState::new(Scanner::app_only(&home));
            let mut screen = UsageScreen::with_state(st.clone());
            let tr = I18n::default();
            st.refresh();
            wait(&st);
            assert_eq!(st.snapshot().records.is_empty(), !data);
            for tab in [Tab::Overview, Tab::Activity, Tab::Breakdowns, Tab::Runs] {
                screen.tab = tab;
                screen.include_cache = tab == Tab::Runs;
                render(|ui| {
                    let mut out = Intents::default();
                    let mut req = Vec::new();
                    let mut c = FeatureCtx {
                        home: &home,
                        team: None,
                        view: None,
                        out: &mut out,
                        tr: &tr,
                        req: &mut req,
                    };
                    screen.tick(ui.ctx(), &mut c);
                    screen.show(ui, &mut c);
                });
            }
            screen.dialog.open(&home, None);
            render(|ui| {
                let mut out = Intents::default();
                let mut req = Vec::new();
                let mut c = FeatureCtx {
                    home: &home,
                    team: None,
                    view: None,
                    out: &mut out,
                    tr: &tr,
                    req: &mut req,
                };
                screen.show(ui, &mut c);
            });
        }
    }

    #[test]
    fn summary_reflects_filters_and_cache_toggle() {
        let home = home_with_runs("filters");
        let st = UsageState::new(Scanner::app_only(&home));
        st.refresh();
        wait(&st);
        let mut s = UsageScreen::with_state(st);
        let total = s.compute(&home).summary.totals.tokens;
        assert_eq!(total, 10 * 1500);
        s.include_cache = true;
        assert_eq!(s.compute(&home).summary.totals.tokens, 10 * 10_500);
        s.teams.insert("other".into());
        assert_eq!(s.compute(&home).summary.totals.requests, 0);
        assert_eq!(s.id(), TabId::Usage);
    }

    #[test]
    fn tick_toasts_budget_alerts_once() {
        let home = home_with_runs("tick");
        let b = budget::BudgetSettings {
            global: Some(budget::Budget {
                token_limit: Some(1000),
                ..Default::default()
            }),
            ..Default::default()
        };
        budget::save(&home, &b).unwrap();
        let st = UsageState::new(Scanner::app_only(&home));
        let mut s = UsageScreen::with_state(st.clone());
        st.refresh();
        wait(&st);
        let ctx = egui::Context::default();
        let tr = I18n::default();
        let mut out = Intents::default();
        let mut req = Vec::new();
        let mut c = FeatureCtx {
            home: &home,
            team: None,
            view: None,
            out: &mut out,
            tr: &tr,
            req: &mut req,
        };
        s.tick(&ctx, &mut c);
        s.tick(&ctx, &mut c);
        assert_eq!(
            req.iter()
                .filter(|r| matches!(r, ShellRequest::Toast(_)))
                .count(),
            2
        ); // 80% and 100%
    }
}
