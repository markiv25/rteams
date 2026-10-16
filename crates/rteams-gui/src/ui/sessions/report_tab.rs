//! Session report tab: section cards for overview, cost, tokens, tools, git, friction,
//! quality, errors, subagents and the activity timeline, plus takeaways and badges.

use eframe::egui::{self, RichText};
use rteams_services::i18n::I18n;
use rteams_services::sessions::report::{Level, Report};

use super::{fmt_cost, fmt_date, fmt_dur, fmt_tokens, tt};
use crate::theme;

fn card(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::CARD)
        .stroke(egui::Stroke::new(1.0, theme::hairline(0.07)))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).strong().size(14.0));
            ui.add_space(4.0);
            body(ui);
        });
    ui.add_space(8.0);
}

fn kv(ui: &mut egui::Ui, k: &str, v: impl Into<String>) {
    ui.label(RichText::new(k).color(theme::MUTED));
    ui.label(v.into());
    ui.end_row();
}

fn level_color(l: Level) -> egui::Color32 {
    match l {
        Level::Good => theme::SUCCESS,
        Level::Warn => theme::WARNING,
        Level::Bad => theme::ERROR,
    }
}

/// Horizontal bar of `frac` (0..1) in `color`.
fn bar(ui: &mut egui::Ui, frac: f32, width: f32, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 8.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 3.0, theme::RAISED);
    let w = (rect.width() * frac.clamp(0.0, 1.0)).max(if frac > 0.0 { 2.0 } else { 0.0 });
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min, egui::vec2(w, rect.height())),
        3.0,
        color,
    );
}

/// Draw the report. Returns true when "Copy report" was pressed.
pub fn show(ui: &mut egui::Ui, tr: &I18n, r: &Report) -> bool {
    let mut copy = false;
    let t = |key: &str, d: &str| tt(tr, key, d);
    egui::ScrollArea::vertical()
        .id_salt("session_report")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for a in &r.assessments {
                    theme::pill(ui, format!("{}: {}", a.key, a.label), level_color(a.level));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(t("sessions.copyReport", "Copy report")).clicked() {
                        copy = true;
                    }
                });
            });
            ui.add_space(8.0);

            let o = &r.overview;
            card(ui, &t("report.overview.title", "Overview"), |ui| {
                egui::Grid::new("rep_overview")
                    .num_columns(2)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        kv(
                            ui,
                            &t("report.overview.metrics.sessionId", "Session ID"),
                            theme::short_id(&o.id),
                        );
                        kv(
                            ui,
                            &t("report.overview.metrics.project", "Project"),
                            o.cwd.clone(),
                        );
                        if !o.branch.is_empty() {
                            kv(
                                ui,
                                &t("report.overview.metrics.branch", "Branch"),
                                o.branch.clone(),
                            );
                        }
                        kv(ui, "Source", o.source);
                        kv(ui, "Model", o.model.clone());
                        if let Some(s) = o.start_ms {
                            kv(ui, "Started", fmt_date(s));
                        }
                        if let Some(d) = o.duration_ms {
                            kv(
                                ui,
                                &t("report.overview.metrics.duration", "Duration"),
                                fmt_dur(d),
                            );
                        }
                        kv(
                            ui,
                            &t("report.overview.metrics.messages", "Messages"),
                            o.messages.to_string(),
                        );
                        kv(ui, "Turns", o.turns.to_string());
                        kv(ui, "Tool calls", o.tool_calls.to_string());
                        kv(
                            ui,
                            &t("report.overview.metrics.subagents", "Subagents"),
                            o.subagents.to_string(),
                        );
                    });
            });

            card(ui, &t("report.cost.title", "Cost Analysis"), |ui| {
                egui::Grid::new("rep_cost")
                    .num_columns(2)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        kv(ui, &t("report.cost.total", "Total"), fmt_cost(r.cost.total));
                        kv(
                            ui,
                            &t("report.cost.parentCost", "Parent Cost"),
                            fmt_cost(r.cost.parent),
                        );
                        kv(
                            ui,
                            &t("report.cost.subagentCost", "Subagent Cost"),
                            fmt_cost(r.cost.subagent),
                        );
                        if let Some(c) = r.cost.per_commit {
                            kv(ui, &t("report.cost.perCommit", "Per Commit"), fmt_cost(c));
                        }
                    });
            });

            card(ui, &t("report.tokens.title", "Token Usage"), |ui| {
                let u = r.tokens.usage;
                let total = r.tokens.total.max(1) as f32;
                egui::Grid::new("rep_tokens")
                    .num_columns(3)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        for (name, v, c) in [
                            (t("report.tokens.input", "Input"), u.input, theme::ACCENT),
                            (
                                t("report.tokens.output", "Output"),
                                u.output,
                                theme::SUCCESS,
                            ),
                            (
                                t("report.tokens.cacheRead", "Cache Read"),
                                u.cache_read,
                                theme::INFO,
                            ),
                            (
                                t("report.tokens.cacheCreate", "Cache Create"),
                                u.cache_write,
                                theme::WARNING,
                            ),
                        ] {
                            ui.label(RichText::new(name).color(theme::MUTED));
                            ui.label(fmt_tokens(v));
                            bar(ui, v as f32 / total, 160.0, c);
                            ui.end_row();
                        }
                        ui.label(
                            RichText::new(t("report.tokens.cacheEfficiency", "Cache Efficiency"))
                                .color(theme::MUTED),
                        );
                        ui.label(format!("{:.0}%", r.tokens.cache_hit_ratio * 100.0));
                        ui.end_row();
                    });
                if !r.tokens.by_model.is_empty() {
                    ui.add_space(4.0);
                    egui::Grid::new("rep_models")
                        .striped(true)
                        .spacing([14.0, 3.0])
                        .show(ui, |ui| {
                            for m in &r.tokens.by_model {
                                ui.label(RichText::new(&m.model).small());
                                ui.label(fmt_tokens(m.usage.total()));
                                ui.label(fmt_cost(m.cost_usd));
                                ui.end_row();
                            }
                        });
                }
            });

            card(ui, &t("report.tools.title", "Tool Usage"), |ui| {
                let max = r.tools.iter().map(|x| x.calls).max().unwrap_or(1).max(1) as f32;
                egui::Grid::new("rep_tools")
                    .spacing([14.0, 3.0])
                    .show(ui, |ui| {
                        for x in &r.tools {
                            ui.label(&x.name);
                            ui.label(x.calls.to_string());
                            bar(
                                ui,
                                x.calls as f32 / max,
                                140.0,
                                if x.errors > 0 {
                                    theme::WARNING
                                } else {
                                    theme::TOOL_FG
                                },
                            );
                            if x.errors > 0 {
                                ui.label(
                                    RichText::new(format!("{} err", x.errors))
                                        .small()
                                        .color(theme::ERROR),
                                );
                            }
                            ui.end_row();
                        }
                    });
                if !r.files_edited.is_empty() {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!("Files edited ({})", r.files_edited.len())).strong(),
                    );
                    for f in r.files_edited.iter().take(30) {
                        ui.label(
                            RichText::new(format!("{}  x{}", f.path, f.edits))
                                .small()
                                .color(theme::TEXT_2),
                        );
                    }
                }
                if !r.files_read.is_empty() {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!("Files read ({})", r.files_read.len())).strong(),
                    );
                    for f in r.files_read.iter().take(20) {
                        ui.label(RichText::new(f).small().color(theme::MUTED));
                    }
                }
            });

            card(ui, &t("report.git.title", "Git Activity"), |ui| {
                let g = &r.git;
                if g.commits + g.pushes + g.checkouts + g.pull_requests == 0 {
                    ui.label(RichText::new("No git activity").color(theme::MUTED));
                } else {
                    ui.label(format!(
                        "{} commits, {} pushes, {} branch switches, {} pull requests",
                        g.commits, g.pushes, g.checkouts, g.pull_requests
                    ));
                    for c in g.commands.iter().take(10) {
                        ui.label(RichText::new(c).monospace().small().color(theme::TEXT_2));
                    }
                }
            });

            card(ui, &t("report.friction.title", "Friction Signals"), |ui| {
                let f = &r.friction;
                ui.label(format!(
                    "{} retries after failures, {} failure streaks, {} interruptions",
                    f.retries, f.failure_streaks, f.interruptions
                ));
            });

            card(ui, &t("report.quality.title", "Quality Signals"), |ui| {
                let q = &r.quality;
                ui.label(format!(
                    "Test runs: {} ({} failed). Build runs: {} ({} failed).",
                    q.test_runs, q.test_failures, q.build_runs, q.build_failures
                ));
            });

            card(
                ui,
                &format!(
                    "{} ({})",
                    t("report.errors.title", "Errors"),
                    r.errors.len()
                ),
                |ui| {
                    if r.errors.is_empty() {
                        ui.label(RichText::new("No tool errors").color(theme::SUCCESS));
                    }
                    for e in r.errors.iter().take(40) {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(&e.tool).strong().color(theme::ERROR_FG));
                            ui.label(RichText::new(&e.summary).small().color(theme::MUTED));
                        });
                        ui.label(
                            RichText::new(&e.message)
                                .monospace()
                                .small()
                                .color(theme::TEXT_2),
                        );
                        ui.add_space(2.0);
                    }
                },
            );

            if !r.subagents.is_empty() {
                card(ui, &t("report.subagents.title", "Subagents"), |ui| {
                    egui::Grid::new("rep_subs")
                        .striped(true)
                        .spacing([14.0, 3.0])
                        .show(ui, |ui| {
                            for s in &r.subagents {
                                ui.label(&s.description);
                                ui.label(format!("{} tools", s.tool_calls));
                                ui.label(fmt_tokens(s.tokens));
                                ui.label(fmt_cost(s.cost_usd));
                                ui.label(s.duration_ms.map(fmt_dur).unwrap_or_default());
                                ui.end_row();
                            }
                        });
                });
            }

            card(
                ui,
                &t("report.timeline.title", "Timeline & Activity"),
                |ui| {
                    if r.timeline.is_empty() {
                        ui.label(RichText::new("No timestamps").color(theme::MUTED));
                        return;
                    }
                    let h = 48.0;
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), h),
                        egui::Sense::hover(),
                    );
                    let max = r
                        .timeline
                        .iter()
                        .map(|b| b.events)
                        .max()
                        .unwrap_or(1)
                        .max(1) as f32;
                    let n = r.timeline.len() as f32;
                    let bw = rect.width() / n;
                    for (i, b) in r.timeline.iter().enumerate() {
                        let bh = (b.events as f32 / max) * (h - 4.0);
                        let x = rect.left() + i as f32 * bw;
                        let br = egui::Rect::from_min_max(
                            egui::pos2(
                                x + 0.5,
                                rect.bottom() - bh.max(if b.events > 0 { 2.0 } else { 0.0 }),
                            ),
                            egui::pos2(x + bw - 0.5, rect.bottom()),
                        );
                        ui.painter().rect_filled(br, 1.0, theme::ACCENT);
                    }
                    resp.on_hover_text(format!(
                        "{} - {}",
                        fmt_date(r.timeline[0].start_ms),
                        r.overview.end_ms.map(fmt_date).unwrap_or_default()
                    ));
                },
            );

            card(ui, &t("report.takeaways.title", "Key Takeaways"), |ui| {
                for k in &r.takeaways {
                    ui.label(format!("\u{2022} {k}"));
                }
            });
        });
    copy
}

#[cfg(test)]
mod tests {
    use super::super::viewer::tests::session;
    use super::*;

    #[test]
    fn renders_report() {
        let s = session();
        let r = rteams_services::sessions::report::build(&s);
        let tr = I18n::default();
        crate::view::testing::render(|ui| {
            show(ui, &tr, &r);
        });
    }
}
