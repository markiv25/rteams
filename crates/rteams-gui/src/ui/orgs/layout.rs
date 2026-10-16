//! Deterministic layouts for the organization map: nested containers ("structure") and a tidy
//! tree ("hierarchy"). Pure geometry, no painting.

use eframe::egui::{pos2, vec2, Color32, Pos2, Rect, Vec2};
use rteams_services::orgs::model::RelationKind;
use rteams_services::orgs::projection::{OrgNode, OrgView, TeamSummary};

pub const CARD: Vec2 = vec2(210.0, 66.0);
const PAD: f32 = 16.0;
const HEADER: f32 = 40.0;
const GAP: f32 = 14.0;
const NODE_W: f32 = 214.0;
const NODE_H: f32 = 66.0;
const SLOT: f32 = NODE_W + 28.0;
const LEVEL: f32 = NODE_H + 70.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Nested,
    Hierarchy,
}

/// Identity of a node on the map.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    Root,
    Org(String),
    Group(String, String),
    Team(String),
    Unplaced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    pub teams: usize,
    pub members: usize,
    pub running: usize,
    pub open_tasks: usize,
    pub in_progress: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub key: Key,
    pub rect: Rect,
    pub parent: Option<usize>,
    pub depth: usize,
    pub label: String,
    pub color: Color32,
    pub stats: Stats,
}

impl Node {
    pub fn is_container(&self) -> bool {
        matches!(self.key, Key::Org(_) | Key::Group(..) | Key::Unplaced)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// Pre-order: parents come before their children.
    pub nodes: Vec<Node>,
    /// Parent -> child (hierarchy mode only).
    pub edges: Vec<(usize, usize)>,
    pub bounds: Rect,
}

impl Default for Layout {
    fn default() -> Self {
        Layout {
            nodes: Vec::new(),
            edges: Vec::new(),
            bounds: Rect::from_min_size(Pos2::ZERO, vec2(1.0, 1.0)),
        }
    }
}

/// A relation drawn as an edge between two nodes. `kind` is `None` for runtime message links.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RelEdge {
    pub a: usize,
    pub b: usize,
    pub kind: Option<RelationKind>,
    pub weight: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Overview {
    pub orgs: usize,
    pub groups: usize,
    pub teams: usize,
    pub running: usize,
    pub open_tasks: usize,
    pub agents: usize,
}

pub fn parse_hex(s: &str) -> Color32 {
    let h = s.trim().trim_start_matches('#');
    if h.len() == 6 {
        if let Ok(v) = u32::from_str_radix(h, 16) {
            return Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        }
    }
    crate::theme::ACCENT
}

// ------------------------------------------------------------------ prototype tree

struct Proto {
    key: Key,
    label: String,
    color: Color32,
    stats: Stats,
    children: Vec<Proto>,
}

fn team_proto(t: &TeamSummary, color: Color32) -> Proto {
    Proto {
        key: Key::Team(t.dir.clone()),
        label: t.name.clone(),
        color,
        stats: Stats {
            teams: 1,
            members: t.members,
            running: usize::from(t.running),
            open_tasks: t.open_tasks,
            in_progress: t.in_progress,
        },
        children: Vec::new(),
    }
}

fn org_proto(n: &OrgNode) -> Proto {
    let color = parse_hex(&n.org.color);
    let mut children: Vec<Proto> = n.children.iter().map(org_proto).collect();
    for g in &n.groups {
        let gc = parse_hex(&g.group.color);
        children.push(Proto {
            key: Key::Group(n.org.id.clone(), g.group.id.clone()),
            label: g.group.name.clone(),
            color: gc,
            stats: totals(&g.totals),
            children: g.teams.iter().map(|t| team_proto(t, gc)).collect(),
        });
    }
    children.extend(n.teams.iter().map(|t| team_proto(t, color)));
    Proto {
        key: Key::Org(n.org.id.clone()),
        label: n.org.name.clone(),
        color,
        stats: totals(&n.totals),
        children,
    }
}

fn totals(t: &rteams_services::orgs::projection::Totals) -> Stats {
    Stats {
        teams: t.teams,
        members: t.members,
        running: t.running,
        open_tasks: t.open_tasks,
        in_progress: 0,
    }
}

fn top_level(view: &OrgView) -> Vec<Proto> {
    let mut out: Vec<Proto> = view.roots.iter().map(org_proto).collect();
    if !view.unplaced.is_empty() {
        let color = crate::theme::MUTED;
        let children: Vec<Proto> = view.unplaced.iter().map(|t| team_proto(t, color)).collect();
        let mut stats = Stats::default();
        for c in &children {
            stats.teams += 1;
            stats.members += c.stats.members;
            stats.running += c.stats.running;
            stats.open_tasks += c.stats.open_tasks;
        }
        out.push(Proto {
            key: Key::Unplaced,
            label: "Unassigned Teams".into(),
            color,
            stats,
            children,
        });
    }
    out
}

// ------------------------------------------------------------------ nested

/// Row-wrapped flow: item offsets and the total size.
fn flow(sizes: &[Vec2], max_w: f32) -> (Vec<Vec2>, Vec2) {
    let mut offs = Vec::with_capacity(sizes.len());
    let (mut x, mut y, mut row_h, mut w) = (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32);
    for s in sizes {
        if x > 0.0 && x + s.x > max_w {
            y += row_h + GAP;
            x = 0.0;
            row_h = 0.0;
        }
        offs.push(vec2(x, y));
        x += s.x + GAP;
        w = w.max(x - GAP);
        row_h = row_h.max(s.y);
    }
    (
        offs,
        vec2(w, if sizes.is_empty() { 0.0 } else { y + row_h }),
    )
}

fn flow_width(sizes: &[Vec2]) -> f32 {
    let widest = sizes.iter().map(|s| s.x).fold(0.0, f32::max);
    let area: f32 = sizes.iter().map(|s| s.x * s.y).sum();
    (area.sqrt() * 1.45).max(widest)
}

struct Block<'a> {
    p: &'a Proto,
    size: Vec2,
    kids: Vec<(Vec2, Block<'a>)>,
}

fn measure(p: &Proto) -> Block<'_> {
    if p.children.is_empty() && matches!(p.key, Key::Team(_)) {
        return Block {
            p,
            size: CARD,
            kids: Vec::new(),
        };
    }
    let blocks: Vec<Block> = p.children.iter().map(measure).collect();
    let sizes: Vec<Vec2> = blocks.iter().map(|b| b.size).collect();
    let (offs, inner) = flow(&sizes, flow_width(&sizes));
    let size = vec2(
        (inner.x + 2.0 * PAD).max(170.0),
        HEADER + inner.y.max(14.0) + PAD,
    );
    Block {
        p,
        size,
        kids: offs.into_iter().zip(blocks).collect(),
    }
}

fn emit(b: &Block, at: Pos2, parent: Option<usize>, depth: usize, out: &mut Vec<Node>) {
    let idx = out.len();
    out.push(Node {
        key: b.p.key.clone(),
        rect: Rect::from_min_size(at, b.size),
        parent,
        depth,
        label: b.p.label.clone(),
        color: b.p.color,
        stats: b.p.stats,
    });
    for (off, k) in &b.kids {
        emit(k, at + vec2(PAD, HEADER) + *off, Some(idx), depth + 1, out);
    }
}

fn finish(nodes: Vec<Node>, edges: Vec<(usize, usize)>) -> Layout {
    let mut bounds: Option<Rect> = None;
    for n in &nodes {
        bounds = Some(bounds.map_or(n.rect, |b| b.union(n.rect)));
    }
    Layout {
        nodes,
        edges,
        bounds: bounds.unwrap_or_else(|| Layout::default().bounds),
    }
}

pub fn nested(view: &OrgView) -> Layout {
    let tops = top_level(view);
    let blocks: Vec<Block> = tops.iter().map(measure).collect();
    let sizes: Vec<Vec2> = blocks.iter().map(|b| b.size).collect();
    let (offs, _) = flow(&sizes, flow_width(&sizes).max(900.0));
    let mut nodes = Vec::new();
    for (b, o) in blocks.iter().zip(offs) {
        emit(b, Pos2::ZERO + o, None, 0, &mut nodes);
    }
    finish(nodes, Vec::new())
}

// ------------------------------------------------------------------ hierarchy

fn place(p: &Proto, depth: usize, parent: Option<usize>, slot: &mut f32, lay: &mut Layout) -> f32 {
    let idx = lay.nodes.len();
    lay.nodes.push(Node {
        key: p.key.clone(),
        rect: Rect::ZERO,
        parent,
        depth,
        label: p.label.clone(),
        color: p.color,
        stats: p.stats,
    });
    if let Some(par) = parent {
        lay.edges.push((par, idx));
    }
    let x = if p.children.is_empty() {
        let x = *slot * SLOT;
        *slot += 1.0;
        x
    } else {
        let xs: Vec<f32> = p
            .children
            .iter()
            .map(|c| place(c, depth + 1, Some(idx), slot, lay))
            .collect();
        (xs[0] + xs[xs.len() - 1]) / 2.0
    };
    lay.nodes[idx].rect =
        Rect::from_center_size(pos2(x, depth as f32 * LEVEL), vec2(NODE_W, NODE_H));
    x
}

pub fn hierarchy(view: &OrgView) -> Layout {
    let children = top_level(view);
    if children.is_empty() {
        return Layout::default();
    }
    let mut stats = Stats::default();
    for c in children.iter().filter(|c| matches!(c.key, Key::Org(_))) {
        stats.teams += c.stats.teams;
        stats.members += c.stats.members;
        stats.running += c.stats.running;
        stats.open_tasks += c.stats.open_tasks;
    }
    let root = Proto {
        key: Key::Root,
        label: "All Organizations".into(),
        color: crate::theme::ACCENT,
        stats,
        children,
    };
    let mut lay = Layout {
        nodes: Vec::new(),
        edges: Vec::new(),
        bounds: Rect::ZERO,
    };
    place(&root, 0, None, &mut 0.0, &mut lay);
    finish(lay.nodes, lay.edges)
}

pub fn build(view: &OrgView, mode: Mode) -> Layout {
    match mode {
        Mode::Nested => nested(view),
        Mode::Hierarchy => hierarchy(view),
    }
}

// ------------------------------------------------------------------ queries

impl Layout {
    pub fn find(&self, key: &Key) -> Option<usize> {
        self.nodes.iter().position(|n| &n.key == key)
    }

    /// Deepest / topmost-painted node containing `p` (world space).
    pub fn hit(&self, p: Pos2) -> Option<usize> {
        self.nodes.iter().rposition(|n| n.rect.contains(p))
    }

    /// Smallest container (org / group / unassigned) under `p`.
    pub fn drop_target(&self, p: Pos2) -> Option<usize> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.is_container() && n.rect.contains(p))
            .min_by(|a, b| {
                a.1.rect
                    .area()
                    .partial_cmp(&b.1.rect.area())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
    }

    pub fn is_ancestor(&self, anc: usize, mut i: usize) -> bool {
        while let Some(p) = self.nodes[i].parent {
            if p == anc {
                return true;
            }
            i = p;
        }
        false
    }

    pub fn overview(&self) -> Overview {
        let mut o = Overview::default();
        for n in &self.nodes {
            match &n.key {
                Key::Org(_) => o.orgs += 1,
                Key::Group(..) => o.groups += 1,
                Key::Team(_) => {
                    o.teams += 1;
                    o.running += n.stats.running;
                    o.open_tasks += n.stats.open_tasks;
                    o.agents += n.stats.members;
                }
                _ => {}
            }
        }
        o
    }

    /// Relation + runtime edges resolved against this layout (team dir first, then org id).
    pub fn relation_edges(&self, view: &OrgView) -> Vec<RelEdge> {
        let find = |id: &str| {
            self.find(&Key::Team(id.to_string()))
                .or_else(|| self.find(&Key::Org(id.to_string())))
        };
        let mut out = Vec::new();
        for r in &view.relations {
            if let (Some(a), Some(b)) = (find(&r.relation.source), find(&r.relation.target)) {
                if a != b {
                    out.push(RelEdge {
                        a,
                        b,
                        kind: Some(r.relation.kind),
                        weight: r.weight,
                    });
                }
            }
        }
        for (x, y, n) in &view.runtime {
            if let (Some(a), Some(b)) = (find(x), find(y)) {
                if a != b {
                    out.push(RelEdge {
                        a,
                        b,
                        kind: None,
                        weight: *n,
                    });
                }
            }
        }
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rteams_services::orgs::model::OrgMap;
    use std::collections::BTreeMap;

    /// 3 orgs (one nested), groups, direct teams and unplaced teams.
    pub fn synthetic() -> OrgView {
        let mut map = OrgMap::default();
        let a = map.create_org("Alpha", None).unwrap();
        let b = map.create_org("Beta", Some(&a)).unwrap();
        let c = map.create_org("Gamma", None).unwrap();
        let g1 = map.create_group(&a, "Squad One").unwrap();
        let g2 = map.create_group(&c, "Squad Two").unwrap();
        let mut teams = BTreeMap::new();
        for i in 0..14 {
            let d = format!("team{i}");
            match i % 5 {
                0 => map.place_team(&d, &a, Some(&g1)).unwrap(),
                1 => map.place_team(&d, &b, None).unwrap(),
                2 => map.place_team(&d, &c, Some(&g2)).unwrap(),
                3 => map.place_team(&d, &c, None).unwrap(),
                _ => {}
            }
            teams.insert(
                d.clone(),
                TeamSummary {
                    dir: d.clone(),
                    name: format!("Team {i}"),
                    members: i % 4 + 1,
                    open_tasks: i,
                    running: i % 3 == 0,
                    ..Default::default()
                },
            );
        }
        map.add_relation("team0", "team1", RelationKind::DependsOn)
            .unwrap();
        let mut counts = BTreeMap::new();
        counts.insert(("team2".to_string(), "team3".to_string()), 4);
        OrgView::build(&map, &teams, &counts)
    }

    pub fn view() -> OrgView {
        synthetic()
    }

    #[test]
    fn nested_no_overlap_and_contained() {
        let l = nested(&view());
        assert!(l.nodes.len() > 14);
        for (i, a) in l.nodes.iter().enumerate() {
            if let Some(p) = a.parent {
                assert!(
                    l.nodes[p].rect.contains_rect(a.rect),
                    "{} outside parent",
                    a.label
                );
            }
            for (j, b) in l.nodes.iter().enumerate().skip(i + 1) {
                if l.is_ancestor(i, j) || l.is_ancestor(j, i) {
                    continue;
                }
                assert!(
                    !a.rect.intersects(b.rect),
                    "{} overlaps {}",
                    a.label,
                    b.label
                );
            }
        }
    }

    #[test]
    fn hierarchy_no_overlap_edges_and_root() {
        let l = hierarchy(&view());
        assert_eq!(l.nodes[0].key, Key::Root);
        assert_eq!(l.edges.len(), l.nodes.len() - 1);
        for (i, a) in l.nodes.iter().enumerate() {
            for b in l.nodes.iter().skip(i + 1) {
                assert!(
                    !a.rect.intersects(b.rect),
                    "{} overlaps {}",
                    a.label,
                    b.label
                );
            }
        }
        for &(p, c) in &l.edges {
            assert!(l.nodes[p].rect.center().y < l.nodes[c].rect.center().y);
        }
    }

    #[test]
    fn layouts_are_deterministic() {
        for m in [Mode::Nested, Mode::Hierarchy] {
            assert_eq!(build(&view(), m), build(&view(), m));
        }
    }

    #[test]
    fn empty_view_is_finite() {
        for m in [Mode::Nested, Mode::Hierarchy] {
            let l = build(&OrgView::default(), m);
            assert!(l.nodes.is_empty() && l.bounds.width() > 0.0);
        }
    }

    #[test]
    fn relations_resolve_and_overview_counts() {
        let v = view();
        let l = nested(&v);
        let rels = l.relation_edges(&v);
        assert_eq!(rels.len(), 2);
        assert!(rels.iter().any(|r| r.kind.is_none() && r.weight == 4));
        let o = l.overview();
        assert_eq!((o.orgs, o.groups, o.teams), (3, 2, 14));
        assert_eq!(
            l.drop_target(
                l.nodes[l.find(&Key::Team("team0".into())).unwrap()]
                    .rect
                    .center()
            )
            .map(|i| l.nodes[i].key.clone()),
            Some(Key::Group("alpha".into(), "squad-one".into()))
        );
    }
}
