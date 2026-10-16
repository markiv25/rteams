//! A small d3-force clone (many-body charge, links, centering, collision) plus a rectangle
//! separation pass. Used by the Force layout: start from the slot positions, run a bounded
//! number of ticks, then freeze (no per-frame simulation).

use eframe::egui::{vec2, Pos2, Vec2};
use rteams_services::graph_model::{EdgeKind, GraphModel};

use super::model::{Layout, PlacedKind};

// Constants of the original (`canvas-constants.ts`).
pub const CHARGE: f32 = -800.0;
pub const CENTER_STRENGTH: f32 = 0.03;
pub const COLLIDE_RADIUS: f32 = 100.0;
pub const ALPHA_DECAY: f32 = 0.02;
pub const VELOCITY_DECAY: f32 = 0.4;
pub const MAX_TICKS: usize = 400;
const ALPHA_MIN: f32 = 0.001;

pub fn link_distance(k: EdgeKind) -> f32 {
    match k {
        EdgeKind::ParentChild => 500.0,
        EdgeKind::Ownership => 150.0,
        EdgeKind::Blocking => 200.0,
        EdgeKind::Related => 200.0,
        EdgeKind::Review => 250.0,
        EdgeKind::Message => 300.0,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Link {
    pub a: usize,
    pub b: usize,
    pub distance: f32,
}

pub struct Sim {
    pub pos: Vec<Pos2>,
    vel: Vec<Vec2>,
    /// Collision radius per node.
    pub radius: Vec<f32>,
    pub links: Vec<Link>,
    pub alpha: f32,
}

impl Sim {
    pub fn new(pos: Vec<Pos2>, radius: Vec<f32>, links: Vec<Link>) -> Sim {
        let n = pos.len();
        Sim {
            pos,
            vel: vec![Vec2::ZERO; n],
            radius,
            links,
            alpha: 1.0,
        }
    }

    pub fn tick(&mut self) {
        let n = self.pos.len();
        if n == 0 {
            return;
        }
        let alpha = self.alpha;
        // Many-body charge.
        for i in 0..n {
            for j in (i + 1)..n {
                let mut d = self.pos[j] - self.pos[i];
                if d.length_sq() < 1.0 {
                    // Deterministic nudge so coincident nodes separate.
                    d = vec2(
                        ((i * 7 + j) % 5) as f32 - 2.0 + 0.5,
                        ((i + j * 3) % 5) as f32 - 2.0 + 0.5,
                    );
                }
                let l2 = d.length_sq().max(100.0);
                let w = CHARGE * alpha / l2;
                self.vel[i] += d * w;
                self.vel[j] -= d * w;
            }
        }
        // Links (strength 1 / min degree, as d3 does).
        let mut deg = vec![0usize; n];
        for l in &self.links {
            deg[l.a] += 1;
            deg[l.b] += 1;
        }
        for l in &self.links {
            let mut d = (self.pos[l.b] + self.vel[l.b]) - (self.pos[l.a] + self.vel[l.a]);
            let len = d.length().max(1e-3);
            d *= (len - l.distance) / len * alpha / deg[l.a].min(deg[l.b]).max(1) as f32;
            let bias = deg[l.a] as f32 / (deg[l.a] + deg[l.b]) as f32;
            self.vel[l.b] -= d * bias;
            self.vel[l.a] += d * (1.0 - bias);
        }
        // Center.
        let mean = self.pos.iter().fold(Vec2::ZERO, |a, p| a + p.to_vec2()) / n as f32;
        for p in &mut self.pos {
            *p -= mean * CENTER_STRENGTH;
        }
        // Collide (circles).
        for i in 0..n {
            for j in (i + 1)..n {
                let d = self.pos[j] - self.pos[i];
                let min = self.radius[i] + self.radius[j];
                let len = d.length();
                if len < min && len > 1e-3 {
                    let push = d / len * ((min - len) * 0.5 * 0.7);
                    self.vel[j] += push;
                    self.vel[i] -= push;
                }
            }
        }
        for (p, v) in self.pos.iter_mut().zip(&mut self.vel) {
            *v *= 1.0 - VELOCITY_DECAY;
            *p += *v;
        }
        self.alpha *= 1.0 - ALPHA_DECAY;
    }

    /// Run until cooled or `max_ticks`.
    pub fn run(&mut self, max_ticks: usize) {
        for _ in 0..max_ticks {
            if self.alpha < ALPHA_MIN {
                break;
            }
            self.tick();
        }
    }
}

/// Push overlapping rects apart along the axis of least penetration until none overlap (or
/// `iters` passes). `gap` is the minimum free space between rects. Returns true when clean.
pub fn separate_rects(pos: &mut [Pos2], size: &[Vec2], gap: f32, iters: usize) -> bool {
    let n = pos.len();
    for _ in 0..iters {
        let mut moved = false;
        for i in 0..n {
            for j in (i + 1)..n {
                let mut d = pos[j] - pos[i];
                let ox = (size[i].x + size[j].x) / 2.0 + gap - d.x.abs();
                let oy = (size[i].y + size[j].y) / 2.0 + gap - d.y.abs();
                if ox > 0.0 && oy > 0.0 {
                    if d.x == 0.0 && d.y == 0.0 {
                        d = vec2(1.0 + (i % 3) as f32, 0.5 + (j % 3) as f32);
                    }
                    let sign = |v: f32| if v >= 0.0 { 1.0 } else { -1.0 };
                    let m = if ox < oy {
                        vec2(sign(d.x) * ox / 2.0, 0.0)
                    } else {
                        vec2(0.0, sign(d.y) * oy / 2.0)
                    };
                    pos[i] -= m;
                    pos[j] += m;
                    moved = true;
                }
            }
        }
        if !moved {
            return true;
        }
    }
    // Dense clusters can oscillate: place rects one by one, sliding each right of whatever it
    // hits. Every move increases x, so this terminates.
    for i in 0..n {
        loop {
            let hit = (0..i).find(|&j| {
                (pos[i].x - pos[j].x).abs() < (size[i].x + size[j].x) / 2.0 + gap - 0.01
                    && (pos[i].y - pos[j].y).abs() < (size[i].y + size[j].y) / 2.0 + gap - 0.01
            });
            match hit {
                Some(j) => pos[i].x = pos[j].x + (size[i].x + size[j].x) / 2.0 + gap,
                None => break,
            }
        }
    }
    true
}

/// Relax a slots layout with the simulation, then guarantee no overlaps.
pub fn relax_layout(l: &mut Layout, model: &GraphModel) {
    let n = l.items.len();
    if n < 2 {
        return;
    }
    let index: std::collections::HashMap<&str, usize> = l
        .items
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.as_str(), i))
        .collect();
    let mut links = Vec::new();
    for e in &model.edges {
        let a = l
            .resolve(&e.from)
            .and_then(|p| index.get(p.id.as_str()).copied());
        let b = l
            .resolve(&e.to)
            .and_then(|p| index.get(p.id.as_str()).copied());
        if let (Some(a), Some(b)) = (a, b) {
            if a != b && e.kind != EdgeKind::Message {
                links.push(Link {
                    a,
                    b,
                    distance: link_distance(e.kind),
                });
            }
        }
    }
    for (id, ov) in l
        .items
        .iter()
        .enumerate()
        .filter_map(|(i, p)| match &p.kind {
            PlacedKind::Overflow { owner: Some(o), .. } => Some((i, o.clone())),
            _ => None,
        })
    {
        if let Some(&o) = index.get(ov.as_str()) {
            links.push(Link {
                a: o,
                b: id,
                distance: link_distance(EdgeKind::Ownership),
            });
        }
    }
    let pos: Vec<Pos2> = l.items.iter().map(|p| p.pos).collect();
    let radius: Vec<f32> = l
        .items
        .iter()
        .map(|p| (p.size.x / 2.0).min(COLLIDE_RADIUS))
        .collect();
    let mut sim = Sim::new(pos, radius, links);
    // Start cooler than 1.0 so the slot structure is mostly kept.
    sim.alpha = 0.5;
    sim.run(MAX_TICKS);
    let mut pos = sim.pos;
    let size: Vec<Vec2> = l.items.iter().map(|p| p.size).collect();
    separate_rects(&mut pos, &size, 12.0, 400);
    for (p, np) in l.items.iter_mut().zip(pos) {
        if np.x.is_finite() && np.y.is_finite() {
            p.pos = np;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{pos2, Rect};

    fn overlaps(pos: &[Pos2], size: &[Vec2]) -> bool {
        for i in 0..pos.len() {
            for j in (i + 1)..pos.len() {
                let a = Rect::from_center_size(pos[i], size[i]);
                let b = Rect::from_center_size(pos[j], size[j]);
                if a.intersects(b) && a.intersect(b).area() > 0.5 {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    fn graph_force_no_nan_and_cools() {
        let pos: Vec<Pos2> = (0..30)
            .map(|i| pos2((i % 5) as f32, (i / 5) as f32))
            .collect();
        let links = (1..30)
            .map(|i| Link {
                a: 0,
                b: i,
                distance: 200.0,
            })
            .collect();
        let mut s = Sim::new(pos, vec![50.0; 30], links);
        s.run(MAX_TICKS);
        assert!(s.alpha < 0.01);
        assert!(s.pos.iter().all(|p| p.x.is_finite() && p.y.is_finite()));
        // Charge spreads coincident-ish nodes.
        let spread = s
            .pos
            .iter()
            .map(|p| p.distance(s.pos[0]))
            .fold(0.0, f32::max);
        assert!(spread > 100.0);
    }

    #[test]
    fn graph_force_empty_and_single() {
        let mut s = Sim::new(vec![], vec![], vec![]);
        s.run(10);
        let mut s = Sim::new(vec![pos2(5.0, 5.0)], vec![10.0], vec![]);
        s.run(10);
        assert!(s.pos[0].x.is_finite());
    }

    #[test]
    fn graph_force_separate_rects_removes_overlap() {
        // Deterministic pseudo-random cluster, including exact duplicates.
        let mut pos: Vec<Pos2> = (0..60)
            .map(|i| pos2(((i * 37) % 11) as f32 * 20.0, ((i * 53) % 7) as f32 * 15.0))
            .collect();
        pos[3] = pos[4];
        let size: Vec<Vec2> = (0..60)
            .map(|i| {
                if i % 3 == 0 {
                    vec2(200.0, 56.0)
                } else {
                    vec2(200.0, 36.0)
                }
            })
            .collect();
        assert!(overlaps(&pos, &size));
        assert!(separate_rects(&mut pos, &size, 8.0, 2000));
        assert!(!overlaps(&pos, &size));
        assert!(pos.iter().all(|p| p.x.is_finite() && p.y.is_finite()));
    }

    #[test]
    fn graph_force_relax_layout_has_no_overlaps() {
        use super::super::layout_slots;
        use rteams_services::graph_model::{assign_slots, EdgeKind, GraphEdge};
        use std::collections::BTreeMap;
        let (mut m, _) = (GraphModel::default(), ());
        let mut names = vec![];
        let mk = |id: &str, kind, owner: Option<&str>| rteams_services::graph_model::GraphNode {
            id: id.into(),
            kind,
            label: id.into(),
            state: rteams_services::graph_model::NodeState::Idle,
            status: Some(rteams_core::Status::Pending),
            owner: owner.map(String::from),
            runtime: None,
            role: None,
            online: false,
            current_task: None,
            last_line: None,
            url: None,
        };
        use rteams_services::graph_model::NodeKind;
        m.nodes.push(mk("boss", NodeKind::Lead, None));
        for i in 0..9 {
            let n = format!("m{i}");
            m.nodes.push(mk(&n, NodeKind::Member, None));
            m.edges.push(GraphEdge {
                from: "boss".into(),
                to: n.clone(),
                kind: EdgeKind::ParentChild,
            });
            for t in 0..4 {
                let id = format!("task:{i}-{t}");
                m.nodes.push(mk(&id, NodeKind::Task, Some(&n)));
                m.edges.push(GraphEdge {
                    from: n.clone(),
                    to: id,
                    kind: EdgeKind::Ownership,
                });
            }
            names.push(n);
        }
        let mut slots = BTreeMap::new();
        assign_slots(&mut slots, &names);
        let mut l = layout_slots::layout(&m, &slots, true);
        relax_layout(&mut l, &m);
        let pos: Vec<Pos2> = l.items.iter().map(|p| p.pos).collect();
        let size: Vec<Vec2> = l.items.iter().map(|p| p.size).collect();
        assert!(!overlaps(&pos, &size));
        assert!(pos.iter().all(|p| p.x.is_finite() && p.y.is_finite()));
    }
}
