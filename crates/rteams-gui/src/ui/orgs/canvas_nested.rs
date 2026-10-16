//! Nested map canvas: organizations as rounded containers, groups inside, team cards inside.
//! Also hosts the node / relation drawing shared with the hierarchy canvas.

use eframe::egui::{pos2, vec2, Color32, Pos2, Rect, Stroke, StrokeKind};
use rteams_services::i18n::I18n;
use rteams_services::orgs::model::RelationKind;

use super::layout::{Key, Layout, Node, RelEdge};
use crate::icons::{self, Icon};
use crate::theme;
use crate::ui::graph::camera::{semantic_zoom, Detail};
use crate::ui::graph::kit::{
    anchor_on_rect, draw_badge, draw_dot, draw_edge, draw_pill, draw_text_fit, mix, Bezier, Canvas,
    EdgeStyle, PillStyle,
};

/// Per-frame drawing inputs.
pub struct DrawOpts<'a> {
    pub tr: &'a I18n,
    /// `Some(flags)` dims every node whose flag is false.
    pub dim: Option<&'a [bool]>,
    pub hover: Option<usize>,
    /// Container currently under a dragged team card.
    pub drop_hot: Option<usize>,
    /// Team being dragged (drawn faded in place).
    pub dragging: Option<&'a str>,
}

const DIM: f32 = 0.28;

fn k_of(o: &DrawOpts, i: usize) -> f32 {
    if o.dim.is_some_and(|d| d.get(i) == Some(&false)) {
        DIM
    } else {
        1.0
    }
}

fn fade(c: Color32, k: f32) -> Color32 {
    c.gamma_multiply(k)
}

pub fn draw(cv: &Canvas, l: &Layout, o: &DrawOpts) {
    for (i, n) in l.nodes.iter().enumerate() {
        draw_node(cv, n, i, o, false);
    }
}

/// Summary line under a node's name.
pub fn summary(tr: &I18n, n: &Node) -> String {
    let s = &n.stats;
    match n.key {
        Key::Team(_) => format!(
            "{} · {}",
            tr.t_count("team.organizations.graph.canvas.agents", s.members as u64),
            tr.t_count(
                "team.organizations.graph.overviewCard.activeTasks",
                s.open_tasks as u64
            ),
        ),
        _ => tr.t_args(
            "team.organizations.graph.canvas.groupSummary",
            &[
                ("teamCount", &s.teams.to_string()),
                ("activeTeamCount", &s.running.to_string()),
                ("taskCount", &s.open_tasks.to_string()),
            ],
        ),
    }
}

fn icon_of(key: &Key) -> Icon {
    match key {
        Key::Root | Key::Org(_) => Icon::Sitemap,
        Key::Group(..) | Key::Unplaced => Icon::Folder,
        Key::Team(_) => Icon::Users,
    }
}

/// Rounded square tile with an icon, tinted with `color`.
fn icon_tile(p: &eframe::egui::Painter, r: Rect, icon: Icon, color: Color32) {
    p.rect_filled(r, r.width() * 0.26, theme::tint(color, 0.16));
    p.rect_stroke(
        r,
        r.width() * 0.26,
        Stroke::new(1.0, theme::tint(color, 0.3)),
        StrokeKind::Inside,
    );
    icons::draw(
        p,
        Rect::from_center_size(r.center(), r.size() * 0.58),
        icon,
        color,
    );
}

/// Draw one node. `compact` = hierarchy style (every node a card); otherwise containers are
/// drawn as open frames with a header.
pub fn draw_node(cv: &Canvas, n: &Node, i: usize, o: &DrawOpts, compact: bool) {
    let r = cv.rect(n.rect);
    if !cv.visible(r) || r.width() < 1.0 {
        return;
    }
    let p = cv.painter;
    let z = cv.cam.zoom;
    let detail = semantic_zoom(z);
    let mut k = k_of(o, i);
    if matches!(&n.key, Key::Team(t) if Some(t.as_str()) == o.dragging) {
        k *= 0.35;
    }
    let hot = o.drop_hot == Some(i);
    let hover = o.hover == Some(i);
    let color = n.color;
    let is_card = compact || !n.is_container();
    let radius = if is_card {
        (11.0 * z).clamp(2.0, 20.0)
    } else {
        (18.0 * z).clamp(3.0, 30.0)
    };
    let (fill, stroke) = if is_card {
        (
            fade(
                mix(theme::CARD, color, if hover { 0.12 } else { 0.06 }),
                k.max(0.6),
            ),
            fade(color, if hover { 0.95 } else { 0.55 } * k),
        )
    } else {
        (
            theme::tint(color, 0.045 * k),
            theme::tint(color, if hot { 0.9 } else { 0.28 * k }),
        )
    };
    let stroke = if hot { theme::ACCENT } else { stroke };
    draw_pill(
        p,
        r,
        PillStyle {
            fill,
            stroke,
            stroke_width: if hot || hover { 1.8 } else { 1.0 },
            radius,
            glow: (hot || (hover && k >= 1.0) || (is_card && n.stats.running > 0 && k >= 1.0))
                .then_some(color),
        },
    );
    if detail == Detail::Dot {
        return;
    }
    let pad = 12.0 * z;
    let text_c = fade(theme::TEXT, k);
    let sub_c = fade(theme::TEXT_2, k);
    if is_card {
        let bar = Rect::from_min_size(
            r.min + vec2(0.0, radius * 0.6),
            vec2((4.0 * z).max(1.5), r.height() - 1.2 * radius),
        );
        p.rect_filled(bar, 2.0, fade(color, k));
        let th = r.height() * 0.54;
        let tile = Rect::from_min_size(
            pos2(r.left() + pad + 4.0 * z, r.center().y - th / 2.0),
            vec2(th, th),
        );
        icon_tile(p, tile, icon_of(&n.key), fade(color, k));
        let tx = tile.right() + 10.0 * z;
        let right = r.right() - pad - 10.0 * z;
        let top = if detail == Detail::Full {
            r.top() + r.height() * 0.36
        } else {
            r.center().y
        };
        draw_text_fit(p, pos2(tx, top), &n.label, 14.0 * z, text_c, right - tx);
        let online = n.stats.running > 0;
        draw_dot(
            p,
            pos2(r.right() - pad, r.top() + 13.0 * z),
            3.2 * z,
            fade(if online { theme::SUCCESS } else { theme::MUTED }, k),
        );
        if detail == Detail::Full {
            draw_text_fit(
                p,
                pos2(tx, r.top() + r.height() * 0.7),
                &summary(o.tr, n),
                10.5 * z,
                sub_c,
                r.right() - pad - tx,
            );
        }
    } else {
        let hdr = 34.0 * z;
        let tile = Rect::from_center_size(
            pos2(r.left() + pad + 12.0 * z, r.top() + hdr / 2.0 + 2.0 * z),
            vec2(24.0 * z, 24.0 * z),
        );
        icon_tile(p, tile, icon_of(&n.key), fade(color, k));
        let right = if detail == Detail::Full && n.stats.teams > 0 {
            let b = draw_badge(
                p,
                pos2(r.right() - pad, tile.center().y),
                &o.tr.t_count(
                    "team.organizations.graph.canvas.teams",
                    n.stats.teams as u64,
                ),
                10.0 * z,
                fade(color, k),
            );
            b.left()
        } else {
            r.right()
        };
        draw_text_fit(
            p,
            pos2(tile.right() + 8.0 * z, tile.center().y),
            &n.label,
            (14.0 * z).max(5.0),
            text_c,
            right - tile.right() - 14.0 * z,
        );
    }
}

fn kind_style(kind: Option<RelationKind>) -> (Color32, bool) {
    match kind {
        Some(RelationKind::DependsOn) => (theme::WARNING, false),
        Some(RelationKind::Delegates) => (theme::ACCENT, false),
        Some(RelationKind::Observes) => (theme::INFO, true),
        Some(RelationKind::ReportsTo) => (theme::SUCCESS, false),
        None => (theme::SEND, true),
    }
}

/// Stroke width in world-independent screen points: grows with the weight.
pub fn edge_width(weight: usize) -> f32 {
    1.4 + (1.0 + weight as f32).ln().min(3.0)
}

pub fn draw_relations(cv: &Canvas, l: &Layout, rels: &[RelEdge], o: &DrawOpts) {
    let detail = semantic_zoom(cv.cam.zoom);
    for r in rels {
        let (ra, rb) = (cv.rect(l.nodes[r.a].rect), cv.rect(l.nodes[r.b].rect));
        if !cv.visible(ra.union(rb)) {
            continue;
        }
        let k = k_of(o, r.a).max(k_of(o, r.b));
        let (c, dashed) = kind_style(r.kind);
        let (pa, pb) = (
            anchor_on_rect(ra, rb.center()),
            anchor_on_rect(rb, ra.center()),
        );
        let bez = Bezier::connector(pa, pb, 0.18);
        draw_edge(
            cv.painter,
            &bez,
            EdgeStyle {
                color: fade(c, 0.85 * k),
                width: edge_width(r.weight),
                dashed,
                arrow: true,
            },
        );
        if r.weight > 0 && detail == Detail::Full {
            let mid: Pos2 = bez.at(0.5);
            draw_badge(
                cv.painter,
                mid + vec2(14.0, 0.0),
                &r.weight.to_string(),
                10.0,
                fade(c, k),
            );
        }
    }
}
