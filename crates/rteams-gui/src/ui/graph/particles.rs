//! Particles flying along edges for recent events (message, comment, assign, review).

use std::collections::HashSet;

use eframe::egui::Color32;
use rteams_services::graph_model::{Flow, FlowKind};

use super::draw_edges::{connect, EdgeGeom};
use super::kit::{self, Bezier, Canvas};
use super::model::Layout;
use crate::theme;

/// Seconds a particle takes to fly along its edge.
pub const DURATION: f64 = 1.6;
/// Flows older than this (ms) are not animated when first seen.
pub const MAX_AGE_MS: i64 = 4_000;

pub fn color(k: FlowKind) -> Color32 {
    match k {
        FlowKind::Message => theme::SEND,
        FlowKind::Comment => theme::THINK_FG,
        FlowKind::Assign => theme::ACCENT,
        FlowKind::ReviewRequest => theme::COL_REVIEW,
        FlowKind::ReviewResponse => theme::SUCCESS,
    }
}

#[derive(Debug, Clone)]
struct Particle {
    from: String,
    to: String,
    kind: FlowKind,
    born: f64,
}

#[derive(Default)]
pub struct ParticleSystem {
    seen: HashSet<(u64, String, String)>,
    live: Vec<Particle>,
}

impl ParticleSystem {
    /// Spawn particles for flows not seen before and drop finished ones. `time` is egui
    /// time in seconds, `now_ms` wall-clock ms.
    pub fn update(&mut self, flows: &[Flow], time: f64, now_ms: i64, enabled: bool) {
        for f in flows {
            let key = (f.seq, f.from.clone(), f.to.clone());
            if self.seen.insert(key) && enabled && now_ms - f.ts <= MAX_AGE_MS {
                self.live.push(Particle {
                    from: f.from.clone(),
                    to: f.to.clone(),
                    kind: f.kind,
                    born: time,
                });
            }
        }
        self.live.retain(|p| time - p.born < DURATION);
        if self.seen.len() > 2000 {
            self.seen.clear();
        }
    }

    pub fn active(&self) -> bool {
        !self.live.is_empty()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.live.len()
    }

    /// Curve (world) a particle from `from` to `to` follows, and whether to travel it reversed.
    fn path(geoms: &[EdgeGeom], layout: &Layout, from: &str, to: &str) -> Option<Bezier> {
        let a = layout.resolve(from)?;
        let b = layout.resolve(to)?;
        if a.id == b.id {
            return None;
        }
        if let Some(g) = geoms
            .iter()
            .find(|g| g.from_item == a.id && g.to_item == b.id)
        {
            return Some(g.bez);
        }
        if let Some(g) = geoms
            .iter()
            .find(|g| g.from_item == b.id && g.to_item == a.id)
        {
            let r = g.bez;
            return Some(Bezier {
                p0: r.p3,
                p1: r.p2,
                p2: r.p1,
                p3: r.p0,
            });
        }
        Some(connect(a, b, 0.2))
    }

    pub fn draw(&self, cv: &Canvas, geoms: &[EdgeGeom], layout: &Layout, time: f64) {
        for p in &self.live {
            let Some(b) = Self::path(geoms, layout, &p.from, &p.to) else {
                continue;
            };
            let t = ease(((time - p.born) / DURATION) as f32);
            let screen = b.map(|q| cv.pt(q));
            kit::draw_particle(
                cv.painter,
                &screen,
                t,
                color(p.kind),
                (3.0 * cv.cam.zoom).clamp(2.0, 6.0),
            );
        }
    }
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flow(seq: u64, ts: i64) -> Flow {
        Flow {
            seq,
            ts,
            from: "a".into(),
            to: "b".into(),
            kind: FlowKind::Message,
        }
    }

    #[test]
    fn graph_particles_spawn_once_and_expire() {
        let mut ps = ParticleSystem::default();
        let fl = [flow(1, 9_000), flow(2, 1_000)];
        ps.update(&fl, 0.0, 10_000, true);
        assert_eq!(ps.len(), 1, "old flow is not animated");
        ps.update(&fl, 0.5, 10_500, true);
        assert_eq!(ps.len(), 1, "no respawn");
        assert!(ps.active());
        ps.update(&fl, DURATION + 0.1, 12_000, true);
        assert!(!ps.active());
    }

    #[test]
    fn graph_particles_disabled_spawns_nothing() {
        let mut ps = ParticleSystem::default();
        ps.update(&[flow(1, 10_000)], 0.0, 10_000, false);
        assert!(!ps.active());
    }

    #[test]
    fn graph_particles_ease_bounds() {
        assert_eq!(ease(-1.0), 0.0);
        assert_eq!(ease(2.0), 1.0);
        assert!((ease(0.5) - 0.5).abs() < 1e-6);
    }
}
