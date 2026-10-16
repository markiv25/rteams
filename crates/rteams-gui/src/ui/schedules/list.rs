//! The schedule list: search, status filter, one card per schedule with next run, toggle and
//! actions, and the last runs.

use std::collections::HashMap;

use eframe::egui::{self, RichText};
use rteams_core::Schedule;
use rteams_services::i18n::I18n;
use rteams_services::schedule::{cron, history::RunRecord, runner};

use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusFilter {
    #[default]
    All,
    Active,
    Paused,
}

#[derive(Debug, Default)]
pub struct ListState {
    pub search: String,
    pub status: StatusFilter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListAction {
    Add,
    Edit(String),
    Delete(String),
    Toggle(String, bool),
    RunNow(String),
    OpenLog(String),
}

/// Schedules that pass the filters.
pub fn visible<'a>(all: &'a [Schedule], st: &ListState) -> Vec<&'a Schedule> {
    let q = st.search.trim().to_lowercase();
    all.iter()
        .filter(|s| match st.status {
            StatusFilter::All => true,
            StatusFilter::Active => s.enabled,
            StatusFilter::Paused => !s.enabled,
        })
        .filter(|s| {
            q.is_empty()
                || s.id.to_lowercase().contains(&q)
                || s.member.to_lowercase().contains(&q)
                || s.prompt.to_lowercase().contains(&q)
        })
        .collect()
}

/// `Every 30 minutes` / the cron description.
pub fn cadence(s: &Schedule) -> String {
    match s.cron.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => cron::describe(c),
        None if s.every_minutes.is_multiple_of(60) && s.every_minutes >= 60 => {
            let h = s.every_minutes / 60;
            if h == 1 {
                "Every hour".into()
            } else {
                format!("Every {h} hours")
            }
        }
        None => format!("Every {} minutes", s.every_minutes),
    }
}

pub fn show(
    ui: &mut egui::Ui,
    tr: &I18n,
    st: &mut ListState,
    all: &[Schedule],
    history: &HashMap<String, Vec<RunRecord>>,
    now_ms: i64,
) -> Vec<ListAction> {
    let mut out = Vec::new();
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut st.search)
                .hint_text(tr.t("common.schedules.searchPlaceholder"))
                .desired_width(220.0),
        );
        for (f, key) in [
            (StatusFilter::All, "common.schedules.status.all"),
            (StatusFilter::Active, "common.schedules.status.active"),
            (StatusFilter::Paused, "common.schedules.status.paused"),
        ] {
            if ui.selectable_label(st.status == f, tr.t(key)).clicked() {
                st.status = f;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button(tr.t("common.schedules.actions.addSchedule"))
                .clicked()
            {
                out.push(ListAction::Add);
            }
        });
    });
    ui.add_space(6.0);
    if all.is_empty() {
        ui.label(RichText::new(tr.t("team.schedule.empty.title")).strong());
        ui.label(RichText::new(tr.t("team.schedule.empty.description")).color(theme::MUTED));
        return out;
    }
    let rows = visible(all, st);
    if rows.is_empty() {
        ui.label(RichText::new(tr.t("common.schedules.empty.noMatches")).color(theme::MUTED));
        return out;
    }
    for s in rows {
        let runs = history.get(&s.id).map(Vec::as_slice).unwrap_or(&[]);
        egui::Frame::new()
            .fill(theme::CARD)
            .corner_radius(6)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&s.member).strong());
                    ui.label(RichText::new(cadence(s)).color(theme::TEXT_2));
                    if let Some(tz) = s.timezone.as_deref().filter(|t| !t.is_empty()) {
                        ui.label(RichText::new(tz).size(11.0).color(theme::MUTED));
                    }
                    let (label, color) = if s.enabled {
                        (tr.t("common.schedules.status.active"), theme::SUCCESS)
                    } else {
                        (tr.t("common.schedules.status.paused"), theme::WARNING)
                    };
                    theme::pill(ui, label, color);
                });
                ui.label(RichText::new(s.prompt.lines().next().unwrap_or("")).color(theme::TEXT_2));
                let last = runs.iter().map(|r| r.scheduled_for).max();
                if let Some(next) = runner::next_run(s, last, now_ms) {
                    let when = cron::format_in(next, s.timezone.as_deref());
                    ui.label(
                        RichText::new(tr.t_args("team.schedule.nextRun", &[("next", &when)]))
                            .size(12.0)
                            .color(theme::MUTED),
                    );
                }
                match runs.last() {
                    None => {
                        ui.label(
                            RichText::new(tr.t("team.schedule.runHistory.empty"))
                                .size(12.0)
                                .color(theme::MUTED),
                        );
                    }
                    Some(r) => {
                        ui.horizontal(|ui| {
                            let color = status_color(r.status);
                            theme::pill(ui, tr.t(r.status.key()), color);
                            ui.label(
                                RichText::new(cron::format_in(
                                    r.scheduled_for,
                                    s.timezone.as_deref(),
                                ))
                                .size(12.0)
                                .color(theme::MUTED),
                            );
                            if ui
                                .small_button(tr.t("team.schedule.runLog.title"))
                                .clicked()
                            {
                                out.push(ListAction::OpenLog(s.id.clone()));
                            }
                        });
                    }
                }
                ui.horizontal(|ui| {
                    if ui
                        .small_button(tr.t("common.schedules.actions.runNow"))
                        .clicked()
                    {
                        out.push(ListAction::RunNow(s.id.clone()));
                    }
                    let key = if s.enabled {
                        "common.schedules.actions.pause"
                    } else {
                        "common.schedules.actions.resume"
                    };
                    if ui.small_button(tr.t(key)).clicked() {
                        out.push(ListAction::Toggle(s.id.clone(), !s.enabled));
                    }
                    if ui
                        .small_button(tr.t("common.schedules.actions.edit"))
                        .clicked()
                    {
                        out.push(ListAction::Edit(s.id.clone()));
                    }
                    if ui
                        .small_button(tr.t("common.schedules.actions.delete"))
                        .clicked()
                    {
                        out.push(ListAction::Delete(s.id.clone()));
                    }
                });
            });
        ui.add_space(6.0);
    }
    out
}

pub fn status_color(s: rteams_services::schedule::history::RunStatus) -> egui::Color32 {
    use rteams_services::schedule::history::RunStatus::*;
    match s {
        Completed => theme::SUCCESS,
        Failed => theme::ERROR,
        Running | WarmingUp | Warm => theme::INFO,
        Pending => theme::MUTED,
        Interrupted | Cancelled => theme::WARNING,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(id: &str, member: &str, enabled: bool) -> Schedule {
        Schedule {
            id: id.into(),
            member: member.into(),
            prompt: "Review PRs".into(),
            every_minutes: 120,
            enabled,
            cron: None,
            timezone: None,
            warmup_minutes: None,
        }
    }

    #[test]
    fn filters_by_status_and_search() {
        let all = vec![s("s1", "ada", true), s("s2", "bob", false)];
        let mut st = ListState::default();
        assert_eq!(visible(&all, &st).len(), 2);
        st.status = StatusFilter::Paused;
        assert_eq!(visible(&all, &st)[0].id, "s2");
        st.status = StatusFilter::All;
        st.search = "ADA".into();
        assert_eq!(visible(&all, &st)[0].id, "s1");
        st.search = "review".into();
        assert_eq!(visible(&all, &st).len(), 2);
    }

    #[test]
    fn cadence_text() {
        let mut x = s("a", "b", true);
        assert_eq!(cadence(&x), "Every 2 hours");
        x.every_minutes = 45;
        assert_eq!(cadence(&x), "Every 45 minutes");
        x.cron = Some("0 9 * * 1-5".into());
        assert_eq!(cadence(&x), "At 09:00, Monday through Friday");
    }
}
