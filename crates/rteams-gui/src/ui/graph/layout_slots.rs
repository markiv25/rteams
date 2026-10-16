//! Slots layout: the lead in the middle, members on rings at their stable (ring, sector) slots,
//! each member's processes and tasks hanging in a column beneath it (overflow collapsed).

use std::collections::BTreeMap;
use std::f32::consts::TAU;

use eframe::egui::{pos2, Pos2};
use rteams_core::Status;
use rteams_services::graph_model::{ring_capacity, GraphModel, GraphNode, NodeKind, Slot};

use super::layout_kanban::split_overflow;
use super::model::{node_size, Layout, Placed, PlacedKind, LEAD_SIZE, MEMBER_SIZE, OVERFLOW_SIZE};

pub const RING_BASE: f32 = 360.0;
pub const RING_STEP: f32 = 480.0;
pub const MAX_STACK: usize = 5;

/// Where a slot sits (center of the member node), world space.
pub fn slot_pos(s: Slot) -> Pos2 {
    let r = RING_BASE + s.ring as f32 * RING_STEP;
    let cap = ring_capacity(s.ring).max(1);
    // Rings are rotated a little against each other so columns do not line up.
    let a = -TAU / 4.0 + TAU * (s.sector % cap) as f32 / cap as f32 + s.ring as f32 * 0.2;
    pos2(r * a.cos(), r * a.sin())
}

fn status_rank(s: Option<Status>) -> u8 {
    match s {
        Some(Status::InProgress) => 0,
        Some(Status::NeedsFix) => 1,
        Some(Status::Review) => 2,
        Some(Status::Blocked) => 3,
        Some(Status::Pending) => 4,
        Some(Status::Approved) => 5,
        Some(Status::Done) => 6,
        None => 7,
    }
}

/// Hang `items` (processes, then tasks) in a column below `top_center`.
fn hang(
    l: &mut Layout,
    owner: Option<&str>,
    key: &str,
    top_center: Pos2,
    up: bool,
    mut items: Vec<&GraphNode>,
) {
    items.sort_by_key(|n| {
        (
            n.kind != NodeKind::Process,
            status_rank(n.status),
            n.id.trim_start_matches("task:")
                .parse::<u64>()
                .unwrap_or(u64::MAX),
        )
    });
    let (visible, hidden) = split_overflow(items, MAX_STACK);
    // `up`: stack away from the middle (above the owner) so columns never run into the lead.
    let dir = if up { -1.0 } else { 1.0 };
    let mut y = top_center.y;
    for n in &visible {
        let sz = node_size(n);
        l.push(Placed {
            id: n.id.clone(),
            pos: pos2(top_center.x, y + dir * sz.y / 2.0),
            size: sz,
            kind: PlacedKind::Node,
        });
        y += dir * (sz.y + 10.0);
    }
    if !hidden.is_empty() {
        let id = format!("overflow:{key}");
        for h in &hidden {
            l.hidden_in.insert(h.id.clone(), id.clone());
        }
        l.push(Placed {
            id,
            pos: pos2(top_center.x, y + dir * OVERFLOW_SIZE.y / 2.0),
            size: OVERFLOW_SIZE,
            kind: PlacedKind::Overflow {
                owner: owner.map(String::from),
                hidden: hidden.iter().map(|h| h.id.clone()).collect(),
            },
        });
    }
}

pub fn layout(model: &GraphModel, slots: &BTreeMap<String, Slot>, show_tasks: bool) -> Layout {
    let mut l = Layout::default();
    let lead = model.lead();
    if let Some(n) = lead {
        l.push(Placed {
            id: n.id.clone(),
            pos: Pos2::ZERO,
            size: LEAD_SIZE,
            kind: PlacedKind::Node,
        });
    }
    // Members without a stored slot (should not happen after `assign_slots`) go on the last ring.
    let mut fallback = 0u32;
    for n in model.nodes.iter().filter(|n| n.kind == NodeKind::Member) {
        let slot = slots.get(&n.id).copied().unwrap_or_else(|| {
            fallback += 1;
            Slot {
                ring: 9,
                sector: fallback,
            }
        });
        l.push(Placed {
            id: n.id.clone(),
            pos: slot_pos(slot),
            size: MEMBER_SIZE,
            kind: PlacedKind::Node,
        });
    }

    let owned = |owner: &str| -> Vec<&GraphNode> {
        model
            .nodes
            .iter()
            .filter(|n| match n.kind {
                NodeKind::Task => show_tasks && n.owner.as_deref() == Some(owner),
                NodeKind::Process => n.owner.as_deref() == Some(owner),
                _ => false,
            })
            .collect()
    };
    for n in model.members() {
        let Some(p) = l.get(&n.id).cloned() else {
            continue;
        };
        let items = owned(&n.id);
        if items.is_empty() {
            continue;
        }
        let up = p.pos.y < -60.0;
        let top = if up {
            pos2(p.pos.x, p.pos.y - p.size.y / 2.0 - 22.0)
        } else {
            pos2(p.pos.x, p.pos.y + p.size.y / 2.0 + 22.0)
        };
        hang(&mut l, Some(&n.id), &n.id, top, up, items);
    }
    // Tasks nobody owns hang under the lead (or the origin when there is none).
    let members: std::collections::HashSet<&str> = model.members().map(|m| m.id.as_str()).collect();
    let orphans: Vec<&GraphNode> = model
        .nodes
        .iter()
        .filter(|n| match n.kind {
            NodeKind::Task => {
                show_tasks && !n.owner.as_deref().is_some_and(|o| members.contains(o))
            }
            NodeKind::Process => !n.owner.as_deref().is_some_and(|o| members.contains(o)),
            _ => false,
        })
        .collect();
    if !orphans.is_empty() {
        let top = pos2(0.0, LEAD_SIZE.y / 2.0 + 30.0);
        hang(&mut l, None, "unowned", top, false, orphans);
    }
    l.reindex();
    l
}

#[cfg(test)]
mod tests {
    use super::*;
    use rteams_services::graph_model::{assign_slots, NodeState};

    fn node(id: &str, kind: NodeKind, owner: Option<&str>) -> GraphNode {
        GraphNode {
            id: id.into(),
            kind,
            label: id.into(),
            state: NodeState::Idle,
            status: (kind == NodeKind::Task).then_some(Status::Pending),
            owner: owner.map(String::from),
            runtime: None,
            role: None,
            online: false,
            current_task: None,
            last_line: None,
            url: None,
        }
    }

    pub fn big_model(members: usize, tasks_each: usize) -> (GraphModel, BTreeMap<String, Slot>) {
        let mut m = GraphModel::default();
        m.nodes.push(node("boss", NodeKind::Lead, None));
        let mut names = vec![];
        for i in 0..members {
            let n = format!("m{i}");
            m.nodes.push(node(&n, NodeKind::Member, None));
            for t in 0..tasks_each {
                m.nodes
                    .push(node(&format!("task:{i}-{t}"), NodeKind::Task, Some(&n)));
            }
            names.push(n);
        }
        m.nodes.push(node("task:free", NodeKind::Task, None));
        let mut slots = BTreeMap::new();
        assign_slots(&mut slots, &names);
        (m, slots)
    }

    #[test]
    fn graph_slots_places_everything_finite() {
        let (m, s) = big_model(8, 3);
        let l = layout(&m, &s, true);
        assert_eq!(l.items.len(), m.nodes.len());
        for p in &l.items {
            assert!(p.pos.x.is_finite() && p.pos.y.is_finite() && p.size.x >= 0.0);
        }
        // Lead at the origin; members off-center.
        assert_eq!(l.get("boss").unwrap().pos, Pos2::ZERO);
        assert!(l.get("m0").unwrap().pos.to_vec2().length() > 100.0);
        // Task hangs in a column above or below its owner, never beside the lead.
        let (t, o) = (l.get("task:0-0").unwrap(), l.get("m0").unwrap());
        assert!((t.pos.y - o.pos.y).abs() > o.size.y / 2.0);
        assert_eq!(t.pos.x, o.pos.x);
    }

    #[test]
    fn graph_slots_overflow_collapses() {
        let (m, s) = big_model(1, 20);
        let l = layout(&m, &s, true);
        let ov = l.get("overflow:m0").expect("overflow stack");
        match &ov.kind {
            PlacedKind::Overflow { hidden, owner } => {
                assert_eq!(hidden.len(), 20 - (MAX_STACK - 1));
                assert_eq!(owner.as_deref(), Some("m0"));
            }
            _ => panic!(),
        }
        assert_eq!(l.resolve("task:0-19").unwrap().id, "overflow:m0");
    }

    #[test]
    fn graph_slots_hide_tasks() {
        let (m, s) = big_model(3, 2);
        let l = layout(&m, &s, false);
        assert_eq!(l.items.len(), 4);
    }

    #[test]
    fn graph_slots_positions_are_stable() {
        let (m, s) = big_model(5, 1);
        let a = layout(&m, &s, true);
        let (m2, mut s2) = big_model(5, 1);
        let mut more: Vec<String> = s.keys().cloned().collect();
        more.push("extra".into());
        assign_slots(&mut s2, &more);
        let b = layout(&m2, &s2, true);
        for id in ["m0", "m1", "m4"] {
            assert_eq!(a.get(id).unwrap().pos, b.get(id).unwrap().pos);
        }
    }
}
