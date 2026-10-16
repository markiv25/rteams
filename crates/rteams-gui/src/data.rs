//! Read adapter over the event log. OWNER: vector.
//!
//! `Data` opens one team's store read-only (never creates files, never appends) and tails the
//! log: call `refresh()` on a poll, then read. Reads use what the last `refresh()` loaded, so a
//! frame sees one consistent view. Everything returned is a plain, owned, renderable struct.
//!
//! Stable signatures (echo/scout rely on them): `open`, `refresh`, `state`, `board`.
//! Added: `seq`, `team_name`, `snapshot`, `task_detail`, `activity`.

use std::path::PathBuf;

use rteams_core::{
    kanban, Card, Event, EventKind, KanbanBoard, Scope, State, Status,
};
use rteams_store::FileStore;

/// Read-only view of one team's log. Never appends.
pub struct Data {
    store: FileStore,
    /// Seq of the last event seen by `refresh()`; the cursor. 0 = nothing yet.
    cursor: u64,
    /// False until the first `refresh()`, which always reports a change.
    primed: bool,
}

impl Data {
    /// Open `<home>/<team>` read-only and fold the existing log. A missing team is an error.
    pub fn open(home: PathBuf, team: &str) -> Result<Self, String> {
        let store = FileStore::open_read_only(&home, team).map_err(|e| e.to_string())?;
        Ok(Data {
            store,
            cursor: 0,
            primed: false,
        })
    }

    /// Catch up with other writers. `Ok(true)` when new events arrived since the last call
    /// (so the UI knows to repaint); the first call after `open` reports `true`.
    pub fn refresh(&mut self) -> Result<bool, String> {
        self.store.refresh().map_err(|e| e.to_string())?;
        let last = self.seq();
        let changed = !self.primed || last != self.cursor;
        self.primed = true;
        self.cursor = last;
        Ok(changed)
    }

    /// Seq of the newest event loaded (0 for an empty log).
    pub fn seq(&self) -> u64 {
        self.store.loaded_events().last().map_or(0, |e| e.seq)
    }

    /// Current state as of the last `refresh`.
    pub fn state(&mut self) -> Result<&State, String> {
        Ok(self.store.loaded_state())
    }

    /// Kanban projection (archived tasks hidden) as of the last `refresh`.
    pub fn board(&mut self) -> Result<KanbanBoard, String> {
        Ok(kanban(self.store.loaded_state()))
    }

    pub fn team_name(&self) -> Option<String> {
        self.store.loaded_state().team.as_ref().map(|t| t.name.clone())
    }

    /// Board with everything a card needs to render, plus the cursor it was built at.
    pub fn snapshot(&self) -> BoardSnapshot {
        let state = self.store.loaded_state();
        let board = kanban(state);
        let col = |name: &'static str, cards: Vec<Card>| Column {
            name,
            cards: cards
                .into_iter()
                .map(|c| CardView {
                    waiting_on: state
                        .task(&c.id)
                        .map(|t| {
                            state
                                .unmet_dependencies(t)
                                .into_iter()
                                .map(String::from)
                                .collect()
                        })
                        .unwrap_or_default(),
                    id: c.id,
                    subject: c.subject,
                    owner: c.owner,
                    status: c.status,
                })
                .collect(),
        };
        BoardSnapshot {
            seq: self.seq(),
            team: self.team_name(),
            columns: vec![
                col("To do", board.todo),
                col("In progress", board.in_progress),
                col("Review", board.review),
                col("Approved", board.approved),
                col("Done", board.done),
            ],
        }
    }

    /// Everything about one task, or `None` if unknown (archived tasks are still found).
    pub fn task_detail(&self, id: &str) -> Option<TaskDetail> {
        let state = self.store.loaded_state();
        let t = state.task(id)?;
        let comments = state
            .messages
            .iter()
            .filter(|m| matches!(&m.scope, Scope::Task { task } if task == id))
            .map(|m| CommentView {
                seq: m.seq,
                ts: m.ts,
                from: m.from.clone(),
                text: m.text.clone(),
            })
            .collect();
        let history = self
            .store
            .loaded_events()
            .iter()
            .filter(|e| event_task(&e.kind) == Some(id))
            .map(history_item)
            .collect();
        Some(TaskDetail {
            id: t.id.clone(),
            subject: t.subject.clone(),
            description: t.description.clone(),
            owner: t.owner.clone(),
            status: t.status,
            reviewer: t.reviewer.clone(),
            blocked_reason: t.blocked_reason.clone(),
            archived: t.archived,
            blocked_by: t.blocked_by.clone(),
            waiting_on: state
                .unmet_dependencies(t)
                .into_iter()
                .map(String::from)
                .collect(),
            blocks: state.dependents(id).into_iter().map(String::from).collect(),
            comments,
            history,
        })
    }

    /// Newest-first activity feed, at most `limit` items.
    pub fn activity(&self, limit: usize) -> Vec<ActivityItem> {
        self.store
            .loaded_events()
            .iter()
            .rev()
            .take(limit)
            .map(|e| ActivityItem {
                seq: e.seq,
                ts: e.ts,
                actor: e.actor.clone(),
                task: event_task(&e.kind).map(String::from),
                summary: summarize(&e.kind),
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardView {
    pub id: String,
    pub subject: String,
    pub owner: Option<String>,
    pub status: Status,
    /// Unmet dependencies (not done yet), for a "waiting on" badge.
    pub waiting_on: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: &'static str,
    pub cards: Vec<CardView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardSnapshot {
    /// Log position this snapshot was built at.
    pub seq: u64,
    pub team: Option<String>,
    /// Always five, in order: To do, In progress, Review, Approved, Done.
    pub columns: Vec<Column>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentView {
    pub seq: u64,
    pub ts: i64,
    pub from: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryItem {
    pub seq: u64,
    pub ts: i64,
    pub actor: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDetail {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub owner: Option<String>,
    pub status: Status,
    pub reviewer: Option<String>,
    pub blocked_reason: Option<String>,
    pub archived: bool,
    pub blocked_by: Vec<String>,
    /// Subset of `blocked_by` that is not done yet.
    pub waiting_on: Vec<String>,
    /// Tasks that list this one as a dependency.
    pub blocks: Vec<String>,
    /// Task comments, oldest first.
    pub comments: Vec<CommentView>,
    /// Every event touching the task, oldest first.
    pub history: Vec<HistoryItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityItem {
    pub seq: u64,
    pub ts: i64,
    pub actor: String,
    pub task: Option<String>,
    pub summary: String,
}

fn history_item(e: &Event) -> HistoryItem {
    HistoryItem {
        seq: e.seq,
        ts: e.ts,
        actor: e.actor.clone(),
        summary: summarize(&e.kind),
    }
}

/// The task an event is about, if any.
fn event_task(k: &EventKind) -> Option<&str> {
    use EventKind::*;
    match k {
        TaskCreated { task, .. }
        | TaskDependenciesSet { task, .. }
        | TaskAssigned { task, .. }
        | TaskTransitioned { task, .. }
        | TaskBlocked { task, .. }
        | TaskUnblocked { task, .. }
        | TaskArchived { task, .. }
        | TaskCommented { task, .. }
        | ReviewRequested { task, .. }
        | ReviewApproved { task, .. }
        | ReviewChangesRequested { task, .. } => Some(task),
        TeamCreated { .. }
        | MemberAdded { .. }
        | MessageSent { .. }
        | ProcessRegistered { .. }
        | ProcessStopped { .. } => None,
    }
}

fn status_label(s: Status) -> &'static str {
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

/// One line, no actor prefix (the UI shows the actor separately).
fn summarize(k: &EventKind) -> String {
    use EventKind::*;
    match k {
        TeamCreated { name } => format!("created team {name}"),
        MemberAdded { name, role } => format!("added {name} ({role})"),
        TaskCreated { task, subject, .. } => format!("created {task}: {subject}"),
        TaskDependenciesSet { task, blocked_by } if blocked_by.is_empty() => {
            format!("cleared dependencies of {task}")
        }
        TaskDependenciesSet { task, blocked_by } => {
            format!("{task} now depends on {}", blocked_by.join(", "))
        }
        TaskAssigned { task, owner } => match owner {
            Some(o) => format!("assigned {task} to {o}"),
            None => format!("unassigned {task}"),
        },
        TaskTransitioned { task, from, to } => {
            format!("{task}: {} -> {}", status_label(*from), status_label(*to))
        }
        TaskBlocked { task, reason, .. } => format!("blocked {task}: {reason}"),
        TaskUnblocked { task, to } => format!("unblocked {task} -> {}", status_label(*to)),
        TaskArchived { task, archived: true } => format!("archived {task}"),
        TaskArchived { task, archived: false } => format!("restored {task}"),
        TaskCommented { task, text } => format!("commented on {task}: {}", clip(text)),
        ReviewRequested { task, reviewer } => format!("requested review of {task} from {reviewer}"),
        ReviewApproved { task, .. } => format!("approved {task}"),
        ReviewChangesRequested { task, comment } => {
            format!("requested changes on {task}: {}", clip(comment))
        }
        MessageSent { to: Some(to), text } => format!("messaged {to}: {}", clip(text)),
        MessageSent { to: None, text } => format!("broadcast: {}", clip(text)),
        ProcessRegistered { pid, label, .. } => format!("started process {label} (pid {pid})"),
        ProcessStopped { pid } => format!("stopped process (pid {pid})"),
    }
}

/// First line, at most 80 chars, for one-line summaries.
fn clip(s: &str) -> String {
    let line = s.lines().next().unwrap_or("");
    if line.chars().count() > 80 || s.lines().nth(1).is_some() {
        let head: String = line.chars().take(80).collect();
        format!("{head}…")
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rteams_core::Command;
    use rteams_store::Store;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("rteams-gui-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn create_team(home: &PathBuf) -> FileStore {
        let mut w = FileStore::open(home, "t").unwrap();
        w.execute("lead", 1, Command::CreateTeam { name: "t".into() })
            .unwrap();
        w
    }

    #[test]
    fn missing_team_is_an_error_and_nothing_is_created() {
        let home = tmp("missing");
        assert!(Data::open(home.clone(), "nope").is_err());
        assert!(!home.join("nope").exists());
    }

    #[test]
    fn refresh_tails_the_log_by_seq() {
        let home = tmp("tail");
        let mut w = create_team(&home);
        let mut d = Data::open(home.clone(), "t").unwrap();
        assert!(d.refresh().unwrap(), "first refresh reports a change");
        assert!(!d.refresh().unwrap(), "nothing new");
        let seq = d.seq();
        w.execute(
            "lead",
            2,
            Command::CreateTask {
                subject: "A".into(),
                description: "d".into(),
                owner: None,
                blocked_by: vec![],
            },
        )
        .unwrap();
        assert_eq!(d.seq(), seq, "reads only move on refresh");
        assert!(d.refresh().unwrap());
        assert!(d.seq() > seq);
        assert_eq!(d.board().unwrap().todo.len(), 1);
        assert!(!d.refresh().unwrap());
        let _ = std::fs::remove_dir_all(&home);
    }
}
