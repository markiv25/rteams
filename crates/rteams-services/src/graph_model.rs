//! graph_model: the pure data behind the team graph (agent-graph).
//!
//! `build` folds a `State` plus the recent event log and the launcher's per-member runtime
//! facts into nodes (lead, members, tasks, processes), edges (parent-child, ownership,
//! blocking, related, review, message) and short-lived message `Flow`s that the canvas animates
//! as particles. Stable member slots (ring / sector) are kept in `graph-layout.json` so a
//! member never jumps when others join or leave. No egui here.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use rteams_core::{Event, EventKind, State, Status};
use serde::{Deserialize, Serialize};

/// What a node represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeKind {
    Lead,
    Member,
    Task,
    Process,
}

/// Visual state of a node (original: idle/active/thinking/tool_calling/waiting/complete/error).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeState {
    Idle,
    Active,
    Thinking,
    ToolCalling,
    Waiting,
    Complete,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeKind {
    ParentChild,
    Ownership,
    Blocking,
    Related,
    Review,
    Message,
}

/// What a flow (particle) represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FlowKind {
    Message,
    Comment,
    Assign,
    ReviewRequest,
    ReviewResponse,
}

/// Launcher facts for one member, supplied by the caller (the services crate cannot see the
/// launcher).
#[derive(Debug, Clone, Default)]
pub struct MemberRuntime {
    pub name: String,
    pub running: bool,
    pub failed: bool,
    pub rate_limited: bool,
    pub runtime: Option<String>,
    pub last_line: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphNode {
    /// Member name, `task:<id>` or `proc:<pid>`.
    pub id: String,
    pub kind: NodeKind,
    pub label: String,
    pub state: NodeState,
    /// Task nodes only.
    pub status: Option<Status>,
    /// Task owner / process owner.
    pub owner: Option<String>,
    pub runtime: Option<String>,
    pub role: Option<String>,
    /// Members: launched and running right now.
    pub online: bool,
    /// Members: subject of the first in-progress task.
    pub current_task: Option<String>,
    pub last_line: Option<String>,
    /// Processes: the url to open.
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}

/// One recent event between two nodes; drawn as a particle travelling along the edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flow {
    pub seq: u64,
    pub ts: i64,
    pub from: String,
    pub to: String,
    pub kind: FlowKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityItem {
    pub seq: u64,
    pub ts: i64,
    pub actor: String,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct GraphModel {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub flows: Vec<Flow>,
    /// Newest first.
    pub activity: Vec<ActivityItem>,
}

/// How long an event counts as recent for flows.
pub const FLOW_WINDOW_MS: i64 = 20_000;
const MAX_FLOWS: usize = 24;
const MAX_ACTIVITY: usize = 40;

pub fn task_node_id(task: &str) -> String {
    format!("task:{task}")
}

impl GraphModel {
    pub fn node(&self, id: &str) -> Option<&GraphNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn lead(&self) -> Option<&GraphNode> {
        self.nodes.iter().find(|n| n.kind == NodeKind::Lead)
    }

    pub fn members(&self) -> impl Iterator<Item = &GraphNode> {
        self.nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Lead | NodeKind::Member))
    }

    pub fn tasks(&self) -> impl Iterator<Item = &GraphNode> {
        self.nodes.iter().filter(|n| n.kind == NodeKind::Task)
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// Build the graph. `actor` is the human (excluded from the roster, like `View::agent_members`),
/// unless the human holds the lead role: the lead is always drawn, in the middle.
pub fn build(
    state: &State,
    events: &[Event],
    runtime: &[MemberRuntime],
    actor: Option<&str>,
    now_ms: i64,
) -> GraphModel {
    let mut g = GraphModel::default();
    let Some(team) = state.team.as_ref() else {
        return g;
    };
    let rt: HashMap<&str, &MemberRuntime> = runtime.iter().map(|r| (r.name.as_str(), r)).collect();
    let roster: Vec<_> = team
        .members
        .iter()
        .filter(|m| !m.removed && (Some(m.name.as_str()) != actor || m.role == "lead"))
        .collect();
    let lead_name = roster
        .iter()
        .find(|m| m.role == "lead")
        .map(|m| m.name.clone());
    let reviewing: HashSet<&str> = state
        .tasks
        .iter()
        .filter(|t| !t.archived && t.status == Status::Review)
        .filter_map(|t| t.reviewer.as_deref())
        .collect();

    // Lead first so it is the stable center.
    let mut ordered = roster.clone();
    ordered.sort_by_key(|m| m.role != "lead");
    for m in ordered {
        let r = rt.get(m.name.as_str());
        let working = state.tasks.iter().find(|t| {
            !t.archived && t.status == Status::InProgress && t.owner.as_deref() == Some(&m.name)
        });
        let running = r.is_some_and(|r| r.running);
        let state_ = if r.is_some_and(|r| r.failed || r.rate_limited) {
            NodeState::Error
        } else if running && working.is_some() {
            NodeState::Active
        } else if running && reviewing.contains(m.name.as_str()) {
            NodeState::Waiting
        } else {
            NodeState::Idle
        };
        g.nodes.push(GraphNode {
            id: m.name.clone(),
            kind: if m.role == "lead" {
                NodeKind::Lead
            } else {
                NodeKind::Member
            },
            label: m.name.clone(),
            state: state_,
            status: None,
            owner: None,
            runtime: r
                .and_then(|r| r.runtime.clone())
                .or_else(|| m.runtime.clone()),
            role: Some(m.role.clone()),
            online: running,
            current_task: working.map(|t| t.subject.clone()),
            last_line: r.and_then(|r| r.last_line.clone()),
            url: None,
        });
    }
    let member_ids: HashSet<String> = g.nodes.iter().map(|n| n.id.clone()).collect();

    if let Some(lead) = &lead_name {
        for n in g.nodes.iter().filter(|n| n.kind == NodeKind::Member) {
            g.edges.push(GraphEdge {
                from: lead.clone(),
                to: n.id.clone(),
                kind: EdgeKind::ParentChild,
            });
        }
    }

    let live: Vec<_> = state.tasks.iter().filter(|t| !t.archived).collect();
    let task_ids: HashSet<&str> = live.iter().map(|t| t.id.as_str()).collect();
    for t in &live {
        let st = match t.status {
            Status::Done | Status::Approved => NodeState::Complete,
            Status::NeedsFix => NodeState::Error,
            Status::Blocked => NodeState::Waiting,
            Status::InProgress => NodeState::Active,
            Status::Review => NodeState::Waiting,
            Status::Pending => NodeState::Idle,
        };
        let st = if t.clarification.is_some() {
            NodeState::Waiting
        } else {
            st
        };
        g.nodes.push(GraphNode {
            id: task_node_id(&t.id),
            kind: NodeKind::Task,
            label: t.subject.clone(),
            state: st,
            status: Some(t.status),
            owner: t.owner.clone().filter(|o| member_ids.contains(o)),
            runtime: None,
            role: None,
            online: false,
            current_task: None,
            last_line: None,
            url: None,
        });
        if let Some(o) = t.owner.as_ref().filter(|o| member_ids.contains(*o)) {
            g.edges.push(GraphEdge {
                from: o.clone(),
                to: task_node_id(&t.id),
                kind: EdgeKind::Ownership,
            });
        }
        if t.status == Status::Review {
            if let Some(r) = t.reviewer.as_ref().filter(|r| member_ids.contains(*r)) {
                g.edges.push(GraphEdge {
                    from: r.clone(),
                    to: task_node_id(&t.id),
                    kind: EdgeKind::Review,
                });
            }
        }
        for b in t
            .blocked_by
            .iter()
            .filter(|b| task_ids.contains(b.as_str()))
        {
            g.edges.push(GraphEdge {
                from: task_node_id(b),
                to: task_node_id(&t.id),
                kind: EdgeKind::Blocking,
            });
        }
        for r in t
            .related
            .iter()
            .filter(|r| task_ids.contains(r.as_str()) && **r > t.id)
        {
            g.edges.push(GraphEdge {
                from: task_node_id(&t.id),
                to: task_node_id(r),
                kind: EdgeKind::Related,
            });
        }
    }

    for p in state.processes.iter().filter(|p| p.running) {
        let id = format!("proc:{}", p.pid);
        g.nodes.push(GraphNode {
            id: id.clone(),
            kind: NodeKind::Process,
            label: p.label.clone(),
            state: NodeState::Active,
            status: None,
            owner: Some(p.owner.clone()),
            runtime: None,
            role: None,
            online: true,
            current_task: None,
            last_line: p.command.clone(),
            url: p.url.clone(),
        });
        if member_ids.contains(&p.owner) {
            g.edges.push(GraphEdge {
                from: p.owner.clone(),
                to: id,
                kind: EdgeKind::Ownership,
            });
        }
    }

    // Flows and activity from the event tail.
    let node_ids: HashSet<String> = g.nodes.iter().map(|n| n.id.clone()).collect();
    let task_owner = |id: &str| -> Option<String> {
        state
            .task(id)
            .and_then(|t| t.owner.clone())
            .filter(|o| member_ids.contains(o))
    };
    let mut flows: Vec<Flow> = Vec::new();
    for e in events.iter().rev().take(400) {
        let mut push = |from: String, to: String, kind: FlowKind| {
            if from != to && node_ids.contains(&from) && node_ids.contains(&to) {
                flows.push(Flow {
                    seq: e.seq,
                    ts: e.ts,
                    from,
                    to,
                    kind,
                });
            }
        };
        let recent = now_ms - e.ts <= FLOW_WINDOW_MS;
        let a = e.actor.clone();
        let lead = lead_name.clone().unwrap_or_default();
        match &e.kind {
            EventKind::MessageSent { to, .. } if recent => match to {
                Some(t) => push(a, t.clone(), FlowKind::Message),
                None => {
                    for m in member_ids.iter().filter(|m| **m != a) {
                        push(a.clone(), m.clone(), FlowKind::Message);
                    }
                }
            },
            EventKind::TaskCommented { task, .. } if recent => {
                push(a, task_node_id(task), FlowKind::Comment)
            }
            EventKind::TaskAssigned { task, .. } if recent => {
                push(a, task_node_id(task), FlowKind::Assign)
            }
            EventKind::ReviewRequested { task, reviewer } if recent => {
                push(a, reviewer.clone(), FlowKind::ReviewRequest);
                push(
                    reviewer.clone(),
                    task_node_id(task),
                    FlowKind::ReviewRequest,
                );
            }
            EventKind::ReviewApproved { task, .. }
            | EventKind::ReviewChangesRequested { task, .. }
                if recent =>
            {
                let to = task_owner(task).unwrap_or(lead);
                push(a, to, FlowKind::ReviewResponse)
            }
            _ => {}
        }
        if flows.len() >= MAX_FLOWS {
            break;
        }
    }
    flows.truncate(MAX_FLOWS);
    g.flows = flows;

    // Message edges for the pairs that talked recently.
    let mut seen = HashSet::new();
    for f in g.flows.iter().filter(|f| f.kind == FlowKind::Message) {
        let key = if f.from < f.to {
            (f.from.clone(), f.to.clone())
        } else {
            (f.to.clone(), f.from.clone())
        };
        if seen.insert(key) {
            g.edges.push(GraphEdge {
                from: f.from.clone(),
                to: f.to.clone(),
                kind: EdgeKind::Message,
            });
        }
    }

    for e in events.iter().rev() {
        if g.activity.len() >= MAX_ACTIVITY {
            break;
        }
        if let Some(text) = describe(&e.kind, state) {
            g.activity.push(ActivityItem {
                seq: e.seq,
                ts: e.ts,
                actor: e.actor.clone(),
                text,
            });
        }
    }
    g
}

/// Human label of a status.
pub fn status_label(s: Status) -> &'static str {
    match s {
        Status::Pending => "pending",
        Status::InProgress => "in progress",
        Status::Review => "review",
        Status::Approved => "approved",
        Status::Done => "done",
        Status::NeedsFix => "needs fix",
        Status::Blocked => "blocked",
    }
}

fn subject(state: &State, id: &str) -> String {
    state
        .task(id)
        .map(|t| format!("#{id} {}", t.subject))
        .unwrap_or_else(|| format!("#{id}"))
}

fn describe(k: &EventKind, s: &State) -> Option<String> {
    Some(match k {
        EventKind::TaskCreated { task, .. } => format!("created {}", subject(s, task)),
        EventKind::TaskAssigned { task, owner } => match owner {
            Some(o) => format!("assigned {} to {o}", subject(s, task)),
            None => format!("unassigned {}", subject(s, task)),
        },
        EventKind::TaskTransitioned { task, to, .. } => {
            format!("{} -> {}", subject(s, task), status_label(*to))
        }
        EventKind::TaskCommented { task, .. } => format!("commented on {}", subject(s, task)),
        EventKind::ReviewRequested { task, reviewer } => {
            format!("requested review of {} from {reviewer}", subject(s, task))
        }
        EventKind::ReviewApproved { task, .. } => format!("approved {}", subject(s, task)),
        EventKind::ReviewChangesRequested { task, .. } => {
            format!("requested changes on {}", subject(s, task))
        }
        EventKind::TaskBlocked { task, .. } => format!("blocked {}", subject(s, task)),
        EventKind::MessageSent { to, .. } => match to {
            Some(t) => format!("messaged {t}"),
            None => "messaged the team".to_string(),
        },
        EventKind::MemberAdded { name, .. } => format!("added {name}"),
        _ => return None,
    })
}

// ---- stable member slots ----

/// A member's place around the lead: ring 0 is closest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    pub ring: u32,
    pub sector: u32,
}

/// Sectors on `ring` (6 on the first, 4 more on every further ring).
pub fn ring_capacity(ring: u32) -> u32 {
    6 + 4 * ring
}

/// Give every name in `members` a slot, keeping existing ones; slots of names that are gone
/// are freed. Returns whether anything changed.
pub fn assign_slots(slots: &mut BTreeMap<String, Slot>, members: &[String]) -> bool {
    let before = slots.clone();
    let keep: HashSet<&String> = members.iter().collect();
    slots.retain(|k, _| keep.contains(k));
    let mut taken: HashSet<(u32, u32)> = slots.values().map(|s| (s.ring, s.sector)).collect();
    for m in members {
        if slots.contains_key(m) {
            continue;
        }
        let mut ring = 0;
        'find: loop {
            let cap = ring_capacity(ring);
            // Spread over the ring: stride by a coprime-ish step so the first few sit apart.
            for i in 0..cap {
                let sector = (i * 5) % cap;
                let sector = if cap.is_multiple_of(5) { i } else { sector };
                if taken.insert((ring, sector)) {
                    slots.insert(m.clone(), Slot { ring, sector });
                    break 'find;
                }
            }
            ring += 1;
        }
    }
    *slots != before
}

pub fn slots_path(team_dir: &Path) -> std::path::PathBuf {
    team_dir.join("graph-layout.json")
}

pub fn load_slots(path: &Path) -> BTreeMap<String, Slot> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_slots(path: &Path, slots: &BTreeMap<String, Slot>) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(slots).map_err(std::io::Error::other)?;
    crate::atomic::write(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rteams_core::{Member, Task, Team};

    fn member(name: &str, role: &str) -> Member {
        Member {
            name: name.into(),
            role: role.into(),
            prompt: None,
            model: None,
            removed: false,
            runtime: None,
            worktree: false,
            icon: None,
        }
    }

    fn task(id: &str, owner: Option<&str>, status: Status, blocked_by: &[&str]) -> Task {
        Task {
            id: id.into(),
            subject: format!("subject {id}"),
            description: String::new(),
            owner: owner.map(String::from),
            status,
            blocked_by: blocked_by.iter().map(|s| s.to_string()).collect(),
            reviewer: None,
            blocked_from: None,
            blocked_reason: None,
            archived: false,
            created_seq: id.parse().unwrap_or(0),
            clarification: None,
            attachments: vec![],
            related: vec![],
            review_started: false,
        }
    }

    fn fixture() -> State {
        let mut s = State::default();
        s.team = Some(Team {
            name: "t".into(),
            members: vec![
                member("ada", "dev"),
                member("boss", "lead"),
                member("bob", "dev"),
            ],
            project_path: None,
            lead_prompt: None,
            deleted: false,
            column_order: Default::default(),
            schedules: vec![],
        });
        s.tasks = vec![
            task("1", Some("ada"), Status::InProgress, &[]),
            task("2", Some("bob"), Status::Pending, &["1"]),
            task("3", None, Status::Done, &[]),
        ];
        s
    }

    fn ev(seq: u64, ts: i64, actor: &str, kind: EventKind) -> Event {
        Event {
            seq,
            ts,
            actor: actor.into(),
            kind,
        }
    }

    #[test]
    fn graph_model_nodes_and_edges() {
        let rt = [MemberRuntime {
            name: "ada".into(),
            running: true,
            ..Default::default()
        }];
        let g = build(&fixture(), &[], &rt, None, 0);
        assert_eq!(g.lead().unwrap().id, "boss");
        assert_eq!(g.nodes[0].id, "boss", "lead is first");
        assert_eq!(g.members().count(), 3);
        assert_eq!(g.tasks().count(), 3);
        assert_eq!(g.node("ada").unwrap().state, NodeState::Active);
        assert_eq!(g.node("bob").unwrap().state, NodeState::Idle);
        assert_eq!(g.node("task:3").unwrap().state, NodeState::Complete);
        let has = |f: &str, t: &str, k| {
            g.edges
                .iter()
                .any(|e| e.from == f && e.to == t && e.kind == k)
        };
        assert!(has("boss", "ada", EdgeKind::ParentChild));
        assert!(has("ada", "task:1", EdgeKind::Ownership));
        assert!(has("task:1", "task:2", EdgeKind::Blocking));
        assert!(!g.edges.iter().any(|e| e.to == "task:3"));
    }

    #[test]
    fn graph_model_actor_excluded_and_no_team() {
        let g = build(&fixture(), &[], &[], Some("bob"), 0);
        assert!(g.node("bob").is_none());
        assert!(g.node("task:2").unwrap().owner.is_none());
        assert!(build(&State::default(), &[], &[], None, 0).is_empty());
    }

    #[test]
    fn graph_model_flows_are_recent_only() {
        let evs = vec![
            ev(
                1,
                1_000,
                "boss",
                EventKind::MessageSent {
                    to: Some("ada".into()),
                    text: "x".into(),
                    mode: None,
                    from_team: None,
                },
            ),
            ev(
                2,
                99_000,
                "ada",
                EventKind::TaskCommented {
                    task: "1".into(),
                    text: "y".into(),
                },
            ),
            ev(
                3,
                99_500,
                "boss",
                EventKind::MessageSent {
                    to: None,
                    text: "all".into(),
                    mode: None,
                    from_team: None,
                },
            ),
            ev(
                4,
                99_900,
                "boss",
                EventKind::ReviewRequested {
                    task: "1".into(),
                    reviewer: "bob".into(),
                },
            ),
        ];
        let g = build(&fixture(), &evs, &[], None, 100_000);
        assert!(!g.flows.iter().any(|f| f.seq == 1));
        assert!(g
            .flows
            .iter()
            .any(|f| f.kind == FlowKind::Comment && f.to == "task:1"));
        assert_eq!(g.flows.iter().filter(|f| f.seq == 3).count(), 2);
        assert!(g
            .flows
            .iter()
            .any(|f| f.kind == FlowKind::ReviewRequest && f.to == "bob"));
        assert!(g.edges.iter().any(|e| e.kind == EdgeKind::Message));
        assert_eq!(g.activity.first().unwrap().seq, 4);
    }

    #[test]
    fn graph_model_slots_are_stable() {
        let mut slots = BTreeMap::new();
        let names: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert!(assign_slots(&mut slots, &names));
        let a = slots["a"];
        let c = slots["c"];
        let mut more = names.clone();
        more.push("d".into());
        assert!(assign_slots(&mut slots, &more));
        assert_eq!(slots["a"], a);
        assert_eq!(slots["c"], c);
        let uniq: HashSet<_> = slots.values().map(|s| (s.ring, s.sector)).collect();
        assert_eq!(uniq.len(), 4);
        // removal frees the slot, others stay
        let b = slots["b"];
        assert!(assign_slots(
            &mut slots,
            &["a".into(), "c".into(), "d".into()]
        ));
        assert!(!slots.contains_key("b"));
        assert_eq!(slots["a"], a);
        let _ = b;
        assert!(!assign_slots(
            &mut slots,
            &["a".into(), "c".into(), "d".into()]
        ));
    }

    #[test]
    fn graph_model_many_members_spill_to_rings() {
        let mut slots = BTreeMap::new();
        let names: Vec<String> = (0..20).map(|i| format!("m{i}")).collect();
        assign_slots(&mut slots, &names);
        let uniq: HashSet<_> = slots.values().map(|s| (s.ring, s.sector)).collect();
        assert_eq!(uniq.len(), 20);
        assert!(slots.values().any(|s| s.ring >= 1));
        assert!(slots.values().all(|s| s.sector < ring_capacity(s.ring)));
    }

    #[test]
    fn graph_model_slots_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let p = slots_path(d.path());
        assert!(load_slots(&p).is_empty());
        let mut s = BTreeMap::new();
        assign_slots(&mut s, &["x".into(), "y".into()]);
        save_slots(&p, &s).unwrap();
        assert_eq!(load_slots(&p), s);
    }
}
