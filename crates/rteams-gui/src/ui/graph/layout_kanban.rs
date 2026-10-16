//! Kanban layout: tasks in status lanes, the team in a row above them, with overflow collapsed
//! after `MAX_ROWS` tasks per lane (`collapseOverflowStacks` in the original).

use eframe::egui::{pos2, vec2, Rect};
use rteams_core::Status;
use rteams_services::graph_model::{GraphModel, GraphNode, NodeKind};

use super::model::{
    node_size, Lane, Layout, Placed, PlacedKind, LEAD_SIZE, MEMBER_SIZE, OVERFLOW_SIZE, TASK_SIZE,
};

pub const MAX_ROWS: usize = 10;
pub const LANE_GAP: f32 = 40.0;
pub const ROW_STEP: f32 = TASK_SIZE.y + 10.0;
pub const LANE_TITLES: [&str; 5] = ["To do", "In progress", "Review", "Approved", "Done"];

/// Lane of a status.
pub fn lane_of(s: Status) -> usize {
    match s {
        Status::Pending | Status::Blocked => 0,
        Status::InProgress => 1,
        Status::Review | Status::NeedsFix => 2,
        Status::Approved => 3,
        Status::Done => 4,
    }
}

/// Split `items` (already in display order) into the visible head and the hidden tail.
pub fn split_overflow<T>(mut items: Vec<T>, max: usize) -> (Vec<T>, Vec<T>) {
    if items.len() <= max {
        return (items, Vec::new());
    }
    // Keep room for the "+N more" stack in the last row.
    let hidden = items.split_off(max.saturating_sub(1));
    (items, hidden)
}

pub fn layout(model: &GraphModel, show_tasks: bool) -> Layout {
    let mut l = Layout::default();
    let lane_w = TASK_SIZE.x + LANE_GAP;
    let total_w = lane_w * LANE_TITLES.len() as f32 - LANE_GAP;
    let x0 = -total_w / 2.0;

    // Team row above the lanes: lead centered on top, members and processes in a row below.
    let lead_y = -(LEAD_SIZE.y / 2.0) - 330.0;
    let row_y = lead_y + LEAD_SIZE.y / 2.0 + 90.0 + MEMBER_SIZE.y / 2.0;
    if let Some(n) = model.lead() {
        l.push(Placed {
            id: n.id.clone(),
            pos: pos2(0.0, lead_y),
            size: node_size(n),
            kind: PlacedKind::Node,
        });
    }
    let row: Vec<&GraphNode> = model
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Member | NodeKind::Process))
        .collect();
    let step = MEMBER_SIZE.x + 30.0;
    let rx0 = -(row.len().saturating_sub(1) as f32) * step / 2.0;
    for (i, n) in row.iter().enumerate() {
        l.push(Placed {
            id: n.id.clone(),
            pos: pos2(rx0 + i as f32 * step, row_y),
            size: node_size(n),
            kind: PlacedKind::Node,
        });
    }

    let top = row_y + MEMBER_SIZE.y / 2.0 + 70.0;
    let mut lanes: Vec<Vec<&GraphNode>> = vec![Vec::new(); LANE_TITLES.len()];
    if show_tasks {
        let mut tasks: Vec<&GraphNode> = model.tasks().collect();
        // Stable order: by numeric id.
        tasks.sort_by_key(|t| {
            t.id.trim_start_matches("task:")
                .parse::<u64>()
                .unwrap_or(u64::MAX)
        });
        for t in tasks {
            lanes[lane_of(t.status.unwrap_or(Status::Pending))].push(t);
        }
    }
    for (i, lane) in lanes.into_iter().enumerate() {
        let cx = x0 + i as f32 * lane_w + TASK_SIZE.x / 2.0;
        let count = lane.len();
        let (visible, hidden) = split_overflow(lane, MAX_ROWS);
        for (r, t) in visible.iter().enumerate() {
            l.push(Placed {
                id: t.id.clone(),
                pos: pos2(cx, top + r as f32 * ROW_STEP + TASK_SIZE.y / 2.0),
                size: TASK_SIZE,
                kind: PlacedKind::Node,
            });
        }
        if !hidden.is_empty() {
            let id = format!("overflow:lane{i}");
            for h in &hidden {
                l.hidden_in.insert(h.id.clone(), id.clone());
            }
            l.push(Placed {
                id,
                pos: pos2(
                    cx,
                    top + visible.len() as f32 * ROW_STEP + OVERFLOW_SIZE.y / 2.0,
                ),
                size: OVERFLOW_SIZE,
                kind: PlacedKind::Overflow {
                    owner: None,
                    hidden: hidden.iter().map(|h| h.id.clone()).collect(),
                },
            });
        }
        let rows = (visible.len() + usize::from(!hidden.is_empty())).max(1);
        l.lanes.push(Lane {
            title: LANE_TITLES[i].to_string(),
            count,
            rect: Rect::from_min_size(
                pos2(x0 + i as f32 * lane_w - 10.0, top - 42.0),
                vec2(TASK_SIZE.x + 20.0, 34.0 + rows as f32 * ROW_STEP + 20.0),
            ),
            color_idx: i,
        });
    }
    l.reindex();
    l
}

#[cfg(test)]
mod tests {
    use super::*;
    use rteams_services::graph_model::NodeState;

    pub fn task(id: u32, st: Status) -> GraphNode {
        GraphNode {
            id: format!("task:{id}"),
            kind: NodeKind::Task,
            label: format!("t{id}"),
            state: NodeState::Idle,
            status: Some(st),
            owner: None,
            runtime: None,
            role: None,
            online: false,
            current_task: None,
            last_line: None,
            url: None,
        }
    }

    #[test]
    fn graph_kanban_lanes_and_overflow() {
        let mut m = GraphModel::default();
        for i in 0..25 {
            m.nodes.push(task(i, Status::Done));
        }
        m.nodes.push(task(100, Status::Pending));
        let l = layout(&m, true);
        assert_eq!(l.lanes.len(), 5);
        assert_eq!(l.lanes[4].count, 25);
        let ov = l.items.iter().find(|p| p.id == "overflow:lane4").unwrap();
        match &ov.kind {
            PlacedKind::Overflow { hidden, .. } => assert_eq!(hidden.len(), 25 - (MAX_ROWS - 1)),
            _ => panic!(),
        }
        assert!(l.resolve("task:24").unwrap().id == "overflow:lane4");
        assert!(l.get("task:100").is_some());
        // The task in lane 0 sits left of the Done tasks.
        assert!(l.get("task:100").unwrap().pos.x < l.get("task:0").unwrap().pos.x);
        for p in &l.items {
            assert!(p.pos.x.is_finite() && p.pos.y.is_finite());
        }
    }

    #[test]
    fn graph_kanban_hides_tasks_when_toggled() {
        let mut m = GraphModel::default();
        m.nodes.push(task(1, Status::Pending));
        assert!(layout(&m, false).items.is_empty());
    }

    #[test]
    fn graph_kanban_split_overflow() {
        let (v, h) = split_overflow((0..5).collect(), 10);
        assert_eq!((v.len(), h.len()), (5, 0));
        let (v, h) = split_overflow((0..12).collect(), 10);
        assert_eq!((v.len(), h.len()), (9, 3));
        let (v, h) = split_overflow((0..3).collect::<Vec<_>>(), 0);
        assert_eq!((v.len(), h.len()), (0, 3));
    }
}
