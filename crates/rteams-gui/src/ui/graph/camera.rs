//! Pan / zoom camera shared by every canvas built on the graph kit.
//!
//! World space is the layout's coordinate system; screen space is egui points inside a
//! `viewport` rect. `center` is the world point shown at the viewport's center. Every method is
//! NaN-safe: non-finite inputs are ignored and zoom is always clamped to `MIN_ZOOM..=MAX_ZOOM`.

use eframe::egui::{Pos2, Rect, Vec2};

pub const MIN_ZOOM: f32 = 0.02;
pub const MAX_ZOOM: f32 = 5.0;
/// Multiplier of one zoom-in / zoom-out step (original: 1.08 / 0.92).
pub const ZOOM_IN: f32 = 1.08;
pub const ZOOM_OUT: f32 = 0.92;

/// How much of a node to draw at the current zoom ("semantic zoom").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Detail {
    /// Only a dot / bar; no text.
    Dot,
    /// Name only.
    Compact,
    /// Everything: badges, secondary lines.
    Full,
}

pub fn semantic_zoom(zoom: f32) -> Detail {
    if zoom < 0.2 {
        Detail::Dot
    } else if zoom < 0.38 {
        Detail::Compact
    } else {
        Detail::Full
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub center: Pos2,
    pub zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            center: Pos2::ZERO,
            zoom: 1.0,
        }
    }
}

fn finite(p: Pos2) -> bool {
    p.x.is_finite() && p.y.is_finite()
}

impl Camera {
    pub fn world_to_screen(&self, p: Pos2, viewport: Rect) -> Pos2 {
        viewport.center() + (p - self.center) * self.zoom
    }

    pub fn screen_to_world(&self, p: Pos2, viewport: Rect) -> Pos2 {
        self.center + (p - viewport.center()) / self.zoom
    }

    /// World rect to screen rect.
    pub fn rect_to_screen(&self, r: Rect, viewport: Rect) -> Rect {
        Rect::from_min_max(
            self.world_to_screen(r.min, viewport),
            self.world_to_screen(r.max, viewport),
        )
    }

    /// The part of world space currently visible in `viewport`.
    pub fn visible_world(&self, viewport: Rect) -> Rect {
        Rect::from_min_max(
            self.screen_to_world(viewport.min, viewport),
            self.screen_to_world(viewport.max, viewport),
        )
    }

    /// Multiply the zoom by `factor`, keeping the world point under `anchor` (screen) fixed.
    pub fn zoom_at(&mut self, anchor: Pos2, viewport: Rect, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 || !finite(anchor) {
            return;
        }
        let before = self.screen_to_world(anchor, viewport);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let after = self.screen_to_world(anchor, viewport);
        self.center += before - after;
    }

    /// Zoom around the viewport center (toolbar buttons).
    pub fn zoom_step(&mut self, viewport: Rect, zoom_in: bool) {
        self.zoom_at(
            viewport.center(),
            viewport,
            if zoom_in { ZOOM_IN } else { ZOOM_OUT },
        );
    }

    /// Move by a screen-space drag delta.
    pub fn pan_screen(&mut self, delta: Vec2) {
        if delta.x.is_finite() && delta.y.is_finite() {
            self.center -= delta / self.zoom;
        }
    }

    /// Center `bounds` (world) in `viewport` with `margin` screen points around it. Never zooms
    /// in past 1.2 so a lone node is not blown up.
    pub fn fit(&mut self, bounds: Rect, viewport: Rect, margin: f32) {
        if !(finite(bounds.min) && finite(bounds.max)) || bounds.is_negative() {
            *self = Camera::default();
            return;
        }
        let avail_w = (viewport.width() - 2.0 * margin).max(1.0);
        let avail_h = (viewport.height() - 2.0 * margin).max(1.0);
        let zx = avail_w / bounds.width().max(1.0);
        let zy = avail_h / bounds.height().max(1.0);
        self.zoom = zx.min(zy).clamp(MIN_ZOOM, 1.2);
        self.center = bounds.center();
    }

    pub fn clamp(&mut self) {
        if !self.zoom.is_finite() {
            self.zoom = 1.0;
        }
        self.zoom = self.zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        if !finite(self.center) {
            self.center = Pos2::ZERO;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{pos2, vec2};

    fn vp() -> Rect {
        Rect::from_min_size(pos2(100.0, 50.0), vec2(800.0, 600.0))
    }

    #[test]
    fn graph_camera_roundtrip() {
        let c = Camera {
            center: pos2(37.0, -12.0),
            zoom: 0.5,
        };
        let p = pos2(-200.0, 340.0);
        let s = c.world_to_screen(p, vp());
        let back = c.screen_to_world(s, vp());
        assert!((back - p).length() < 1e-3);
        assert_eq!(c.world_to_screen(c.center, vp()), vp().center());
    }

    #[test]
    fn graph_camera_zoom_keeps_anchor() {
        let mut c = Camera::default();
        let anchor = pos2(300.0, 200.0);
        let w = c.screen_to_world(anchor, vp());
        c.zoom_at(anchor, vp(), 1.5);
        assert!((c.screen_to_world(anchor, vp()) - w).length() < 1e-3);
        assert!((c.zoom - 1.5).abs() < 1e-6);
    }

    #[test]
    fn graph_camera_clamps_and_ignores_nan() {
        let mut c = Camera::default();
        for _ in 0..500 {
            c.zoom_at(vp().center(), vp(), 0.5);
        }
        assert_eq!(c.zoom, MIN_ZOOM);
        for _ in 0..500 {
            c.zoom_at(vp().center(), vp(), 2.0);
        }
        assert_eq!(c.zoom, MAX_ZOOM);
        let before = c;
        c.zoom_at(vp().center(), vp(), f32::NAN);
        c.pan_screen(vec2(f32::NAN, 1.0));
        assert_eq!(c, before);
    }

    #[test]
    fn graph_camera_fit_contains_bounds() {
        let mut c = Camera::default();
        let b = Rect::from_min_max(pos2(-1000.0, -500.0), pos2(1000.0, 500.0));
        c.fit(b, vp(), 20.0);
        let s = c.rect_to_screen(b, vp());
        assert!(s.width() <= vp().width() - 40.0 + 1.0);
        assert!(s.height() <= vp().height() - 40.0 + 1.0);
        assert!((s.center() - vp().center()).length() < 1e-3);
        // Degenerate viewports and bounds stay finite.
        c.fit(b, Rect::from_min_size(Pos2::ZERO, vec2(10.0, 10.0)), 50.0);
        assert!(c.zoom.is_finite() && c.zoom >= MIN_ZOOM);
        c.fit(Rect::NOTHING, vp(), 0.0);
        assert_eq!(c, Camera::default());
    }

    #[test]
    fn graph_camera_semantic_zoom() {
        assert_eq!(semantic_zoom(0.1), Detail::Dot);
        assert_eq!(semantic_zoom(0.3), Detail::Compact);
        assert_eq!(semantic_zoom(0.6), Detail::Full);
    }

    #[test]
    fn graph_camera_pan() {
        let mut c = Camera {
            center: Pos2::ZERO,
            zoom: 2.0,
        };
        c.pan_screen(vec2(20.0, 0.0));
        assert_eq!(c.center, pos2(-10.0, 0.0));
    }
}
