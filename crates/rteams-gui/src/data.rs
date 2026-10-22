//! Read adapter over the event log. OWNER: vector.
//!
//! `Data` opens one team's store read-only (never creates files, never appends) and tails the
//! log: call `refresh()` on a poll, then read. Reads use what the last `refresh()` loaded, so a
//! frame sees one consistent view. Everything returned is a plain, owned, renderable struct.
//!
//! Stable signatures (echo/scout rely on them): `open`, `refresh`, `state`.
//! Added: `seq`, `team_name`, `task_detail`.

use std::path::PathBuf;

use rteams_core::{Event, EventKind, Scope, State, Status};
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

    /// The team's directory (`<home>/<team>`), for per-team sidecar files.
    pub fn dir(&self) -> &std::path::Path {
        self.store.dir()
    }

    /// Seq of the newest event loaded (0 for an empty log).
    pub fn seq(&self) -> u64 {
        self.store.loaded_events().last().map_or(0, |e| e.seq)
    }

    /// Every event loaded as of the last `refresh`, oldest first.
    pub fn events(&self) -> &[Event] {
        self.store.loaded_events()
    }

    /// Current state as of the last `refresh`.
    pub fn state(&self) -> &State {
        self.store.loaded_state()
    }

    pub fn team_name(&self) -> Option<String> {
        self.store
            .loaded_state()
            .team
            .as_ref()
            .map(|t| t.name.clone())
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
        | TaskUpdated { task, .. }
        | TaskTransitioned { task, .. }
        | TaskBlocked { task, .. }
        | TaskUnblocked { task, .. }
        | TaskArchived { task, .. }
        | TaskCommented { task, .. }
        | ReviewRequested { task, .. }
        | ReviewApproved { task, .. }
        | ReviewChangesRequested { task, .. }
        | TaskClarificationSet { task, .. }
        | TaskFileAttached { task, .. }
        | TasksLinked { task, .. }
        | ReviewStarted { task }
        | ReviewerAdded { task, .. }
        | ReviewerRemoved { task, .. }
        | TaskColumnSet { task, .. } => Some(task),
        TeamCreated { .. }
        | MemberAdded { .. }
        | MessageSent { .. }
        | TeamConfigured { .. }
        | MemberUpdated { .. }
        | MemberRemoved { .. }
        | ProcessRegistered { .. }
        | ProcessStopped { .. }
        | ColumnOrderSet { .. }
        | TeamDeleted { .. }
        | TeamRenamed { .. }
        | MessageReacted { .. }
        | ScheduleSet { .. }
        | ScheduleRemoved { .. }
        | PermissionRequested { .. }
        | PermissionDecided { .. }
        | GroupChatCreated { .. }
        | GroupChatUpdated { .. }
        | GroupChatDeleted { .. }
        | ColumnCleared { .. }
        | MemberHeartbeat { .. }
        | LaunchRequested { .. }
        | StopRequested { .. } => None,
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
        MemberAdded { name, role, .. } => format!("added {name} ({role})"),
        TeamConfigured { .. } => "configured the team".to_string(),
        MemberUpdated { name, .. } => format!("updated {name}"),
        MemberRemoved {
            name,
            removed: true,
        } => format!("removed {name}"),
        MemberRemoved {
            name,
            removed: false,
        } => format!("restored {name}"),
        TaskCreated { task, subject, .. } => format!("created {task}: {subject}"),
        TaskDependenciesSet { task, blocked_by } if blocked_by.is_empty() => {
            format!("cleared dependencies of {task}")
        }
        TaskDependenciesSet { task, blocked_by } => {
            format!("{task} now depends on {}", blocked_by.join(", "))
        }
        TaskUpdated { task, .. } => format!("edited {task}"),
        TaskAssigned { task, owner } => match owner {
            Some(o) => format!("assigned {task} to {o}"),
            None => format!("unassigned {task}"),
        },
        TaskTransitioned { task, from, to } => {
            format!("{task}: {} -> {}", status_label(*from), status_label(*to))
        }
        TaskBlocked { task, reason, .. } => format!("blocked {task}: {reason}"),
        TaskUnblocked { task, to } => format!("unblocked {task} -> {}", status_label(*to)),
        TaskArchived {
            task,
            archived: true,
        } => format!("archived {task}"),
        TaskArchived {
            task,
            archived: false,
        } => format!("restored {task}"),
        TaskCommented { task, text, .. } => format!("commented on {task}: {}", clip(text)),
        ReviewRequested { task, reviewer } => format!("requested review of {task} from {reviewer}"),
        ReviewApproved { task, .. } => format!("approved {task}"),
        ReviewChangesRequested { task, comment } => {
            format!("requested changes on {task}: {}", clip(comment))
        }
        MessageSent {
            chat: Some(chat),
            text,
            ..
        } => format!("posted in group {chat}: {}", clip(text)),
        MessageSent {
            to: Some(to), text, ..
        } => format!("messaged {to}: {}", clip(text)),
        MessageSent { to: None, text, .. } => format!("broadcast: {}", clip(text)),
        ProcessRegistered { pid, label, .. } => format!("started process {label} (pid {pid})"),
        ProcessStopped { pid } => format!("stopped process (pid {pid})"),
        TaskClarificationSet {
            task,
            question: Some(q),
        } => format!("asked on {task}: {}", clip(q)),
        TaskClarificationSet { task, .. } => format!("cleared the question on {task}"),
        TaskFileAttached { task, name, .. } => format!("attached {name} to {task}"),
        TasksLinked {
            task,
            related,
            linked: true,
        } => format!("linked {task} with {related}"),
        TasksLinked { task, related, .. } => format!("unlinked {task} from {related}"),
        ReviewStarted { task } => format!("started reviewing {task}"),
        ColumnOrderSet { column, .. } => format!("reordered the {column} column"),
        TeamRenamed { name } => format!("renamed the team to {name}"),
        TeamDeleted { deleted: true } => "deleted the team".to_string(),
        TeamDeleted { .. } => "restored the team".to_string(),
        MessageReacted { emoji, .. } => format!("reacted {emoji}"),
        ScheduleSet { schedule } => format!("scheduled {} for {}", schedule.id, schedule.member),
        ScheduleRemoved { id } => format!("removed schedule {id}"),
        PermissionRequested { member, tool, .. } => format!("{member} requests {tool}"),
        PermissionDecided { id, allow, .. } => {
            format!("{} request {id}", if *allow { "allowed" } else { "denied" })
        }
        GroupChatCreated { name, .. } => format!("created group chat {name}"),
        GroupChatUpdated { id, .. } => format!("updated group chat {id}"),
        GroupChatDeleted { id } => format!("deleted group chat {id}"),
        ReviewerAdded { task, reviewer } => format!("added reviewer {reviewer} to {task}"),
        ReviewerRemoved { task, reviewer } => format!("removed reviewer {reviewer} from {task}"),
        TaskColumnSet { task, from, to } => {
            format!(
                "moved {task}: {} -> {}",
                status_label(*from),
                status_label(*to)
            )
        }
        ColumnCleared { column, tasks } => {
            format!("cleared {} task(s) from the {column} column", tasks.len())
        }
        MemberHeartbeat { member, status, .. } => format!("{member} is {status}"),
        LaunchRequested { members } => format!("requested launch of {}", list_or_all(members)),
        StopRequested { members } => format!("requested stop of {}", list_or_all(members)),
    }
}

fn list_or_all(members: &[String]) -> String {
    if members.is_empty() {
        "everyone".to_string()
    } else {
        members.join(", ")
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
                id: "T1".into(),
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
        assert_eq!(d.state().tasks.len(), 1);
        assert!(!d.refresh().unwrap());
        let _ = std::fs::remove_dir_all(&home);
    }

    fn task(id: &str, deps: &[&str]) -> Command {
        Command::CreateTask {
            id: id.into(),
            subject: format!("subject {id}"),
            description: format!("desc {id}"),
            owner: Some("vector".into()),
            blocked_by: deps.iter().map(|d| d.to_string()).collect(),
        }
    }

    #[test]
    fn task_detail_renders_the_log() {
        let home = tmp("views");
        let mut w = create_team(&home);
        w.execute(
            "lead",
            2,
            Command::AddMember {
                name: "vector".into(),
                role: "dev".into(),
                runtime: None,
            },
        )
        .unwrap();
        w.execute("lead", 3, task("T1", &[])).unwrap();
        w.execute("lead", 4, task("T2", &["T1"])).unwrap();
        w.execute(
            "vector",
            5,
            Command::Comment {
                task: "T1".into(),
                text: "hello\nworld".into(),
                attachments: vec![],
            },
        )
        .unwrap();
        w.execute(
            "vector",
            6,
            Command::Transition {
                task: "T1".into(),
                to: Status::InProgress,
            },
        )
        .unwrap();
        let mut d = Data::open(home.clone(), "t").unwrap();
        d.refresh().unwrap();

        let t1 = d.task_detail("T1").unwrap();
        assert_eq!(t1.description, "desc T1");
        assert_eq!(t1.blocks, vec!["T2"]);
        assert_eq!(t1.comments.len(), 1);
        assert_eq!(t1.comments[0].text, "hello\nworld");
        assert_eq!(t1.history.len(), 3, "created, commented, transitioned");
        let t2 = d.task_detail("T2").unwrap();
        assert_eq!(t2.waiting_on, vec!["T1"]);
        assert!(d.task_detail("nope").is_none());

        let _ = std::fs::remove_dir_all(&home);
    }
}
