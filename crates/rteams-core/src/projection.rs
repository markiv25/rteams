use serde::{Deserialize, Serialize};

use crate::domain::{Member, Message, ProcessEntry, Scope, Task};
use crate::lifecycle::Status;
use crate::state::State;

// ---- Kanban: derived from status, never stored ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    pub subject: String,
    pub owner: Option<String>,
    /// Real status, so a UI can badge needs_fix / blocked cards.
    pub status: Status,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KanbanBoard {
    pub todo: Vec<Card>,
    pub in_progress: Vec<Card>,
    pub review: Vec<Card>,
    pub approved: Vec<Card>,
    pub done: Vec<Card>,
}

/// Archived tasks are hidden. needs_fix sits in `todo`; a blocked task stays in the
/// column of the status it was blocked from. Cards are in creation order.
pub fn kanban(state: &State) -> KanbanBoard {
    let mut b = KanbanBoard::default();
    for t in state.tasks.iter().filter(|t| !t.archived) {
        let effective = match (t.status, t.blocked_from) {
            (Status::Blocked, Some(from)) => from,
            (s, _) => s,
        };
        let col = match effective {
            Status::Pending | Status::NeedsFix | Status::Blocked => &mut b.todo,
            Status::InProgress => &mut b.in_progress,
            Status::Review => &mut b.review,
            Status::Approved => &mut b.approved,
            Status::Done => &mut b.done,
        };
        col.push(Card {
            id: t.id.clone(),
            subject: t.subject.clone(),
            owner: t.owner.clone(),
            status: t.status,
        });
    }
    b
}

// ---- Briefing: what should this member do now (server-derived, no tokens) ----

/// Why a task is in someone's briefing. Declaration order of the actionable
/// variants is their priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// Actionable: you own it and it is in progress.
    Resume,
    /// Actionable: you own it and review requested changes.
    Fix,
    /// Actionable: you are the reviewer.
    Review,
    /// Actionable: you own it and it is approved; mark it done.
    Finish,
    /// Actionable: you own it and it has not started.
    Start,
    /// Awareness: you own it and it is waiting on a reviewer.
    AwaitingReview,
    /// Awareness: you own it and it is blocked.
    Blocked,
    /// Awareness: you own it, it has not started, and a task it depends on is not done.
    WaitingOnDependencies,
}

impl Reason {
    pub fn is_actionable(self) -> bool {
        self < Reason::AwaitingReview
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BriefingItem {
    pub task: String,
    pub subject: String,
    pub status: Status,
    pub reason: Reason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Briefing {
    pub member: String,
    /// Ordered by priority, then creation.
    pub actionable: Vec<BriefingItem>,
    /// Watch-only; ordered by creation.
    pub awareness: Vec<BriefingItem>,
}

fn reason_for(state: &State, t: &Task, member: &str) -> Option<Reason> {
    if t.status == Status::Review
        && (t.reviewer.as_deref() == Some(member) || t.reviewers.iter().any(|r| r == member))
    {
        return Some(Reason::Review);
    }
    if t.owner.as_deref() != Some(member) {
        return None;
    }
    match t.status {
        Status::InProgress => Some(Reason::Resume),
        Status::NeedsFix => Some(Reason::Fix),
        Status::Approved => Some(Reason::Finish),
        Status::Pending if state.unmet_dependencies(t).is_empty() => Some(Reason::Start),
        Status::Pending => Some(Reason::WaitingOnDependencies),
        Status::Review => Some(Reason::AwaitingReview),
        Status::Blocked => Some(Reason::Blocked),
        Status::Done => None,
    }
}

pub fn briefing(state: &State, member: &str) -> Briefing {
    let mut items: Vec<(Reason, u64, BriefingItem)> = state
        .tasks
        .iter()
        .filter(|t| !t.archived)
        .filter_map(|t| {
            let reason = reason_for(state, t, member)?;
            let item = BriefingItem {
                task: t.id.clone(),
                subject: t.subject.clone(),
                status: t.status,
                reason,
            };
            Some((reason, t.created_seq, item))
        })
        .collect();
    items.sort_by_key(|(r, seq, _)| (*r, *seq));
    let (actionable, awareness): (Vec<_>, Vec<_>) =
        items.into_iter().partition(|(r, _, _)| r.is_actionable());
    Briefing {
        member: member.to_string(),
        actionable: actionable.into_iter().map(|(_, _, i)| i).collect(),
        awareness: awareness.into_iter().map(|(_, _, i)| i).collect(),
    }
}

// ---- Lead briefing: team-wide queue for whoever runs the team ----

/// Why a task is in the lead's briefing. Declaration order of the actionable
/// variants is their priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeadReason {
    /// Actionable: a live task nobody owns; assign it.
    Unassigned,
    /// Actionable: parked as blocked; someone has to unblock or reassign it.
    Blocked,
    /// Awareness: in review, waiting on the reviewer.
    InReview,
    /// Awareness: approved, waiting on the owner to mark it done.
    Approved,
}

impl LeadReason {
    pub fn is_actionable(self) -> bool {
        self < LeadReason::InReview
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeadItem {
    pub task: String,
    pub subject: String,
    pub status: Status,
    pub owner: Option<String>,
    pub reviewer: Option<String>,
    pub reason: LeadReason,
}

/// How much open work a member holds. Archived and done tasks do not count.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberLoad {
    pub member: String,
    pub role: String,
    pub in_progress: usize,
    /// pending + needs_fix
    pub todo: usize,
    pub review: usize,
    pub blocked: usize,
}

/// Column sizes of the kanban board.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardCounts {
    pub todo: usize,
    pub in_progress: usize,
    pub review: usize,
    pub approved: usize,
    pub done: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeadBriefing {
    /// Ordered by priority, then creation.
    pub actionable: Vec<LeadItem>,
    /// Watch-only; ordered by creation.
    pub awareness: Vec<LeadItem>,
    pub counts: BoardCounts,
    /// One row per member, in roster order.
    pub load: Vec<MemberLoad>,
}

fn lead_reason_for(t: &Task) -> Option<LeadReason> {
    match (t.status, t.owner.is_some()) {
        (Status::Done, _) => None,
        (_, false) => Some(LeadReason::Unassigned),
        (Status::Blocked, _) => Some(LeadReason::Blocked),
        (Status::Review, _) => Some(LeadReason::InReview),
        (Status::Approved, _) => Some(LeadReason::Approved),
        _ => None,
    }
}

pub fn lead_briefing(state: &State) -> LeadBriefing {
    let mut items: Vec<(LeadReason, u64, LeadItem)> = state
        .tasks
        .iter()
        .filter(|t| !t.archived)
        .filter_map(|t| {
            let reason = lead_reason_for(t)?;
            let item = LeadItem {
                task: t.id.clone(),
                subject: t.subject.clone(),
                status: t.status,
                owner: t.owner.clone(),
                reviewer: t.reviewer.clone(),
                reason,
            };
            Some((reason, t.created_seq, item))
        })
        .collect();
    items.sort_by_key(|(r, seq, _)| (*r, *seq));
    let (actionable, awareness): (Vec<_>, Vec<_>) =
        items.into_iter().partition(|(r, _, _)| r.is_actionable());

    let board = kanban(state);
    let counts = BoardCounts {
        todo: board.todo.len(),
        in_progress: board.in_progress.len(),
        review: board.review.len(),
        approved: board.approved.len(),
        done: board.done.len(),
    };

    let load = state
        .team
        .iter()
        .flat_map(|t| &t.members)
        .filter(|m| !m.removed)
        .map(|m| {
            let mut l = MemberLoad {
                member: m.name.clone(),
                role: m.role.clone(),
                ..MemberLoad::default()
            };
            let owned = state
                .tasks
                .iter()
                .filter(|t| !t.archived && t.owner.as_deref() == Some(m.name.as_str()));
            for t in owned {
                match t.status {
                    Status::InProgress => l.in_progress += 1,
                    Status::Pending | Status::NeedsFix => l.todo += 1,
                    Status::Review => l.review += 1,
                    Status::Blocked => l.blocked += 1,
                    Status::Approved | Status::Done => {}
                }
            }
            l
        })
        .collect();

    LeadBriefing {
        actionable: actionable.into_iter().map(|(_, _, i)| i).collect(),
        awareness: awareness.into_iter().map(|(_, _, i)| i).collect(),
        counts,
        load,
    }
}

// ---- Member briefing: bootstrap context, separate from the work queue ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueSummary {
    pub actionable: usize,
    pub awareness: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberBriefing {
    pub team: String,
    pub you: Member,
    /// Names of members whose role is `lead`.
    pub leads: Vec<String>,
    pub roster: Vec<Member>,
    /// Sizes only; fetch `task_briefing` for the items themselves.
    pub queue: QueueSummary,
    pub processes: Vec<ProcessEntry>,
    /// Working directory of the project, if the lead configured one.
    #[serde(default)]
    pub project_path: Option<String>,
    /// Standing instructions from the lead for everyone.
    #[serde(default)]
    pub lead_prompt: Option<String>,
}

/// `None` when there is no team or `member` is not on it.
pub fn member_briefing(state: &State, member: &str) -> Option<MemberBriefing> {
    let team = state.team.as_ref()?;
    let you = team
        .members
        .iter()
        .find(|m| m.name == member && !m.removed)?
        .clone();
    let q = briefing(state, member);
    Some(MemberBriefing {
        team: team.name.clone(),
        you,
        leads: team
            .members
            .iter()
            .filter(|m| !m.removed && m.role == "lead")
            .map(|m| m.name.clone())
            .collect(),
        roster: team
            .members
            .iter()
            .filter(|m| !m.removed)
            .cloned()
            .collect(),
        queue: QueueSummary {
            actionable: q.actionable.len(),
            awareness: q.awareness.len(),
        },
        processes: process_list(state),
        project_path: team.project_path.clone(),
        lead_prompt: team.lead_prompt.clone(),
    })
}

// ---- Inbox: messages a member should see, after a cursor ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inbox {
    pub member: String,
    pub items: Vec<Message>,
    /// Pass back as `after` to get only newer items. Unchanged if nothing new.
    pub next_cursor: u64,
}

/// Visible to `member`: direct messages to them; broadcasts from others; comments on
/// tasks they own or review, written by others. `after` is exclusive (0 = from the start).
pub fn inbox(state: &State, member: &str, after: u64) -> Inbox {
    let items: Vec<Message> = state
        .messages
        .iter()
        .filter(|m| m.seq > after && m.from != member)
        .filter(|m| match &m.scope {
            Scope::Direct { to } => to == member,
            Scope::Broadcast => true,
            Scope::Task { task } => state.task(task).is_some_and(|t| {
                t.owner.as_deref() == Some(member)
                    || t.reviewer.as_deref() == Some(member)
                    || t.reviewers.iter().any(|r| r == member)
            }),
            Scope::Group { chat } => state
                .group(chat)
                .is_some_and(|g| g.members.iter().any(|x| x == member)),
        })
        .cloned()
        .collect();
    let next_cursor = items.last().map_or(after, |m| m.seq);
    Inbox {
        member: member.to_string(),
        items,
        next_cursor,
    }
}

// ---- Process list ----

/// Running processes in registration order.
pub fn process_list(state: &State) -> Vec<ProcessEntry> {
    state
        .processes
        .iter()
        .filter(|p| p.running)
        .cloned()
        .collect()
}
