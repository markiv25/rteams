//! "Claude Logs" section: the summarized output lines of every launched agent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, CornerRadius, RichText};

use crate::icons::{self, Icon};
use crate::theme;
use crate::view::View;

const MAX_ROWS: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Session,
    Tool,
    Result,
    Error,
    Thinking,
    Output,
}

pub fn classify(line: &str) -> LineKind {
    if line.starts_with("tool: ") {
        LineKind::Tool
    } else if line.starts_with("result: ") {
        LineKind::Result
    } else if line.starts_with("error: ") || line.starts_with("stderr: ") {
        LineKind::Error
    } else if line == "session started" {
        LineKind::Session
    } else if line.to_lowercase().starts_with("thinking") {
        LineKind::Thinking
    } else {
        LineKind::Output
    }
}

/// The newest run file (`<unix_ms>.jsonl`) in `dir`, by the number in its name.
fn newest_run(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let ms = p.file_stem()?.to_str()?.parse::<u64>().ok()?;
            (p.extension()? == "jsonl").then_some((ms, p))
        })
        .max_by_key(|(ms, _)| *ms)
        .map(|(_, p)| p)
}

/// Summarized lines of a raw stream-json run, keeping the last `MAX_ROWS`.
pub fn summarize_run(raw: &str) -> Vec<String> {
    let mut lines: Vec<String> = raw
        .lines()
        .flat_map(crate::launcher::summarize_line)
        .collect();
    let skip = lines.len().saturating_sub(MAX_ROWS);
    lines.drain(..skip);
    lines
}

#[derive(Default)]
pub struct Logs {
    member: Option<String>,
    search: String,
    /// Summarized newest run per member, loaded from disk; shown when the agent has no live
    /// lines (e.g. after a restart).
    history: BTreeMap<String, Vec<String>>,
}

impl Logs {
    /// Read the newest run file per member under `<home>/<team>/runs/<member>/`.
    pub fn load_history(&mut self, home: &Path, team: &str) {
        self.history.clear();
        let Ok(dirs) = std::fs::read_dir(home.join(team).join("runs")) else {
            return;
        };
        for d in dirs.flatten() {
            let Some(member) = d.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Some(file) = newest_run(&d.path()) else {
                continue;
            };
            let Ok(raw) = std::fs::read_to_string(file) else {
                continue;
            };
            let lines = summarize_run(&raw);
            if !lines.is_empty() {
                self.history.insert(member, lines);
            }
        }
    }

    /// `(member, lines)`: live lines when the agent has any, else the loaded history.
    fn sources<'a>(&'a self, v: &'a View) -> Vec<(&'a str, &'a [String])> {
        let mut out: Vec<(&str, &[String])> = Vec::new();
        for a in v.agents {
            let hist = self.history.get(&a.member);
            let lines = match hist {
                Some(h) if a.last_lines.is_empty() => h.as_slice(),
                _ => a.last_lines.as_slice(),
            };
            out.push((a.member.as_str(), lines));
        }
        for (m, h) in &self.history {
            if !v.agents.iter().any(|a| &a.member == m) {
                out.push((m.as_str(), h.as_slice()));
            }
        }
        out
    }

    pub fn show(&mut self, ui: &mut egui::Ui, v: &View) {
        let total: usize = self.sources(v).iter().map(|(_, l)| l.len()).sum();
        theme::section(
            ui,
            "logs",
            "\u{1F4DC}",
            &v.tr.t("common.layout.sections.claudeLogs"),
            Some(total),
            true,
            |_| {},
            |ui| self.body(ui, v, total),
        );
    }

    fn body(&mut self, ui: &mut egui::Ui, v: &View, total: usize) {
        ui.horizontal(|ui| {
            let sel = self
                .member
                .clone()
                .unwrap_or_else(|| v.tr.t("rteams.logs.allAgents"));
            let names: Vec<String> = self.sources(v).iter().map(|(m, _)| m.to_string()).collect();
            egui::ComboBox::from_id_salt("logs_member")
                .selected_text(sel)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.member, None, v.tr.t("rteams.logs.allAgents"));
                    for m in names {
                        ui.selectable_value(&mut self.member, Some(m.clone()), m);
                    }
                });
            let w = (ui.available_width() - 8.0).clamp(60.0, 260.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text(v.tr.t("team.claudeLogs.searchPlaceholder"))
                    .desired_width(w),
            );
        });
        if total == 0 {
            ui.add_space(4.0);
            ui.label(RichText::new(v.tr.t("rteams.logs.empty")).color(theme::MUTED));
            return;
        }
        let needle = self.search.to_lowercase();
        let all = self.member.is_none();
        let sources = self.sources(v);
        egui::ScrollArea::vertical()
            .id_salt("logs_scroll")
            .max_height(320.0)
            .auto_shrink([false, true])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                let mut rows = 0;
                for (member, all_lines) in sources
                    .iter()
                    .filter(|(m, _)| self.member.as_deref().is_none_or(|sel| sel == *m))
                {
                    let lines: Vec<&String> = all_lines
                        .iter()
                        .filter(|l| needle.is_empty() || l.to_lowercase().contains(&needle))
                        .collect();
                    let skip = lines.len().saturating_sub(MAX_ROWS.saturating_sub(rows));
                    let lines = &lines[skip..];
                    if lines.is_empty() {
                        continue;
                    }
                    if all {
                        ui.label(
                            RichText::new(*member)
                                .size(11.0)
                                .strong()
                                .color(v.color(member)),
                        );
                    }
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for l in lines {
                        row(ui, l);
                    }
                    rows += lines.len();
                }
            });
    }
}

/// Rounded icon tile (24px) in `color`, as in the original's log rows.
fn tile(ui: &mut egui::Ui, icon: Icon, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(24.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(7), theme::tint(color, 0.16));
    icons::draw(
        ui.painter(),
        egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(14.0)),
        icon,
        color,
    );
}

fn row(ui: &mut egui::Ui, line: &str) {
    let kind = classify(line);
    let (icon, color) = match kind {
        LineKind::Tool => (Icon::Code, theme::TOOL_FG),
        LineKind::Result => (Icon::Check, theme::RESULT_FG),
        LineKind::Error => (Icon::X, theme::ERROR_FG),
        LineKind::Thinking => (Icon::Eye, theme::THINK_FG),
        LineKind::Session => (Icon::Play, theme::MUTED),
        LineKind::Output => (Icon::MessageSquare, theme::TEXT_2),
    };
    egui::Frame::new()
        .fill(theme::tint(color, 0.05))
        .stroke(egui::Stroke::new(1.0, theme::hairline(0.05)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width().max(0.0));
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                tile(ui, icon, color);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    match kind {
                        LineKind::Tool => {
                            let rest = &line["tool: ".len()..];
                            let (name, input) = rest.split_once(' ').unwrap_or((rest, ""));
                            ui.label(RichText::new(name).strong().size(13.0).color(color));
                            if !input.is_empty() {
                                ui.label(
                                    RichText::new(input)
                                        .monospace()
                                        .size(11.5)
                                        .color(theme::TEXT_2),
                                );
                            }
                        }
                        LineKind::Session => {
                            ui.label(RichText::new(line).size(12.0).color(theme::MUTED));
                        }
                        LineKind::Output => {
                            ui.label(RichText::new(line).size(13.0));
                        }
                        _ => {
                            ui.label(RichText::new(line).size(12.5).color(color));
                        }
                    }
                });
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher::{AgentStatus, AgentView};
    use crate::view::testing::{fixture, render};

    #[test]
    fn classifies_lines() {
        assert_eq!(classify("tool: Read {\"a\":1}"), LineKind::Tool);
        assert_eq!(classify("result: ok"), LineKind::Result);
        assert_eq!(classify("error: boom"), LineKind::Error);
        assert_eq!(classify("stderr: warn"), LineKind::Error);
        assert_eq!(classify("session started"), LineKind::Session);
        assert_eq!(classify("Thinking about it"), LineKind::Thinking);
        assert_eq!(classify("Hello there"), LineKind::Output);
        assert_eq!(classify("tool:"), LineKind::Output);
    }

    #[test]
    fn history_loads_newest_run_and_shows_without_live_lines() {
        let (home, data) = fixture("logshist");
        let dir = home.join("t").join("runs").join("ada");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("100.jsonl"),
            "{\"type\":\"system\",\"subtype\":\"init\"}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("200.jsonl"),
            "{\"type\":\"result\",\"result\":\"all done\"}\nnot json\n\n",
        )
        .unwrap();
        std::fs::write(dir.join("junk.txt"), "x").unwrap();
        let mut logs = Logs::default();
        logs.load_history(&home, "t");
        assert_eq!(
            logs.history["ada"],
            vec!["result: all done".to_string(), "not json".to_string()]
        );
        let v = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        assert_eq!(logs.sources(&v).len(), 1);
        render(|ui| logs.show(ui, &v));
        logs.load_history(&home.join("nope"), "t");
        assert!(logs.history.is_empty());
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn renders_empty_and_filled() {
        let (home, data) = fixture("logs");
        let lines = |n: &str| -> Vec<String> {
            vec![
                "session started".into(),
                format!("hello from {n}"),
                "tool: Bash {\"command\":\"ls\"}".into(),
                "result: files".into(),
                "stderr: oops".into(),
                "tool: Solo".into(),
            ]
        };
        let agents = [
            AgentView {
                member: "ada".into(),
                status: AgentStatus::Running { pid: 1 },
                last_lines: lines("ada"),
                rate_limited_until: None,
                last_error: None,
                runtime: String::new(),
            },
            AgentView {
                member: "bob".into(),
                status: AgentStatus::Exited { code: Some(1) },
                last_lines: lines("bob"),
                rate_limited_until: None,
                last_error: None,
                runtime: String::new(),
            },
        ];
        for ag in [&agents[..], &[]] {
            let v = View {
                state: data.state(),
                data: &data,
                actor: Some("boss"),
                agents: ag,
                tr: crate::view::testing::tr(),
            };
            let mut logs = Logs {
                search: "o".into(),
                ..Default::default()
            };
            render(|ui| logs.show(ui, &v));
            logs.member = Some("ada".into());
            render(|ui| logs.show(ui, &v));
        }
        let _ = std::fs::remove_dir_all(home);
    }
}
