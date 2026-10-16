//! Task pills and "+N more" overflow stacks.

use eframe::egui::{Align2, Color32, Pos2, Rect, Stroke};
use rteams_core::Status;
use rteams_services::graph_model::GraphNode;

use super::camera::Detail;
use super::kit::{self, Canvas, PillStyle};
use crate::theme;

pub fn status_color(s: Status) -> Color32 {
    match s {
        Status::Pending => theme::COL_TODO,
        Status::InProgress => theme::COL_IN_PROGRESS,
        Status::Review => theme::COL_REVIEW,
        Status::Approved => theme::COL_APPROVED,
        Status::Done => theme::COL_DONE,
        Status::NeedsFix => theme::ERROR,
        Status::Blocked => theme::WARNING,
    }
}

pub fn status_label(s: Status) -> &'static str {
    match s {
        Status::Pending => "To do",
        Status::InProgress => "In progress",
        Status::Review => "Review",
        Status::Approved => "Approved",
        Status::Done => "Done",
        Status::NeedsFix => "Needs fix",
        Status::Blocked => "Blocked",
    }
}

/// One task chip: status stripe, subject, `#id - owner`, status badge, owner avatar.
pub fn draw(
    cv: &Canvas,
    world_rect: Rect,
    node: &GraphNode,
    owner_color: Option<Color32>,
    hovered: bool,
    detail: Detail,
) {
    let r = cv.rect(world_rect);
    if !cv.visible(r) {
        return;
    }
    let p = cv.painter;
    let status = node.status.unwrap_or(Status::Pending);
    let col = status_color(status);
    if detail == Detail::Dot {
        p.rect_filled(r, 3.0, col);
        return;
    }
    let h = r.height();
    let active = status == Status::InProgress;
    kit::draw_pill(
        p,
        r,
        PillStyle {
            fill: kit::mix(theme::CARD, col, if hovered { 0.12 } else { 0.05 }),
            stroke: if hovered {
                col
            } else {
                theme::tint(col, if active { 0.45 } else { 0.22 })
            },
            stroke_width: if hovered { 1.6 } else { 1.0 },
            radius: h * 0.28,
            glow: active.then_some(theme::tint(col, 0.8)),
        },
    );
    let stripe = Rect::from_min_max(
        Pos2::new(r.left() + h * 0.13, r.top() + h * 0.2),
        Pos2::new(
            r.left() + h * 0.13 + (h * 0.07).max(1.5),
            r.bottom() - h * 0.2,
        ),
    );
    p.rect_filled(stripe, 3.0, col);
    let x = stripe.right() + h * 0.2;
    let id = node.id.trim_start_matches("task:");
    if detail != Detail::Full {
        kit::draw_text_fit(
            p,
            Pos2::new(x, r.center().y),
            &format!("#{id} {}", node.label),
            h * 0.3,
            theme::TEXT,
            r.right() - x - h * 0.2,
        );
        return;
    }
    let av_r = h * 0.15;
    let av_c = Pos2::new(r.right() - h * 0.22 - av_r, r.top() + h * 0.33);
    kit::draw_text_fit(
        p,
        Pos2::new(x, r.top() + h * 0.33),
        &node.label,
        h * 0.29,
        theme::TEXT,
        av_c.x - av_r - x - h * 0.12,
    );
    if let (Some(owner), Some(oc)) = (&node.owner, owner_color) {
        p.circle_filled(av_c, av_r, theme::tint(oc, 0.25));
        p.circle_stroke(av_c, av_r, Stroke::new(1.2, oc));
        let initial: String = owner.chars().next().unwrap_or('?').to_uppercase().collect();
        kit::draw_label(p, av_c, Align2::CENTER_CENTER, &initial, av_r * 1.15, oc);
    }
    let sub = match &node.owner {
        Some(o) => format!("#{id} \u{b7} {o}"),
        None => format!("#{id} \u{b7} unassigned"),
    };
    let badge = kit::draw_badge(
        p,
        Pos2::new(r.right() - h * 0.2, r.top() + h * 0.75),
        status_label(status),
        h * 0.2,
        col,
    );
    kit::draw_text_fit(
        p,
        Pos2::new(x, r.top() + h * 0.75),
        &sub,
        h * 0.215,
        theme::MUTED,
        badge.left() - x - h * 0.1,
    );
}

/// "+N more" stack.
pub fn draw_overflow(cv: &Canvas, world_rect: Rect, text: &str, hovered: bool) {
    let r = cv.rect(world_rect);
    if !cv.visible(r) {
        return;
    }
    let p = cv.painter;
    kit::draw_pill(
        p,
        r,
        PillStyle {
            fill: theme::tint(theme::RAISED, if hovered { 1.0 } else { 0.7 }),
            stroke: theme::hairline(if hovered { 0.4 } else { 0.2 }),
            stroke_width: 1.0,
            radius: r.height() * 0.5,
            glow: None,
        },
    );
    p.line_segment(
        [
            Pos2::new(r.left() + 6.0, r.bottom() + 3.0),
            Pos2::new(r.right() - 6.0, r.bottom() + 3.0),
        ],
        Stroke::new(1.0, theme::hairline(0.15)),
    );
    kit::draw_label(
        p,
        r.center(),
        Align2::CENTER_CENTER,
        text,
        r.height() * 0.45,
        theme::TEXT_2,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_tasks_status_colors_cover_all() {
        let set: std::collections::HashSet<_> =
            Status::ALL.iter().map(|s| status_color(*s)).collect();
        assert_eq!(set.len(), Status::ALL.len());
    }
}
