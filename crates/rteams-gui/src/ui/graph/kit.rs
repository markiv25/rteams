//! Canvas kit: the reusable drawing and hit-testing primitives of the graph canvas. The org map
//! reuses it, so nothing here knows about teams, tasks or agents.
//!
//! * `Camera` (see `camera.rs`) maps world to screen; `Canvas` bundles a painter, viewport and
//!   camera so callers draw in world space with `canvas.pt(..)` / `canvas.rect(..)`.
//! * `Bezier` is a cubic curve (`connector` builds a gently bent one between two points) with
//!   `at`, `tangent`, `sample`, `distance_to` (hit-testing) and `map` (world -> screen).
//! * `draw_edge` strokes a curve (optionally dashed, with an arrowhead); `draw_particle` draws
//!   a glowing dot with a short fading trail at `t` along a curve.
//! * `draw_pill` / `draw_text_fit` / `draw_badge` draw rounded labels with an optional status
//!   dot; text is ellipsized to the available width.
//! * `anchor_on_rect` finds where a line toward another point leaves a rect;
//!   `point_in_rounded_rect` is the node hit test.
//!
//! All screen sizes are clamped to non-negative values so egui never sees a negative width.

use eframe::egui::{
    self, epaint::StrokeKind, text::TextWrapping, Align2, Color32, CornerRadius, FontId, Painter,
    Pos2, Rect, Shape, Stroke, Vec2,
};

use super::camera::Camera;
use crate::theme;

/// A painter, the screen rect it draws into and the camera: draw in world coordinates.
pub struct Canvas<'a> {
    pub painter: &'a Painter,
    pub viewport: Rect,
    pub cam: Camera,
}

impl<'a> Canvas<'a> {
    pub fn new(painter: &'a Painter, viewport: Rect, cam: Camera) -> Self {
        Canvas {
            painter,
            viewport,
            cam,
        }
    }

    pub fn pt(&self, world: Pos2) -> Pos2 {
        self.cam.world_to_screen(world, self.viewport)
    }

    pub fn rect(&self, world: Rect) -> Rect {
        self.cam.rect_to_screen(world, self.viewport)
    }

    pub fn world(&self, screen: Pos2) -> Pos2 {
        self.cam.screen_to_world(screen, self.viewport)
    }

    /// World length to screen length.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn len(&self, world: f32) -> f32 {
        world * self.cam.zoom
    }

    /// Whether a screen rect is (partly) inside the viewport; used to cull drawing.
    pub fn visible(&self, screen: Rect) -> bool {
        self.viewport.expand(40.0).intersects(screen)
    }
}

// ---------------------------------------------------------------- curves

/// Cubic bezier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bezier {
    pub p0: Pos2,
    pub p1: Pos2,
    pub p2: Pos2,
    pub p3: Pos2,
}

impl Bezier {
    /// A curve from `a` to `b` bent sideways by `bend` (fraction of the length; 0 = straight).
    pub fn connector(a: Pos2, b: Pos2, bend: f32) -> Bezier {
        let d = b - a;
        let n = Vec2::new(-d.y, d.x) * bend;
        Bezier {
            p0: a,
            p1: a + d / 3.0 + n,
            p2: a + d * (2.0 / 3.0) + n,
            p3: b,
        }
    }

    pub fn at(&self, t: f32) -> Pos2 {
        let u = 1.0 - t;
        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        Pos2::new(
            a * self.p0.x + b * self.p1.x + c * self.p2.x + d * self.p3.x,
            a * self.p0.y + b * self.p1.y + c * self.p2.y + d * self.p3.y,
        )
    }

    /// Unit direction of travel at `t` (zero for a degenerate curve).
    pub fn tangent(&self, t: f32) -> Vec2 {
        let u = 1.0 - t;
        let d = 3.0 * u * u * (self.p1 - self.p0)
            + 6.0 * u * t * (self.p2 - self.p1)
            + 3.0 * t * t * (self.p3 - self.p2);
        let n = d.normalized();
        if n.x.is_finite() && n.y.is_finite() {
            n
        } else {
            Vec2::ZERO
        }
    }

    pub fn sample(&self, n: usize) -> Vec<Pos2> {
        let n = n.max(1);
        (0..=n).map(|i| self.at(i as f32 / n as f32)).collect()
    }

    /// Distance from `p` to the curve, sampled at 16 segments.
    pub fn distance_to(&self, p: Pos2) -> f32 {
        let pts = self.sample(16);
        pts.windows(2)
            .map(|w| dist_to_segment(p, w[0], w[1]))
            .fold(f32::INFINITY, f32::min)
    }

    pub fn map(&self, f: impl Fn(Pos2) -> Pos2) -> Bezier {
        Bezier {
            p0: f(self.p0),
            p1: f(self.p1),
            p2: f(self.p2),
            p3: f(self.p3),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn length(&self) -> f32 {
        self.sample(16)
            .windows(2)
            .map(|w| (w[1] - w[0]).length())
            .sum()
    }
}

pub fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let l2 = ab.length_sq();
    if l2 <= f32::EPSILON {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

/// Where the line from the center of `rect` toward `toward` leaves the rect.
pub fn anchor_on_rect(rect: Rect, toward: Pos2) -> Pos2 {
    let c = rect.center();
    let d = toward - c;
    if d.x.abs() < 1e-6 && d.y.abs() < 1e-6 {
        return c;
    }
    let hw = rect.width() / 2.0;
    let hh = rect.height() / 2.0;
    let tx = if d.x.abs() > 1e-6 {
        hw / d.x.abs()
    } else {
        f32::INFINITY
    };
    let ty = if d.y.abs() > 1e-6 {
        hh / d.y.abs()
    } else {
        f32::INFINITY
    };
    c + d * tx.min(ty)
}

/// Hit test of a rounded rect.
pub fn point_in_rounded_rect(rect: Rect, radius: f32, p: Pos2) -> bool {
    if !rect.contains(p) {
        return false;
    }
    let r = radius
        .min(rect.width() / 2.0)
        .min(rect.height() / 2.0)
        .max(0.0);
    let inner = rect.shrink(r);
    let q = Pos2::new(
        p.x.clamp(inner.left(), inner.right().max(inner.left())),
        p.y.clamp(inner.top(), inner.bottom().max(inner.top())),
    );
    (p - q).length() <= r + 1e-3
}

// ---------------------------------------------------------------- drawing

#[derive(Debug, Clone, Copy)]
pub struct EdgeStyle {
    pub color: Color32,
    pub width: f32,
    pub dashed: bool,
    pub arrow: bool,
}

/// Filled triangle with its tip at `tip` pointing along `dir`.
pub fn arrow_head(painter: &Painter, tip: Pos2, dir: Vec2, size: f32, color: Color32) {
    if dir == Vec2::ZERO || size < 1.0 {
        return;
    }
    let n = Vec2::new(-dir.y, dir.x);
    let base = tip - dir * size;
    painter.add(Shape::convex_polygon(
        vec![tip, base + n * (size * 0.5), base - n * (size * 0.5)],
        color,
        Stroke::NONE,
    ));
}

/// Stroke a curve already in screen space.
pub fn draw_edge(painter: &Painter, bez: &Bezier, style: EdgeStyle) {
    let width = style.width.max(0.5);
    let mut pts = bez.sample(24);
    let head = if style.arrow {
        let dir = bez.tangent(1.0);
        let size = (width * 4.0).clamp(6.0, 16.0);
        // Stop the line at the arrow base so the head is not covered.
        if let Some(last) = pts.last_mut() {
            *last = bez.p3 - dir * size * 0.8;
        }
        Some((dir, size))
    } else {
        None
    };
    if style.dashed {
        painter.extend(Shape::dashed_line(
            &pts,
            Stroke::new(width, style.color),
            8.0,
            6.0,
        ));
    } else {
        painter.add(Shape::line(pts, Stroke::new(width, style.color)));
    }
    if let Some((dir, size)) = head {
        arrow_head(painter, bez.p3, dir, size, style.color);
    }
}

/// A dot with a fading trail at `t` (0..1) along a screen-space curve.
pub fn draw_particle(painter: &Painter, bez: &Bezier, t: f32, color: Color32, radius: f32) {
    let radius = radius.max(1.0);
    for (i, back) in [0.0_f32, 0.03, 0.06, 0.09].iter().enumerate() {
        let tt = (t - back).clamp(0.0, 1.0);
        let a = 1.0 - i as f32 * 0.25;
        painter.circle_filled(
            bez.at(tt),
            radius * (1.0 - i as f32 * 0.15),
            theme::tint(color, 0.9 * a),
        );
    }
    painter.circle_filled(
        bez.at(t.clamp(0.0, 1.0)),
        radius * 2.2,
        theme::tint(color, 0.18),
    );
}

#[derive(Debug, Clone, Copy)]
pub struct PillStyle {
    pub fill: Color32,
    pub stroke: Color32,
    pub stroke_width: f32,
    /// Corner radius in screen points.
    pub radius: f32,
    /// Soft glow color (two translucent strokes), if any.
    pub glow: Option<Color32>,
}

/// Rounded rect with border and optional glow.
pub fn draw_pill(painter: &Painter, rect: Rect, s: PillStyle) {
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }
    let cr = CornerRadius::same(s.radius.clamp(0.0, 255.0) as u8);
    if let Some(g) = s.glow {
        for (grow, a) in [(10.0, 0.03), (6.0, 0.06), (3.0, 0.12)] {
            painter.rect_stroke(
                rect.expand(grow),
                CornerRadius::same((s.radius + grow).clamp(0.0, 255.0) as u8),
                Stroke::new(4.0, theme::tint(g, a)),
                StrokeKind::Outside,
            );
        }
    }
    painter.rect_filled(rect, cr, s.fill);
    if s.stroke_width > 0.0 {
        painter.rect_stroke(
            rect,
            cr,
            Stroke::new(s.stroke_width, s.stroke),
            StrokeKind::Inside,
        );
    }
}

/// Single-line text with its left-center at `left_center`, ellipsized to `max_width`. Returns
/// the width used.
pub fn draw_text_fit(
    painter: &Painter,
    left_center: Pos2,
    text: &str,
    size: f32,
    color: Color32,
    max_width: f32,
) -> f32 {
    if max_width < 4.0 || size < 4.0 || text.is_empty() {
        return 0.0;
    }
    let mut job = egui::text::LayoutJob::simple_singleline(
        text.to_string(),
        FontId::proportional(size),
        color,
    );
    job.wrap = TextWrapping::truncate_at_width(max_width);
    let galley = painter.layout_job(job);
    let w = galley.size().x;
    let h = galley.size().y;
    painter.galley(
        Pos2::new(left_center.x, left_center.y - h / 2.0),
        galley,
        color,
    );
    w
}

/// Small tinted capsule with `text`, right-aligned at `right_center`. Returns its rect.
pub fn draw_badge(
    painter: &Painter,
    right_center: Pos2,
    text: &str,
    size: f32,
    color: Color32,
) -> Rect {
    if size < 4.0 {
        return Rect::from_center_size(right_center, Vec2::ZERO);
    }
    let galley = painter.layout_no_wrap(text.to_string(), FontId::proportional(size), color);
    let pad = size * 0.5;
    let w = galley.size().x + pad * 2.0;
    let h = galley.size().y + 2.0;
    let rect = Rect::from_min_size(
        Pos2::new(right_center.x - w, right_center.y - h / 2.0),
        Vec2::new(w, h),
    );
    painter.rect_filled(rect, CornerRadius::same(255), theme::tint(color, 0.16));
    painter.galley(
        Pos2::new(rect.left() + pad, rect.top() + 1.0),
        galley,
        color,
    );
    rect
}

/// Status dot with a halo.
pub fn draw_dot(painter: &Painter, c: Pos2, r: f32, color: Color32) {
    if r < 0.8 {
        return;
    }
    painter.circle_filled(c, r * 1.9, theme::tint(color, 0.18));
    painter.circle_filled(c, r, color);
}

/// Centered text helper for labels that must not be ellipsized.
pub fn draw_label(
    painter: &Painter,
    pos: Pos2,
    align: Align2,
    text: &str,
    size: f32,
    color: Color32,
) {
    if size >= 4.0 {
        painter.text(pos, align, text, FontId::proportional(size), color);
    }
}

/// Opaque blend of `a` towards `b` by `t` (0..1).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Radial gradient disc: `color` (with `alpha` 0..1) at the center fading to transparent at
/// `radius` (all in screen space).
pub fn radial_glow(painter: &Painter, center: Pos2, radius: f32, color: Color32, alpha: f32) {
    use egui::epaint::{Mesh, Vertex, WHITE_UV};
    const N: usize = 40;
    if radius < 2.0 || alpha <= 0.0 {
        return;
    }
    let mut mesh = Mesh::default();
    mesh.vertices.push(Vertex {
        pos: center,
        uv: WHITE_UV,
        color: theme::tint(color, alpha),
    });
    for i in 0..N {
        let a = std::f32::consts::TAU * i as f32 / N as f32;
        mesh.vertices.push(Vertex {
            pos: center + Vec2::angled(a) * radius,
            uv: WHITE_UV,
            color: theme::tint(color, 0.0),
        });
    }
    for i in 0..N {
        mesh.add_triangle(0, 1 + i as u32, 1 + ((i + 1) % N) as u32);
    }
    painter.add(mesh);
}

/// The canvas backdrop: deep void, a soft light pool in the middle, a world-anchored dot grid
/// and a vignette at the edges (original: space-themed hex grid).
pub fn draw_background(painter: &Painter, rect: Rect, cam: &Camera, focus: Pos2) {
    painter.rect_filled(rect, 0.0, Color32::from_rgb(0x08, 0x09, 0x10));
    let reach = rect.width().max(rect.height()) * 0.75;
    radial_glow(painter, focus, reach, theme::ACCENT, 0.075);
    radial_glow(
        painter,
        rect.center(),
        reach * 0.8,
        Color32::from_rgb(0x38, 0xbd, 0xf8),
        0.02,
    );
    // Dot grid: spacing follows the zoom but stays between 18 and 36 screen px.
    let mut step = 40.0 * cam.zoom;
    while step < 18.0 {
        step *= 2.0;
    }
    while step > 36.0 {
        step /= 2.0;
    }
    let origin = cam.world_to_screen(Pos2::ZERO, rect);
    let x0 = rect.left() + (origin.x - rect.left()).rem_euclid(step);
    let y0 = rect.top() + (origin.y - rect.top()).rem_euclid(step);
    let dot = theme::tint(Color32::from_rgb(0x94, 0xa3, 0xb8), 0.13);
    let mut y = y0;
    while y < rect.bottom() {
        let mut x = x0;
        while x < rect.right() {
            painter.rect_filled(
                Rect::from_center_size(Pos2::new(x, y), Vec2::splat(1.6)),
                0.8,
                dot,
            );
            x += step;
        }
        y += step;
    }
    // Vignette: four soft edge gradients.
    use egui::epaint::{Mesh, Vertex, WHITE_UV};
    let dark = Color32::from_rgba_unmultiplied(4, 5, 10, 170);
    let clear = Color32::from_rgba_unmultiplied(4, 5, 10, 0);
    let d = (rect.width().min(rect.height()) * 0.18).max(30.0);
    let edges = [
        (rect.left_top(), rect.right_top(), Vec2::new(0.0, d)),
        (rect.left_bottom(), rect.right_bottom(), Vec2::new(0.0, -d)),
        (rect.left_top(), rect.left_bottom(), Vec2::new(d, 0.0)),
        (rect.right_top(), rect.right_bottom(), Vec2::new(-d, 0.0)),
    ];
    for (a, b, inward) in edges {
        let mut m = Mesh::default();
        for (pos, color) in [
            (a, dark),
            (b, dark),
            (b + inward, clear),
            (a + inward, clear),
        ] {
            m.vertices.push(Vertex {
                pos,
                uv: WHITE_UV,
                color,
            });
        }
        m.add_triangle(0, 1, 2);
        m.add_triangle(0, 2, 3);
        painter.add(m);
    }
}

/// Glyph of a canvas tool button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    ZoomIn,
    ZoomOut,
    Fit,
}

/// 28px icon button for canvas toolbars (zoom / fit glyphs drawn with the painter, matching
/// `theme::icon_button`).
pub fn tool_button(ui: &mut egui::Ui, tool: Tool, tooltip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() {
            ui.painter()
                .rect_filled(rect, 8.0, Color32::from_rgb(0x25, 0x27, 0x33));
        }
        let fg = if resp.hovered() {
            theme::TEXT
        } else {
            theme::TEXT_2
        };
        let c = rect.center();
        let st = Stroke::new(1.5, fg);
        let p = ui.painter();
        match tool {
            Tool::ZoomIn | Tool::ZoomOut => {
                p.circle_stroke(c + Vec2::new(-1.0, -1.0), 5.5, st);
                p.line_segment([c + Vec2::new(3.0, 3.0), c + Vec2::new(7.0, 7.0)], st);
                p.line_segment([c + Vec2::new(-3.5, -1.0), c + Vec2::new(1.5, -1.0)], st);
                if tool == Tool::ZoomIn {
                    p.line_segment([c + Vec2::new(-1.0, -3.5), c + Vec2::new(-1.0, 1.5)], st);
                }
            }
            Tool::Fit => {
                let s = 6.0;
                let k = 3.5;
                for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                    let corner = c + Vec2::new(sx * s, sy * s);
                    p.line_segment([corner, corner + Vec2::new(-sx * k, 0.0)], st);
                    p.line_segment([corner, corner + Vec2::new(0.0, -sy * k)], st);
                }
            }
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tooltip)
}

/// Floating pill-shaped bar frame for canvas toolbars.
pub fn pill_bar_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::tint(theme::SIDEBAR, 0.94))
        .stroke(Stroke::new(1.0, theme::hairline(0.1)))
        .corner_radius(255)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .shadow(egui::Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(90),
        })
}

/// Floating soft card frame for HUDs and toolbars on a canvas.
pub fn hud_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::tint(theme::SIDEBAR, 0.92))
        .stroke(Stroke::new(1.0, theme::hairline(0.1)))
        .corner_radius(14)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .shadow(egui::Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(90),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{pos2, vec2};

    #[test]
    fn graph_kit_bezier_endpoints_and_tangent() {
        let b = Bezier::connector(pos2(0.0, 0.0), pos2(100.0, 0.0), 0.2);
        assert_eq!(b.at(0.0), pos2(0.0, 0.0));
        assert_eq!(b.at(1.0), pos2(100.0, 0.0));
        let t = b.tangent(1.0);
        assert!((t.length() - 1.0).abs() < 1e-4);
        let straight = Bezier::connector(pos2(0.0, 0.0), pos2(100.0, 0.0), 0.0);
        assert!(straight.distance_to(pos2(50.0, 0.0)) < 1e-3);
        assert!((straight.distance_to(pos2(50.0, 10.0)) - 10.0).abs() < 1e-3);
        assert!((straight.length() - 100.0).abs() < 1e-2);
        let degenerate = Bezier::connector(pos2(5.0, 5.0), pos2(5.0, 5.0), 0.3);
        assert_eq!(degenerate.tangent(0.5), Vec2::ZERO);
        assert!(degenerate.distance_to(pos2(5.0, 5.0)).is_finite());
    }

    #[test]
    fn graph_kit_anchor_leaves_border() {
        let r = Rect::from_center_size(pos2(0.0, 0.0), vec2(200.0, 40.0));
        let a = anchor_on_rect(r, pos2(1000.0, 0.0));
        assert!((a - pos2(100.0, 0.0)).length() < 1e-3);
        let a = anchor_on_rect(r, pos2(0.0, -1000.0));
        assert!((a - pos2(0.0, -20.0)).length() < 1e-3);
        assert_eq!(anchor_on_rect(r, r.center()), r.center());
        let a = anchor_on_rect(r, pos2(300.0, 300.0));
        assert!(r.expand(0.01).contains(a));
    }

    #[test]
    fn graph_kit_rounded_rect_hit() {
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 40.0));
        assert!(point_in_rounded_rect(r, 20.0, pos2(50.0, 20.0)));
        assert!(
            !point_in_rounded_rect(r, 20.0, pos2(1.0, 1.0)),
            "cut corner"
        );
        assert!(!point_in_rounded_rect(r, 20.0, pos2(-1.0, 20.0)));
        assert!(point_in_rounded_rect(r, 0.0, pos2(1.0, 1.0)));
    }

    #[test]
    fn graph_kit_canvas_maps() {
        let ctx = egui::Context::default();
        let painter = Painter::new(ctx, egui::LayerId::background(), Rect::EVERYTHING);
        let vp = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0));
        let cv = Canvas::new(
            &painter,
            vp,
            Camera {
                center: pos2(10.0, 10.0),
                zoom: 2.0,
            },
        );
        assert_eq!(cv.pt(pos2(10.0, 10.0)), vp.center());
        assert!((cv.world(cv.pt(pos2(3.0, 4.0))) - pos2(3.0, 4.0)).length() < 1e-4);
        assert_eq!(cv.len(5.0), 10.0);
        assert!(cv.visible(vp));
        assert!(!cv.visible(Rect::from_min_size(pos2(5000.0, 0.0), vec2(10.0, 10.0))));
    }
}
