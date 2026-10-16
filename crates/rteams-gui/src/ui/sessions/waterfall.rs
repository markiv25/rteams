//! Waterfall timeline: one row per step, a bar positioned by start time and sized by duration.
//! Sessions without timestamps (older run files) fall back to equal bars in sequence.

use eframe::egui::{self, RichText};
use rteams_services::i18n::I18n;
use rteams_services::sessions::chunks::Session;
use rteams_services::sessions::steps::StepKind;

use super::{fmt_dur, tt};
use crate::theme;

#[derive(Debug, Clone, PartialEq)]
pub struct Bar {
    pub chunk: usize,
    pub step: usize,
    pub kind: StepKind,
    pub error: bool,
    pub label: String,
    /// 0..1 of the track.
    pub x0: f32,
    pub x1: f32,
    pub duration_ms: Option<i64>,
}

/// Bars for every step. `min_frac` is the minimum bar width as a fraction of the track.
pub fn layout(s: &Session, min_frac: f32) -> (Vec<Bar>, bool) {
    let steps: Vec<(usize, usize, &rteams_services::sessions::steps::Step)> = s
        .chunks
        .iter()
        .enumerate()
        .flat_map(|(ci, c)| c.steps.iter().enumerate().map(move |(si, st)| (ci, si, st)))
        .collect();
    let timed = steps.iter().any(|(_, _, st)| st.start_ms.is_some());
    let t0 = steps
        .iter()
        .filter_map(|(_, _, st)| st.start_ms)
        .min()
        .unwrap_or(0);
    let t1 = steps
        .iter()
        .filter_map(|(_, _, st)| st.end_ms.or(st.start_ms))
        .max()
        .unwrap_or(t0)
        .max(t0 + 1);
    let n = steps.len().max(1) as f32;
    let bars = steps
        .iter()
        .enumerate()
        .map(|(i, (ci, si, st))| {
            let (x0, x1) = if timed {
                let a = st.start_ms.unwrap_or(t0);
                let b = st.end_ms.unwrap_or(a).max(a);
                let raw0 = (a - t0) as f32 / (t1 - t0) as f32;
                let raw1 = (b - t0) as f32 / (t1 - t0) as f32;
                let mut x1 = raw1.max(raw0 + min_frac);
                let mut x0 = raw0;
                if x1 > 1.0 {
                    x1 = 1.0;
                    x0 = x0.min(1.0 - min_frac);
                }
                (x0, x1)
            } else {
                (i as f32 / n, (i as f32 + 1.0) / n)
            };
            Bar {
                chunk: *ci,
                step: *si,
                kind: st.kind,
                error: st.tool.as_ref().is_some_and(|t| t.is_error),
                label: st.label(),
                x0,
                x1,
                duration_ms: st.duration_ms(),
            }
        })
        .collect();
    (bars, timed)
}

fn color(b: &Bar) -> egui::Color32 {
    if b.error {
        return theme::ERROR;
    }
    match b.kind {
        StepKind::Text => theme::ACCENT,
        StepKind::Thinking => theme::THINK_FG,
        StepKind::ToolCall => theme::TOOL_FG,
        StepKind::Subagent => theme::SEND,
    }
}

const ROW_H: f32 = 20.0;
const LABEL_W: f32 = 230.0;

/// Draw the waterfall; returns the chunk index of a clicked row.
pub fn show(ui: &mut egui::Ui, tr: &I18n, s: &Session) -> Option<usize> {
    let (bars, timed) = layout(s, 0.002);
    if bars.is_empty() {
        ui.label(RichText::new(tt(tr, "sessions.noSteps", "No steps")).color(theme::MUTED));
        return None;
    }
    let mut clicked = None;
    ui.horizontal(|ui| {
        for (k, name, c) in [
            (StepKind::Text, "Text", theme::ACCENT),
            (StepKind::Thinking, "Thinking", theme::THINK_FG),
            (StepKind::ToolCall, "Tool", theme::TOOL_FG),
            (StepKind::Subagent, "Subagent", theme::SEND),
        ] {
            let _ = k;
            theme::pill(ui, name, c);
        }
        theme::pill(ui, "Error", theme::ERROR);
        if !timed {
            ui.label(
                RichText::new(tt(
                    tr,
                    "sessions.noTimestamps",
                    "No timestamps: steps shown in sequence",
                ))
                .small()
                .color(theme::MUTED),
            );
        } else if let Some(d) = s.duration_ms() {
            ui.label(RichText::new(fmt_dur(d)).small().color(theme::MUTED));
        }
    });
    ui.add_space(4.0);
    egui::ScrollArea::vertical()
        .id_salt("session_waterfall")
        .auto_shrink([false, false])
        .show_rows(ui, ROW_H, bars.len(), |ui, range| {
            let w = ui.available_width();
            let track = (w - LABEL_W - 8.0).max(40.0);
            for i in range {
                let b = &bars[i];
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(w, ROW_H), egui::Sense::click());
                let p = ui.painter();
                if resp.hovered() {
                    p.rect_filled(rect, 2.0, theme::tint(theme::ACCENT, 0.08));
                }
                let label_rect =
                    egui::Rect::from_min_size(rect.min, egui::vec2(LABEL_W.min(w), ROW_H));
                p.with_clip_rect(label_rect).text(
                    label_rect.left_center() + egui::vec2(4.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    format!("#{} {}", b.chunk + 1, b.label),
                    egui::FontId::proportional(11.5),
                    theme::TEXT_2,
                );
                let x = rect.left() + LABEL_W.min(w) + 4.0;
                let bar = egui::Rect::from_min_max(
                    egui::pos2(x + b.x0 * track, rect.top() + 4.0),
                    egui::pos2(
                        (x + b.x1 * track).max(x + b.x0 * track + 2.0),
                        rect.bottom() - 4.0,
                    ),
                );
                p.rect_filled(bar, 2.0, color(b));
                let resp = resp.on_hover_text(match b.duration_ms {
                    Some(d) => format!("{}\n{}", b.label, fmt_dur(d)),
                    None => b.label.clone(),
                });
                if resp.clicked() {
                    clicked = Some(b.chunk);
                }
            }
        });
    clicked
}

#[cfg(test)]
mod tests {
    use super::super::viewer::tests::session;
    use super::*;

    #[test]
    fn bars_follow_time_and_stay_in_range() {
        let s = session();
        let (bars, timed) = layout(&s, 0.002);
        assert!(timed);
        assert_eq!(bars.len(), 3);
        for b in &bars {
            assert!(
                (0.0..=1.0).contains(&b.x0) && (0.0..=1.0).contains(&b.x1),
                "{b:?}"
            );
            assert!(b.x1 > b.x0);
        }
        // the tool call spans from 10:00:02 to 10:00:09, the whole tail of the session
        let tool = bars.iter().find(|b| b.kind == StepKind::ToolCall).unwrap();
        assert!(tool.error);
        assert!((tool.x1 - 1.0).abs() < 1e-6);
        assert_eq!(tool.duration_ms, Some(7000));
        assert!(bars[0].x0 <= bars[1].x0 && bars[1].x0 <= bars[2].x0);
    }

    #[test]
    fn falls_back_to_sequence_without_clock() {
        let raw = [
            r#"{"type":"user","message":{"role":"user","content":"go"}}"#,
            r#"{"type":"assistant","message":{"id":"a","content":[{"type":"text","text":"x"},{"type":"text","text":"y"}]}}"#,
        ]
        .join("\n");
        let s = rteams_services::sessions::chunks::Session::from_msgs(
            std::path::Path::new("/x/r.jsonl"),
            rteams_services::sessions::paths::Source::Rteams,
            rteams_services::sessions::chunks::parse_str(
                &raw,
                rteams_services::sessions::paths::Source::Rteams,
            ),
        );
        let (bars, timed) = layout(&s, 0.002);
        assert!(!timed);
        assert_eq!(bars.len(), 2);
        assert!((bars[0].x1 - bars[1].x0).abs() < 1e-6);
    }

    #[test]
    fn renders() {
        let s = session();
        let tr = I18n::default();
        crate::view::testing::render(|ui| {
            show(ui, &tr, &s);
        });
    }
}
