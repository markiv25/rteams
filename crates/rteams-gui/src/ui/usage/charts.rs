//! Small drawing helpers for the Usage screen: cards, metric tiles, the trend bar chart
//! (painter), a segmented bar and the activity heatmap (painter).

use eframe::egui::{self, Color32, CornerRadius, RichText, Sense, Stroke, Vec2};
use rteams_services::usage::summary::{day_label, DayRow};

use crate::theme;

const DAY_MS: u64 = 86_400_000;

pub fn fmt_tokens(n: u64) -> String {
    let f = n as f64;
    if n >= 1_000_000_000 {
        format!("{:.1}B", f / 1e9)
    } else if n >= 1_000_000 {
        format!("{:.1}M", f / 1e6)
    } else if n >= 10_000 {
        format!("{:.1}K", f / 1e3)
    } else {
        n.to_string()
    }
}

pub fn fmt_cost(c: f64) -> String {
    if c >= 100.0 {
        format!("${c:.0}")
    } else if c >= 0.01 || c == 0.0 {
        format!("${c:.2}")
    } else {
        format!("${c:.4}")
    }
}

/// A titled panel.
pub fn card(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::RAISED)
        .stroke(Stroke::new(1.0, theme::hairline(0.07)))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).size(15.0).strong().color(theme::TEXT));
            ui.add_space(8.0);
            body(ui);
        });
}

pub fn empty(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(theme::MUTED));
}

/// Daily token bars drawn with the painter (egui_plot 0.37 targets egui 0.36, so it cannot be
/// used with this workspace's egui 0.37). Hover shows the day's tokens and cost.
pub fn trend(ui: &mut egui::Ui, _id: &str, days: &[DayRow], height: f32) {
    let w = ui.available_width().max(40.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, height), Sense::hover());
    let p = ui.painter_at(rect);
    let plot = egui::Rect::from_min_max(
        rect.min + Vec2::new(44.0, 4.0),
        rect.max - Vec2::new(4.0, 16.0),
    );
    let max = days.iter().map(|d| d.tokens).max().unwrap_or(0).max(1);
    let font = egui::FontId::proportional(10.0);
    for f in [0.0f32, 0.5, 1.0] {
        let y = plot.bottom() - plot.height() * f;
        p.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            Stroke::new(1.0, theme::hairline(0.07)),
        );
        p.text(
            egui::pos2(rect.left(), y),
            egui::Align2::LEFT_CENTER,
            fmt_tokens((max as f32 * f) as u64),
            font.clone(),
            theme::MUTED,
        );
    }
    if days.is_empty() || plot.width() <= 0.0 || plot.height() <= 0.0 {
        return;
    }
    let slot = plot.width() / days.len() as f32;
    let mut hover = None;
    for (i, d) in days.iter().enumerate() {
        let h = plot.height() * (d.tokens as f32 / max as f32);
        let x = plot.left() + slot * i as f32;
        let bar = egui::Rect::from_min_max(
            egui::pos2(x + slot * 0.1, plot.bottom() - h),
            egui::pos2(x + slot * 0.9, plot.bottom()),
        );
        p.rect_filled(bar, CornerRadius::same(1), theme::ACCENT);
        let col = egui::Rect::from_min_max(
            egui::pos2(x, plot.top()),
            egui::pos2(x + slot, plot.bottom()),
        );
        if resp.hover_pos().is_some_and(|m| col.contains(m)) {
            p.rect_filled(col, CornerRadius::ZERO, theme::tint(theme::TEXT, 0.06));
            hover = Some(format!(
                "{}: {} / {}",
                day_label(d.day_ms),
                fmt_tokens(d.tokens),
                fmt_cost(d.cost)
            ));
        }
    }
    if let (Some(first), Some(last)) = (days.first(), days.last()) {
        p.text(
            egui::pos2(plot.left(), rect.bottom()),
            egui::Align2::LEFT_BOTTOM,
            day_label(first.day_ms),
            font.clone(),
            theme::MUTED,
        );
        if days.len() > 1 {
            p.text(
                egui::pos2(plot.right(), rect.bottom()),
                egui::Align2::RIGHT_BOTTOM,
                day_label(last.day_ms),
                font,
                theme::MUTED,
            );
        }
    }
    if let Some(h) = hover {
        resp.on_hover_text(h);
    }
}

/// One horizontal bar split into proportional segments with a legend below.
pub fn segment_bar(ui: &mut egui::Ui, segs: &[(String, u64, Color32)]) {
    let total: u64 = segs.iter().map(|s| s.1).sum();
    let w = ui.available_width().max(10.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 10.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(5), theme::hairline(0.07));
    if total > 0 {
        let mut x = rect.left();
        for (_, v, c) in segs {
            let sw = rect.width() * (*v as f32 / total as f32);
            if sw > 0.0 {
                p.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(x, rect.top()),
                        Vec2::new(sw, rect.height()),
                    ),
                    CornerRadius::ZERO,
                    *c,
                );
            }
            x += sw;
        }
    }
    ui.horizontal_wrapped(|ui| {
        for (label, v, c) in segs {
            let pct = if total == 0 {
                0.0
            } else {
                *v as f64 / total as f64 * 100.0
            };
            ui.label(RichText::new("•").color(*c));
            ui.label(
                RichText::new(format!("{label} {} ({pct:.0}%)", fmt_tokens(*v)))
                    .small()
                    .color(theme::TEXT_2),
            );
        }
    });
}

/// Weekday of a UTC day start, Monday = 0.
fn weekday(day_ms: u64) -> usize {
    ((day_ms / DAY_MS + 3) % 7) as usize
}

/// GitHub-style heatmap: one column per week, one row per weekday, shaded by tokens.
pub fn heatmap(ui: &mut egui::Ui, days: &[DayRow], less: &str, more: &str) {
    let days = &days[days.len().saturating_sub(182)..];
    let Some(first) = days.first() else { return };
    let max = days.iter().map(|d| d.tokens).max().unwrap_or(0).max(1);
    let offset = weekday(first.day_ms);
    let cols = (days.len() + offset).div_ceil(7);
    let cell = 14.0;
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(cols as f32 * cell, 7.0 * cell), Sense::hover());
    let level = |t: u64| -> Color32 {
        if t == 0 {
            return theme::hairline(0.07);
        }
        let r = t as f32 / max as f32;
        theme::tint(
            theme::ACCENT,
            if r > 0.75 {
                1.0
            } else if r > 0.5 {
                0.75
            } else if r > 0.25 {
                0.5
            } else {
                0.3
            },
        )
    };
    let mut hover = None;
    for (i, d) in days.iter().enumerate() {
        let (col, row) = ((i + offset) / 7, (i + offset) % 7);
        let r = egui::Rect::from_min_size(
            rect.min + Vec2::new(col as f32 * cell, row as f32 * cell),
            Vec2::splat(cell - 2.0),
        );
        ui.painter()
            .rect_filled(r, CornerRadius::same(2), level(d.tokens));
        if resp.hover_pos().is_some_and(|p| r.expand(1.0).contains(p)) {
            hover = Some(format!(
                "{}: {} / {}",
                day_label(d.day_ms),
                fmt_tokens(d.tokens),
                fmt_cost(d.cost)
            ));
        }
    }
    if let Some(h) = hover {
        resp.on_hover_text(h);
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new(less).small().color(theme::MUTED));
        for t in [0, 1, 40, 60, 100] {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter()
                .rect_filled(r, CornerRadius::same(2), level(t * max / 100));
        }
        ui.label(RichText::new(more).small().color(theme::MUTED));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(12_345), "12.3K");
        assert_eq!(fmt_tokens(2_500_000), "2.5M");
        assert_eq!(fmt_cost(1.234), "$1.23");
        assert_eq!(fmt_cost(0.0004), "$0.0004");
        assert_eq!(fmt_cost(250.0), "$250");
        assert_eq!(weekday(0), 3); // 1970-01-01 was a Thursday
    }
}
