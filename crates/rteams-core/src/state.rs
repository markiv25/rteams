use std::collections::{BTreeMap, HashMap, VecDeque};

use crate::command::Command;
use crate::domain::{
    Attachment, GroupChat, HeartbeatInfo, Member, Message, PermissionRequest, ProcessEntry,
    Reaction, Scope, Task, Team,
};
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
    /// Tool-approval requests, in request order.
    pub permissions: Vec<PermissionRequest>,
    /// Last heartbeat per member.
    pub heartbeats: BTreeMap<String, HeartbeatInfo>,
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
            .is_some_and(|t| t.members.iter().any(|m| m.name == name && !m.removed))
    }

    /// Includes removed members (their names stay reserved).
    fn has_member_record(&self, name: &str) -> bool {
        self.team
            .as_ref()
            .is_some_and(|t| t.members.iter().any(|m| m.name == name))
    }

    fn member_mut(&mut self, name: &str) -> Option<&mut Member> {
        self.team
            .as_mut()?
            .members
            .iter_mut()
            .find(|m| m.name == name)
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
                    project_path: None,
                    lead_prompt: None,
                    deleted: false,
                    column_order: Default::default(),
                    schedules: vec![],
                    group_chats: vec![],
                });
            }
            MemberAdded {
                name,
                role,
                runtime,
            } => {
                if let Some(t) = &mut self.team {
                    t.members.push(Member {
                        name: name.clone(),
                        role: role.clone(),
                        prompt: None,
                        model: None,
                        removed: false,
                        runtime: runtime.clone(),
                        worktree: false,
                        icon: None,
                        effort: None,
                        fast: false,
                        context_limit: None,
                        mcp_allow: vec![],
                    });
                }
            }
            TeamConfigured {
                project_path,
                lead_prompt,
            } => {
                if let Some(t) = &mut self.team {
                    if let Some(p) = project_path {
                        t.project_path = Some(p.clone());
                    }
                    if let Some(p) = lead_prompt {
                        t.lead_prompt = Some(p.clone());
                    }
                }
            }
            MemberUpdated {
                name,
                role,
                prompt,
                model,
                runtime,
                worktree,
                icon,
                effort,
                fast,
                context_limit,
                mcp_allow,
            } => {
                if let Some(m) = self.member_mut(name) {
                    if let Some(x) = effort {
                        m.effort = (!x.trim().is_empty()).then(|| x.trim().to_string());
                    }
                    if let Some(f) = fast {
                        m.fast = *f;
                    }
                    if let Some(c) = context_limit {
                        m.context_limit = (*c > 0).then_some(*c);
                    }
                    if let Some(l) = mcp_allow {
                        m.mcp_allow = l.clone();
                    }
                    if let Some(i) = icon {
                        m.icon = (!i.trim().is_empty()).then(|| i.trim().to_string());
                    }
                    if let Some(r) = runtime {
                        m.runtime = Some(r.clone());
                    }
                    if let Some(w) = worktree {
                        m.worktree = *w;
                    }
                    if let Some(r) = role {
                        m.role = r.clone();
                    }
                    if let Some(p) = prompt {
                        m.prompt = Some(p.clone());
                    }
                    if let Some(x) = model {
                        m.model = Some(x.clone());
                    }
                }
            }
            MemberRemoved { name, removed } => {
                if let Some(m) = self.member_mut(name) {
                    m.removed = *removed;
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
                    reviewers: vec![],
                    blocked_from: None,
                    blocked_reason: None,
                    archived: false,
                    created_seq: seq,
                    clarification: None,
                    attachments: vec![],
                    related: vec![],
                    review_started: false,
                });
            }
            TaskDependenciesSet { task, blocked_by } => {
                if let Some(t) = self.task_mut(task) {
                    t.blocked_by = blocked_by.clone();
                }
            }
            TaskUpdated {
                task,
                subject,
                description,
            } => {
                if let Some(t) = self.task_mut(task) {
                    if let Some(s) = subject {
                        t.subject = s.clone();
                    }
                    if let Some(d) = description {
                        t.description = d.clone();
                    }
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
                    t.review_started = false;
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
            TaskCommented {
                task,
                text,
                attachments,
            } => {
                self.push_message(seq, ts, e, Scope::Task { task: task.clone() }, text.clone());
                if let Some(m) = self.messages.last_mut() {
                    m.attachments = attachments.clone();
                }
            }
            ReviewRequested { task, reviewer } => {
                if let Some(t) = self.task_mut(task) {
                    t.status = Status::Review;
                    t.reviewer = Some(reviewer.clone());
                    t.review_started = false;
                }
            }
            ReviewApproved { task, note } => {
                if let Some(t) = self.task_mut(task) {
                    t.status = Status::Approved;
                    t.review_started = false;
                }
                if let Some(note) = note {
                    self.push_message(seq, ts, e, Scope::Task { task: task.clone() }, note.clone());
                }
            }
            ReviewChangesRequested { task, comment } => {
                if let Some(t) = self.task_mut(task) {
                    t.status = Status::NeedsFix;
                    t.review_started = false;
                }
                self.push_message(
                    seq,
                    ts,
                    e,
                    Scope::Task { task: task.clone() },
                    comment.clone(),
                );
            }
            MessageSent {
                to,
                text,
                mode,
                from_team,
                chat,
                attachments,
            } => {
                let scope = match (chat, to) {
                    (Some(chat), _) => Scope::Group { chat: chat.clone() },
                    (None, Some(to)) => Scope::Direct { to: to.clone() },
                    (None, None) => Scope::Broadcast,
                };
                self.push_message(seq, ts, e, scope, text.clone());
                if let Some(m) = self.messages.last_mut() {
                    m.mode = mode.clone();
                    m.from_team = from_team.clone();
                    m.attachments = attachments.clone();
                }
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
            TaskClarificationSet { task, question } => {
                if let Some(t) = self.task_mut(task) {
                    t.clarification = question.clone();
                }
            }
            TaskFileAttached { task, name, path } => {
                if let Some(t) = self.task_mut(task) {
                    t.attachments.push(Attachment {
                        name: name.clone(),
                        path: path.clone(),
                    });
                }
            }
            TasksLinked {
                task,
                related,
                linked,
            } => {
                for (a, b) in [(task, related), (related, task)] {
                    if let Some(t) = self.task_mut(a) {
                        t.related.retain(|r| r != b);
                        if *linked {
                            t.related.push(b.clone());
                        }
                    }
                }
            }
            ReviewStarted { task } => {
                if let Some(t) = self.task_mut(task) {
                    t.review_started = true;
                }
            }
            ColumnOrderSet { column, order } => {
                if let Some(t) = &mut self.team {
                    t.column_order.insert(column.clone(), order.clone());
                }
            }
            TeamRenamed { name } => {
                if let Some(t) = &mut self.team {
                    t.name = name.clone();
                }
            }
            TeamDeleted { deleted } => {
                if let Some(t) = &mut self.team {
                    t.deleted = *deleted;
                }
            }
            MessageReacted { message_seq, emoji } => {
                if let Some(m) = self.messages.iter_mut().find(|m| m.seq == *message_seq) {
                    let before = m.reactions.len();
                    m.reactions
                        .retain(|r| !(r.from == e.actor && r.emoji == *emoji));
                    if m.reactions.len() == before {
                        m.reactions.push(Reaction {
                            from: e.actor.clone(),
                            emoji: emoji.clone(),
                        });
                    }
                }
            }
            ScheduleSet { schedule } => {
                if let Some(t) = &mut self.team {
                    match t.schedules.iter_mut().find(|s| s.id == schedule.id) {
                        Some(s) => *s = schedule.clone(),
                        None => t.schedules.push(schedule.clone()),
                    }
                }
            }
            ScheduleRemoved { id } => {
                if let Some(t) = &mut self.team {
                    t.schedules.retain(|s| s.id != *id);
                }
            }
            PermissionRequested {
                id,
                member,
                tool,
                input,
            } => self.permissions.push(PermissionRequest {
                id: id.clone(),
                member: member.clone(),
                tool: tool.clone(),
                input: input.clone(),
                decided: None,
                note: None,
            }),
            PermissionDecided { id, allow, note } => {
                if let Some(p) = self.permissions.iter_mut().find(|p| p.id == *id) {
                    p.decided = Some(*allow);
                    p.note = note.clone();
                }
            }
            GroupChatCreated { id, name, members } => {
                if let Some(t) = &mut self.team {
                    t.group_chats.push(GroupChat {
                        id: id.clone(),
                        name: name.clone(),
                        members: members.clone(),
                        created_by: e.actor.clone(),
                    });
                }
            }
            GroupChatUpdated { id, name, members } => {
                if let Some(g) = self
                    .team
                    .as_mut()
                    .and_then(|t| t.group_chats.iter_mut().find(|g| g.id == *id))
                {
                    if let Some(n) = name {
                        g.name = n.clone();
                    }
                    if let Some(m) = members {
                        g.members = m.clone();
                    }
                }
            }
            GroupChatDeleted { id } => {
                if let Some(t) = &mut self.team {
                    t.group_chats.retain(|g| g.id != *id);
                }
            }
            ReviewerAdded { task, reviewer } => {
                if let Some(t) = self.task_mut(task) {
                    if !t.reviewers.contains(reviewer) {
                        t.reviewers.push(reviewer.clone());
                    }
                }
            }
            ReviewerRemoved { task, reviewer } => {
                if let Some(t) = self.task_mut(task) {
                    t.reviewers.retain(|r| r != reviewer);
                }
            }
            TaskColumnSet { task, to, .. } => {
                if let Some(t) = self.task_mut(task) {
                    t.status = *to;
                    t.review_started = false;
                }
            }
            ColumnCleared { tasks, .. } => {
                for id in tasks {
                    if let Some(t) = self.task_mut(id) {
                        t.archived = true;
                    }
                }
            }
            MemberHeartbeat {
                member,
                status,
                task,
                note,
            } => {
                self.heartbeats.insert(
                    member.clone(),
                    HeartbeatInfo {
                        status: status.clone(),
                        task: task.clone(),
                        note: note.clone(),
                        ts,
                    },
                );
            }
            LaunchRequested { .. } | StopRequested { .. } => {}
        }
    }

    fn push_message(&mut self, seq: u64, ts: i64, e: &Event, scope: Scope, text: String) {
        self.messages.push(Message {
            seq,
            ts,
            from: e.actor.clone(),
            scope,
            text,
            mode: None,
            from_team: None,
            reactions: vec![],
            attachments: vec![],
        });
    }

    /// True if `name` is an active lead and no other active lead exists.
    fn is_only_lead(&self, name: &str) -> bool {
        let Some(t) = &self.team else { return false };
        let mut leads = t.members.iter().filter(|m| !m.removed && m.role == "lead");
        leads.clone().any(|m| m.name == name) && leads.all(|m| m.name == name)
    }

    fn require_task(&self, id: &str) -> Result<&Task, CoreError> {
        self.task(id)
            .ok_or_else(|| CoreError::TaskNotFound(id.to_string()))
    }

    pub fn group(&self, id: &str) -> Option<&GroupChat> {
        self.team.as_ref()?.group_chats.iter().find(|g| g.id == id)
    }

    fn require_group(&self, id: &str) -> Result<&GroupChat, CoreError> {
        self.group(id)
            .ok_or_else(|| CoreError::GroupChatNotFound(id.to_string()))
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
        if self.team.as_ref().is_some_and(|t| t.deleted)
            && !matches!(cmd, C::DeleteTeam { deleted: false })
        {
            return Err(CoreError::TeamDeleted);
        }
        match cmd {
            C::CreateTeam { name } => {
                if self.team.is_some() {
                    return Err(CoreError::TeamExists);
                }
                Ok(E::TeamCreated { name })
            }
            C::AddMember {
                name,
                role,
                runtime,
            } => {
                if self.team.is_none() {
                    return Err(CoreError::TeamNotCreated);
                }
                check_runtime(&runtime)?;
                // Removed members keep their name reserved: restore them instead.
                if self.has_member_record(&name) {
                    return Err(CoreError::MemberExists(name));
                }
                Ok(E::MemberAdded {
                    name,
                    role,
                    runtime,
                })
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
            C::UpdateTask {
                task,
                subject,
                description,
            } => {
                self.require_task(&task)?;
                if subject.is_none() && description.is_none() {
                    return Err(CoreError::EmptyUpdate);
                }
                if subject.as_ref().is_some_and(|s| s.trim().is_empty()) {
                    return Err(CoreError::EmptySubject);
                }
                Ok(E::TaskUpdated {
                    task,
                    subject,
                    description,
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
            C::Comment {
                task,
                text,
                attachments,
            } => {
                self.require_task(&task)?;
                check_attachments(&attachments)?;
                Ok(E::TaskCommented {
                    task,
                    text,
                    attachments,
                })
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
            C::SendMessage {
                to,
                text,
                mode,
                from_team,
                chat,
                attachments,
            } => {
                if let Some(to) = &to {
                    self.require_member(to)?;
                }
                if let Some(chat) = &chat {
                    if to.is_some() {
                        return Err(CoreError::ChatWithRecipient);
                    }
                    self.require_group(chat)?;
                }
                check_attachments(&attachments)?;
                if let Some(m) = &mode {
                    if !matches!(m.as_str(), "do" | "ask" | "delegate") {
                        return Err(CoreError::InvalidMode(m.clone()));
                    }
                }
                Ok(E::MessageSent {
                    to,
                    text,
                    mode,
                    from_team,
                    chat,
                    attachments,
                })
            }
            C::ConfigureTeam {
                project_path,
                lead_prompt,
            } => {
                if self.team.is_none() {
                    return Err(CoreError::TeamNotCreated);
                }
                if project_path.is_none() && lead_prompt.is_none() {
                    return Err(CoreError::EmptyConfig);
                }
                Ok(E::TeamConfigured {
                    project_path,
                    lead_prompt,
                })
            }
            C::UpdateMember {
                name,
                role,
                prompt,
                model,
                runtime,
                worktree,
                icon,
                effort,
                fast,
                context_limit,
                mcp_allow,
            } => {
                self.require_member(&name)?;
                if role.is_none()
                    && prompt.is_none()
                    && model.is_none()
                    && runtime.is_none()
                    && worktree.is_none()
                    && icon.is_none()
                    && effort.is_none()
                    && fast.is_none()
                    && context_limit.is_none()
                    && mcp_allow.is_none()
                {
                    return Err(CoreError::EmptyMemberUpdate);
                }
                check_runtime(&runtime)?;
                if let Some(x) = &effort {
                    let x = x.trim();
                    if !x.is_empty() && !matches!(x, "low" | "medium" | "high") {
                        return Err(CoreError::InvalidEffort(x.to_string()));
                    }
                }
                let mcp_allow = mcp_allow.map(dedup);
                if role.as_ref().is_some_and(|r| r.trim().is_empty()) {
                    return Err(CoreError::EmptyRole);
                }
                // Demoting the only lead would leave the team leaderless.
                if let Some(r) = &role {
                    if r != "lead" && self.is_only_lead(&name) {
                        return Err(CoreError::LastLead(name));
                    }
                }
                Ok(E::MemberUpdated {
                    name,
                    role,
                    prompt,
                    model,
                    runtime,
                    worktree,
                    icon,
                    effort,
                    fast,
                    context_limit,
                    mcp_allow,
                })
            }
            C::RemoveMember { name, removed } => {
                if removed {
                    self.require_member(&name)?;
                    if self.is_only_lead(&name) {
                        return Err(CoreError::LastLead(name));
                    }
                } else if !self.has_member_record(&name) {
                    return Err(CoreError::UnknownMember(name));
                }
                Ok(E::MemberRemoved { name, removed })
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
            C::SetClarification { task, question } => {
                self.require_task(&task)?;
                if question.as_ref().is_some_and(|q| q.trim().is_empty()) {
                    return Err(CoreError::EmptyField("clarification question"));
                }
                Ok(E::TaskClarificationSet { task, question })
            }
            C::AttachFile { task, name, path } => {
                self.require_task(&task)?;
                if name.trim().is_empty() {
                    return Err(CoreError::EmptyField("attachment name"));
                }
                if path.trim().is_empty() {
                    return Err(CoreError::EmptyField("attachment path"));
                }
                Ok(E::TaskFileAttached { task, name, path })
            }
            C::LinkTasks {
                task,
                related,
                linked,
            } => {
                self.require_task(&task)?;
                self.require_task(&related)?;
                if task == related {
                    return Err(CoreError::InvalidLink { task, related });
                }
                Ok(E::TasksLinked {
                    task,
                    related,
                    linked,
                })
            }
            C::StartReview { task } => {
                let t = self.require_task(&task)?;
                if t.status != Status::Review {
                    return Err(CoreError::NotInReview {
                        task,
                        status: t.status,
                    });
                }
                Ok(E::ReviewStarted { task })
            }
            C::SetColumnOrder { column, order } => {
                if self.team.is_none() {
                    return Err(CoreError::TeamNotCreated);
                }
                if serde_json::from_value::<Status>(serde_json::Value::String(column.clone()))
                    .is_err()
                {
                    return Err(CoreError::UnknownColumn(column));
                }
                for id in &order {
                    self.require_task(id)?;
                }
                Ok(E::ColumnOrderSet {
                    column,
                    order: dedup(order),
                })
            }
            C::RenameTeam { name } => {
                let name = name.trim().to_string();
                if name.is_empty() {
                    return Err(CoreError::EmptyField("team name"));
                }
                if self.team.is_none() {
                    return Err(CoreError::TeamNotCreated);
                }
                Ok(E::TeamRenamed { name })
            }
            C::DeleteTeam { deleted } => {
                if self.team.is_none() {
                    return Err(CoreError::TeamNotCreated);
                }
                Ok(E::TeamDeleted { deleted })
            }
            C::React { message_seq, emoji } => {
                if !self.messages.iter().any(|m| m.seq == message_seq) {
                    return Err(CoreError::MessageNotFound(message_seq));
                }
                if emoji.trim().is_empty() {
                    return Err(CoreError::EmptyField("emoji"));
                }
                Ok(E::MessageReacted { message_seq, emoji })
            }
            C::SetSchedule { schedule } => {
                if self.team.is_none() {
                    return Err(CoreError::TeamNotCreated);
                }
                if schedule.id.trim().is_empty() {
                    return Err(CoreError::EmptyField("schedule id"));
                }
                if schedule.prompt.trim().is_empty() {
                    return Err(CoreError::EmptyField("schedule prompt"));
                }
                if schedule.every_minutes < 1 {
                    return Err(CoreError::InvalidInterval);
                }
                self.require_member(&schedule.member)?;
                Ok(E::ScheduleSet { schedule })
            }
            C::RemoveSchedule { id } => {
                let exists = self
                    .team
                    .as_ref()
                    .is_some_and(|t| t.schedules.iter().any(|s| s.id == id));
                if !exists {
                    return Err(CoreError::ScheduleNotFound(id));
                }
                Ok(E::ScheduleRemoved { id })
            }
            C::RequestPermission {
                id,
                member,
                tool,
                input,
            } => {
                self.require_member(&member)?;
                if id.trim().is_empty() {
                    return Err(CoreError::EmptyField("permission id"));
                }
                if tool.trim().is_empty() {
                    return Err(CoreError::EmptyField("permission tool"));
                }
                if self.permissions.iter().any(|p| p.id == id) {
                    return Err(CoreError::PermissionExists(id));
                }
                Ok(E::PermissionRequested {
                    id,
                    member,
                    tool,
                    input,
                })
            }
            C::DecidePermission { id, allow, note } => {
                match self.permissions.iter().find(|p| p.id == id) {
                    None => Err(CoreError::PermissionNotFound(id)),
                    Some(p) if p.decided.is_some() => Err(CoreError::PermissionDecided(id)),
                    Some(_) => Ok(E::PermissionDecided { id, allow, note }),
                }
            }
            C::CreateGroupChat { id, name, members } => {
                if self.team.is_none() {
                    return Err(CoreError::TeamNotCreated);
                }
                if id.trim().is_empty() {
                    return Err(CoreError::EmptyField("group chat id"));
                }
                if name.trim().is_empty() {
                    return Err(CoreError::EmptyField("group chat name"));
                }
                if self.group(&id).is_some() {
                    return Err(CoreError::GroupChatExists(id));
                }
                for m in &members {
                    self.require_member(m)?;
                }
                Ok(E::GroupChatCreated {
                    id,
                    name,
                    members: dedup(members),
                })
            }
            C::UpdateGroupChat { id, name, members } => {
                self.require_group(&id)?;
                if name.is_none() && members.is_none() {
                    return Err(CoreError::EmptyGroupUpdate);
                }
                if name.as_ref().is_some_and(|n| n.trim().is_empty()) {
                    return Err(CoreError::EmptyField("group chat name"));
                }
                if let Some(ms) = &members {
                    for m in ms {
                        self.require_member(m)?;
                    }
                }
                Ok(E::GroupChatUpdated {
                    id,
                    name,
                    members: members.map(dedup),
                })
            }
            C::DeleteGroupChat { id } => {
                self.require_group(&id)?;
                Ok(E::GroupChatDeleted { id })
            }
            C::AddReviewer { task, reviewer } => {
                let t = self.require_task(&task)?;
                self.require_member(&reviewer)?;
                if t.reviewer.as_deref() == Some(reviewer.as_str())
                    || t.reviewers.contains(&reviewer)
                {
                    return Err(CoreError::ReviewerExists { task, reviewer });
                }
                Ok(E::ReviewerAdded { task, reviewer })
            }
            C::RemoveReviewer { task, reviewer } => {
                let t = self.require_task(&task)?;
                if !t.reviewers.contains(&reviewer) {
                    return Err(CoreError::ReviewerNotFound { task, reviewer });
                }
                Ok(E::ReviewerRemoved { task, reviewer })
            }
            C::RestoreTask { task } => {
                if !self.require_task(&task)?.archived {
                    return Err(CoreError::NotArchived(task));
                }
                Ok(E::TaskArchived {
                    task,
                    archived: false,
                })
            }
            C::SetColumn { task, column } => {
                let t = self.require_task(&task)?;
                let to =
                    serde_json::from_value::<Status>(serde_json::Value::String(column.clone()))
                        .map_err(|_| CoreError::UnknownColumn(column.clone()))?;
                if matches!(to, Status::NeedsFix | Status::Blocked) {
                    return Err(CoreError::UseDedicatedCommand { task, to });
                }
                if t.status == Status::Blocked || !status_path_exists(t.status, to) {
                    return Err(CoreError::NoPath {
                        task,
                        from: t.status,
                        to,
                    });
                }
                Ok(E::TaskColumnSet {
                    task,
                    from: t.status,
                    to,
                })
            }
            C::ClearColumn { column } => {
                let st =
                    serde_json::from_value::<Status>(serde_json::Value::String(column.clone()))
                        .map_err(|_| CoreError::UnknownColumn(column.clone()))?;
                if !matches!(st, Status::Approved | Status::Done) {
                    return Err(CoreError::ColumnNotClearable(column));
                }
                let tasks: Vec<String> = self
                    .tasks
                    .iter()
                    .filter(|t| !t.archived && t.status == st)
                    .map(|t| t.id.clone())
                    .collect();
                if tasks.is_empty() {
                    return Err(CoreError::ColumnEmpty(column));
                }
                Ok(E::ColumnCleared { column, tasks })
            }
            C::Heartbeat {
                member,
                status,
                task,
                note,
            } => {
                self.require_member(&member)?;
                if !matches!(
                    status.as_str(),
                    "starting" | "ready" | "working" | "idle" | "stuck"
                ) {
                    return Err(CoreError::InvalidHeartbeat(status));
                }
                if let Some(t) = &task {
                    self.require_task(t)?;
                }
                Ok(E::MemberHeartbeat {
                    member,
                    status,
                    task,
                    note,
                })
            }
            C::LaunchRequest { members } => {
                for m in &members {
                    self.require_member(m)?;
                }
                Ok(E::LaunchRequested {
                    members: dedup(members),
                })
            }
            C::StopRequest { members } => {
                for m in &members {
                    self.require_member(m)?;
                }
                Ok(E::StopRequested {
                    members: dedup(members),
                })
            }
        }
    }
}

fn check_attachments(list: &[Attachment]) -> Result<(), CoreError> {
    for a in list {
        if a.name.trim().is_empty() {
            return Err(CoreError::EmptyField("attachment name"));
        }
        if a.path.trim().is_empty() {
            return Err(CoreError::EmptyField("attachment path"));
        }
    }
    Ok(())
}

/// True if `to` is reachable from `from` over legal edges without passing through Blocked/NeedsFix
/// (those have dedicated commands) and `from != to`.
fn status_path_exists(from: Status, to: Status) -> bool {
    if from == to {
        return false;
    }
    let mut seen = vec![from];
    let mut q = VecDeque::from([from]);
    while let Some(s) = q.pop_front() {
        for n in Status::ALL {
            if matches!(n, Status::Blocked | Status::NeedsFix)
                || seen.contains(&n)
                || !s.can_transition_to(n)
            {
                continue;
            }
            if n == to {
                return true;
            }
            seen.push(n);
            q.push_back(n);
        }
    }
    false
}

fn check_runtime(runtime: &Option<String>) -> Result<(), CoreError> {
    match runtime {
        Some(r) if r.trim().is_empty() => Err(CoreError::EmptyRuntime),
        _ => Ok(()),
    }
}

fn dedup(mut ids: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    ids.retain(|id| seen.insert(id.clone()));
    ids
}
