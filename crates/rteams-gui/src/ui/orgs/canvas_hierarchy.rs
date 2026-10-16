//! Hierarchy canvas: a tidy tree of organizations, groups and teams with reporting lines.

use eframe::egui::{pos2, Pos2, Shape, Stroke};

use super::canvas_nested::{draw_node, DrawOpts};
use super::layout::Layout;
use crate::theme;
use crate::ui::graph::kit::Canvas;

/// Orthogonal connector `a` -> `b` (down, across, down) with rounded corners.
fn elbow(a: Pos2, b: Pos2, r: f32) -> Vec<Pos2> {
    let my = (a.y + b.y) / 2.0;
    let dx = b.x - a.x;
    if dx.abs() < 1.0 {
        return vec![a, b];
    }
    let r = r.min(dx.abs() / 2.0).min((my - a.y).abs()).max(0.0);
    let s = dx.signum();
    let mut pts = vec![a, pos2(a.x, my - r)];
    let (c1x, c1y) = (a.x + s * r, my - r);
    for i in 1..=5 {
        let t = i as f32 / 5.0 * std::f32::consts::FRAC_PI_2;
        pts.push(pos2(c1x - s * r * t.cos(), c1y + r * t.sin()));
    }
    let (c2x, c2y) = (b.x - s * r, my + r);
    for i in 0..=5 {
        let t = i as f32 / 5.0 * std::f32::consts::FRAC_PI_2;
        pts.push(pos2(c2x + s * r * t.sin(), c2y - r * t.cos()));
    }
    pts.push(b);
    pts
}

pub fn draw(cv: &Canvas, l: &Layout, o: &DrawOpts) {
    for &(a, b) in &l.edges {
        let (ra, rb) = (cv.rect(l.nodes[a].rect), cv.rect(l.nodes[b].rect));
        if !cv.visible(ra.union(rb)) {
            continue;
        }
        let lit = o.dim.is_none_or(|d| d[a] && d[b]);
        let col = theme::tint(theme::TEXT_2, if lit { 0.4 } else { 0.1 });
        cv.painter.add(Shape::line(
            elbow(ra.center_bottom(), rb.center_top(), 10.0 * cv.cam.zoom),
            Stroke::new((1.5 * cv.cam.zoom).clamp(1.0, 2.2), col),
        ));
    }
    for (i, n) in l.nodes.iter().enumerate() {
        draw_node(cv, n, i, o, true);
    }
}
