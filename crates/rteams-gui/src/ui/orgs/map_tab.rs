//! Organization Map tab: nested / hierarchy canvas with toolbar, HUDs, search focus and
//! drag-to-place teams.

use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, vec2, Pos2, Rect, RichText, Sense};
use rteams_services::orgs::model::OrgMap;
use rteams_services::orgs::projection::OrgView;
use rteams_services::orgs::structure::Edit;

use super::canvas_nested::{self, DrawOpts};
use super::focus::{self, Focus};
use super::layout::{self, Key, Layout, Mode, RelEdge};
use super::toolbar::{self, Action, Toolbar};
use super::{canvas_hierarchy, hud_legend, OrgsPanel};
use crate::icons::{self, Icon};
use crate::theme;
use crate::ui::graph::camera::Camera;
use crate::ui::graph::kit::{self, Canvas};
use crate::ui::graph::minimap;
use crate::ui::{FeatureCtx, Screen, ShellRequest, TabId};

const REFRESH: Duration = Duration::from_secs(5);

pub struct OrgMapScreen {
    map: OrgMap,
    view: OrgView,
    layout: Layout,
    rels: Vec<RelEdge>,
    loaded: Option<Instant>,
    pub toolbar: Toolbar,
    pub focus: Focus,
    cam: Camera,
    fit_pending: bool,
    /// Node to fly the camera to (resolved once the viewport size is known).
    fly_to: Option<usize>,
    fly: Option<(Pos2, f32)>,
    /// Team card being dragged.
    drag: Option<String>,
    error: Option<String>,
    /// The Organizations admin panel is open next to (or instead of) the canvas.
    edit_open: bool,
    panel: OrgsPanel,
    last_size: egui::Vec2,
}

impl Default for OrgMapScreen {
    fn default() -> Self {
        Self::new()
    }
}

/// The `Edit` that dropping `team` on `target` means, or `None` when it would not change anything.
pub fn drop_edit(map: &OrgMap, team: &str, target: &Key) -> Option<Edit> {
    let cur = map.placement(team);
    match target {
        Key::Org(o) if cur != Some((o.as_str(), None)) => Some(Edit::PlaceTeam {
            team: team.into(),
            org: o.clone(),
            group: None,
        }),
        Key::Group(o, g) if cur != Some((o.as_str(), Some(g.as_str()))) => Some(Edit::PlaceTeam {
            team: team.into(),
            org: o.clone(),
            group: Some(g.clone()),
        }),
        Key::Unplaced if cur.is_some() => Some(Edit::RemoveTeam { team: team.into() }),
        _ => None,
    }
}

impl OrgMapScreen {
    pub fn new() -> Self {
        OrgMapScreen {
            map: OrgMap::default(),
            view: OrgView::default(),
            layout: Layout::default(),
            rels: Vec::new(),
            loaded: None,
            toolbar: Toolbar::default(),
            focus: Focus::default(),
            cam: Camera::default(),
            fit_pending: true,
            fly_to: None,
            fly: None,
            drag: None,
            error: None,
            edit_open: false,
            panel: OrgsPanel::new(),
            last_size: egui::Vec2::ZERO,
        }
    }

    fn reload(&mut self, home: &Path) {
        self.map = OrgMap::load(home);
        self.view = OrgView::load(home, &self.map);
        self.loaded = Some(Instant::now());
        self.relayout();
    }

    fn relayout(&mut self) {
        self.layout = layout::build(&self.view, self.toolbar.mode);
        self.rels = self.layout.relation_edges(&self.view);
    }

    /// Apply an edit, save atomically, reload.
    pub fn apply(&mut self, home: &Path, e: Edit) {
        match self.map.apply(e) {
            Ok(_) => {
                self.error = self
                    .map
                    .save(home)
                    .err()
                    .map(|e| format!("Could not save organizations: {e}"));
            }
            Err(err) => self.error = Some(err.to_string()),
        }
        self.reload(home);
    }

    /// Focus a node: search box text, highlight, and camera flight.
    pub fn focus_on(&mut self, idx: usize) {
        if let Some(n) = self.layout.nodes.get(idx) {
            self.focus.query = n.label.clone();
            self.focus.target = Some(n.key.clone());
            self.fly_to = Some(idx);
        }
    }

    /// Fit the whole map into the canvas, leaving room for the floating controls on top.
    fn fit(&mut self, rect: Rect) {
        let inner = Rect::from_min_max(rect.min + vec2(0.0, 64.0), rect.max - vec2(0.0, 20.0));
        self.cam.fit(self.layout.bounds, inner, 40.0);
        self.cam.center -= (inner.center() - rect.center()) / self.cam.zoom;
        // A tree that is shorter than the canvas hangs from the top like the original.
        let b = self.layout.bounds;
        if self.toolbar.mode == Mode::Hierarchy
            && b.height() * self.cam.zoom < inner.height() - 120.0
        {
            let want_top = rect.top() + 120.0;
            self.cam.center.y = b.top() + (rect.center().y - want_top) / self.cam.zoom;
        }
    }

    fn start_fly(&mut self, idx: usize, viewport: Rect) {
        let r = self.layout.nodes[idx].rect;
        let z = ((viewport.width() / (r.width() + 120.0))
            .min(viewport.height() / (r.height() + 120.0)))
        .clamp(0.05, 1.3);
        self.fly = Some((r.center(), z));
    }

    fn step_fly(&mut self, ctx: &egui::Context) {
        if let Some((c, z)) = self.fly {
            self.cam.center += (c - self.cam.center) * 0.25;
            self.cam.zoom += (z - self.cam.zoom) * 0.25;
            if (c - self.cam.center).length() < 0.5 && (z - self.cam.zoom).abs() < 0.002 {
                self.cam.center = c;
                self.cam.zoom = z;
                self.fly = None;
            }
            ctx.request_repaint();
        }
    }

    fn act(&mut self, a: Action, viewport: Rect) {
        match a {
            Action::Fit => {
                self.fly = None;
                self.fit(viewport);
            }
            Action::ZoomIn => self.cam.zoom_step(viewport, true),
            Action::ZoomOut => self.cam.zoom_step(viewport, false),
            Action::Reset | Action::ClearFocus => {
                self.focus.clear();
                if a == Action::Reset {
                    self.fit_pending = true;
                }
            }
            Action::SetMode(m) => {
                self.toolbar.mode = m;
                self.relayout();
                self.fit_pending = true;
            }
        }
    }
}

impl OrgMapScreen {
    /// Header band: icon tile, title, stats line; Refresh and Edit on the right.
    fn header(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        let o = self.layout.overview();
        let stats: [(Icon, String); 5] = [
            (
                Icon::Sitemap,
                format!("{} {}", o.orgs, if o.orgs == 1 { "org" } else { "orgs" }),
            ),
            (
                Icon::Users,
                c.tr.t_count("team.organizations.graph.canvas.teams", o.teams as u64),
            ),
            (
                Icon::Cpu,
                c.tr.t_count("team.organizations.graph.canvas.agents", o.agents as u64),
            ),
            (
                Icon::Link,
                format!(
                    "{} {}",
                    self.view.relations.len(),
                    if self.view.relations.len() == 1 {
                        "relation"
                    } else {
                        "relations"
                    }
                ),
            ),
            (
                Icon::Network,
                format!("{} runtime links", self.view.runtime.len()),
            ),
        ];
        let frame = egui::Frame::new()
            .fill(theme::SIDEBAR)
            .inner_margin(egui::Margin::symmetric(20, 10));
        let resp = frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (tile, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
                ui.painter()
                    .rect_filled(tile, 10.0, theme::tint(theme::ACCENT, 0.1));
                ui.painter().rect_stroke(
                    tile,
                    10.0,
                    egui::Stroke::new(1.0, theme::hairline(0.1)),
                    egui::StrokeKind::Inside,
                );
                icons::draw(
                    ui.painter(),
                    Rect::from_center_size(tile.center(), vec2(20.0, 20.0)),
                    Icon::Sitemap,
                    theme::TEXT_2,
                );
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.label(
                        RichText::new(c.tr.t("team.organizations.graph.canvas.allOrganizations"))
                            .size(17.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 14.0;
                        for (icon, text) in &stats {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 5.0;
                                icons::icon(ui, *icon, 13.0, theme::MUTED);
                                ui.label(RichText::new(text).size(12.0).color(theme::TEXT_2));
                            });
                        }
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::pill_icon_button(ui, Some(Icon::Edit), "Edit", self.edit_open)
                        .clicked()
                    {
                        self.edit_open = !self.edit_open;
                        self.fit_pending = true;
                    }
                    if theme::pill_icon_button(ui, Some(Icon::Refresh), "Refresh", false).clicked()
                    {
                        self.reload(c.home);
                    }
                });
            });
        });
        ui.painter().line_segment(
            [
                resp.response.rect.left_bottom(),
                resp.response.rect.right_bottom(),
            ],
            egui::Stroke::new(1.0, theme::hairline(0.08)),
        );
    }
}

impl Screen for OrgMapScreen {
    fn id(&self) -> TabId {
        TabId::OrgMap
    }

    fn title_key(&self) -> &'static str {
        "team.organizations.graph.canvas.organizationMap"
    }

    fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        let refresh = if self.edit_open {
            Duration::from_millis(1500)
        } else {
            REFRESH
        };
        if self.loaded.is_none_or(|t| t.elapsed() > refresh) {
            self.reload(c.home);
        }
        ui.ctx().request_repaint_after(refresh);
        self.header(ui, c);
        if let Some(e) = &self.error {
            ui.label(egui::RichText::new(e).color(theme::ERROR));
        }
        let avail = ui.available_rect_before_wrap();
        let panel_w = if !self.edit_open {
            0.0
        } else if avail.width() > 1100.0 {
            (avail.width() * 0.55).clamp(640.0, 900.0)
        } else {
            avail.width()
        };
        if panel_w > 0.0 {
            let panel_rect = Rect::from_min_max(
                egui::pos2(avail.right() - panel_w, avail.top()),
                avail.right_bottom(),
            );
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(panel_rect.shrink2(vec2(10.0, 6.0)))
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            self.panel.show(&mut child, c);
            ui.painter().line_segment(
                [panel_rect.left_top(), panel_rect.left_bottom()],
                egui::Stroke::new(1.0, theme::hairline(0.1)),
            );
            if panel_w >= avail.width() {
                return;
            }
        }
        let rect = Rect::from_min_size(
            avail.min,
            vec2(
                (avail.width() - panel_w).max(40.0),
                avail.height().max(40.0),
            ),
        );
        let (acts, anchor) =
            toolbar::show(ui.ctx(), rect, c.tr, &mut self.toolbar, &mut self.focus);
        for a in acts {
            self.act(a, rect);
        }
        // Search results dropdown.
        if !self.focus.query.trim().is_empty() && self.focus.target.is_none() {
            let m = focus::search(&self.layout, &self.focus.query);
            if let Some(i) = focus::show_results(ui.ctx(), c.tr, &self.layout, &m, anchor) {
                self.focus_on(i);
            }
        }
        if let Some(i) = self.fly_to.take() {
            if i < self.layout.nodes.len() {
                self.start_fly(i, rect);
            }
        }
        // Fit on open, after a view switch and whenever the canvas is resized.
        if (rect.size() - self.last_size).abs().max_elem() > 2.0 {
            self.last_size = rect.size();
            self.fit_pending = true;
        }
        if self.fit_pending && !self.layout.nodes.is_empty() {
            self.fit(rect);
            self.fit_pending = false;
            self.fly = None;
        }
        self.step_fly(ui.ctx());

        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        kit::draw_background(
            &painter,
            rect,
            &self.cam,
            self.cam.world_to_screen(self.layout.bounds.center(), rect),
        );
        if self.layout.nodes.is_empty() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                c.tr.t("team.organizations.editor.noOrganizationUnits"),
                egui::FontId::proportional(14.0),
                theme::MUTED,
            );
            return;
        }

        // Pan / zoom / drag-and-drop.
        if resp.hovered() {
            let (scroll, pinch, ptr) = ui.input(|i| {
                (
                    i.smooth_scroll_delta.y,
                    i.zoom_delta(),
                    i.pointer.hover_pos(),
                )
            });
            let f = pinch * (1.0 + scroll * 0.0015).clamp(0.5, 2.0);
            if (f - 1.0).abs() > 1e-4 {
                self.fly = None;
                self.cam.zoom_at(ptr.unwrap_or(rect.center()), rect, f);
            }
        }
        let ptr_world = resp
            .interact_pointer_pos()
            .or_else(|| resp.hover_pos())
            .map(|p| self.cam.screen_to_world(p, rect));
        if resp.drag_started() {
            self.drag = ptr_world.and_then(|w| self.layout.hit(w)).and_then(|i| {
                match &self.layout.nodes[i].key {
                    Key::Team(t) => Some(t.clone()),
                    _ => None,
                }
            });
        }
        let mut drop_hot = None;
        if resp.dragged() {
            match &self.drag {
                None => {
                    self.fly = None;
                    self.cam.pan_screen(resp.drag_delta());
                }
                Some(_) => {
                    drop_hot = ptr_world.and_then(|w| self.layout.drop_target(w));
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                }
            }
        }
        if resp.drag_stopped() {
            if let (Some(team), Some(w)) = (self.drag.take(), ptr_world) {
                let edit = self
                    .layout
                    .drop_target(w)
                    .and_then(|i| drop_edit(&self.map, &team, &self.layout.nodes[i].key));
                if let Some(e) = edit {
                    self.apply(c.home, e);
                }
            }
        }
        let hover = if self.drag.is_none() {
            resp.hover_pos()
                .and_then(|p| self.layout.hit(self.cam.screen_to_world(p, rect)))
        } else {
            None
        };
        if let (Some(i), None) = (hover, &self.drag) {
            if matches!(self.layout.nodes[i].key, Key::Team(_)) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
        }
        if resp.double_clicked() {
            if let Some(i) = hover {
                if !matches!(self.layout.nodes[i].key, Key::Root | Key::Team(_)) {
                    self.focus_on(i);
                }
            }
        } else if resp.clicked() {
            if let Some(i) = hover {
                if let Key::Team(t) = &self.layout.nodes[i].key {
                    c.req.push(ShellRequest::OpenTeam(t.clone()));
                }
            }
        }

        // Paint.
        let dim = self.focus.highlight(&self.layout, &self.rels);
        let opts = DrawOpts {
            tr: c.tr,
            dim: dim.as_deref(),
            hover,
            drop_hot,
            dragging: self.drag.as_deref(),
        };
        let cv = Canvas::new(&painter, rect, self.cam);
        match self.toolbar.mode {
            Mode::Nested => canvas_nested::draw(&cv, &self.layout, &opts),
            Mode::Hierarchy => canvas_hierarchy::draw(&cv, &self.layout, &opts),
        }
        if self.toolbar.relations {
            canvas_nested::draw_relations(&cv, &self.layout, &self.rels, &opts);
        }
        if let (Some(team), Some(p)) =
            (&self.drag, resp.hover_pos().or(resp.interact_pointer_pos()))
        {
            let ghost = Rect::from_center_size(p, vec2(150.0, 28.0));
            painter.rect_filled(ghost, 8.0, theme::tint(theme::ACCENT, 0.35));
            painter.text(
                ghost.center(),
                egui::Align2::CENTER_CENTER,
                team,
                egui::FontId::proportional(12.0),
                theme::TEXT,
            );
        }
        if self.toolbar.legend {
            hud_legend::draw(&painter, rect, c.tr, self.toolbar.relations);
        }
        let dots: Vec<(Rect, egui::Color32)> = self
            .layout
            .nodes
            .iter()
            .filter(|n| !matches!(n.key, Key::Group(..) | Key::Root))
            .map(|n| (n.rect, n.color))
            .collect();
        // The zoom bar docks under the minimap.
        let mut area = minimap::area_for(rect);
        area = area.translate(vec2(2.0, -44.0));
        if minimap::show_in(ui, area, rect, self.layout.bounds, &dots, &mut self.cam) {
            self.fly = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::{fixture, render};
    use crate::view::Intents;
    use rteams_services::i18n::I18n;

    fn setup(tag: &str) -> std::path::PathBuf {
        let (home, _d) = fixture(tag);
        let mut m = OrgMap::default();
        let a = m.create_org("Acme", None).unwrap();
        let b = m.create_org("Beta", Some(&a)).unwrap();
        let g = m.create_group(&a, "Squad").unwrap();
        m.place_team("t", &a, Some(&g)).unwrap();
        m.place_team("ghost", &b, None).unwrap();
        m.save(&home).unwrap();
        home
    }

    #[test]
    fn orgs_map_renders_both_views_at_three_sizes() {
        let home = setup("orgmap-render");
        let tr = I18n::default();
        let mut out = Intents::default();
        let mut req = Vec::new();
        let mut s = OrgMapScreen::new();
        assert_eq!(s.id(), TabId::OrgMap);
        for mode in [Mode::Nested, Mode::Hierarchy] {
            s.toolbar.mode = mode;
            s.relayout();
            s.fit_pending = true;
            for focus in ["", "acme", "zzz"] {
                s.focus.query = focus.into();
                render(|ui| {
                    let mut c = FeatureCtx {
                        home: &home,
                        team: None,
                        view: None,
                        out: &mut out,
                        tr: &tr,
                        req: &mut req,
                    };
                    s.show(ui, &mut c);
                });
            }
        }
        s.focus_on(0);
        render(|ui| {
            let mut c = FeatureCtx {
                home: &home,
                team: None,
                view: None,
                out: &mut out,
                tr: &tr,
                req: &mut req,
            };
            s.show(ui, &mut c);
        });
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn orgs_map_drop_applies_and_saves() {
        let home = setup("orgmap-drop");
        let mut s = OrgMapScreen::new();
        s.reload(&home);
        let e = drop_edit(&s.map, "t", &Key::Org("beta".into())).unwrap();
        assert!(drop_edit(&s.map, "t", &Key::Group("acme".into(), "squad".into())).is_none());
        assert!(drop_edit(&s.map, "t", &Key::Unplaced).is_some());
        s.apply(&home, e);
        assert!(s.error.is_none(), "{:?}", s.error);
        assert_eq!(OrgMap::load(&home).placement("t"), Some(("beta", None)));
        let _ = std::fs::remove_dir_all(&home);
    }

    /// `RTEAMS_SHOTS_HOME=<demo home> RTEAMS_SHOTS_OUT=<dir> cargo test -p rteams-gui
    /// orgs_map_shots -- --ignored`: renders a populated map in all three views.
    #[test]
    #[ignore]
    fn orgs_map_shots() {
        let src = std::path::PathBuf::from(std::env::var("RTEAMS_SHOTS_HOME").unwrap());
        let out = std::path::PathBuf::from(std::env::var("RTEAMS_SHOTS_OUT").unwrap());
        let home = std::env::temp_dir().join(format!("rteams-orgshots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let names = [
            "platform",
            "runtime",
            "workspace",
            "intel",
            "knowledge",
            "quality",
            "reliability",
            "security",
        ];
        for n in names {
            let st = std::process::Command::new("cp")
                .arg("-R")
                .arg(src.join("demo"))
                .arg(home.join(n))
                .status()
                .unwrap();
            assert!(st.success());
        }
        let mut m = OrgMap::default();
        let atlas = m.create_org("Atlas AI", None).unwrap();
        let product = m.create_org("Product & Platform", Some(&atlas)).unwrap();
        let trust = m.create_org("Trust & Operations", Some(&atlas)).unwrap();
        let agent = m.create_group(&product, "Agent Platform").unwrap();
        let know = m.create_group(&trust, "Knowledge Systems").unwrap();
        m.place_team("platform", &product, Some(&agent)).unwrap();
        m.place_team("runtime", &product, Some(&agent)).unwrap();
        m.place_team("workspace", &product, None).unwrap();
        m.place_team("intel", &trust, Some(&know)).unwrap();
        m.place_team("knowledge", &trust, Some(&know)).unwrap();
        m.place_team("quality", &trust, None).unwrap();
        m.place_team("reliability", &trust, None).unwrap();
        m.add_relation(
            "platform",
            "runtime",
            rteams_services::orgs::model::RelationKind::DependsOn,
        )
        .unwrap();
        m.add_relation(
            "quality",
            "platform",
            rteams_services::orgs::model::RelationKind::Observes,
        )
        .unwrap();
        m.save(&home).unwrap();
        let tr = I18n::english();
        for (name, view) in [
            ("orgmap_hier", 0usize),
            ("orgmap_struct", 1),
            ("orgmap_rel", 2),
        ] {
            let mut s = OrgMapScreen::new();
            s.toolbar.set_view(view);
            s.relayout();
            let mut out_i = Intents::default();
            let mut req = Vec::new();
            let mut h = egui_kittest::Harness::builder()
                .with_size(egui::vec2(1600.0, 1000.0))
                .with_accessibility_check(false)
                .wgpu()
                .build_ui(|ui| {
                    crate::theme::apply(ui.ctx());
                    let mut c = FeatureCtx {
                        home: &home,
                        team: None,
                        view: None,
                        out: &mut out_i,
                        tr: &tr,
                        req: &mut req,
                    };
                    s.show(ui, &mut c);
                });
            h.run_steps(8);
            h.render()
                .unwrap()
                .save(out.join(format!("{name}.png")))
                .unwrap();
        }
        let _ = std::fs::remove_dir_all(&home);
    }
}
