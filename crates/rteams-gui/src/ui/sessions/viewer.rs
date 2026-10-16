//! Right pane, conversation tab: user turns and AI response groups as rendered markdown,
//! thinking blocks and collapsible tool-call cards (input / output), subagent runs nested.
//! The chunk list is virtualised by remembering each chunk's measured height.

use std::collections::{HashMap, HashSet};

use eframe::egui::{self, RichText};
use rteams_services::i18n::I18n;
use rteams_services::sessions::chunks::{Chunk, Session};
use rteams_services::sessions::steps::{Step, StepKind};
use serde_json::Value;

use super::{fmt_dur, fmt_tokens, tt};
use crate::theme;
use crate::ui::editor::md_preview;

#[derive(Default)]
pub struct ViewerState {
    /// Expanded cards, by key (`c<chunk>.s<step>` and nested `/s<sub>.<chunk>.<step>`).
    pub open: HashSet<String>,
    /// Expand thinking blocks by default.
    pub show_thinking: bool,
    /// Chunk to scroll to on the next frame.
    pub scroll_to: Option<usize>,
    heights: HashMap<usize, f32>,
    last_width: f32,
}

impl ViewerState {
    /// Forget measurements and expansion (new session).
    pub fn reset(&mut self) {
        self.open.clear();
        self.heights.clear();
        self.scroll_to = None;
    }

    pub fn expand_all_tools(&mut self, s: &Session) {
        for (ci, c) in s.chunks.iter().enumerate() {
            for (si, st) in c.steps.iter().enumerate() {
                if st.tool.is_some() {
                    self.open.insert(format!("c{ci}.s{si}"));
                }
            }
        }
        self.heights.clear();
    }

    pub fn collapse_all(&mut self) {
        self.open.clear();
        self.heights.clear();
    }
}

fn looks_like_markdown(t: &str) -> bool {
    t.contains("```")
        || t.contains("**")
        || t.contains('`')
        || t.lines().any(|l| {
            let l = l.trim_start();
            l.starts_with('#')
                || l.starts_with("- ")
                || l.starts_with("* ")
                || l.starts_with("> ")
                || l.starts_with('|')
                || l.starts_with("1. ")
        })
        || t.contains("](")
}

fn est_height(t: &str, width: f32) -> f32 {
    let cols = (width / 7.2).max(10.0);
    let lines: f32 = t
        .lines()
        .map(|l| ((l.chars().count() as f32) / cols).ceil().max(1.0))
        .sum();
    (lines * 19.0 + 10.0).clamp(26.0, 520.0)
}

/// Plain text or rendered markdown (the markdown renderer scrolls inside a fixed box, so the
/// box height is estimated from the text).
pub fn rich_text(ui: &mut egui::Ui, id: &str, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if looks_like_markdown(text) {
        let w = ui.available_width();
        let h = est_height(text, w);
        ui.push_id(id, |ui| {
            ui.allocate_ui(egui::vec2(w, h), |ui| md_preview::show(ui, text));
        });
    } else {
        ui.add(egui::Label::new(text).wrap());
    }
}

fn clip_chars(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut o: String = s.chars().take(n).collect();
    o.push_str(&format!(
        "\n\u{2026} ({} more characters)",
        s.chars().count() - n
    ));
    o
}

/// Readable form of a tool call's input.
pub fn input_text(name: &str, input: &Value) -> String {
    if matches!(name, "Bash" | "shell") {
        if let Some(c) = rteams_services::sessions::tools::command_of(input) {
            return c;
        }
    }
    serde_json::to_string_pretty(input).unwrap_or_default()
}

fn mono_block(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    egui::Frame::new()
        .fill(theme::CODE_BG)
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(
                    RichText::new(clip_chars(text, 6000))
                        .monospace()
                        .size(11.5)
                        .color(color),
                )
                .wrap(),
            );
        });
}

fn tool_card(
    ui: &mut egui::Ui,
    tr: &I18n,
    open: &mut HashSet<String>,
    key: &str,
    step: &Step,
    dirty: &mut bool,
) {
    let Some(t) = step.tool.as_ref() else { return };
    let is_open = open.contains(key);
    let border = if t.is_error {
        theme::tint(theme::ERROR, 0.5)
    } else {
        theme::hairline(0.1)
    };
    egui::Frame::new()
        .fill(theme::CARD)
        .stroke(egui::Stroke::new(1.0, border))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let chevron = if is_open { "\u{23F7}" } else { "\u{23F5}" };
            let mut head = format!("{chevron} \u{1F527} {}", t.name);
            let sum = t.summary();
            if !sum.is_empty() {
                head.push_str("  ");
                head.push_str(&sum);
            }
            let color = if t.is_error {
                theme::ERROR_FG
            } else {
                theme::TOOL_FG
            };
            let r = ui
                .horizontal(|ui| {
                    let r = ui.add(
                        egui::Label::new(RichText::new(head).color(color).size(12.5))
                            .truncate()
                            .sense(egui::Sense::click()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(d) = t.duration_ms() {
                            ui.label(RichText::new(fmt_dur(d)).small().color(theme::MUTED));
                        }
                        if t.is_error {
                            theme::pill(ui, tt(tr, "sessions.error", "error"), theme::ERROR);
                        } else if t.result.is_none() {
                            theme::pill(ui, tt(tr, "sessions.noResult", "no result"), theme::MUTED);
                        }
                    });
                    r
                })
                .inner;
            if r.clicked() {
                if is_open {
                    open.remove(key);
                } else {
                    open.insert(key.to_string());
                }
                *dirty = true;
            }
            if is_open {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(tt(tr, "sessions.input", "Input"))
                        .small()
                        .color(theme::MUTED),
                );
                mono_block(ui, &input_text(&t.name, &t.input), theme::TEXT_2);
                ui.add_space(2.0);
                ui.label(
                    RichText::new(tt(tr, "sessions.output", "Output"))
                        .small()
                        .color(theme::MUTED),
                );
                match &t.result {
                    Some(r) if !r.trim().is_empty() => mono_block(
                        ui,
                        r,
                        if t.is_error {
                            theme::ERROR_FG
                        } else {
                            theme::RESULT_FG
                        },
                    ),
                    Some(_) => {
                        ui.label(RichText::new("(empty)").small().color(theme::MUTED));
                    }
                    None => {
                        ui.label(
                            RichText::new("(no result recorded)")
                                .small()
                                .color(theme::MUTED),
                        );
                    }
                }
            }
        });
}

#[allow(clippy::too_many_arguments)]
fn show_steps(
    ui: &mut egui::Ui,
    tr: &I18n,
    s: &Session,
    key: &str,
    steps: &[Step],
    open: &mut HashSet<String>,
    show_thinking: bool,
    dirty: &mut bool,
) {
    for (si, step) in steps.iter().enumerate() {
        let k = format!("{key}.s{si}");
        match step.kind {
            StepKind::Text => {
                rich_text(ui, &k, &step.text);
            }
            StepKind::Thinking => {
                let is_open = open.contains(&k) != show_thinking;
                let chevron = if is_open { "\u{23F7}" } else { "\u{23F5}" };
                let r = ui.add(
                    egui::Label::new(
                        RichText::new(format!(
                            "{chevron} \u{1F4AD} {}",
                            tt(tr, "sessions.thinking", "Thinking")
                        ))
                        .small()
                        .color(theme::THINK_FG),
                    )
                    .sense(egui::Sense::click()),
                );
                if r.clicked() {
                    if open.contains(&k) {
                        open.remove(&k);
                    } else {
                        open.insert(k.clone());
                    }
                    *dirty = true;
                }
                if is_open {
                    ui.add(
                        egui::Label::new(
                            RichText::new(clip_chars(step.text.trim(), 8000))
                                .italics()
                                .color(theme::TEXT_2),
                        )
                        .wrap(),
                    );
                }
            }
            StepKind::ToolCall => tool_card(ui, tr, open, &k, step, dirty),
            StepKind::Subagent => {
                let sub = step.subagent.and_then(|i| s.subagents.get(i));
                tool_card(ui, tr, open, &k, step, dirty);
                if let (Some(sub), true) = (sub, open.contains(&k)) {
                    ui.indent(format!("{k}-sub"), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} {} \u{00B7} {} tools \u{00B7} {} tokens",
                                tt(tr, "sessions.subagent", "Subagent"),
                                sub.agent_id,
                                sub.tool_count,
                                fmt_tokens(sub.totals.usage.total())
                            ))
                            .small()
                            .color(theme::SEND),
                        );
                        for (ci, c) in sub.chunks.iter().enumerate() {
                            if let Some(p) = &c.prompt {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(clip_chars(p, 600))
                                            .small()
                                            .color(theme::TEXT_2),
                                    )
                                    .wrap(),
                                );
                            }
                            show_steps(
                                ui,
                                tr,
                                s,
                                &format!("{k}/{ci}"),
                                &c.steps,
                                open,
                                show_thinking,
                                dirty,
                            );
                        }
                    });
                }
            }
        }
        ui.add_space(4.0);
    }
}

#[allow(clippy::too_many_arguments)]
fn show_chunk(
    ui: &mut egui::Ui,
    tr: &I18n,
    s: &Session,
    ci: usize,
    c: &Chunk,
    open: &mut HashSet<String>,
    show_thinking: bool,
    dirty: &mut bool,
) {
    if let Some(p) = &c.prompt {
        egui::Frame::new()
            .fill(theme::tint(theme::ACCENT, 0.08))
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(tt(tr, "sessions.you", "You"))
                            .strong()
                            .color(theme::ACCENT),
                    );
                    if let Some(t) = c.start_ms {
                        ui.label(RichText::new(theme::hhmm(t)).small().color(theme::MUTED));
                    }
                    ui.label(
                        RichText::new(format!("#{}", ci + 1))
                            .small()
                            .color(theme::MUTED),
                    );
                });
                rich_text(ui, &format!("c{ci}.p"), p);
            });
        ui.add_space(6.0);
    }
    if !c.steps.is_empty() {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(tt(tr, "sessions.assistant", "Assistant"))
                    .strong()
                    .color(theme::SUCCESS),
            );
            if let Some(d) = c.duration_ms() {
                ui.label(RichText::new(fmt_dur(d)).small().color(theme::MUTED));
            }
        });
        ui.indent(format!("c{ci}-steps"), |ui| {
            show_steps(
                ui,
                tr,
                s,
                &format!("c{ci}"),
                &c.steps,
                open,
                show_thinking,
                dirty,
            );
        });
    }
    ui.add_space(12.0);
}

/// Draw the conversation.
pub fn show(ui: &mut egui::Ui, tr: &I18n, st: &mut ViewerState, s: &Session) {
    if s.chunks.is_empty() {
        ui.label(
            RichText::new(tt(
                tr,
                "common.sessions.empty.noSessions",
                "No sessions found",
            ))
            .color(theme::MUTED),
        );
        return;
    }
    let w = ui.available_width();
    if (w - st.last_width).abs() > 6.0 {
        st.heights.clear();
        st.last_width = w;
    }
    let mut area = egui::ScrollArea::vertical()
        .id_salt("session_viewer")
        .auto_shrink([false, false]);
    if let Some(target) = st.scroll_to.take() {
        let off: f32 = (0..target)
            .map(|i| st.heights.get(&i).copied().unwrap_or(240.0))
            .sum();
        area = area.vertical_scroll_offset(off);
    }
    let ViewerState {
        open,
        show_thinking,
        heights,
        ..
    } = st;
    let show_thinking = *show_thinking;
    let mut dirty = false;
    area.show_viewport(ui, |ui, viewport| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut y = 0.0;
        for (ci, c) in s.chunks.iter().enumerate() {
            if let Some(&h) = heights.get(&ci) {
                if y + h < viewport.min.y - 300.0 || y > viewport.max.y + 300.0 {
                    ui.add_space(h);
                    y += h;
                    continue;
                }
            }
            let r = ui.scope(|ui| show_chunk(ui, tr, s, ci, c, open, show_thinking, &mut dirty));
            let h = r.response.rect.height();
            heights.insert(ci, h);
            y += h;
        }
    });
    if dirty {
        heights.clear();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rteams_services::sessions::chunks::{parse_str, Session};
    use rteams_services::sessions::paths::Source;

    pub(crate) fn session() -> Session {
        let raw = [
            r#"{"type":"user","uuid":"u1","sessionId":"s1","cwd":"/work/proj","timestamp":"2025-03-01T10:00:00.000Z","message":{"role":"user","content":"Fix the **login** bug"}}"#,
            r#"{"type":"assistant","uuid":"a1","timestamp":"2025-03-01T10:00:02.000Z","message":{"id":"m1","model":"claude-sonnet-4-5","content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"Reading:\n\n```rust\nfn main() {}\n```"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}}],"usage":{"input_tokens":10,"output_tokens":5}}}"#,
            r#"{"type":"user","uuid":"u2","timestamp":"2025-03-01T10:00:09.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"boom","is_error":true}]}}"#,
        ]
        .join("\n");
        Session::from_msgs(
            std::path::Path::new("/x/s1.jsonl"),
            Source::Claude,
            parse_str(&raw, Source::Claude),
        )
    }

    #[test]
    fn markdown_detection() {
        assert!(looks_like_markdown("a **b**"));
        assert!(looks_like_markdown("- item"));
        assert!(!looks_like_markdown("plain words here"));
        assert!(est_height("a\nb\nc", 400.0) < est_height(&"x".repeat(2000), 400.0));
    }

    #[test]
    fn renders_collapsed_and_expanded() {
        let s = session();
        let tr = I18n::default();
        let mut st = ViewerState::default();
        crate::view::testing::render(|ui| show(ui, &tr, &mut st, &s));
        st.expand_all_tools(&s);
        st.show_thinking = true;
        crate::view::testing::render(|ui| show(ui, &tr, &mut st, &s));
        st.scroll_to = Some(0);
        crate::view::testing::render(|ui| show(ui, &tr, &mut st, &s));
        assert!(st.open.contains("c0.s2"));
        st.collapse_all();
        assert!(st.open.is_empty());
    }

    #[test]
    fn input_text_prefers_command() {
        assert_eq!(
            input_text("Bash", &serde_json::json!({"command":"ls"})),
            "ls"
        );
        assert!(input_text("Read", &serde_json::json!({"file_path":"/a"})).contains("file_path"));
    }
}
