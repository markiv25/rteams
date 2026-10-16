//! Notifications: system notifications, sound, snooze, team notification toggles, ignored repos.
//! Fields without a slot in `AppConfig.notifications` live in its `extra` map.

use eframe::egui;
use rteams_services::claude_root::{format_local, next_local_midnight, now_unix};
use rteams_services::config::AppConfig;
use rteams_services::i18n::I18n;
use serde_json::{json, Value};

use super::{card, extra_bool, extra_set, hint, toggle};

#[derive(Default)]
pub struct State {
    pub new_repo: String,
}

/// (extra key, i18n base, default)
pub const TEAM_TOGGLES: [(&str, &str, bool); 9] = [
    ("task_comments", "taskComments", true),
    ("task_created", "taskCreated", true),
    ("clarifications", "clarifications", true),
    ("all_tasks_completed", "allTasksCompleted", true),
    ("team_launched", "teamLaunched", true),
    ("lead_inbox", "leadInbox", true),
    ("cross_team_message", "crossTeamMessage", true),
    ("tool_approval", "toolApproval", true),
    ("auto_resume_rate_limit", "autoResumeOnRateLimit", false),
];

/// (status id stored, i18n option key)
pub const STATUSES: [(&str, &str); 7] = [
    ("approved", "approved"),
    ("completed", "completed"),
    ("deleted", "deleted"),
    ("in_progress", "in_progress"),
    ("needs_fix", "needsFix"),
    ("pending", "pending"),
    ("review", "review"),
];

/// (minutes, i18n option key); -1 = until tomorrow.
pub const SNOOZE: [(i64, &str); 6] = [
    (15, "15"),
    (30, "30"),
    (60, "60"),
    (120, "120"),
    (240, "240"),
    (-1, "-1"),
];

/// Unix second at which a snooze of `minutes` ends (`-1` = next local midnight).
pub fn snooze_deadline(now: i64, minutes: i64) -> i64 {
    if minutes >= 0 {
        return now + minutes * 60;
    }
    next_local_midnight(now)
}

pub fn statuses(cfg: &AppConfig) -> Vec<String> {
    match cfg.notifications.extra.get("status_change_statuses") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => vec!["completed".into(), "review".into(), "needs_fix".into()],
    }
}

pub fn show(ui: &mut egui::Ui, cfg: &mut AppConfig, st: &mut State, tr: &I18n) -> bool {
    let mut changed = false;
    let now = now_unix();

    card(ui, &tr.t("settings.notifications.settings.title"), |ui| {
        changed |= toggle(
            ui,
            &mut cfg.notifications.enabled,
            &tr.t("settings.notifications.settings.enabled.label"),
            &tr.t("settings.notifications.settings.enabled.description"),
        );
        changed |= toggle(
            ui,
            &mut cfg.notifications.sound,
            &tr.t("settings.notifications.settings.sound.label"),
            &tr.t("settings.notifications.settings.sound.description"),
        );
        let mut sub = extra_bool(&cfg.notifications.extra, "subagent_errors", true);
        if toggle(
            ui,
            &mut sub,
            &tr.t("settings.notifications.settings.subagentErrors.label"),
            &tr.t("settings.notifications.settings.subagentErrors.description"),
        ) {
            extra_set(&mut cfg.notifications.extra, "subagent_errors", json!(sub));
            changed = true;
        }
    });

    card(
        ui,
        &tr.t("settings.notifications.snooze.label"),
        |ui| match cfg.notifications.snooze_until.filter(|t| *t > now) {
            Some(t) => {
                let when = format_local(t);
                hint(
                    ui,
                    &tr.t_args(
                        "settings.notifications.snooze.descriptionWithTime",
                        &[("time", &when)],
                    ),
                );
                if ui
                    .button(tr.t("settings.notifications.snooze.clear"))
                    .clicked()
                {
                    cfg.notifications.snooze_until = None;
                    changed = true;
                }
            }
            None => {
                hint(ui, &tr.t("settings.notifications.snooze.description"));
                egui::ComboBox::from_id_salt("settings-snooze")
                    .selected_text(tr.t("settings.notifications.snooze.selectDuration"))
                    .show_ui(ui, |ui| {
                        for (m, k) in SNOOZE {
                            if ui
                                .selectable_label(
                                    false,
                                    tr.t(&format!("settings.notifications.snooze.options.{k}")),
                                )
                                .clicked()
                            {
                                cfg.notifications.snooze_until = Some(snooze_deadline(now, m));
                                changed = true;
                            }
                        }
                    });
            }
        },
    );

    card(ui, &tr.t("settings.notifications.team.title"), |ui| {
        let mut on = extra_bool(&cfg.notifications.extra, "status_change", true);
        if toggle(
            ui,
            &mut on,
            &tr.t("settings.notifications.team.statusChange.label"),
            &tr.t("settings.notifications.team.statusChange.description"),
        ) {
            extra_set(&mut cfg.notifications.extra, "status_change", json!(on));
            changed = true;
        }
        if on {
            ui.indent("status-change", |ui| {
                let mut solo =
                    extra_bool(&cfg.notifications.extra, "status_change_only_solo", false);
                if toggle(
                    ui,
                    &mut solo,
                    &tr.t("settings.notifications.team.statusChange.onlySolo.label"),
                    &tr.t("settings.notifications.team.statusChange.onlySolo.description"),
                ) {
                    extra_set(
                        &mut cfg.notifications.extra,
                        "status_change_only_solo",
                        json!(solo),
                    );
                    changed = true;
                }
                ui.label(tr.t("settings.notifications.team.statusChange.statuses.label"));
                let mut cur = statuses(cfg);
                ui.horizontal_wrapped(|ui| {
                    for (id, k) in STATUSES {
                        let mut has = cur.iter().any(|s| s == id);
                        let label = tr.t(&format!(
                            "settings.notifications.team.statusChange.statuses.options.{k}"
                        ));
                        if ui.checkbox(&mut has, label).changed() {
                            cur.retain(|s| s != id);
                            if has {
                                cur.push(id.to_string());
                            }
                            extra_set(
                                &mut cfg.notifications.extra,
                                "status_change_statuses",
                                json!(cur),
                            );
                            changed = true;
                        }
                    }
                });
            });
        }
        for (key, base, default) in TEAM_TOGGLES {
            let mut v = extra_bool(&cfg.notifications.extra, key, default);
            if toggle(
                ui,
                &mut v,
                &tr.t(&format!("settings.notifications.team.{base}.label")),
                &tr.t(&format!("settings.notifications.team.{base}.description")),
            ) {
                extra_set(&mut cfg.notifications.extra, key, json!(v));
                changed = true;
            }
        }
    });

    card(
        ui,
        &tr.t("settings.notifications.ignoredRepositories.title"),
        |ui| {
            hint(
                ui,
                &tr.t("settings.notifications.ignoredRepositories.description"),
            );
            if cfg.notifications.ignored_repos.is_empty() {
                hint(
                    ui,
                    &tr.t("settings.notifications.ignoredRepositories.empty"),
                );
            }
            let mut remove = None;
            for (i, r) in cfg.notifications.ignored_repos.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.monospace(r);
                    if ui.small_button("x").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                cfg.notifications.ignored_repos.remove(i);
                changed = true;
            }
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut st.new_repo)
                        .desired_width(240.0)
                        .hint_text(
                            tr.t("settings.notifications.ignoredRepositories.selectPlaceholder"),
                        ),
                );
                let name = st.new_repo.trim().to_string();
                if ui
                    .add_enabled(!name.is_empty(), egui::Button::new("+"))
                    .clicked()
                {
                    if !cfg.notifications.ignored_repos.contains(&name) {
                        cfg.notifications.ignored_repos.push(name);
                        changed = true;
                    }
                    st.new_repo.clear();
                }
            });
        },
    );
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snooze_deadlines() {
        assert_eq!(snooze_deadline(1000, 15), 1900);
        let now = 1_700_000_000;
        let t = snooze_deadline(now, -1);
        assert!(t > now && t <= now + 25 * 3600);
    }

    #[test]
    fn default_statuses_and_override() {
        let mut c = AppConfig::default();
        assert!(statuses(&c).contains(&"review".to_string()));
        extra_set(
            &mut c.notifications.extra,
            "status_change_statuses",
            json!(["approved"]),
        );
        assert_eq!(statuses(&c), vec!["approved"]);
    }
}
