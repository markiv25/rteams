//! Team Graph screen: the lead in the middle, members on stable slots around it, tasks hanging
//! under their owners (or in kanban lanes), edges for ownership / review / dependencies, and
//! particles flying along edges for recent messages.
//!
//! The drawing and hit-testing primitives live in `kit` / `camera` and are reused by the org
//! map. Data comes from `rteams_services::graph_model`.

pub mod camera;
pub mod draw_agents;
pub mod draw_edges;
pub mod draw_tasks;
pub mod force;
pub mod hud_activity;
pub mod kit;
pub mod layout_kanban;
pub mod layout_slots;
pub mod minimap;
pub mod model;
pub mod particles;
pub mod popovers;
pub mod provisioning_hud;

use std::collections::{hash_map::DefaultHasher, BTreeMap};
use std::hash::{Hash, Hasher};

use eframe::egui::{self, vec2, Color32, Pos2, Rect, RichText, Sense};
use rteams_services::graph_model::{self, GraphModel, MemberRuntime, NodeKind, Slot};
use rteams_services::i18n::I18n;

use super::{FeatureCtx, Screen, TabId};
use crate::launcher::AgentStatus;
use crate::theme;
use crate::view::{Intent, View};
use camera::{semantic_zoom, Detail};
use kit::Canvas;
use model::{GraphUi, Layout, LayoutMode, Placed, PlacedKind, Popover};

/// Translate `key`, or `fallback` when the catalog has no such key.
pub fn tr_or(tr: &I18n, key: &str, fallback: &str) -> String {
    if tr.has(key) {
        tr.t(key)
    } else {
        fallback.to_string()
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Launcher facts for the model.
pub fn runtimes(view: &View, now: i64) -> Vec<MemberRuntime> {
    view.agents
        .iter()
        .map(|a| MemberRuntime {
            name: a.member.clone(),
            running: matches!(a.status, AgentStatus::Running { .. }),
            failed: matches!(a.status, AgentStatus::Failed(_)),
            rate_limited: a.rate_limited_until.is_some_and(|t| t > now),
            runtime: Some(a.runtime.clone()),
            last_line: popovers::last_line(a),
        })
        .collect()
}

pub struct GraphScreen {
    ui: GraphUi,
    prefs_home: Option<std::path::PathBuf>,
    slots: BTreeMap<String, Slot>,
    slots_team: String,
    layout: Layout,
    layout_key: u64,
    particles: particles::ParticleSystem,
    /// Process url clicked this frame; opened after the frame.
    open_url: Option<String>,
}

impl Default for GraphScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphScreen {
    pub fn new() -> Self {
        GraphScreen {
            ui: GraphUi::default(),
            prefs_home: None,
            slots: BTreeMap::new(),
            slots_team: String::new(),
            layout: Layout::default(),
            layout_key: 0,
            particles: particles::ParticleSystem::default(),
            open_url: None,
        }
    }

    fn prefs_path(home: &std::path::Path) -> std::path::PathBuf {
        home.join("graph-ui.json")
    }

    fn load_prefs(&mut self, home: &std::path::Path) {
        if self.prefs_home.as_deref() == Some(home) {
            return;
        }
        self.prefs_home = Some(home.to_path_buf());
        if let Some(v) = std::fs::read(Self::prefs_path(home))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        {
            self.ui.prefs = model::Prefs::from_json(&v);
        }
    }

    fn save_prefs(&self, home: &std::path::Path) {
        if let Ok(b) = serde_json::to_vec_pretty(&self.ui.prefs.to_json()) {
            let _ = rteams_services::atomic::write(&Self::prefs_path(home), &b);
        }
    }

    /// Keep slots in sync with the roster; persist when changed.
    fn sync_slots(&mut self, home: &std::path::Path, team: &str, members: &[String]) {
        let path = graph_model::slots_path(&home.join(team));
        if self.slots_team != team {
            self.slots = graph_model::load_slots(&path);
            self.slots_team = team.to_string();
            self.layout_key = 0;
        }
        if graph_model::assign_slots(&mut self.slots, members) {
            let _ = graph_model::save_slots(&path, &self.slots);
        }
    }

    fn key_for(&self, model: &GraphModel) -> u64 {
        let mut h = DefaultHasher::new();
        (self.ui.prefs.mode, self.ui.prefs.show_tasks).hash(&mut h);
        for n in &model.nodes {
            (&n.id, n.status, &n.owner, n.kind as u8).hash(&mut h);
        }
        for (k, v) in &self.slots {
            (k, v.ring, v.sector).hash(&mut h);
        }
        h.finish()
    }

    /// The whole frame, separated from `Screen::show` so tests can drive it with a view.
    fn paint(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx, view: &View) {
        let ctx = ui.ctx().clone();
        self.load_prefs(c.home);
        let now = now_ms();
        let team_dir = c.team.unwrap_or("team");
        let members: Vec<String> = view
            .agent_members()
            .iter()
            .map(|m| m.name.clone())
            .collect();
        self.sync_slots(c.home, team_dir, &members);
        let rt = runtimes(view, now);
        let model = graph_model::build(view.state, view.data.events(), &rt, view.actor, now);
        if model.is_empty() {
            empty_state(ui, c.tr);
            return;
        }
        let key = self.key_for(&model);
        if key != self.layout_key {
            let first = self.layout_key == 0;
            self.layout = model::build_layout(
                &model,
                &self.slots,
                self.ui.prefs.mode,
                self.ui.prefs.show_tasks,
            );
            self.layout_key = key;
            if first || self.ui.want_fit {
                self.ui.want_fit = true;
            }
        }

        let rect = ui.available_rect_before_wrap();
        let rect = Rect::from_min_size(
            rect.min,
            vec2(rect.width().max(1.0), rect.height().max(1.0)),
        );
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        // Fit on open, on layout changes and whenever the viewport is resized.
        if (rect.size() - self.ui.last_size).abs().max_elem() > 2.0 {
            self.ui.last_size = rect.size();
            self.ui.want_fit = true;
        }
        if self.ui.want_fit {
            // Leave room for the toolbar at the top.
            let inner = Rect::from_min_max(rect.min + vec2(0.0, 52.0), rect.max);
            self.ui.cam.fit(self.layout.bounds(), inner, 40.0);
            self.ui.cam.center -= (inner.center() - rect.center()) / self.ui.cam.zoom;
            self.ui.want_fit = false;
        }

        // Camera input (only when the pointer is on the canvas itself).
        if resp.dragged() {
            self.ui.cam.pan_screen(resp.drag_delta());
            self.ui.popover = None;
        }
        if let Some(ptr) = resp.hover_pos() {
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
            if pinch != 1.0 {
                self.ui.cam.zoom_at(ptr, rect, pinch);
            } else if scroll.y != 0.0 {
                self.ui.cam.zoom_at(ptr, rect, (scroll.y * 0.0015).exp());
            }
            if scroll.x != 0.0 && pinch == 1.0 {
                self.ui.cam.pan_screen(vec2(scroll.x, 0.0));
            }
        }
        self.ui.cam.clamp();

        let painter = ui.painter_at(rect);
        let focus = self
            .layout
            .items
            .iter()
            .find(|i| model.node(&i.id).is_some_and(|n| n.kind == NodeKind::Lead))
            .map_or(Pos2::ZERO, |i| i.pos);
        kit::draw_background(
            &painter,
            rect,
            &self.ui.cam,
            self.ui.cam.world_to_screen(focus, rect),
        );
        let cv = Canvas::new(&painter, rect, self.ui.cam);
        let detail = semantic_zoom(self.ui.cam.zoom);
        let geoms = draw_edges::compute(&model, &self.layout, self.ui.prefs.mode);

        // Hover: topmost item under the pointer, else a blocking edge.
        let ptr = resp
            .hover_pos()
            .filter(|p| rect.contains(*p) && !minimap::area_for(rect).contains(*p));
        let hit_item: Option<usize> = ptr.and_then(|p| {
            let w = cv.world(p);
            self.layout
                .items
                .iter()
                .rposition(|it| kit::point_in_rounded_rect(it.rect(), it.size.y * 0.4, w))
        });
        let hit_edge = match (hit_item, ptr) {
            (None, Some(p)) => draw_edges::hit_blocking(&cv, &geoms, p),
            _ => None,
        };

        // Lanes (kanban).
        for lane in &self.layout.lanes {
            let r = cv.rect(lane.rect);
            if cv.visible(r) {
                let col = draw_tasks::status_color(
                    [
                        rteams_core::Status::Pending,
                        rteams_core::Status::InProgress,
                        rteams_core::Status::Review,
                        rteams_core::Status::Approved,
                        rteams_core::Status::Done,
                    ][lane.color_idx.min(4)],
                );
                painter.rect_filled(r, 14.0, theme::tint(col, 0.045));
                painter.rect_stroke(
                    r,
                    14.0,
                    egui::Stroke::new(1.0, theme::tint(col, 0.12)),
                    egui::StrokeKind::Inside,
                );
                if detail != Detail::Dot {
                    kit::draw_text_fit(
                        &painter,
                        Pos2::new(
                            r.left() + 10.0 * self.ui.cam.zoom,
                            r.top() + 14.0 * self.ui.cam.zoom,
                        ),
                        &format!("{} {}", lane.title, lane.count),
                        13.0 * self.ui.cam.zoom,
                        col,
                        r.width() - 12.0,
                    );
                }
            }
        }

        let hovered_id = hit_item.map(|i| self.layout.items[i].id.as_str());
        draw_edges::draw(&cv, &geoms, hovered_id, hit_edge);

        for it in &self.layout.items {
            let hovered = hovered_id == Some(it.id.as_str());
            match &it.kind {
                PlacedKind::Overflow { hidden, .. } => {
                    let text = c.tr.t_args(
                        "team.agentGraph.activityHud.more",
                        &[("count", &hidden.len().to_string())],
                    );
                    draw_tasks::draw_overflow(&cv, it.rect(), &text, hovered);
                }
                PlacedKind::Node => {
                    if let Some(n) = model.node(&it.id) {
                        match n.kind {
                            NodeKind::Task => {
                                let oc = n.owner.as_deref().map(|o| view.color(o));
                                draw_tasks::draw(&cv, it.rect(), n, oc, hovered, detail)
                            }
                            _ => draw_agents::draw(
                                &cv,
                                it.rect(),
                                n,
                                view.color(&n.label),
                                hovered,
                                detail,
                                ui.input(|i| i.time),
                            ),
                        }
                    }
                }
            }
        }

        // Particles.
        let time = ui.input(|i| i.time);
        self.particles
            .update(&model.flows, time, now, self.ui.prefs.animate);
        self.particles.draw(&cv, &geoms, &self.layout, time);
        let mut animating = self.particles.active();
        // Ambient pulses on working edges: a slow particle runs lead -> member and member ->
        // task while the target is active.
        if self.ui.prefs.animate {
            for g in &geoms {
                let color = match g.kind {
                    graph_model::EdgeKind::ParentChild => theme::ACCENT,
                    graph_model::EdgeKind::Ownership => theme::COL_IN_PROGRESS,
                    _ => continue,
                };
                if !model
                    .node(&g.to)
                    .is_some_and(|n| n.state == graph_model::NodeState::Active)
                {
                    continue;
                }
                let phase = g.to.bytes().map(f64::from).sum::<f64>() % 100.0 / 100.0;
                let t = ((time * 0.4 + phase) % 1.0) as f32;
                let bez = g.bez.map(|p| cv.pt(p));
                kit::draw_particle(
                    &painter,
                    &bez,
                    t,
                    color,
                    2.4 * self.ui.cam.zoom.clamp(0.7, 1.4),
                );
                animating = true;
            }
        }
        if animating {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }

        // Click handling.
        if resp.clicked() {
            if let Some(p) = ptr {
                match hit_item.map(|i| &self.layout.items[i]) {
                    Some(it) => self.click_item(it.clone(), p, &model, view, c),
                    None => {
                        self.ui.popover = hit_edge.map(|e| Popover::Blocking {
                            from: geoms[e].from.clone(),
                            to: geoms[e].to.clone(),
                            at: p,
                        });
                    }
                }
            }
        }

        // Hover card.
        if let (Some(i), None) = (hit_item, &self.ui.popover) {
            let it = &self.layout.items[i];
            if let (PlacedKind::Node, Some(n)) = (&it.kind, model.node(&it.id)) {
                let sr = cv.rect(it.rect());
                popovers::hover_card(&ctx, sr, rect, |ui| match n.kind {
                    NodeKind::Task => popovers::task_card(ui, n),
                    NodeKind::Process => popovers::process_card(ui, n, c.tr),
                    _ => {
                        popovers::member_card(ui, n, view.color(&n.label), view.agent(&n.id), c.tr)
                    }
                });
            }
        }

        // Overlays.
        hud_activity::show(&ctx, rect, &model, c.tr);
        provisioning_hud::show(&ctx, rect, view, c.tr);
        let dots: Vec<(Rect, Color32)> = self
            .layout
            .items
            .iter()
            .map(|it| {
                let col = match model.node(&it.id) {
                    Some(n) if n.kind == NodeKind::Task => {
                        draw_tasks::status_color(n.status.unwrap_or(rteams_core::Status::Pending))
                    }
                    Some(n) => view.color(&n.label),
                    None => theme::MUTED,
                };
                (it.rect(), col)
            })
            .collect();
        minimap::show(ui, rect, self.layout.bounds(), &dots, &mut self.ui.cam);
        self.toolbar(&ctx, rect, c.tr, c.home);
        self.popover(&ctx, &model, c);
        if let Some(u) = self.open_url.take() {
            ctx.open_url(egui::OpenUrl::new_tab(u));
        }
    }

    fn click_item(
        &mut self,
        it: Placed,
        at: Pos2,
        model: &GraphModel,
        view: &View,
        c: &mut FeatureCtx,
    ) {
        self.ui.popover = None;
        match it.kind {
            PlacedKind::Overflow { .. } => {
                self.ui.popover = Some(Popover::Overflow { id: it.id, at })
            }
            PlacedKind::Node => {
                let Some(n) = model.node(&it.id) else { return };
                match n.kind {
                    NodeKind::Task => c.out.push(Intent::OpenTask(
                        n.id.trim_start_matches("task:").to_string(),
                    )),
                    NodeKind::Lead | NodeKind::Member => {
                        if view.members().iter().any(|m| m.name == n.id) {
                            c.out.push(Intent::OpenMember(n.id.clone()));
                        }
                    }
                    NodeKind::Process => {
                        if let Some(u) = &n.url {
                            self.open_url = Some(u.clone());
                        }
                    }
                }
            }
        }
    }

    fn popover(&mut self, ctx: &egui::Context, model: &GraphModel, c: &mut FeatureCtx) {
        let Some(p) = self.ui.popover.clone() else {
            return;
        };
        let close = match &p {
            Popover::Blocking { from, to, at } => {
                popovers::blocking(ctx, *at, model.node(from), model.node(to), c.out, c.tr)
            }
            Popover::Overflow { id, at } => {
                let hidden = match self.layout.get(id).map(|i| &i.kind) {
                    Some(PlacedKind::Overflow { hidden, .. }) => hidden.clone(),
                    _ => Vec::new(),
                };
                popovers::overflow(ctx, *at, &hidden, model, c.out, c.tr)
            }
        };
        if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.ui.popover = None;
        }
    }

    fn toolbar(&mut self, ctx: &egui::Context, viewport: Rect, tr: &I18n, home: &std::path::Path) {
        let mut changed = false;
        let modes = [LayoutMode::Slots, LayoutMode::Kanban, LayoutMode::Force];
        let mut idx = modes
            .iter()
            .position(|m| *m == self.ui.prefs.mode)
            .unwrap_or(0);
        let pos = Pos2::new(viewport.center().x, viewport.top() + 14.0);
        egui::Area::new(egui::Id::NULL.with("graph_toolbar"))
            .order(egui::Order::Middle)
            .pivot(egui::Align2::CENTER_TOP)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                kit::pill_bar_frame().show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    ui.horizontal(|ui| {
                        if theme::segmented(ui, &mut idx, &["Slots", "Kanban", "Force"])
                            && modes[idx] != self.ui.prefs.mode
                        {
                            self.ui.prefs.mode = modes[idx];
                            self.ui.want_fit = true;
                            changed = true;
                        }
                        divider(ui);
                        let t = tr_or(tr, "team.organizations.graph.toolbar.tasks", "Tasks");
                        ui.label(RichText::new(t).size(12.0).color(theme::TEXT_2));
                        if theme::switch(ui, &mut self.ui.prefs.show_tasks).changed() {
                            changed = true;
                        }
                        let a = tr_or(
                            tr,
                            "team.organizations.graph.toolbar.animation",
                            "Animation",
                        );
                        ui.label(RichText::new(a).size(12.0).color(theme::TEXT_2));
                        if theme::switch(ui, &mut self.ui.prefs.animate).changed() {
                            changed = true;
                        }
                        divider(ui);
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let tip = |k: &str, f: &str| tr_or(tr, k, f);
                        if kit::tool_button(
                            ui,
                            kit::Tool::ZoomOut,
                            &tip("team.organizations.graph.toolbar.zoomOut", "Zoom out"),
                        )
                        .clicked()
                        {
                            self.ui.cam.zoom_step(viewport, false);
                        }
                        if kit::tool_button(
                            ui,
                            kit::Tool::ZoomIn,
                            &tip("team.organizations.graph.toolbar.zoomIn", "Zoom in"),
                        )
                        .clicked()
                        {
                            self.ui.cam.zoom_step(viewport, true);
                        }
                        if kit::tool_button(
                            ui,
                            kit::Tool::Fit,
                            &tip(
                                "team.organizations.graph.toolbar.fit",
                                "Fit meaningful overview",
                            ),
                        )
                        .clicked()
                        {
                            self.ui.want_fit = true;
                            self.layout_key = 0;
                        }
                    });
                });
            });
        if changed {
            self.save_prefs(home);
        }
    }
}

/// Thin vertical rule between toolbar groups.
fn divider(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(1.0, 18.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, theme::hairline(0.12));
}

fn empty_state(ui: &mut egui::Ui, tr: &I18n) {
    ui.centered_and_justified(|ui| {
        ui.label(
            RichText::new(tr_or(
                tr,
                "team.agentGraph.activityHud.noRecentActivity",
                "No recent activity",
            ))
            .color(theme::MUTED),
        );
    });
}

impl Screen for GraphScreen {
    fn id(&self) -> TabId {
        TabId::Graph
    }

    fn title_key(&self) -> &'static str {
        "team.organizations.inspector.actions.graph"
    }

    fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        match c.view {
            Some(view) if view.team().is_some() => self.paint(ui, c, view),
            _ => empty_state(ui, c.tr),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{testing, Intents};

    fn run(view: Option<&View>, out: &mut Intents) {
        let home = std::env::temp_dir().join(format!("rteams-graph-ui-{}", std::process::id()));
        let tr = I18n::english();
        let mut req = Vec::new();
        let mut screen = GraphScreen::new();
        testing::render(|ui| {
            let mut c = FeatureCtx {
                home: &home,
                team: Some("t"),
                view,
                out,
                tr: &tr,
                req: &mut req,
            };
            screen.show(ui, &mut c);
        });
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn graph_renders_empty_state_without_view() {
        let mut out = Intents::default();
        run(None, &mut out);
        assert!(out.0.is_empty());
    }

    #[test]
    fn graph_renders_fixture_at_three_sizes() {
        let (home, data) = testing::fixture("graph-render");
        let view = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let mut out = Intents::default();
        run(Some(&view), &mut out);
        // Also with a different actor (lead included in the roster) and every layout mode.
        let view2 = View {
            actor: None,
            ..View {
                state: data.state(),
                data: &data,
                actor: None,
                agents: &[],
                tr: crate::view::testing::tr(),
            }
        };
        for mode in [LayoutMode::Slots, LayoutMode::Kanban, LayoutMode::Force] {
            let tr = I18n::english();
            let mut req = Vec::new();
            let mut screen = GraphScreen::new();
            screen.ui.prefs.mode = mode;
            screen.prefs_home = Some(home.clone());
            testing::render(|ui| {
                let mut c = FeatureCtx {
                    home: &home,
                    team: Some("t"),
                    view: Some(&view2),
                    out: &mut out,
                    tr: &tr,
                    req: &mut req,
                };
                screen.show(ui, &mut c);
            });
            assert!(!screen.layout.items.is_empty());
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn graph_title_key_exists_in_catalog() {
        let s = GraphScreen::new();
        assert_eq!(s.id(), TabId::Graph);
        assert!(I18n::english().has(s.title_key()));
    }

    #[test]
    fn graph_model_from_fixture_state() {
        let (home, data) = testing::fixture("graph-model");
        let view = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let rt = runtimes(&view, 0);
        assert!(rt.is_empty());
        let g = graph_model::build(view.state, data.events(), &rt, Some("boss"), now_ms());
        assert!(g.node("ada").is_some() && g.node("bob").is_some());
        assert_eq!(
            g.node("boss").map(|n| n.kind),
            Some(NodeKind::Lead),
            "the human lead is drawn in the middle"
        );
        assert_eq!(g.tasks().count(), 4);
        assert!(g
            .edges
            .iter()
            .any(|e| e.kind == graph_model::EdgeKind::Blocking));
        assert!(!g.activity.is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }
}
