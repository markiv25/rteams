//! Edge geometry and styling: curves leave nodes at their borders; kinds differ in color,
//! dash and arrowheads.

use std::collections::HashSet;

use eframe::egui::{Color32, Pos2};
use rteams_services::graph_model::{EdgeKind, GraphModel};

use super::kit::{self, anchor_on_rect, Bezier, Canvas, EdgeStyle};
use super::model::{Layout, LayoutMode};
use crate::theme;

#[derive(Debug, Clone)]
pub struct EdgeGeom {
    pub kind: EdgeKind,
    /// Node ids from the model.
    pub from: String,
    pub to: String,
    /// Placed item ids actually connected (an overflow stack may stand in for a task).
    pub from_item: String,
    pub to_item: String,
    /// World space.
    pub bez: Bezier,
}

pub fn style(kind: EdgeKind, emphasis: bool) -> EdgeStyle {
    let (color, width, dashed, arrow) = match kind {
        EdgeKind::ParentChild => (theme::tint(theme::ACCENT, 0.45), 2.0, false, false),
        EdgeKind::Ownership => (theme::tint(theme::TEXT_2, 0.3), 1.2, false, false),
        EdgeKind::Blocking => (theme::tint(theme::WARNING, 0.85), 1.8, true, true),
        EdgeKind::Related => (theme::tint(theme::MUTED, 0.7), 1.2, true, false),
        EdgeKind::Review => (theme::tint(theme::COL_REVIEW, 0.9), 1.8, false, true),
        EdgeKind::Message => (theme::tint(theme::SEND, 0.7), 1.6, false, true),
    };
    let color = if emphasis { opaque(color) } else { color };
    EdgeStyle {
        color,
        width: if emphasis { width + 1.0 } else { width },
        dashed,
        arrow,
    }
}

fn opaque(c: Color32) -> Color32 {
    Color32::from_rgb(c.r(), c.g(), c.b())
}

/// Curve between two placed items, leaving each at its border.
pub fn connect(a: &super::model::Placed, b: &super::model::Placed, bend: f32) -> Bezier {
    let ra = a.rect();
    let rb = b.rect();
    let pa = anchor_on_rect(ra, rb.center());
    let pb = anchor_on_rect(rb, ra.center());
    Bezier::connector(pa, pb, bend)
}

pub fn compute(model: &GraphModel, layout: &Layout, mode: LayoutMode) -> Vec<EdgeGeom> {
    let mut seen: HashSet<(String, String, EdgeKind)> = HashSet::new();
    let mut out = Vec::new();
    for e in &model.edges {
        if mode == LayoutMode::Kanban && e.kind == EdgeKind::Ownership {
            continue;
        }
        let (Some(a), Some(b)) = (layout.resolve(&e.from), layout.resolve(&e.to)) else {
            continue;
        };
        if a.id == b.id || !seen.insert((a.id.clone(), b.id.clone(), e.kind)) {
            continue;
        }
        let bend = match e.kind {
            EdgeKind::Message => 0.18,
            EdgeKind::Blocking | EdgeKind::Related => 0.12,
            _ => 0.04,
        };
        out.push(EdgeGeom {
            kind: e.kind,
            from: e.from.clone(),
            to: e.to.clone(),
            from_item: a.id.clone(),
            to_item: b.id.clone(),
            bez: connect(a, b, bend),
        });
    }
    out
}

/// Draw all edges; those touching `hovered` (an item id) are emphasized.
pub fn draw(cv: &Canvas, geoms: &[EdgeGeom], hovered: Option<&str>, hovered_edge: Option<usize>) {
    for (i, g) in geoms.iter().enumerate() {
        let b = g.bez.map(|p| cv.pt(p));
        let r = eframe::egui::Rect::from_two_pos(b.p0, b.p3).expand(80.0);
        if !cv.visible(r) {
            continue;
        }
        let emph =
            Some(i) == hovered_edge || hovered.is_some_and(|h| g.from_item == h || g.to_item == h);
        let mut st = style(g.kind, emph);
        // Arrowheads scale a little with zoom but stay legible.
        st.width = (st.width * cv.cam.zoom.clamp(0.6, 1.4)).max(0.8);
        kit::draw_edge(cv.painter, &b, st);
    }
}

/// Index of the blocking edge near a screen point, if any.
pub fn hit_blocking(cv: &Canvas, geoms: &[EdgeGeom], screen: Pos2) -> Option<usize> {
    geoms
        .iter()
        .enumerate()
        .filter(|(_, g)| g.kind == EdgeKind::Blocking)
        .map(|(i, g)| (i, g.bez.map(|p| cv.pt(p)).distance_to(screen)))
        .filter(|(_, d)| *d <= 7.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;
    use rteams_services::graph_model::GraphEdge;

    use super::super::model::{Placed, PlacedKind};

    fn place(l: &mut Layout, id: &str, x: f32, y: f32) {
        l.push(Placed {
            id: id.into(),
            pos: pos2(x, y),
            size: eframe::egui::vec2(200.0, 36.0),
            kind: PlacedKind::Node,
        });
    }

    #[test]
    fn graph_edges_skip_missing_and_dedupe() {
        let mut l = Layout::default();
        place(&mut l, "a", 0.0, 0.0);
        place(&mut l, "b", 400.0, 0.0);
        l.hidden_in.insert("c".into(), "b".into());
        let e = |f: &str, t: &str, k| GraphEdge {
            from: f.into(),
            to: t.into(),
            kind: k,
        };
        let m = GraphModel {
            edges: vec![
                e("a", "b", EdgeKind::Blocking),
                e("a", "c", EdgeKind::Blocking), // folds into the same stack
                e("a", "zzz", EdgeKind::Blocking),
                e("a", "a", EdgeKind::Related),
                e("a", "b", EdgeKind::Ownership),
            ],
            ..Default::default()
        };
        let g = compute(&m, &l, LayoutMode::Slots);
        assert_eq!(g.len(), 2);
        assert_eq!(compute(&m, &l, LayoutMode::Kanban).len(), 1);
        // Curve starts at the border of `a`, not its center.
        assert!((g[0].bez.p0.x - 100.0).abs() < 1.0);
        assert!((g[0].bez.p3.x - 300.0).abs() < 1.0);
    }

    #[test]
    fn graph_edges_styles() {
        assert!(style(EdgeKind::Blocking, false).dashed);
        assert!(style(EdgeKind::Blocking, false).arrow);
        assert!(!style(EdgeKind::ParentChild, false).arrow);
        assert!(style(EdgeKind::Message, true).width > style(EdgeKind::Message, false).width);
    }
}
