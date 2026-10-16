//! Vector icons drawn with the painter (lucide-style: 24-unit grid, ~1.5px strokes, round caps
//! and joins). No icon font and no emoji, so chrome looks the same on every OS.
//!
//! `egui-phosphor` was tried first but its newest release depends on egui 0.36, which would
//! pull a second egui into the build, so the icons are plain paths instead.

#![allow(dead_code)] // shared icon set: not every icon has a caller yet

use eframe::egui::{
    self, epaint::PathShape, Color32, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Vec2,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Users,
    UserPlus,
    LayoutDashboard,
    List,
    Columns,
    Kanban,
    ChartBar,
    Code,
    GitBranch,
    Network,
    Puzzle,
    Clock,
    Sitemap,
    History,
    Settings,
    Cpu,
    Plus,
    Bell,
    MoreHorizontal,
    Terminal,
    Search,
    Filter,
    X,
    Check,
    ChevronDown,
    ChevronRight,
    Play,
    Square,
    MessageSquare,
    Folder,
    File,
    Trash,
    Edit,
    Eye,
    Shield,
    Paperclip,
    Link,
    Send,
    Refresh,
    Globe,
    PanelLeft,
}

/// Drawing context: maps the 24x24 grid into `rect`.
struct Pen<'a> {
    p: &'a Painter,
    c: Pos2,
    s: f32,
    stroke: Stroke,
}

impl Pen<'_> {
    fn pt(&self, x: f32, y: f32) -> Pos2 {
        self.c + (Vec2::new(x, y) - Vec2::splat(12.0)) * self.s
    }

    fn cap(&self, at: Pos2) {
        self.p
            .circle_filled(at, self.stroke.width / 2.0, self.stroke.color);
    }

    /// Open polyline with round caps.
    fn line(&self, pts: &[(f32, f32)]) {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| self.pt(x, y)).collect();
        if v.len() < 2 {
            return;
        }
        self.cap(v[0]);
        self.cap(v[v.len() - 1]);
        self.p.add(Shape::line(v, self.stroke));
    }

    fn closed(&self, pts: &[(f32, f32)]) {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| self.pt(x, y)).collect();
        self.p
            .add(Shape::Path(PathShape::closed_line(v, self.stroke)));
    }

    fn circle(&self, x: f32, y: f32, r: f32) {
        self.p.circle_stroke(self.pt(x, y), r * self.s, self.stroke);
    }

    fn dot(&self, x: f32, y: f32, r: f32) {
        self.p
            .circle_filled(self.pt(x, y), r * self.s, self.stroke.color);
    }

    fn rrect(&self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        let rect = Rect::from_min_max(self.pt(x, y), self.pt(x + w, y + h));
        self.p.rect_stroke(
            rect,
            egui::CornerRadius::same((r * self.s).round().clamp(0.0, 255.0) as u8),
            self.stroke,
            egui::StrokeKind::Middle,
        );
    }
}

/// Points on an arc (degrees, y-down: 0 = right, 90 = down), inclusive.
fn arc(cx: f32, cy: f32, r: f32, a0: f32, a1: f32) -> Vec<(f32, f32)> {
    let n = ((a1 - a0).abs() / 12.0).ceil().max(2.0) as usize;
    (0..=n)
        .map(|i| {
            let a = (a0 + (a1 - a0) * i as f32 / n as f32).to_radians();
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

fn cat(parts: &[Vec<(f32, f32)>]) -> Vec<(f32, f32)> {
    parts.iter().flatten().copied().collect()
}

/// Draw `icon` centred in `rect` (as a square) with `color`.
pub fn draw(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let size = rect.width().min(rect.height());
    let s = size / 24.0;
    let w = (s * 2.0).clamp(1.25, 2.0);
    let pen = Pen {
        p: painter,
        c: rect.center(),
        s,
        stroke: Stroke::new(w, color),
    };
    match icon {
        Icon::Users => {
            pen.circle(9.0, 7.0, 4.0);
            pen.line(&cat(&[
                vec![(2.0, 21.0), (2.0, 19.0)],
                arc(6.0, 19.0, 4.0, 180.0, 90.0),
                arc(12.0, 19.0, 4.0, 90.0, 0.0),
                vec![(16.0, 21.0)],
            ]));
            pen.line(&arc(16.0, 7.0, 4.0, -90.0, 90.0));
            pen.line(&cat(&[
                vec![(22.0, 21.0), (22.0, 19.0)],
                arc(18.0, 19.0, 4.0, 0.0, -60.0),
            ]));
        }
        Icon::UserPlus => {
            pen.circle(9.0, 7.0, 4.0);
            pen.line(&cat(&[
                vec![(2.0, 21.0), (2.0, 19.0)],
                arc(6.0, 19.0, 4.0, 180.0, 90.0),
                arc(12.0, 19.0, 4.0, 90.0, 0.0),
                vec![(16.0, 21.0)],
            ]));
            pen.line(&[(19.0, 8.0), (19.0, 14.0)]);
            pen.line(&[(16.0, 11.0), (22.0, 11.0)]);
        }
        Icon::LayoutDashboard => {
            pen.rrect(3.0, 3.0, 7.0, 9.0, 1.0);
            pen.rrect(14.0, 3.0, 7.0, 5.0, 1.0);
            pen.rrect(14.0, 12.0, 7.0, 9.0, 1.0);
            pen.rrect(3.0, 16.0, 7.0, 5.0, 1.0);
        }
        Icon::List => {
            for y in [6.0, 12.0, 18.0] {
                pen.line(&[(8.0, y), (21.0, y)]);
                pen.dot(3.5, y, 0.9);
            }
        }
        Icon::Columns => {
            pen.rrect(3.0, 3.0, 18.0, 18.0, 2.0);
            pen.line(&[(12.0, 3.0), (12.0, 21.0)]);
        }
        Icon::Kanban => {
            pen.line(&[(6.0, 5.0), (6.0, 16.0)]);
            pen.line(&[(12.0, 5.0), (12.0, 11.0)]);
            pen.line(&[(18.0, 5.0), (18.0, 19.0)]);
        }
        Icon::ChartBar => {
            pen.line(&[(12.0, 20.0), (12.0, 10.0)]);
            pen.line(&[(18.0, 20.0), (18.0, 4.0)]);
            pen.line(&[(6.0, 20.0), (6.0, 16.0)]);
        }
        Icon::Code => {
            pen.line(&[(16.0, 18.0), (22.0, 12.0), (16.0, 6.0)]);
            pen.line(&[(8.0, 6.0), (2.0, 12.0), (8.0, 18.0)]);
        }
        Icon::GitBranch => {
            pen.line(&[(6.0, 3.0), (6.0, 15.0)]);
            pen.circle(18.0, 6.0, 3.0);
            pen.circle(6.0, 18.0, 3.0);
            pen.line(&cat(&[arc(12.0, 9.0, 6.0, 0.0, 90.0), vec![(6.0, 15.0)]]));
        }
        Icon::Network => {
            pen.circle(12.0, 5.0, 2.5);
            pen.circle(5.0, 19.0, 2.5);
            pen.circle(19.0, 19.0, 2.5);
            pen.line(&[(12.0, 7.5), (12.0, 13.0)]);
            pen.line(&[(6.4, 16.9), (12.0, 13.0), (17.6, 16.9)]);
        }
        Icon::Puzzle => {
            pen.closed(&cat(&[
                vec![(3.0, 9.0), (7.5, 9.0)],
                arc(10.0, 9.0, 2.5, 180.0, 360.0),
                vec![(12.5, 9.0), (17.0, 9.0), (17.0, 12.0)],
                arc(17.0, 14.5, 2.5, -90.0, 90.0),
                vec![(17.0, 17.0), (17.0, 21.0), (3.0, 21.0)],
            ]));
        }
        Icon::Clock => {
            pen.circle(12.0, 12.0, 9.0);
            pen.line(&[(12.0, 7.0), (12.0, 12.0), (15.5, 14.0)]);
        }
        Icon::Sitemap => {
            pen.rrect(9.0, 3.0, 6.0, 5.0, 1.0);
            pen.rrect(2.0, 16.0, 6.0, 5.0, 1.0);
            pen.rrect(16.0, 16.0, 6.0, 5.0, 1.0);
            pen.line(&[(12.0, 8.0), (12.0, 12.0)]);
            pen.line(&[(5.0, 16.0), (5.0, 12.0), (19.0, 12.0), (19.0, 16.0)]);
        }
        Icon::History => {
            pen.line(&cat(&[
                arc(12.0, 12.0, 9.0, 180.0, -90.0),
                arc(12.0, 12.0, 9.0, -90.0, -135.0),
                vec![(3.0, 8.0)],
            ]));
            pen.line(&[(3.0, 3.0), (3.0, 8.0), (8.0, 8.0)]);
            pen.line(&[(12.0, 7.0), (12.0, 12.0), (16.0, 14.0)]);
        }
        Icon::Settings => {
            let mut pts = Vec::new();
            for i in 0..8 {
                let base = i as f32 * 45.0;
                for (a, r) in [(-17.0, 7.2), (-10.0, 9.6), (10.0, 9.6), (17.0, 7.2)] {
                    let t = (base + a).to_radians();
                    pts.push((12.0 + r * t.cos(), 12.0 + r * t.sin()));
                }
            }
            pen.closed(&pts);
            pen.circle(12.0, 12.0, 3.0);
        }
        Icon::Cpu => {
            pen.rrect(5.0, 5.0, 14.0, 14.0, 2.0);
            pen.rrect(9.0, 9.0, 6.0, 6.0, 1.0);
            for v in [9.0, 15.0] {
                pen.line(&[(v, 2.0), (v, 5.0)]);
                pen.line(&[(v, 19.0), (v, 22.0)]);
                pen.line(&[(2.0, v), (5.0, v)]);
                pen.line(&[(19.0, v), (22.0, v)]);
            }
        }
        Icon::Plus => {
            pen.line(&[(5.0, 12.0), (19.0, 12.0)]);
            pen.line(&[(12.0, 5.0), (12.0, 19.0)]);
        }
        Icon::Bell => {
            pen.closed(&cat(&[
                arc(12.0, 8.0, 6.0, 180.0, 360.0),
                vec![
                    (18.0, 12.0),
                    (19.5, 15.0),
                    (21.0, 17.0),
                    (3.0, 17.0),
                    (4.5, 15.0),
                    (6.0, 12.0),
                ],
            ]));
            pen.line(&arc(12.0, 19.8, 1.9, 10.0, 170.0));
        }
        Icon::MoreHorizontal => {
            for x in [5.0, 12.0, 19.0] {
                pen.dot(x, 12.0, 1.5);
            }
        }
        Icon::Terminal => {
            pen.line(&[(4.0, 17.0), (10.0, 11.0), (4.0, 5.0)]);
            pen.line(&[(12.0, 19.0), (20.0, 19.0)]);
        }
        Icon::Search => {
            pen.circle(11.0, 11.0, 8.0);
            pen.line(&[(21.0, 21.0), (16.7, 16.7)]);
        }
        Icon::Filter => {
            pen.closed(&[
                (22.0, 3.0),
                (2.0, 3.0),
                (10.0, 12.5),
                (10.0, 19.0),
                (14.0, 21.0),
                (14.0, 12.5),
            ]);
        }
        Icon::X => {
            pen.line(&[(18.0, 6.0), (6.0, 18.0)]);
            pen.line(&[(6.0, 6.0), (18.0, 18.0)]);
        }
        Icon::Check => pen.line(&[(20.0, 6.0), (9.0, 17.0), (4.0, 12.0)]),
        Icon::ChevronDown => pen.line(&[(6.0, 9.0), (12.0, 15.0), (18.0, 9.0)]),
        Icon::ChevronRight => pen.line(&[(9.0, 6.0), (15.0, 12.0), (9.0, 18.0)]),
        Icon::Play => pen.closed(&[(6.0, 3.0), (20.0, 12.0), (6.0, 21.0)]),
        Icon::Square => pen.rrect(5.0, 5.0, 14.0, 14.0, 2.0),
        Icon::MessageSquare => pen.closed(&[
            (5.0, 3.0),
            (19.0, 3.0),
            (21.0, 5.0),
            (21.0, 15.0),
            (19.0, 17.0),
            (7.0, 17.0),
            (3.0, 21.0),
            (3.0, 5.0),
        ]),
        Icon::Folder => pen.closed(&[
            (4.0, 20.0),
            (20.0, 20.0),
            (22.0, 18.0),
            (22.0, 8.0),
            (20.0, 6.0),
            (12.0, 6.0),
            (10.5, 4.5),
            (8.0, 3.0),
            (4.0, 3.0),
            (2.0, 5.0),
            (2.0, 18.0),
        ]),
        Icon::File => {
            pen.closed(&[
                (14.5, 2.0),
                (6.0, 2.0),
                (4.0, 4.0),
                (4.0, 20.0),
                (6.0, 22.0),
                (18.0, 22.0),
                (20.0, 20.0),
                (20.0, 7.5),
            ]);
            pen.line(&[(14.0, 2.0), (14.0, 8.0), (20.0, 8.0)]);
        }
        Icon::Trash => {
            pen.line(&[(3.0, 6.0), (21.0, 6.0)]);
            pen.line(&[
                (19.0, 6.0),
                (19.0, 20.0),
                (17.0, 22.0),
                (7.0, 22.0),
                (5.0, 20.0),
                (5.0, 6.0),
            ]);
            pen.line(&[
                (8.0, 6.0),
                (8.0, 4.0),
                (10.0, 2.0),
                (14.0, 2.0),
                (16.0, 4.0),
                (16.0, 6.0),
            ]);
            pen.line(&[(10.0, 11.0), (10.0, 17.0)]);
            pen.line(&[(14.0, 11.0), (14.0, 17.0)]);
        }
        Icon::Edit => {
            pen.closed(&[
                (18.0, 3.0),
                (21.0, 6.0),
                (7.0, 20.0),
                (3.0, 21.0),
                (4.0, 17.0),
            ]);
            pen.line(&[(15.0, 6.0), (18.0, 9.0)]);
        }
        Icon::Eye => {
            let top: Vec<(f32, f32)> = (0..=16)
                .map(|i| {
                    let t = i as f32 / 16.0;
                    (
                        2.0 + 20.0 * t,
                        12.0 - 7.5 * (std::f32::consts::PI * t).sin(),
                    )
                })
                .collect();
            let bottom: Vec<(f32, f32)> = top.iter().rev().map(|&(x, y)| (x, 24.0 - y)).collect();
            pen.closed(&cat(&[top, bottom]));
            pen.circle(12.0, 12.0, 3.0);
        }
        Icon::Shield => pen.closed(&[
            (12.0, 22.0),
            (6.0, 18.5),
            (4.0, 13.0),
            (4.0, 6.0),
            (5.0, 5.0),
            (8.0, 4.5),
            (12.0, 2.5),
            (16.0, 4.5),
            (19.0, 5.0),
            (20.0, 6.0),
            (20.0, 13.0),
            (18.0, 18.5),
        ]),
        Icon::Paperclip => pen.line(&[
            (21.4, 11.0),
            (12.2, 20.2),
            (10.0, 21.6),
            (7.0, 21.8),
            (4.5, 20.4),
            (3.0, 18.0),
            (3.0, 15.0),
            (4.4, 12.4),
            (12.8, 3.8),
            (14.6, 2.8),
            (17.0, 3.0),
            (19.0, 4.4),
            (20.0, 6.6),
            (19.6, 9.0),
            (9.4, 17.4),
            (8.0, 17.8),
            (6.8, 17.2),
            (6.4, 15.8),
            (6.9, 14.6),
            (15.2, 6.4),
        ]),
        Icon::Link => {
            for (cx, cy) in [(8.5, 15.5), (15.5, 8.5)] {
                // Capsule along the 45 degree axis.
                let a = -45.0_f32.to_radians();
                let (dx, dy) = (a.cos() * 2.5, a.sin() * 2.5);
                let mut pts = arc(cx + dx, cy + dy, 3.5, -135.0, 45.0);
                pts.extend(arc(cx - dx, cy - dy, 3.5, 45.0, 225.0));
                pen.closed(&pts);
            }
            pen.line(&[(9.5, 14.5), (14.5, 9.5)]);
        }
        Icon::Send => pen.closed(&[(22.0, 2.0), (15.0, 22.0), (11.0, 13.0), (2.0, 9.0)]),
        Icon::Refresh => {
            pen.line(&cat(&[
                arc(12.0, 12.0, 9.0, 180.0, 315.0),
                vec![(21.0, 8.0)],
            ]));
            pen.line(&[(21.0, 3.0), (21.0, 8.0), (16.0, 8.0)]);
            pen.line(&cat(&[arc(12.0, 12.0, 9.0, 0.0, 135.0), vec![(3.0, 16.0)]]));
            pen.line(&[(8.0, 16.0), (3.0, 16.0), (3.0, 21.0)]);
        }
        Icon::Globe => {
            pen.circle(12.0, 12.0, 9.5);
            pen.line(&[(2.5, 12.0), (21.5, 12.0)]);
            let m: Vec<(f32, f32)> = (0..=24)
                .map(|i| {
                    let t = (i as f32 / 24.0 * 360.0).to_radians();
                    (12.0 + 4.5 * t.cos(), 12.0 + 9.5 * t.sin())
                })
                .collect();
            pen.closed(&m);
        }
        Icon::PanelLeft => {
            pen.rrect(3.0, 3.0, 18.0, 18.0, 2.0);
            pen.line(&[(9.0, 3.0), (9.0, 21.0)]);
        }
    }
}

/// Square icon of `size` points as a (non-interactive) widget.
pub fn icon(ui: &mut egui::Ui, icon: Icon, size: f32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if ui.is_rect_visible(rect) {
        draw(ui.painter(), rect, icon, color);
    }
    resp
}

/// Icon (16px) followed by `text`, clickable as one unit. Uses the label color; hover brightens.
pub fn icon_label(ui: &mut egui::Ui, icon: Icon, text: &str) -> Response {
    const SIZE: f32 = 16.0;
    const GAP: f32 = 6.0;
    let font = egui::TextStyle::Body.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, Color32::PLACEHOLDER);
    let size = Vec2::new(SIZE + GAP + galley.size().x, galley.size().y.max(SIZE));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let color = if resp.hovered() {
            crate::theme::TEXT
        } else {
            crate::theme::TEXT_2
        };
        let icon_rect = Rect::from_center_size(
            Pos2::new(rect.left() + SIZE / 2.0, rect.center().y),
            Vec2::splat(SIZE),
        );
        draw(ui.painter(), icon_rect, icon, color);
        let pos = Pos2::new(
            rect.left() + SIZE + GAP,
            rect.center().y - galley.size().y / 2.0,
        );
        ui.painter().galley(pos, galley, color);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_draws_headless() {
        let all = [
            Icon::Users,
            Icon::UserPlus,
            Icon::LayoutDashboard,
            Icon::List,
            Icon::Columns,
            Icon::Kanban,
            Icon::ChartBar,
            Icon::Code,
            Icon::GitBranch,
            Icon::Network,
            Icon::Puzzle,
            Icon::Clock,
            Icon::Sitemap,
            Icon::History,
            Icon::Settings,
            Icon::Cpu,
            Icon::Plus,
            Icon::Bell,
            Icon::MoreHorizontal,
            Icon::Terminal,
            Icon::Search,
            Icon::Filter,
            Icon::X,
            Icon::Check,
            Icon::ChevronDown,
            Icon::ChevronRight,
            Icon::Play,
            Icon::Square,
            Icon::MessageSquare,
            Icon::Folder,
            Icon::File,
            Icon::Trash,
            Icon::Edit,
            Icon::Eye,
            Icon::Shield,
            Icon::Paperclip,
            Icon::Link,
            Icon::Send,
            Icon::Refresh,
            Icon::Globe,
            Icon::PanelLeft,
        ];
        crate::view::testing::render(|ui| {
            for i in all {
                icon(ui, i, 16.0, Color32::WHITE);
                assert!(!icon_label(ui, i, "x").clicked());
            }
        });
    }
}
