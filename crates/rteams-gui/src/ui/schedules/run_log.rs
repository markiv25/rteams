//! Run Log window: the runs of one schedule and the output of the selected run.

use std::path::Path;

use eframe::egui::{self, RichText};
use rteams_services::i18n::I18n;
use rteams_services::logtail::{parse_line, LogKind};
use rteams_services::schedule::{cron, history};

use super::list::status_color;
use crate::theme;

/// Max lines read from a run file.
const MAX_LINES: usize = 400;

pub struct RunLog {
    pub schedule_id: String,
    timezone: Option<String>,
    pub selected: Option<String>,
    lines: Vec<(LogKind, String)>,
    loaded_for: Option<String>,
}

impl RunLog {
    pub fn new(schedule_id: &str, _member: &str, timezone: Option<String>) -> Self {
        RunLog {
            schedule_id: schedule_id.to_string(),
            timezone,
            selected: None,
            lines: vec![],
            loaded_for: None,
        }
    }

    fn load(&mut self, home: &Path, team: &str, run: &history::RunRecord) {
        self.lines.clear();
        if let Some(f) = run.run_file.as_deref().filter(|f| !f.contains(['/', '\\'])) {
            let p = home.join(team).join("runs").join(&run.member).join(f);
            let raw = std::fs::read_to_string(p).unwrap_or_default();
            for l in raw.lines().take(MAX_LINES) {
                for e in parse_line(l, &run.member, 0) {
                    self.lines.push((e.kind, e.text));
                }
            }
        }
        self.loaded_for = Some(run.run_id.clone());
    }

    /// Draw as a window; returns `false` once the user closed it.
    pub fn show(&mut self, ctx: &egui::Context, tr: &I18n, home: &Path, team: &str) -> bool {
        let mut open = true;
        let runs = history::read(home, team, &self.schedule_id);
        if self.selected.is_none() {
            self.selected = runs.last().map(|r| r.run_id.clone());
        }
        egui::Window::new(format!(
            "{} - {}",
            tr.t("team.schedule.runLog.title"),
            self.schedule_id
        ))
        .open(&mut open)
        .default_width(560.0)
        .show(ctx, |ui| {
            if runs.is_empty() {
                ui.label(RichText::new(tr.t("team.schedule.runHistory.empty")).color(theme::MUTED));
                return;
            }
            egui::ScrollArea::vertical()
                .id_salt("runlog-runs")
                .max_height(140.0)
                .show(ui, |ui| {
                    for r in runs.iter().rev() {
                        ui.horizontal(|ui| {
                            let sel = self.selected.as_deref() == Some(r.run_id.as_str());
                            if ui
                                .selectable_label(
                                    sel,
                                    cron::format_in(r.scheduled_for, self.timezone.as_deref()),
                                )
                                .clicked()
                            {
                                self.selected = Some(r.run_id.clone());
                                self.loaded_for = None;
                            }
                            theme::pill(ui, tr.t(r.status.key()), status_color(r.status));
                            if let Some(c) = r.exit_code {
                                ui.label(
                                    RichText::new(tr.t_args(
                                        "team.schedule.runLog.exitCode",
                                        &[("code", &c.to_string())],
                                    ))
                                    .size(11.0)
                                    .color(theme::MUTED),
                                );
                            }
                            if r.retry > 0 {
                                ui.label(
                                    RichText::new(tr.t_args(
                                        "team.schedule.runLog.retryCount",
                                        &[("count", &r.retry.to_string()), ("max", "3")],
                                    ))
                                    .size(11.0)
                                    .color(theme::MUTED),
                                );
                            }
                        });
                    }
                });
            ui.separator();
            let Some(run) = runs
                .iter()
                .find(|r| Some(&r.run_id) == self.selected.as_ref())
            else {
                return;
            };
            if self.loaded_for.as_deref() != Some(run.run_id.as_str()) || run.status.is_active() {
                self.load(home, team, run);
            }
            if run.status.is_active() {
                ui.label(
                    RichText::new(tr.t("team.schedule.runLog.stillRunning")).color(theme::INFO),
                );
            }
            if let Some(e) = run.error.as_deref().filter(|e| !e.is_empty()) {
                ui.label(
                    RichText::new(tr.t("team.schedule.runLog.errors"))
                        .strong()
                        .color(theme::ERROR_FG),
                );
                ui.label(RichText::new(e).monospace().color(theme::ERROR_FG));
            }
            egui::ScrollArea::vertical()
                .id_salt("runlog-lines")
                .max_height(260.0)
                .show(ui, |ui| {
                    for (k, t) in &self.lines {
                        ui.label(
                            RichText::new(format!("{}: {t}", k.label()))
                                .monospace()
                                .size(11.5)
                                .color(kind_color(*k)),
                        );
                    }
                });
        });
        open
    }
}

pub fn kind_color(k: LogKind) -> egui::Color32 {
    match k {
        LogKind::Thinking => theme::THINK_FG,
        LogKind::ToolUse => theme::TOOL_FG,
        LogKind::ToolResult | LogKind::Result => theme::RESULT_FG,
        LogKind::Error => theme::ERROR_FG,
        LogKind::Text => theme::TEXT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_run_output_and_renders() {
        let d = tempfile::tempdir().unwrap();
        let runs = d.path().join("t/runs/bob");
        std::fs::create_dir_all(&runs).unwrap();
        std::fs::write(
            runs.join("5.jsonl"),
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]}}\n",
        )
        .unwrap();
        let mut r = history::RunRecord::new("s1", "bob", 5);
        r.run_file = Some("5.jsonl".into());
        r.status = history::RunStatus::Completed;
        history::append(d.path(), "t", &r).unwrap();
        let tr = I18n::english();
        let mut log = RunLog::new("s1", "bob", Some("UTC".into()));
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        for _ in 0..3 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                log.show(ui.ctx(), &tr, d.path(), "t");
            });
            out.textures_delta.clear();
        }
        assert_eq!(log.lines.len(), 1);
        assert_eq!(log.lines[0].1, "hello");
    }
}
