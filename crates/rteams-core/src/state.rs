use std::collections::HashMap;

use crate::command::Command;
use crate::domain::{Member, Message, ProcessEntry, Scope, Task, Team};
use crate::error::CoreError;
use crate::event::{Event, EventKind};
use crate::lifecycle::Status;

/// The fold of the event log: current state of everything. Rebuilt with `from_events`.
#[derive(Debug, Clone, Default)]
pub struct State {
    pub team: Option<Team>,
    /// Creation order.
    pub tasks: Vec<Task>,
    index: HashMap<String, usize>,
    /// Task comments, direct messages and broadcasts, in log order.
    pub messages: Vec<Message>,
    /// Registration order.
    pub processes: Vec<ProcessEntry>,
}

impl State {
    pub fn from_events<'a>(events: impl IntoIterator<Item = &'a Event>) -> State {
        let mut s = State::default();
        for e in events {
            s.apply(e);
        }
        s
    }

    pub fn task(&self, id: &str) -> Option<&Task> {
        self.index.get(id).map(|&i| &self.tasks[i])
    }

    fn task_mut(&mut self, id: &str) -> Option<&mut Task> {
        let i = *self.index.get(id)?;
        Some(&mut self.tasks[i])
    }

    /// True if `from` depends on `target`, directly or through other tasks.
    fn depends_on(&self, from: &str, target: &str) -> bool {
        let mut stack = vec![from];
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            if let Some(t) = self.task(id) {
                for dep in &t.blocked_by {
                    if dep == target {
                        return true;
                    }
                    stack.push(dep);
                }
            }
        }
        false
    }

    /// The tasks `task` is still waiting on: dependencies that are neither done nor archived
    /// (an archived task is abandoned work and cannot hold anyone up).
    pub fn unmet_dependencies<'a>(&self, task: &'a Task) -> Vec<&'a str> {
        task.blocked_by
            .iter()
            .filter(|id| {
                self.task(id)
                    .is_some_and(|d| d.status != Status::Done && !d.archived)
            })
            .map(String::as_str)
            .collect()
    }

    /// The inverse relation ("blocks"): tasks that list `id` in their `blocked_by`.
    pub fn dependents(&self, id: &str) -> Vec<&str> {
        self.tasks
            .iter()
            .filter(|t| t.blocked_by.iter().any(|d| d == id))
            .map(|t| t.id.as_str())
            .collect()
    }

    pub fn is_member(&self, name: &str) -> bool {
        self.team
            .as_ref()
            .is_some_and(|t| t.members.iter().any(|m| m.name == name))
    }

    /// Fold one event. Lenient on purpose: events were validated by `decide` when appended,
    /// so replay never fails; references to unknown tasks are ignored.
    pub fn apply(&mut self, e: &Event) {
        use EventKind::*;
        let (seq, ts) = (e.seq, e.ts);
        match &e.kind {
            TeamCreated { name } => {
                self.team = Some(Team {
                    name: name.clone(),
                    members: vec![],
                });
            }
            MemberAdded { name, role } => {
                if let Some(t) = &mut self.team {
                    t.members.push(Member {
                        name: name.clone(),
                        role: role.clone(),
                    });
                }
            }
            TaskCreated {
                task,
                subject,
                description,
                owner,
                blocked_by,
            } => {
                self.index.insert(task.clone(), self.tasks.len());
                self.tasks.push(Task {
                    id: task.clone(),
                    subject: subject.clone(),
                    description: description.clone(),
                    owner: owner.clone(),
                    status: Status::Pending,
                    blocked_by: blocked_by.clone(),
                    reviewer: None,
                    blocked_from: None,
                    blocked_reason: None,
                    archived: false,
                    created_seq: seq,
                });
            }
            TaskDependenciesSet { task, blocked_by } => {
                if let Some(t) = self.task_mut(task) {
                    t.blocked_by = blocked_by.clone();
                }
            }
            TaskAssigned { task, owner } => {
                if let Some(t) = self.task_mut(task) {
                    t.owner = owner.clone();
                }
            }
            TaskTransitioned { task, to, .. } => {
                if let Some(t) = self.task_mut(task) {
                    t.status = *to;
                }
            }
            TaskBlocked { task, reason, from } => {
                if let Some(t) = self.task_mut(task) {
                    t.status = Status::Blocked;
                    t.blocked_from = Some(*from);
                    t.blocked_reason = Some(reason.clone());
                }
            }
            TaskUnblocked { task, to } => {
                if let Some(t) = self.task_mut(task) {
                    t.status = *to;
                    t.blocked_from = None;
                    t.blocked_reason = None;
                }
            }
            TaskArchived { task, archived } => {
                if let Some(t) = self.task_mut(task) {
                    t.archived = *archived;
                }
            }
            TaskCommented { task, text } => {
                self.push_message(seq, ts, e, Scope::Task { task: task.clone() }, text.clone());
            }
            ReviewRequested { task, reviewer } => {
                if let Some(t) = self.task_mut(task) {
                    t.status = Status::Review;
                    t.reviewer = Some(reviewer.clone());
                }
            }
            ReviewApproved { task, note } => {
                if let Some(t) = self.task_mut(task) {
                    t.status = Status::Approved;
                }
                if let Some(note) = note {
                    self.push_message(seq, ts, e, Scope::Task { task: task.clone() }, note.clone());
                }
            }
            ReviewChangesRequested { task, comment } => {
                if let Some(t) = self.task_mut(task) {
                    t.status = Status::NeedsFix;
                }
                self.push_message(
                    seq,
                    ts,
                    e,
                    Scope::Task { task: task.clone() },
                    comment.clone(),
                );
            }
            MessageSent { to, text } => {
                let scope = match to {
                    Some(to) => Scope::Direct { to: to.clone() },
                    None => Scope::Broadcast,
                };
                self.push_message(seq, ts, e, scope, text.clone());
            }
            ProcessRegistered {
                pid,
                label,
                port,
                url,
                command,
            } => {
                self.processes.retain(|p| p.pid != *pid);
                self.processes.push(ProcessEntry {
                    pid: *pid,
                    label: label.clone(),
                    owner: e.actor.clone(),
                    port: *port,
                    url: url.clone(),
                    command: command.clone(),
                    running: true,
                });
            }
            ProcessStopped { pid } => {
                if let Some(p) = self.processes.iter_mut().find(|p| p.pid == *pid) {
                    p.running = false;
                }
            }
        }
    }

    fn push_message(&mut self, seq: u64, ts: i64, e: &Event, scope: Scope, text: String) {
        self.messages.push(Message {
            seq,
            ts,
            from: e.actor.clone(),
            scope,
            text,
        });
    }

    fn require_task(&self, id: &str) -> Result<&Task, CoreError> {
        self.task(id)
            .ok_or_else(|| CoreError::TaskNotFound(id.to_string()))
    }

    fn require_member(&self, name: &str) -> Result<(), CoreError> {
        if self.is_member(name) {
            Ok(())
        } else {
            Err(CoreError::UnknownMember(name.to_string()))
        }
    }

    fn check(task: &Task, to: Status) -> Result<(), CoreError> {
        if task.status.can_transition_to(to) {
            Ok(())
        } else {
            Err(CoreError::IllegalTransition {
                task: task.id.clone(),
                from: task.status,
                to,
            })
        }
    }

    /// Validate a command against current state and produce the event it results in.
    /// Pure: does not mutate. Append the event and `apply` it to commit.
    pub fn decide(&self, cmd: Command) -> Result<EventKind, CoreError> {
        use Command as C;
        use EventKind as E;
        match cmd {
            C::CreateTeam { name } => {
                if self.team.is_some() {
                    return Err(CoreError::TeamExists);
                }
                Ok(E::TeamCreated { name })
            }
            C::AddMember { name, role } => {
                if self.team.is_none() {
                    return Err(CoreError::TeamNotCreated);
                }
                if self.is_member(&name) {
                    return Err(CoreError::MemberExists(name));
                }
                Ok(E::MemberAdded { name, role })
            }
            C::CreateTask {
                id,
                subject,
                description,
                owner,
                blocked_by,
            } => {
                if self.task(&id).is_some() {
                    return Err(CoreError::TaskExists(id));
                }
                if let Some(o) = &owner {
                    self.require_member(o)?;
                }
                // A new task has no dependents yet, so it cannot close a cycle.
                for dep in &blocked_by {
                    self.require_task(dep)?;
                }
                Ok(E::TaskCreated {
                    task: id,
                    subject,
                    description,
                    owner,
                    blocked_by: dedup(blocked_by),
                })
            }
            C::SetDependencies { task, blocked_by } => {
                self.require_task(&task)?;
                for dep in &blocked_by {
                    self.require_task(dep)?;
                    if *dep == task || self.depends_on(dep, &task) {
                        return Err(CoreError::DependencyCycle {
                            task,
                            depends_on: dep.clone(),
                        });
                    }
                }
                Ok(E::TaskDependenciesSet {
                    task,
                    blocked_by: dedup(blocked_by),
                })
            }
            C::AssignTask { task, owner } => {
                self.require_task(&task)?;
                if let Some(o) = &owner {
                    self.require_member(o)?;
                }
                Ok(E::TaskAssigned { task, owner })
            }
            C::Transition { task, to } => {
                let t = self.require_task(&task)?;
                if matches!(
                    to,
                    Status::Review | Status::Approved | Status::NeedsFix | Status::Blocked
                ) {
                    return Err(CoreError::UseDedicatedCommand { task, to });
                }
                // Leaving Blocked is `Unblock` only: it must restore the prior status.
                if t.status == Status::Blocked {
                    return Err(CoreError::IllegalTransition {
                        task,
                        from: t.status,
                        to,
                    });
                }
                Self::check(t, to)?;
                Ok(E::TaskTransitioned {
                    task,
                    from: t.status,
                    to,
                })
            }
            C::Block { task, reason } => {
                let t = self.require_task(&task)?;
                Self::check(t, Status::Blocked)?;
                Ok(E::TaskBlocked {
                    task,
                    reason,
                    from: t.status,
                })
            }
            C::Unblock { task } => {
                let t = self.require_task(&task)?;
                match t.blocked_from {
                    Some(to) if t.status == Status::Blocked => Ok(E::TaskUnblocked { task, to }),
                    _ => Err(CoreError::NotBlocked(task)),
                }
            }
            C::ArchiveTask { task, archived } => {
                self.require_task(&task)?;
                Ok(E::TaskArchived { task, archived })
            }
            C::Comment { task, text } => {
                self.require_task(&task)?;
                Ok(E::TaskCommented { task, text })
            }
            C::RequestReview { task, reviewer } => {
                let t = self.require_task(&task)?;
                self.require_member(&reviewer)?;
                Self::check(t, Status::Review)?;
                Ok(E::ReviewRequested { task, reviewer })
            }
            C::ApproveReview { task, note } => {
                Self::check(self.require_task(&task)?, Status::Approved)?;
                Ok(E::ReviewApproved { task, note })
            }
            C::RequestChanges { task, comment } => {
                Self::check(self.require_task(&task)?, Status::NeedsFix)?;
                Ok(E::ReviewChangesRequested { task, comment })
            }
            C::SendMessage { to, text } => {
                if let Some(to) = &to {
                    self.require_member(to)?;
                }
                Ok(E::MessageSent { to, text })
            }
            C::RegisterProcess {
                pid,
                label,
                port,
                url,
                command,
            } => {
                if self.processes.iter().any(|p| p.pid == pid && p.running) {
                    return Err(CoreError::ProcessRunning(pid));
                }
                Ok(E::ProcessRegistered {
                    pid,
                    label,
                    port,
                    url,
                    command,
                })
            }
            C::StopProcess { pid } => {
                if !self.processes.iter().any(|p| p.pid == pid && p.running) {
                    return Err(CoreError::ProcessNotFound(pid));
                }
                Ok(E::ProcessStopped { pid })
            }
        }
    }
}

fn dedup(mut ids: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    ids.retain(|id| seen.insert(id.clone()));
    ids
}
