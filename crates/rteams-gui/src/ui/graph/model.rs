//! Layout data types and the UI state of the graph screen. A `Layout` places the nodes of a
//! `rteams_services::graph_model::GraphModel` in world space; the layout modules fill it.

use std::collections::{BTreeMap, HashMap};

use eframe::egui::{vec2, Pos2, Rect, Vec2};
use rteams_services::graph_model::{GraphModel, GraphNode, NodeKind, Slot};

use super::camera::Camera;

pub const LEAD_SIZE: Vec2 = vec2(280.0, 100.0);
pub const MEMBER_SIZE: Vec2 = vec2(236.0, 84.0);
pub const TASK_SIZE: Vec2 = vec2(250.0, 60.0);
pub const PROC_SIZE: Vec2 = vec2(190.0, 36.0);
pub const OVERFLOW_SIZE: Vec2 = vec2(130.0, 30.0);

pub fn node_size(n: &GraphNode) -> Vec2 {
    match n.kind {
        NodeKind::Lead => LEAD_SIZE,
        NodeKind::Member => MEMBER_SIZE,
        NodeKind::Task => TASK_SIZE,
        NodeKind::Process => PROC_SIZE,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LayoutMode {
    /// Members around the lead, their tasks hanging under them.
    #[default]
    Slots,
    /// Tasks in status lanes.
    Kanban,
    /// Slots as the start, then force-directed relaxation.
    Force,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlacedKind {
    Node,
    /// "+N more" stack standing in for `hidden` task ids.
    Overflow {
        owner: Option<String>,
        hidden: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    /// Node id (or `overflow:<owner>` for stacks).
    pub id: String,
    /// Center, world space.
    pub pos: Pos2,
    pub size: Vec2,
    pub kind: PlacedKind,
}

impl Placed {
    pub fn rect(&self) -> Rect {
        Rect::from_center_size(self.pos, self.size)
    }
}

/// A kanban lane header.
#[derive(Debug, Clone, PartialEq)]
pub struct Lane {
    pub title: String,
    pub count: usize,
    pub rect: Rect,
    pub color_idx: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub items: Vec<Placed>,
    pub lanes: Vec<Lane>,
    /// Task id -> id of the overflow stack that hides it.
    pub hidden_in: HashMap<String, String>,
    index: HashMap<String, usize>,
}

impl Layout {
    pub fn push(&mut self, p: Placed) {
        self.index.insert(p.id.clone(), self.items.len());
        self.items.push(p);
    }

    pub fn reindex(&mut self) {
        self.index = self
            .items
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id.clone(), i))
            .collect();
    }

    pub fn get(&self, id: &str) -> Option<&Placed> {
        self.index.get(id).map(|&i| &self.items[i])
    }

    /// Placed item standing for node `id`: itself, or the overflow stack hiding it.
    pub fn resolve(&self, id: &str) -> Option<&Placed> {
        self.get(id)
            .or_else(|| self.hidden_in.get(id).and_then(|o| self.get(o)))
    }

    pub fn bounds(&self) -> Rect {
        let mut it = self.items.iter().map(|p| p.rect());
        let Some(first) = it.next() else {
            return Rect::from_center_size(Pos2::ZERO, vec2(400.0, 300.0));
        };
        let mut r = it.fold(first, |a, b| a.union(b));
        for l in &self.lanes {
            r = r.union(l.rect);
        }
        r
    }
}

/// Compute the layout for `mode`.
pub fn build_layout(
    model: &GraphModel,
    slots: &BTreeMap<String, Slot>,
    mode: LayoutMode,
    show_tasks: bool,
) -> Layout {
    match mode {
        LayoutMode::Slots => super::layout_slots::layout(model, slots, show_tasks),
        LayoutMode::Kanban => super::layout_kanban::layout(model, show_tasks),
        LayoutMode::Force => {
            let mut l = super::layout_slots::layout(model, slots, show_tasks);
            super::force::relax_layout(&mut l, model);
            l
        }
    }
}

/// What a click on the canvas opened.
#[derive(Debug, Clone, PartialEq)]
pub enum Popover {
    /// Blocking edge between two task node ids, anchored at a screen position.
    Blocking { from: String, to: String, at: Pos2 },
    /// Hidden tasks behind an overflow stack.
    Overflow { id: String, at: Pos2 },
}

/// Persisted toggles (`<home>/graph-ui.json`).
#[derive(Debug, Clone)]
pub struct Prefs {
    pub mode: LayoutMode,
    pub show_tasks: bool,
    pub animate: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            mode: LayoutMode::Slots,
            show_tasks: true,
            animate: true,
        }
    }
}

impl LayoutMode {
    pub fn as_str(self) -> &'static str {
        match self {
            LayoutMode::Slots => "slots",
            LayoutMode::Kanban => "kanban",
            LayoutMode::Force => "force",
        }
    }

    pub fn parse(s: &str) -> Option<LayoutMode> {
        match s {
            "slots" => Some(LayoutMode::Slots),
            "kanban" => Some(LayoutMode::Kanban),
            "force" => Some(LayoutMode::Force),
            _ => None,
        }
    }
}

impl Prefs {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"mode": self.mode.as_str(), "show_tasks": self.show_tasks, "animate": self.animate})
    }

    /// Missing or invalid fields keep their defaults.
    pub fn from_json(v: &serde_json::Value) -> Prefs {
        let d = Prefs::default();
        Prefs {
            mode: v
                .get("mode")
                .and_then(|m| m.as_str())
                .and_then(LayoutMode::parse)
                .unwrap_or(d.mode),
            show_tasks: v
                .get("show_tasks")
                .and_then(|m| m.as_bool())
                .unwrap_or(d.show_tasks),
            animate: v
                .get("animate")
                .and_then(|m| m.as_bool())
                .unwrap_or(d.animate),
        }
    }
}

/// Everything the graph screen remembers between frames.
pub struct GraphUi {
    pub prefs: Prefs,
    pub cam: Camera,
    pub want_fit: bool,
    /// Viewport size at the last fit; a resize triggers a new fit.
    pub last_size: Vec2,
    pub popover: Option<Popover>,
}

impl Default for GraphUi {
    fn default() -> Self {
        GraphUi {
            prefs: Prefs::default(),
            cam: Camera::default(),
            want_fit: true,
            last_size: Vec2::ZERO,
            popover: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_prefs_roundtrip_and_defaults() {
        let p = Prefs {
            mode: LayoutMode::Kanban,
            show_tasks: false,
            animate: false,
        };
        let back = Prefs::from_json(&p.to_json());
        assert_eq!(
            (back.mode, back.show_tasks, back.animate),
            (LayoutMode::Kanban, false, false)
        );
        let d = Prefs::from_json(&serde_json::json!({"mode": "bogus"}));
        assert_eq!(
            (d.mode, d.show_tasks, d.animate),
            (LayoutMode::Slots, true, true)
        );
    }

    #[test]
    fn graph_layout_bounds_empty_is_finite() {
        let b = Layout::default().bounds();
        assert!(b.width() > 0.0 && b.height() > 0.0);
    }
}
