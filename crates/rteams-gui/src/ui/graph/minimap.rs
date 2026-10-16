//! Minimap: a small overview of the whole canvas with the current viewport outlined; click or
//! drag to recenter the camera. Generic: callers pass colored world rects.

use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind};

use super::camera::Camera;
use crate::theme;

pub const SIZE: egui::Vec2 = vec2(170.0, 110.0);

/// Minimap rect in the bottom-right corner of `viewport` (shrunk to fit tiny viewports).
pub fn area_for(viewport: Rect) -> Rect {
    let w = SIZE.x.min((viewport.width() - 24.0).max(0.0) * 0.5);
    let h = SIZE.y.min((viewport.height() - 24.0).max(0.0) * 0.5);
    Rect::from_min_size(
        pos2(viewport.right() - 12.0 - w, viewport.bottom() - 12.0 - h),
        vec2(w, h),
    )
}

fn scale_for(area: Rect, bounds: Rect) -> f32 {
    let s = (area.width() / bounds.width().max(1.0)).min(area.height() / bounds.height().max(1.0));
    if s.is_finite() && s > 0.0 {
        s
    } else {
        1.0
    }
}

pub fn world_to_map(p: Pos2, area: Rect, bounds: Rect) -> Pos2 {
    area.center() + (p - bounds.center()) * scale_for(area, bounds)
}

pub fn map_to_world(p: Pos2, area: Rect, bounds: Rect) -> Pos2 {
    bounds.center() + (p - area.center()) / scale_for(area, bounds)
}

/// Draw the minimap; returns true when the camera was moved by the user.
pub fn show(
    ui: &mut egui::Ui,
    viewport: Rect,
    bounds: Rect,
    dots: &[(Rect, Color32)],
    cam: &mut Camera,
) -> bool {
    show_in(ui, area_for(viewport), viewport, bounds, dots, cam)
}

/// [`show`] with an explicit minimap rect (callers that dock other controls below it).
pub fn show_in(
    ui: &mut egui::Ui,
    area: Rect,
    viewport: Rect,
    bounds: Rect,
    dots: &[(Rect, Color32)],
    cam: &mut Camera,
) -> bool {
    if area.width() < 40.0 || area.height() < 30.0 {
        return false;
    }
    let bounds = bounds.expand(60.0);
    let resp = ui.interact(
        area,
        egui::Id::unique("graph_minimap"),
        Sense::click_and_drag(),
    );
    let p = ui.painter_at(area.expand(2.0));
    p.rect_filled(
        area.translate(vec2(0.0, 4.0)).expand(2.0),
        14.0,
        Color32::from_black_alpha(60),
    );
    p.rect_filled(area, 12.0, theme::tint(theme::SIDEBAR, 0.92));
    p.rect_stroke(
        area,
        12.0,
        Stroke::new(1.0, theme::hairline(0.1)),
        StrokeKind::Inside,
    );
    for (r, c) in dots {
        let a = world_to_map(r.min, area, bounds);
        let b = world_to_map(r.max, area, bounds);
        let m = Rect::from_min_max(a, b);
        let m = if m.width() < 2.0 || m.height() < 2.0 {
            Rect::from_center_size(m.center(), vec2(2.0, 2.0))
        } else {
            m
        };
        p.rect_filled(m, 2.0, theme::tint(*c, 0.85));
    }
    let vis = cam.visible_world(viewport);
    let vr = Rect::from_min_max(
        world_to_map(vis.min, area, bounds),
        world_to_map(vis.max, area, bounds),
    )
    .intersect(area);
    if vr.width() > 0.0 && vr.height() > 0.0 {
        p.rect_stroke(vr, 4.0, Stroke::new(1.2, theme::ACCENT), StrokeKind::Inside);
        p.rect_filled(vr, 4.0, theme::tint(theme::ACCENT, 0.08));
    }
    let mut moved = false;
    if resp.clicked() || resp.dragged() {
        if let Some(ptr) = resp.interact_pointer_pos() {
            cam.center = map_to_world(ptr, area, bounds);
            cam.clamp();
            moved = true;
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_minimap_roundtrip_and_sizes() {
        let vp = Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 600.0));
        let a = area_for(vp);
        assert!(vp.contains_rect(a));
        let b = Rect::from_min_size(pos2(-1000.0, -500.0), vec2(2000.0, 1000.0));
        let w = pos2(120.0, -80.0);
        assert!((map_to_world(world_to_map(w, a, b), a, b) - w).length() < 1e-2);
        // Tiny viewport never yields negative sizes.
        let tiny = area_for(Rect::from_min_size(Pos2::ZERO, vec2(10.0, 10.0)));
        assert!(tiny.width() >= 0.0 && tiny.height() >= 0.0);
        // Degenerate bounds stay finite.
        let z = Rect::from_min_size(Pos2::ZERO, vec2(0.0, 0.0));
        assert!(world_to_map(w, a, z).x.is_finite());
    }
}
