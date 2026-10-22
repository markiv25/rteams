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

/// A display item of a member's log: one line, or a run of the lead's consecutive thoughts.
#[derive(Debug, PartialEq, Eq)]
pub enum Item<'a> {
    Line(&'a str),
    Thoughts(Vec<&'a str>),
}

/// True for assistant prose: thinking or plain output (not tool calls, results, errors).
fn is_thought(line: &str) -> bool {
    matches!(classify(line), LineKind::Thinking | LineKind::Output)
}

/// Collapse runs of 2+ consecutive thinking/text lines into one `Thoughts` item. Lines pass
/// through untouched when `group` is false (only the lead's prose is grouped).
pub fn group_thoughts<'a>(lines: &[&'a String], group: bool) -> Vec<Item<'a>> {
    let mut out: Vec<Item<'a>> = Vec::new();
    let mut run: Vec<&'a str> = Vec::new();
    let flush = |run: &mut Vec<&'a str>, out: &mut Vec<Item<'a>>| match run.len() {
        0 => {}
        1 => out.push(Item::Line(run.pop().unwrap())),
        _ => out.push(Item::Thoughts(std::mem::take(run))),
    };
    for l in lines {
        if group && is_thought(l) {
            run.push(l.as_str());
        } else {
            flush(&mut run, &mut out);
            out.push(Item::Line(l.as_str()));
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Is `member` the team lead (named or roled "lead")?
fn is_lead(v: &View, member: &str) -> bool {
    member.eq_ignore_ascii_case("lead")
        || v.members()
            .iter()
            .any(|m| m.name == member && m.role.to_lowercase().contains("lead"))
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
                    let items = group_thoughts(lines, is_lead(v, member));
                    for (i, it) in items.iter().enumerate() {
                        match it {
                            Item::Line(l) => row(ui, l),
                            Item::Thoughts(ts) => thoughts_row(ui, v, member, i, ts),
                        }
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

/// Expandable "N thoughts" row for a run of the lead's prose lines.
fn thoughts_row(ui: &mut egui::Ui, v: &View, member: &str, idx: usize, ts: &[&str]) {
    let title =
        v.tr.t_args("rteams.logs.thoughts", &[("n", &ts.len().to_string())]);
    let id = ui.make_persistent_id(("lead-thoughts", member, idx));
    egui::Frame::new()
        .fill(theme::tint(theme::THINK_FG, 0.05))
        .stroke(egui::Stroke::new(1.0, theme::hairline(0.05)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width().max(0.0));
            let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                id,
                false,
            );
            let head = ui.horizontal(|ui| {
                tile(ui, Icon::Eye, theme::THINK_FG);
                let open = state.is_open();
                icons::icon(
                    ui,
                    if open {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    },
                    12.0,
                    theme::MUTED,
                );
                ui.label(RichText::new(title).size(12.5).color(theme::THINK_FG));
            });
            if ui
                .interact(head.response.rect, id.with("hd"), egui::Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                state.toggle(ui);
            }
            state.show_body_unindented(ui, |ui| {
                ui.add_space(4.0);
                for t in ts {
                    ui.label(RichText::new(*t).size(12.5).color(theme::TEXT_2));
                }
            });
        });
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

/// Entries shown at most in a task's Logs section.
const TASK_ROWS: usize = 150;

fn entry_style(k: crate::task_logs::Kind) -> (Icon, Color32) {
    use crate::task_logs::Kind;
    match k {
        Kind::Tool => (Icon::Code, theme::TOOL_FG),
        Kind::ToolResult | Kind::Result => (Icon::Check, theme::RESULT_FG),
        Kind::Error => (Icon::X, theme::ERROR_FG),
        Kind::Text => (Icon::MessageSquare, theme::TEXT_2),
    }
}

/// "Logs" section of the task detail: the agent log entries attributed to `task_id`, newest
/// last. Each summary line expands to the full detail. Live: the store fills while agents run.
pub fn task_log_section(ui: &mut egui::Ui, v: &View, task_id: &str) {
    let Some(team) = v.team().map(|t| t.name.as_str()) else {
        return;
    };
    let entries = crate::task_logs::entries(team, task_id);
    let title = format!("{} ({})", v.tr.t("rteams.taskLogs.title"), entries.len());
    egui::CollapsingHeader::new(RichText::new(title).strong())
        .id_salt(("task-logs", task_id))
        .default_open(false)
        .show(ui, |ui| {
            if entries.is_empty() {
                ui.label(RichText::new(v.tr.t("rteams.taskLogs.empty")).color(theme::MUTED));
                return;
            }
            let skip = entries.len().saturating_sub(TASK_ROWS);
            egui::ScrollArea::vertical()
                .id_salt(("task-logs-scroll", task_id))
                .max_height(360.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for (i, e) in entries.iter().enumerate().skip(skip) {
                        let (icon, color) = entry_style(e.kind);
                        let head = format!("{} \u{00B7} {}", e.member, e.summary);
                        ui.horizontal(|ui| {
                            icons::icon(ui, icon, 12.0, color);
                            egui::CollapsingHeader::new(
                                RichText::new(head).size(12.0).color(color),
                            )
                            .id_salt(("task-log-entry", task_id, i))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(&e.detail)
                                        .monospace()
                                        .size(11.5)
                                        .color(theme::TEXT_2),
                                );
                            });
                        });
                    }
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
    fn groups_consecutive_lead_prose() {
        let owned: Vec<String> = [
            "session started",
            "hmm",
            "Thinking more",
            "ok",
            "tool: Bash {}",
            "solo",
            "result: x",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let refs: Vec<&String> = owned.iter().collect();
        let g = group_thoughts(&refs, true);
        assert_eq!(
            g,
            vec![
                Item::Line("session started"),
                Item::Thoughts(vec!["hmm", "Thinking more", "ok"]),
                Item::Line("tool: Bash {}"),
                Item::Line("solo"),
                Item::Line("result: x"),
            ]
        );
        assert_eq!(group_thoughts(&refs, false).len(), refs.len());
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

    #[test]
    fn task_log_section_renders_live_entries() {
        let (home, data) = fixture("tasklogs");
        let team = data.state().team.as_ref().unwrap().name.clone();
        let line = serde_json::json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","name":"mcp__rteams__task_start","input":{"task":"zz1"}},
            {"type":"text","text":"doing the thing"}]}})
        .to_string();
        crate::task_logs::ingest(&team, "ada", 1, &line);
        let v = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        assert_eq!(crate::task_logs::entries(&team, "zz1").len(), 2);
        render(|ui| {
            task_log_section(ui, &v, "zz1");
            task_log_section(ui, &v, "none");
        });
        let _ = std::fs::remove_dir_all(home);
    }
}
