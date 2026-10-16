use serde::{Deserialize, Serialize};

use crate::domain::{Message, ProcessEntry, Scope, Task};
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
    if t.status == Status::Review && t.reviewer.as_deref() == Some(member) {
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
                t.owner.as_deref() == Some(member) || t.reviewer.as_deref() == Some(member)
            }),
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
