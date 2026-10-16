//! viz::chart: bar / line / pie drawn straight with the painter (no plotting crate).

use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, Vec2,
};
use rteams_services::viz::{ChartKind, ChartSpec};

use crate::theme;

const COLORS: [Color32; 8] = [
    Color32::from_rgb(0x60, 0xa5, 0xfa),
    Color32::from_rgb(0x34, 0xd3, 0x99),
    Color32::from_rgb(0xfb, 0xbf, 0x24),
    Color32::from_rgb(0xf8, 0x71, 0x71),
    Color32::from_rgb(0xa7, 0x8b, 0xfa),
    Color32::from_rgb(0x22, 0xd3, 0xee),
    Color32::from_rgb(0xfb, 0x92, 0x3c),
    Color32::from_rgb(0xf4, 0x72, 0xb6),
];

pub fn color(i: usize) -> Color32 {
    COLORS[i % COLORS.len()]
}

/// Compact number: integers plain, otherwise up to 2 decimals.
pub fn fmt_num(v: f64) -> String {
    if v.abs() >= 1e15 || (v.fract().abs() < 1e-9) {
        format!("{:.0}", v + 0.0)
    } else {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Axis range including zero, with the top rounded to a "nice" value.
pub fn axis_range(values: impl Iterator<Item = f64>) -> (f64, f64) {
    let (mut lo, mut hi) = (0.0f64, 0.0f64);
    for v in values {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if hi == 0.0 && lo == 0.0 {
        return (0.0, 1.0);
    }
    (nice(lo), nice(hi))
}

fn nice(v: f64) -> f64 {
    if v == 0.0 {
        return 0.0;
    }
    let s = v.signum();
    let a = v.abs();
    let p = 10f64.powf(a.log10().floor());
    let f = a / p;
    let n = if f <= 1.0 {
        1.0
    } else if f <= 2.0 {
        2.0
    } else if f <= 5.0 {
        5.0
    } else {
        10.0
    };
    s * n * p
}

/// Start/end angles (radians, clockwise from 12 o'clock) per slice.
pub fn pie_angles(values: &[f64]) -> Vec<(f32, f32)> {
    let total: f64 = values.iter().sum();
    let mut acc = 0.0;
    values
        .iter()
        .map(|v| {
            let a = (acc / total * std::f64::consts::TAU) as f32;
            acc += v;
            (a, (acc / total * std::f64::consts::TAU) as f32)
        })
        .collect()
}

fn legend(ui: &mut egui::Ui, items: impl Iterator<Item = (usize, String)>) {
    ui.horizontal_wrapped(|ui| {
        for (i, name) in items {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter().rect_filled(r, 2.0, color(i));
            ui.label(RichText::new(name).small().color(theme::TEXT_2));
            ui.add_space(6.0);
        }
    });
}

pub fn show(ui: &mut egui::Ui, c: &ChartSpec) {
    match c.kind {
        ChartKind::Pie => pie(ui, c),
        _ => {
            legend(
                ui,
                c.series
                    .iter()
                    .enumerate()
                    .map(|(i, s)| (i, s.name.clone())),
            );
            cartesian(ui, c);
        }
    }
}

fn cartesian(ui: &mut egui::Ui, c: &ChartSpec) {
    let w = ui.available_width().clamp(120.0, 720.0);
    let (resp, p) = ui.allocate_painter(Vec2::new(w, 220.0), Sense::hover());
    let font = FontId::proportional(10.0);
    let (lo, hi) = axis_range(c.series.iter().flat_map(|s| s.values.iter().copied()));
    let plot = Rect::from_min_max(
        resp.rect.min + Vec2::new(44.0, 6.0),
        resp.rect.max - Vec2::new(6.0, 22.0),
    );
    let y = |v: f64| plot.bottom() - ((v - lo) / (hi - lo)) as f32 * plot.height();
    for k in 0..=4 {
        let v = lo + (hi - lo) * k as f64 / 4.0;
        let yy = y(v);
        p.line_segment(
            [Pos2::new(plot.left(), yy), Pos2::new(plot.right(), yy)],
            Stroke::new(1.0, theme::hairline(0.07)),
        );
        p.text(
            Pos2::new(plot.left() - 4.0, yy),
            Align2::RIGHT_CENTER,
            fmt_num(v),
            font.clone(),
            theme::MUTED,
        );
    }
    let n = c.labels.len();
    let slot = plot.width() / n as f32;
    let stride = ((40.0 / slot).ceil() as usize).max(1);
    for (i, l) in c.labels.iter().enumerate().step_by(stride) {
        let text: String = l.chars().take(12).collect();
        p.text(
            Pos2::new(plot.left() + slot * (i as f32 + 0.5), plot.bottom() + 4.0),
            Align2::CENTER_TOP,
            text,
            font.clone(),
            theme::MUTED,
        );
    }
    let zero = y(0.0);
    let hover = resp
        .hover_pos()
        .filter(|h| plot.contains(*h))
        .map(|h| (((h.x - plot.left()) / slot) as usize).min(n - 1));
    match c.kind {
        ChartKind::Bar => {
            let ns = c.series.len() as f32;
            let bw = (slot * 0.8 / ns).max(1.0);
            for (si, s) in c.series.iter().enumerate() {
                for (i, v) in s.values.iter().enumerate() {
                    let x0 = plot.left() + slot * i as f32 + slot * 0.1 + bw * si as f32;
                    let r =
                        Rect::from_two_pos(Pos2::new(x0, y(*v)), Pos2::new(x0 + bw - 1.0, zero));
                    p.rect_filled(r, 2.0, color(si));
                }
            }
        }
        _ => {
            for (si, s) in c.series.iter().enumerate() {
                let pts: Vec<Pos2> = s
                    .values
                    .iter()
                    .enumerate()
                    .map(|(i, v)| Pos2::new(plot.left() + slot * (i as f32 + 0.5), y(*v)))
                    .collect();
                p.add(Shape::line(pts.clone(), Stroke::new(2.0, color(si))));
                for pt in pts {
                    p.circle_filled(pt, 3.0, color(si));
                }
            }
        }
    }
    if let Some(i) = hover {
        p.rect_filled(
            Rect::from_x_y_ranges(
                plot.left() + slot * i as f32..=plot.left() + slot * (i as f32 + 1.0),
                plot.y_range(),
            ),
            0.0,
            theme::hairline(0.06),
        );
        let mut t = c.labels[i].clone();
        for s in &c.series {
            t.push_str(&format!("\n{}: {}", s.name, fmt_num(s.values[i])));
        }
        resp.on_hover_text_at_pointer(t);
    }
}

fn pie(ui: &mut egui::Ui, c: &ChartSpec) {
    let vals = &c.series[0].values;
    legend(ui, c.labels.iter().enumerate().map(|(i, l)| (i, l.clone())));
    let (resp, p) = ui.allocate_painter(
        Vec2::new(ui.available_width().clamp(120.0, 720.0), 200.0),
        Sense::hover(),
    );
    let center = resp.rect.center();
    let radius = (resp.rect.height() / 2.0 - 6.0).max(10.0);
    let total: f64 = vals.iter().sum();
    let angles = pie_angles(vals);
    let at = |a: f32| center + Vec2::new(a.sin(), -a.cos()) * radius;
    for (i, (a0, a1)) in angles.iter().enumerate() {
        if a1 - a0 <= 0.0 {
            continue;
        }
        let steps = (((a1 - a0) / 0.1).ceil() as usize).max(1);
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(center, color(i));
        for k in 0..=steps {
            mesh.colored_vertex(at(a0 + (a1 - a0) * k as f32 / steps as f32), color(i));
        }
        for k in 0..steps as u32 {
            mesh.add_triangle(0, k + 1, k + 2);
        }
        p.add(Shape::mesh(mesh));
    }
    if let Some(h) = resp.hover_pos() {
        let d = h - center;
        if d.length() <= radius {
            let a = d.x.atan2(-d.y).rem_euclid(std::f32::consts::TAU);
            if let Some(i) = angles.iter().position(|(a0, a1)| a >= *a0 && a < *a1) {
                let pct = vals[i] / total * 100.0;
                resp.on_hover_text_at_pointer(format!(
                    "{}: {} ({:.1}%)",
                    c.labels[i],
                    fmt_num(vals[i]),
                    pct
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_format() {
        assert_eq!(fmt_num(3.0), "3");
        assert_eq!(fmt_num(2.5), "2.5");
        assert_eq!(fmt_num(1.239), "1.24");
        assert_eq!(fmt_num(-0.0), "0");
    }

    #[test]
    fn axis_ranges() {
        assert_eq!(axis_range([0.0, 0.0].into_iter()), (0.0, 1.0));
        assert_eq!(axis_range([3.0, 7.0].into_iter()), (0.0, 10.0));
        assert_eq!(axis_range([-2.0, 1.0].into_iter()), (-2.0, 1.0));
    }

    #[test]
    fn pie_angles_cover_circle() {
        let a = pie_angles(&[1.0, 1.0, 2.0]);
        assert!((a[0].1 - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
        assert!((a[2].1 - std::f32::consts::TAU).abs() < 1e-4);
        assert!((a[1].1 - a[2].0).abs() < 1e-6);
    }
}
