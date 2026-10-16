//! Agent (lead / member) and process nodes.

use eframe::egui::{Align2, Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use rteams_services::graph_model::{GraphNode, NodeKind, NodeState};

use super::camera::Detail;
use super::kit::{self, Canvas, PillStyle};
use crate::theme;

/// Color of a node state (idle slate, active green, thinking violet, tool amber, waiting blue,
/// complete teal, error red).
pub fn state_color(s: NodeState) -> Color32 {
    match s {
        NodeState::Idle => theme::MUTED,
        NodeState::Active => theme::SUCCESS,
        NodeState::Thinking => theme::THINK_FG,
        NodeState::ToolCalling => theme::TOOL_FG,
        NodeState::Waiting => theme::INFO,
        NodeState::Complete => theme::COL_DONE,
        NodeState::Error => theme::ERROR,
    }
}

pub fn state_label(s: NodeState) -> &'static str {
    match s {
        NodeState::Idle => "idle",
        NodeState::Active => "active",
        NodeState::Thinking => "thinking",
        NodeState::ToolCalling => "running tool",
        NodeState::Waiting => "waiting",
        NodeState::Complete => "complete",
        NodeState::Error => "error",
    }
}

/// Short status line for a member without a current task.
fn idle_line(node: &GraphNode) -> &'static str {
    match (node.state, node.online) {
        (NodeState::Error, _) => "needs attention",
        (NodeState::Waiting, _) => "waiting",
        (_, true) => "online",
        _ => "offline",
    }
}

/// Little crown above the lead's avatar.
fn crown(p: &Painter, c: Pos2, w: f32, color: Color32) {
    let h = w * 0.62;
    let (l, r, top, bot) = (c.x - w / 2.0, c.x + w / 2.0, c.y - h / 2.0, c.y + h / 2.0);
    let pts = vec![
        Pos2::new(l, bot),
        Pos2::new(l, top + h * 0.15),
        Pos2::new(l + w * 0.27, top + h * 0.5),
        Pos2::new(c.x, top),
        Pos2::new(r - w * 0.27, top + h * 0.5),
        Pos2::new(r, top + h * 0.15),
        Pos2::new(r, bot),
    ];
    p.add(Shape::convex_polygon(
        pts,
        theme::tint(color, 0.95),
        Stroke::new((w * 0.06).max(0.5), theme::tint(color, 0.4)),
    ));
}

/// Draw one member, lead or process node into `world_rect`. `color` is the member color,
/// `time` the egui time (the glow of working members breathes).
pub fn draw(
    cv: &Canvas,
    world_rect: Rect,
    node: &GraphNode,
    color: Color32,
    hovered: bool,
    detail: Detail,
    time: f64,
) {
    let r = cv.rect(world_rect);
    if !cv.visible(r) {
        return;
    }
    let p = cv.painter;
    let sc = state_color(node.state);
    let lead = node.kind == NodeKind::Lead;
    let dim = !node.online && node.kind != NodeKind::Process && node.state != NodeState::Active;
    let accent = if dim { theme::tint(color, 0.7) } else { color };
    if detail == Detail::Dot {
        let rad = (r.height() * 0.5).max(3.0);
        p.circle_filled(r.center(), rad, accent);
        if node.state == NodeState::Active {
            p.circle_stroke(r.center(), rad + 2.0, Stroke::new(1.5, sc));
        }
        return;
    }
    let h = r.height();
    let breathe = if node.state == NodeState::Active {
        0.75 + 0.25 * (time as f32 * 2.2).sin()
    } else {
        1.0
    };
    let radius = h * 0.3;
    let glow = if node.state == NodeState::Active || lead {
        Some(theme::tint(accent, breathe))
    } else {
        None
    };
    kit::draw_pill(
        p,
        r,
        PillStyle {
            fill: kit::mix(theme::CARD, accent, if hovered { 0.14 } else { 0.07 }),
            stroke: if hovered {
                accent
            } else {
                theme::tint(accent, if lead { 0.75 } else { 0.5 })
            },
            stroke_width: if hovered { 1.8 } else { 1.2 },
            radius,
            glow,
        },
    );
    if node.kind == NodeKind::Process {
        let right = r.right() - h * 0.4;
        kit::draw_text_fit(
            p,
            Pos2::new(r.left() + h * 0.5, r.center().y),
            &node.label,
            h * 0.36,
            theme::TEXT,
            right - r.left() - h * 0.5,
        );
        kit::draw_dot(p, Pos2::new(r.right() - h * 0.3, r.center().y), h * 0.1, sc);
        return;
    }
    // Avatar: tinted disc, member-colored ring, soft halo.
    let ar = h * 0.34;
    let pad = h * 0.16;
    let ac = Pos2::new(r.left() + pad + ar, r.center().y);
    p.circle_filled(ac, ar + h * 0.11, theme::tint(accent, 0.07 * breathe));
    p.circle_filled(ac, ar + h * 0.055, theme::tint(accent, 0.13 * breathe));
    p.circle_filled(ac, ar, theme::tint(accent, 0.22));
    p.circle_stroke(ac, ar, Stroke::new((h * 0.032).max(1.2), accent));
    if lead {
        p.circle_stroke(
            ac,
            ar + h * 0.09,
            Stroke::new((h * 0.014).max(0.8), theme::tint(accent, 0.55)),
        );
        crown(
            p,
            ac + Vec2::new(0.0, -ar - h * 0.13),
            h * 0.22,
            theme::TOOL_FG,
        );
    }
    let icon = crate::names::icon_for(p.ctx(), &node.label);
    kit::draw_label(p, ac, Align2::CENTER_CENTER, &icon, ar * 1.1, accent);
    // Presence / state dot on the avatar.
    let dp = ac + Vec2::angled(0.78) * ar;
    p.circle_filled(dp, h * 0.075 + 1.5, theme::CARD);
    p.circle_filled(dp, h * 0.075, sc);

    let text_x = ac.x + ar + pad * 1.2;
    let right = r.right() - pad;
    if detail == Detail::Full {
        let name_size = h * if lead { 0.22 } else { 0.23 };
        let name_w = kit::draw_text_fit(
            p,
            Pos2::new(text_x, r.top() + h * 0.27),
            &node.label,
            name_size,
            theme::TEXT,
            right - text_x,
        );
        if lead && right - text_x - name_w > h * 0.9 {
            kit::draw_badge(
                p,
                Pos2::new(right, r.top() + h * 0.27),
                "LEAD",
                h * 0.115,
                theme::TOOL_FG,
            );
        }
        let sub = node
            .runtime
            .clone()
            .or_else(|| node.role.clone())
            .unwrap_or_default();
        kit::draw_text_fit(
            p,
            Pos2::new(text_x, r.top() + h * 0.53),
            &sub,
            h * 0.165,
            theme::TEXT_2,
            right - text_x,
        );
        let (line, col) = match &node.current_task {
            Some(t) => (t.as_str(), theme::tint(theme::COL_IN_PROGRESS, 1.0)),
            None => (idle_line(node), theme::MUTED),
        };
        let dot_x = text_x + h * 0.04;
        kit::draw_dot(p, Pos2::new(dot_x, r.top() + h * 0.78), h * 0.035, col);
        kit::draw_text_fit(
            p,
            Pos2::new(dot_x + h * 0.1, r.top() + h * 0.78),
            line,
            h * 0.155,
            if node.current_task.is_some() {
                theme::TEXT
            } else {
                theme::MUTED
            },
            right - dot_x - h * 0.1,
        );
    } else {
        kit::draw_text_fit(
            p,
            Pos2::new(text_x, r.center().y),
            &node.label,
            h * 0.26,
            theme::TEXT,
            right - text_x,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_agents_state_colors_distinct() {
        let all = [
            NodeState::Idle,
            NodeState::Active,
            NodeState::Thinking,
            NodeState::ToolCalling,
            NodeState::Waiting,
            NodeState::Complete,
            NodeState::Error,
        ];
        let set: std::collections::HashSet<_> = all.iter().map(|s| state_color(*s)).collect();
        assert_eq!(set.len(), all.len());
        assert_eq!(state_label(NodeState::ToolCalling), "running tool");
    }
}
