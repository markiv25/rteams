//! Create / edit dialog body for a schedule: member, prompt, cron expression with presets,
//! timezone, warm-up, validation and a preview of the next fire times.

use eframe::egui::{self, RichText};
use rteams_core::Schedule;
use rteams_services::i18n::I18n;
use rteams_services::schedule::cron;

use crate::theme;

const PREVIEW_COUNT: usize = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct Editor {
    /// `None` while creating (an id is assigned on save).
    pub id: Option<String>,
    pub member: String,
    pub prompt: String,
    pub expr: String,
    pub timezone: String,
    pub warmup: u32,
    pub enabled: bool,
}

pub enum EditorResult {
    Save(Schedule),
    Cancel,
}

impl Editor {
    pub fn new(member: &str) -> Self {
        Editor {
            id: None,
            member: member.to_string(),
            prompt: String::new(),
            expr: "0 9 * * *".into(),
            timezone: String::new(),
            warmup: 0,
            enabled: true,
        }
    }

    pub fn from_schedule(s: &Schedule) -> Self {
        let expr = match s.cron.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            Some(c) => c.to_string(),
            None if s.every_minutes > 0 && 60 % s.every_minutes == 0 && s.every_minutes < 60 => {
                format!("*/{} * * * *", s.every_minutes)
            }
            None if s.every_minutes.is_multiple_of(60) && s.every_minutes / 60 < 24 => {
                format!("0 */{} * * *", s.every_minutes / 60)
            }
            None => "0 9 * * *".into(),
        };
        Editor {
            id: Some(s.id.clone()),
            member: s.member.clone(),
            prompt: s.prompt.clone(),
            expr,
            timezone: s.timezone.clone().unwrap_or_default(),
            warmup: s.warmup_minutes.unwrap_or(0),
            enabled: s.enabled,
        }
    }

    fn tz(&self) -> Option<&str> {
        Some(self.timezone.trim()).filter(|t| !t.is_empty())
    }

    /// Validation message for the cron or timezone fields.
    pub fn error(&self) -> Option<String> {
        cron::parse(&self.expr)
            .err()
            .or_else(|| cron::parse_tz(self.tz()).err())
    }

    pub fn preview(&self, now_ms: i64) -> Vec<i64> {
        cron::next_n(&self.expr, self.tz(), now_ms, PREVIEW_COUNT).unwrap_or_default()
    }

    /// The schedule this form describes; `existing_ids` picks a fresh `sN` id when creating.
    pub fn build(&self, now_ms: i64, existing_ids: &[&str]) -> Result<Schedule, String> {
        if let Some(e) = self.error() {
            return Err(e);
        }
        if self.member.trim().is_empty() {
            return Err("Member is required".into());
        }
        if self.prompt.trim().is_empty() {
            return Err("Prompt is required".into());
        }
        let id = self.id.clone().unwrap_or_else(|| next_id(existing_ids));
        // `every_minutes` stays meaningful for readers that ignore `cron`.
        let every = cron::min_interval_minutes(&self.expr, self.tz(), now_ms)
            .unwrap_or(60)
            .clamp(1, 10_080) as u32;
        Ok(Schedule {
            id,
            member: self.member.trim().to_string(),
            prompt: self.prompt.trim().to_string(),
            every_minutes: every,
            enabled: self.enabled,
            cron: Some(self.expr.trim().to_string()),
            timezone: self.tz().map(String::from),
            warmup_minutes: Some(self.warmup).filter(|w| *w > 0),
        })
    }

    /// Draw the form; `members` are the roster names to pick from.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        tr: &I18n,
        members: &[String],
        now_ms: i64,
        existing_ids: &[&str],
    ) -> Option<EditorResult> {
        let mut result = None;
        let editing = self.id.is_some();
        ui.label(
            RichText::new(tr.t(if editing {
                "team.launch.title.editSchedule"
            } else {
                "team.launch.title.createSchedule"
            }))
            .size(15.0)
            .strong(),
        );
        ui.add_space(6.0);
        egui::ComboBox::from_id_salt("sched-member")
            .selected_text(if self.member.is_empty() {
                "-".to_string()
            } else {
                self.member.clone()
            })
            .show_ui(ui, |ui| {
                for m in members {
                    ui.selectable_value(&mut self.member, m.clone(), m);
                }
            });
        ui.add_space(4.0);
        ui.label(RichText::new(tr.t("team.launch.prompt.label")).color(theme::TEXT_2));
        ui.add(
            egui::TextEdit::multiline(&mut self.prompt)
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .hint_text(tr.t("team.launch.prompt.schedulePlaceholder")),
        );
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for (key, _, expr) in cron::PRESETS {
                if ui
                    .selectable_label(self.expr.trim() == *expr, tr.t(key))
                    .clicked()
                {
                    self.expr = (*expr).to_string();
                }
            }
        });
        ui.label(RichText::new(tr.t("team.schedule.cron.expression")).color(theme::TEXT_2));
        ui.add(
            egui::TextEdit::singleline(&mut self.expr)
                .font(egui::TextStyle::Monospace)
                .desired_width(f32::INFINITY),
        );
        match self.error() {
            Some(e) => {
                let msg = if cron::parse(&self.expr).is_err() {
                    if self.expr.trim().is_empty() {
                        tr.t("team.schedule.cron.errors.enterExpression")
                    } else {
                        tr.t("team.schedule.cron.errors.invalidExpression")
                    }
                } else {
                    e
                };
                ui.label(RichText::new(msg).color(theme::ERROR_FG));
            }
            None => {
                ui.label(RichText::new(cron::describe(&self.expr)).color(theme::TEXT_2));
                if cron::is_high_frequency(&self.expr, self.tz(), now_ms) {
                    ui.label(
                        RichText::new(tr.t("team.schedule.cron.highFrequencyWarning"))
                            .color(theme::WARNING),
                    );
                }
                ui.label(RichText::new(tr.t("team.schedule.cron.nextRuns")).color(theme::MUTED));
                for t in self.preview(now_ms) {
                    ui.label(
                        RichText::new(cron::format_in(t, self.tz()))
                            .monospace()
                            .color(theme::TEXT),
                    );
                }
            }
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr.t("team.schedule.cron.timezone")).color(theme::TEXT_2));
            ui.add(
                egui::TextEdit::singleline(&mut self.timezone)
                    .hint_text("UTC")
                    .desired_width(160.0),
            );
            egui::ComboBox::from_id_salt("sched-tz")
                .selected_text(tr.t("team.schedule.cron.selectTimezone"))
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(self.timezone.is_empty(), "Local")
                        .clicked()
                    {
                        self.timezone.clear();
                    }
                    for z in cron::COMMON_TIMEZONES {
                        if ui.selectable_label(self.timezone == *z, *z).clicked() {
                            self.timezone = (*z).to_string();
                        }
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr.t("team.schedule.cron.warmUpTime")).color(theme::TEXT_2));
            let name = |w: u32| match w {
                0 => tr.t("team.schedule.cron.warmUpOptions.none"),
                5 => tr.t("team.schedule.cron.warmUpOptions.fiveMinutes"),
                10 => tr.t("team.schedule.cron.warmUpOptions.tenMinutes"),
                15 => tr.t("team.schedule.cron.warmUpOptions.fifteenMinutes"),
                _ => tr.t("team.schedule.cron.warmUpOptions.thirtyMinutes"),
            };
            egui::ComboBox::from_id_salt("sched-warm")
                .selected_text(name(self.warmup))
                .show_ui(ui, |ui| {
                    for w in cron::WARMUP_CHOICES {
                        ui.selectable_value(&mut self.warmup, *w, name(*w));
                    }
                });
        });
        ui.label(
            RichText::new(tr.t("team.schedule.cron.warmUpDescription"))
                .size(11.0)
                .color(theme::MUTED),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let ok = self.build(now_ms, existing_ids).is_ok();
            if ui
                .add_enabled(ok, egui::Button::new(tr.t("common.actions.save")))
                .clicked()
            {
                if let Ok(s) = self.build(now_ms, existing_ids) {
                    result = Some(EditorResult::Save(s));
                }
            }
            if ui.button(tr.t("common.actions.cancel")).clicked() {
                result = Some(EditorResult::Cancel);
            }
        });
        result
    }
}

/// Next free `sN` id.
pub fn next_id(existing: &[&str]) -> String {
    let n = existing
        .iter()
        .filter_map(|i| i.strip_prefix('s').and_then(|n| n.parse::<u32>().ok()))
        .max()
        .unwrap_or(0);
    format!("s{}", n + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    #[test]
    fn validation_and_preview() {
        let mut e = Editor::new("bob");
        e.prompt = "go".into();
        e.timezone = "UTC".into();
        assert!(e.error().is_none());
        assert_eq!(e.preview(NOW).len(), 5);
        e.expr = "nope".into();
        assert!(e.error().is_some());
        assert!(e.build(NOW, &[]).is_err());
        e.expr = "0 9 * * *".into();
        e.timezone = "Mars/Base".into();
        assert!(e.error().is_some());
    }

    #[test]
    fn build_assigns_ids_and_every_minutes() {
        let mut e = Editor::new("bob");
        e.prompt = " do it ".into();
        e.expr = "*/30 * * * *".into();
        e.timezone = "UTC".into();
        e.warmup = 10;
        let s = e.build(NOW, &["s1", "s7", "x"]).unwrap();
        assert_eq!(
            (s.id.as_str(), s.every_minutes, s.prompt.as_str()),
            ("s8", 30, "do it")
        );
        assert_eq!(
            (s.cron.as_deref(), s.warmup_minutes),
            (Some("*/30 * * * *"), Some(10))
        );
        e.prompt.clear();
        assert!(e.build(NOW, &[]).is_err());
    }

    #[test]
    fn from_schedule_converts_intervals() {
        let mut s = Schedule {
            id: "s1".into(),
            member: "bob".into(),
            prompt: "p".into(),
            every_minutes: 15,
            enabled: true,
            cron: None,
            timezone: None,
            warmup_minutes: None,
        };
        assert_eq!(Editor::from_schedule(&s).expr, "*/15 * * * *");
        s.every_minutes = 180;
        assert_eq!(Editor::from_schedule(&s).expr, "0 */3 * * *");
        s.cron = Some("0 9 * * 1".into());
        assert_eq!(Editor::from_schedule(&s).expr, "0 9 * * 1");
        assert_eq!(Editor::from_schedule(&s).id.as_deref(), Some("s1"));
    }
}
