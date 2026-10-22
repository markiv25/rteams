//! Write path. Every change the GUI makes is a tool call on `rteams_server::Server` over the
//! same `FileStore` the MCP servers use, so ids, authorization policy, locking and validation
//! are exactly the agents' rules. Nothing here talks to the log directly.
//!
//! The store is opened lazily on the first write: a GUI that only watches creates no files.

use std::path::PathBuf;

use rteams_server::Server;
use rteams_services::i18n::I18n;
use rteams_store::FileStore;
use serde_json::{json, Value};

/// One thing the user asked for. `to_call` turns it into a tool name and arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuiCmd {
    CreateTeam {
        name: String,
        /// (name, role); the actor joins as lead automatically.
        members: Vec<(String, String)>,
    },
    CreateTask {
        subject: String,
        description: String,
        owner: Option<String>,
        blocked_by: Vec<String>,
    },
    SetOwner {
        task: String,
        owner: Option<String>,
    },
    Start {
        task: String,
    },
    Complete {
        task: String,
    },
    Block {
        task: String,
        reason: String,
    },
    Unblock {
        task: String,
    },
    Archive {
        task: String,
        archived: bool,
    },
    Comment {
        task: String,
        text: String,
    },
    RequestReview {
        task: String,
        reviewer: String,
    },
    Approve {
        task: String,
        note: Option<String>,
    },
    RequestChanges {
        task: String,
        comment: String,
    },
    /// Edit a task's subject and/or description (`None` = unchanged).
    UpdateTask {
        task: String,
        subject: Option<String>,
        description: Option<String>,
    },
    /// Add a member to the roster (lead only).
    AddMember {
        name: String,
        role: String,
    },
    /// `to: None` broadcasts. `mode` is `do`, `ask` or `delegate` (None = do).
    Message {
        to: Option<String>,
        text: String,
        mode: Option<String>,
    },
    /// Message with attachments and/or into a user-created group chat (`chat` = its id).
    MessageRich {
        to: Option<String>,
        chat: Option<String>,
        text: String,
        mode: Option<String>,
        attachments: Vec<rteams_core::Attachment>,
    },
    GroupChatCreate {
        id: String,
        name: String,
        members: Vec<String>,
    },
    GroupChatUpdate {
        id: String,
        name: Option<String>,
        members: Option<Vec<String>>,
    },
    GroupChatDelete {
        id: String,
    },
    /// A message delivered into THIS team from a member of another team (`from_team`).
    CrossTeamMessage {
        to: Option<String>,
        text: String,
        from_team: String,
    },
    /// Team settings: where agents work and the lead's standing instructions.
    ConfigureTeam {
        project_path: Option<String>,
        lead_prompt: Option<String>,
    },
    /// Edit a member (`None` = unchanged). `prompt` = standing instructions for that agent;
    /// `model` = claude model alias passed at launch (e.g. `sonnet`).
    UpdateMember {
        name: String,
        role: Option<String>,
        prompt: Option<String>,
        model: Option<String>,
        /// Avatar icon; `Some("")` resets it to the default.
        icon: Option<String>,
    },
    /// A member's saved launch settings (`None` = unchanged): effort (`Some("")` clears),
    /// fast mode, and context limit (`Some(0)` clears).
    SetMemberLaunch {
        name: String,
        effort: Option<String>,
        fast: Option<bool>,
        context_limit: Option<u32>,
    },
    /// Remove (`removed: true`) or restore a member; history is kept.
    RemoveMember {
        name: String,
        removed: bool,
    },
    /// Which agent CLI runs this member (`claude`, `codex`, `opencode`, ...) and whether it
    /// works in its own git worktree (`None` = unchanged).
    SetMemberRuntime {
        name: String,
        runtime: Option<String>,
        worktree: Option<bool>,
    },
    /// Extra MCP server names the member's agent may use (`mcp_allow`; replaces the list).
    SetMemberMcp {
        name: String,
        allow: Vec<String>,
    },
    /// `question: None` clears the "waiting for answer" flag.
    SetClarification {
        task: String,
        question: Option<String>,
    },
    AttachFile {
        task: String,
        name: String,
        path: String,
    },
    LinkTasks {
        task: String,
        related: String,
        linked: bool,
    },
    StartReview {
        task: String,
    },
    /// Manual order of one kanban column (`column` = status in snake_case).
    SetColumnOrder {
        column: String,
        order: Vec<String>,
    },
    /// Change the display name (the directory stays the team id).
    RenameTeam {
        name: String,
    },
    DeleteTeam {
        deleted: bool,
    },
    /// Toggle `emoji` on a message.
    React {
        message_seq: u64,
        emoji: String,
    },
    SetSchedule {
        id: String,
        member: String,
        prompt: String,
        every_minutes: u32,
        enabled: bool,
        /// 5-field cron expression; wins over `every_minutes` when set.
        cron: Option<String>,
        timezone: Option<String>,
        warmup_minutes: Option<u32>,
    },
    RemoveSchedule {
        id: String,
    },
    /// Answer an agent's tool-approval request.
    DecidePermission {
        id: String,
        allow: bool,
        note: Option<String>,
    },
    CreateTaskFromMessage {
        message_seq: u64,
        owner: Option<String>,
    },
    /// Kill a registered process (the original's "kill process") and mark it stopped.
    KillProcess {
        pid: u32,
    },
}

impl GuiCmd {
    /// Save `s` as-is (all fields, including cron / timezone / warm-up).
    pub fn set_schedule(s: &rteams_core::Schedule) -> GuiCmd {
        GuiCmd::SetSchedule {
            id: s.id.clone(),
            member: s.member.clone(),
            prompt: s.prompt.clone(),
            every_minutes: s.every_minutes,
            enabled: s.enabled,
            cron: s.cron.clone(),
            timezone: s.timezone.clone(),
            warmup_minutes: s.warmup_minutes,
        }
    }

    /// Tool name and JSON arguments, with `actor` filled in.
    pub fn to_call(&self, actor: &str) -> (&'static str, Value) {
        use GuiCmd::*;
        match self {
            CreateTeam { name, members } => {
                let members: Vec<Value> = members
                    .iter()
                    .map(|(n, r)| json!({ "name": n, "role": r }))
                    .collect();
                (
                    "team_create",
                    json!({ "actor": actor, "name": name, "members": members }),
                )
            }
            CreateTask {
                subject,
                description,
                owner,
                blocked_by,
            } => (
                "task_create",
                json!({ "actor": actor, "subject": subject, "description": description,
                        "owner": owner, "blocked_by": blocked_by }),
            ),
            SetOwner { task, owner } => (
                "task_set_owner",
                json!({ "actor": actor, "task": task, "owner": owner }),
            ),
            Start { task } => ("task_start", json!({ "actor": actor, "task": task })),
            Complete { task } => ("task_complete", json!({ "actor": actor, "task": task })),
            Block { task, reason } => (
                "task_block",
                json!({ "actor": actor, "task": task, "reason": reason }),
            ),
            Unblock { task } => ("task_unblock", json!({ "actor": actor, "task": task })),
            Archive { task, archived } => (
                "task_archive",
                json!({ "actor": actor, "task": task, "archived": archived }),
            ),
            Comment { task, text } => (
                "task_add_comment",
                json!({ "actor": actor, "task": task, "text": text }),
            ),
            RequestReview { task, reviewer } => (
                "review_request",
                json!({ "actor": actor, "task": task, "reviewer": reviewer }),
            ),
            Approve { task, note } => (
                "review_approve",
                json!({ "actor": actor, "task": task, "note": note }),
            ),
            RequestChanges { task, comment } => (
                "review_request_changes",
                json!({ "actor": actor, "task": task, "comment": comment }),
            ),
            UpdateTask {
                task,
                subject,
                description,
            } => (
                "task_update",
                json!({ "actor": actor, "task": task, "subject": subject,
                        "description": description }),
            ),
            AddMember { name, role } => (
                "member_add",
                json!({ "actor": actor, "name": name, "role": role }),
            ),
            Message { to, text, mode } => (
                "message_send",
                json!({ "actor": actor, "to": to, "text": text, "mode": mode }),
            ),
            MessageRich {
                to,
                chat,
                text,
                mode,
                attachments,
            } => (
                "message_send",
                json!({ "actor": actor, "to": to, "chat": chat, "text": text, "mode": mode,
                        "attachments": attachments }),
            ),
            GroupChatCreate { id, name, members } => (
                "group_chat_create",
                json!({ "actor": actor, "id": id, "name": name, "members": members }),
            ),
            GroupChatUpdate { id, name, members } => (
                "group_chat_update",
                json!({ "actor": actor, "id": id, "name": name, "members": members }),
            ),
            GroupChatDelete { id } => ("group_chat_delete", json!({ "actor": actor, "id": id })),
            CrossTeamMessage {
                to,
                text,
                from_team,
            } => (
                "message_send",
                json!({ "actor": actor, "to": to, "text": text, "from_team": from_team }),
            ),
            ConfigureTeam {
                project_path,
                lead_prompt,
            } => (
                "team_configure",
                json!({ "actor": actor, "project_path": project_path,
                        "lead_prompt": lead_prompt }),
            ),
            UpdateMember {
                name,
                role,
                prompt,
                model,
                icon,
            } => (
                "member_update",
                json!({ "actor": actor, "name": name, "role": role, "prompt": prompt,
                        "model": model, "icon": icon }),
            ),
            SetMemberLaunch {
                name,
                effort,
                fast,
                context_limit,
            } => (
                "member_update",
                json!({ "actor": actor, "name": name, "effort": effort, "fast": fast,
                        "context_limit": context_limit }),
            ),
            RemoveMember { name, removed } => (
                "member_remove",
                json!({ "actor": actor, "name": name, "removed": removed }),
            ),
            SetMemberRuntime {
                name,
                runtime,
                worktree,
            } => (
                "member_update",
                json!({ "actor": actor, "name": name, "runtime": runtime,
                        "worktree": worktree }),
            ),
            SetMemberMcp { name, allow } => (
                "member_update",
                json!({ "actor": actor, "name": name, "mcp_allow": allow }),
            ),
            SetClarification { task, question } => (
                "task_set_clarification",
                json!({ "actor": actor, "task": task, "question": question }),
            ),
            AttachFile { task, name, path } => (
                "task_attach_file",
                json!({ "actor": actor, "task": task, "name": name, "path": path }),
            ),
            LinkTasks {
                task,
                related,
                linked,
            } => (
                if *linked { "task_link" } else { "task_unlink" },
                json!({ "actor": actor, "task": task, "related": related }),
            ),
            StartReview { task } => ("review_start", json!({ "actor": actor, "task": task })),
            SetColumnOrder { column, order } => (
                "kanban_set_order",
                json!({ "actor": actor, "column": column, "order": order }),
            ),
            RenameTeam { name } => ("team_update", json!({ "actor": actor, "name": name })),
            DeleteTeam { deleted } => {
                ("team_delete", json!({ "actor": actor, "deleted": deleted }))
            }
            React { message_seq, emoji } => (
                "message_react",
                json!({ "actor": actor, "message_seq": message_seq, "emoji": emoji }),
            ),
            SetSchedule {
                id,
                member,
                prompt,
                every_minutes,
                enabled,
                cron,
                timezone,
                warmup_minutes,
            } => (
                "schedule_set",
                json!({ "actor": actor, "id": id, "member": member, "prompt": prompt,
                        "every_minutes": every_minutes, "enabled": enabled, "cron": cron,
                        "timezone": timezone, "warmup_minutes": warmup_minutes }),
            ),
            RemoveSchedule { id } => ("schedule_remove", json!({ "actor": actor, "id": id })),
            DecidePermission { id, allow, note } => (
                "permission_decide",
                json!({ "actor": actor, "id": id, "allow": allow, "note": note }),
            ),
            CreateTaskFromMessage { message_seq, owner } => (
                "task_create_from_message",
                json!({ "actor": actor, "message_seq": message_seq, "owner": owner }),
            ),
            KillProcess { pid } => ("process_unregister", json!({ "actor": actor, "pid": pid })),
        }
    }

    /// Short past-tense line for the status bar after success, in the active language.
    pub fn done_label(&self, tr: &I18n) -> String {
        use GuiCmd::*;
        match self {
            CreateTeam { name, .. } => {
                tr.t_args("rteams.done.createTeam", &[("name", &name.to_string())])
            }
            CreateTask { subject, .. } => tr.t_args(
                "rteams.done.createTask",
                &[("subject", &subject.to_string())],
            ),
            SetOwner {
                task,
                owner: Some(o),
            } => tr.t_args(
                "rteams.done.assign",
                &[("task", &task.to_string()), ("o", &o.to_string())],
            ),
            SetOwner { task, owner: None } => {
                tr.t_args("rteams.done.unassign", &[("task", &task.to_string())])
            }
            Start { task } => tr.t_args("rteams.done.start", &[("task", &task.to_string())]),
            Complete { task } => tr.t_args("rteams.done.complete", &[("task", &task.to_string())]),
            Block { task, .. } => tr.t_args("rteams.done.block", &[("task", &task.to_string())]),
            Unblock { task } => tr.t_args("rteams.done.unblock", &[("task", &task.to_string())]),
            Archive {
                task,
                archived: true,
            } => tr.t_args("rteams.done.archive", &[("task", &task.to_string())]),
            Archive { task, .. } => {
                tr.t_args("rteams.done.restoreTask", &[("task", &task.to_string())])
            }
            Comment { task, .. } => {
                tr.t_args("rteams.done.comment", &[("task", &task.to_string())])
            }
            RequestReview { task, reviewer } => tr.t_args(
                "rteams.done.requestReview",
                &[
                    ("reviewer", &reviewer.to_string()),
                    ("task", &task.to_string()),
                ],
            ),
            Approve { task, .. } => {
                tr.t_args("rteams.done.approve", &[("task", &task.to_string())])
            }
            RequestChanges { task, .. } => {
                tr.t_args("rteams.done.requestChanges", &[("task", &task.to_string())])
            }
            Message { to: Some(t), .. } => {
                tr.t_args("rteams.done.messageTo", &[("t", &t.to_string())])
            }
            Message { to: None, .. } => tr.t("rteams.done.broadcast"),
            MessageRich { to: Some(t), .. } => {
                tr.t_args("rteams.done.messageTo", &[("t", &t.to_string())])
            }
            MessageRich { to: None, .. } => tr.t("rteams.done.broadcast"),
            GroupChatCreate { name, .. } => {
                tr.t_args("rteams.done.groupCreate", &[("name", &name.to_string())])
            }
            GroupChatUpdate { .. } => tr.t("rteams.done.groupUpdate"),
            GroupChatDelete { .. } => tr.t("rteams.done.groupDelete"),
            UpdateTask { task, .. } => {
                tr.t_args("rteams.done.updateTask", &[("task", &task.to_string())])
            }
            AddMember { name, .. } => {
                tr.t_args("rteams.done.addMember", &[("name", &name.to_string())])
            }
            ConfigureTeam { .. } => tr.t("rteams.done.configureTeam"),
            CrossTeamMessage { from_team, .. } => tr.t_args(
                "rteams.done.crossTeam",
                &[("from_team", &from_team.to_string())],
            ),
            UpdateMember { name, .. } => {
                tr.t_args("rteams.done.updateMember", &[("name", &name.to_string())])
            }
            RemoveMember {
                name,
                removed: true,
            } => tr.t_args("rteams.done.removeMember", &[("name", &name.to_string())]),
            RemoveMember { name, .. } => {
                tr.t_args("rteams.done.restoreMember", &[("name", &name.to_string())])
            }
            SetMemberMcp { name, .. } => {
                tr.t_args("rteams.done.updateMember", &[("name", &name.to_string())])
            }
            SetMemberLaunch { name, .. } => {
                tr.t_args("rteams.done.updateMember", &[("name", &name.to_string())])
            }
            SetMemberRuntime { name, .. } => {
                tr.t_args("rteams.done.memberRuntime", &[("name", &name.to_string())])
            }
            SetClarification {
                task,
                question: Some(_),
            } => tr.t_args(
                "rteams.done.askClarification",
                &[("task", &task.to_string())],
            ),
            SetClarification { task, .. } => tr.t_args(
                "rteams.done.clearClarification",
                &[("task", &task.to_string())],
            ),
            AttachFile { task, name, .. } => tr.t_args(
                "rteams.done.attach",
                &[("name", &name.to_string()), ("task", &task.to_string())],
            ),
            LinkTasks {
                task,
                related,
                linked: true,
            } => tr.t_args(
                "rteams.done.link",
                &[
                    ("task", &task.to_string()),
                    ("related", &related.to_string()),
                ],
            ),
            LinkTasks { task, related, .. } => tr.t_args(
                "rteams.done.unlink",
                &[
                    ("task", &task.to_string()),
                    ("related", &related.to_string()),
                ],
            ),
            StartReview { task } => {
                tr.t_args("rteams.done.startReview", &[("task", &task.to_string())])
            }
            SetColumnOrder { column, .. } => {
                tr.t_args("rteams.done.reorder", &[("column", &column.to_string())])
            }
            RenameTeam { .. } => tr.t("rteams.done.renameTeam"),
            DeleteTeam { deleted: true } => tr.t("rteams.done.deleteTeam"),
            DeleteTeam { .. } => tr.t("rteams.done.restoreTeam"),
            React { emoji, .. } => tr.t_args("rteams.done.react", &[("emoji", &emoji.to_string())]),
            SetSchedule { id, .. } => {
                tr.t_args("rteams.done.setSchedule", &[("id", &id.to_string())])
            }
            RemoveSchedule { id } => {
                tr.t_args("rteams.done.removeSchedule", &[("id", &id.to_string())])
            }
            DecidePermission { allow: true, .. } => tr.t("rteams.done.allow"),
            DecidePermission { .. } => tr.t("rteams.done.deny"),
            CreateTaskFromMessage { .. } => tr.t("rteams.done.fromMessage"),
            KillProcess { pid } => format!("Killed process {pid}"),
        }
    }
}

/// Executes `GuiCmd`s for one team as one actor.
pub struct Commander {
    home: PathBuf,
    team: String,
    server: Option<Server>,
}

impl Commander {
    pub fn new(home: PathBuf, team: String) -> Self {
        Commander {
            home,
            team,
            server: None,
        }
    }

    /// Run `cmd` as `actor`. `Ok` is the tool's JSON result; `Err` is a readable message
    /// (domain error, forbidden, store failure).
    pub fn run(&mut self, actor: &str, cmd: &GuiCmd) -> Result<Value, String> {
        if self.server.is_none() {
            let store = FileStore::open(&self.home, &self.team).map_err(|e| e.to_string())?;
            self.server = Some(Server::with_store(store));
        }
        let (name, args) = cmd.to_call(actor);
        let server = self.server.as_ref().expect("opened above");
        if let GuiCmd::KillProcess { pid } = cmd {
            // Only a process an agent registered as running in this team is ever signalled.
            let list = server
                .call_tool("process_list", json!({}))
                .map_err(|e| e.to_string())?;
            let live = list["processes"]
                .as_array()
                .is_some_and(|ps| ps.iter().any(|p| p["pid"] == *pid && p["running"] == true));
            if !live {
                return Err(format!("process {pid} is not a running registered process"));
            }
            let out = server.call_tool(name, args).map_err(|e| e.to_string())?;
            kill_pid(*pid)?;
            return Ok(out);
        }
        server.call_tool(name, args).map_err(|e| e.to_string())
    }
}

/// Ask the OS to terminate `pid` (SIGTERM; `taskkill /T` on Windows). Refuses pid 0 and 1.
pub fn kill_pid(pid: u32) -> Result<(), String> {
    if pid <= 1 {
        return Err("refusing to signal pid 0/1".to_string());
    }
    #[cfg(unix)]
    let out = std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .output();
    #[cfg(windows)]
    let out = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        // Already gone counts as killed: the registry entry is what matters.
        Ok(o) if String::from_utf8_lossy(&o.stderr).contains("No such process") => Ok(()),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::Data;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("rteams-gui-act-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[cfg(unix)]
    #[test]
    fn kill_process_signals_only_registered_running_pids() {
        let home = tmp("kill");
        let mut c = Commander::new(home.clone(), "t".into());
        c.run(
            "boss",
            &GuiCmd::CreateTeam {
                name: "t".into(),
                members: vec![("ada".into(), "dev".into())],
            },
        )
        .unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id();
        // Not registered: refused, and the child lives on.
        assert!(c.run("boss", &GuiCmd::KillProcess { pid }).is_err());
        assert!(child.try_wait().unwrap().is_none());
        c.server
            .as_ref()
            .unwrap()
            .call_tool(
                "process_register",
                json!({ "actor": "ada", "pid": pid, "label": "sleeper" }),
            )
            .unwrap();
        c.run("boss", &GuiCmd::KillProcess { pid }).unwrap();
        let status = child.wait().unwrap();
        assert!(!status.success());
        let d = Data::open(home.clone(), "t").unwrap();
        assert!(d.state().processes.iter().all(|p| !p.running));
        assert!(kill_pid(1).is_err());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn commands_go_through_the_server_and_show_up_in_data() {
        let home = tmp("flow");
        let mut c = Commander::new(home.clone(), "t".into());
        c.run(
            "boss",
            &GuiCmd::CreateTeam {
                name: "t".into(),
                members: vec![("ada".into(), "dev".into()), ("bob".into(), "dev".into())],
            },
        )
        .unwrap();
        let r = c
            .run(
                "boss",
                &GuiCmd::CreateTask {
                    subject: "build it".into(),
                    description: "".into(),
                    owner: Some("ada".into()),
                    blocked_by: vec![],
                },
            )
            .unwrap();
        let id = r["task"]["id"].as_str().unwrap().to_string();
        for cmd in [
            GuiCmd::Start { task: id.clone() },
            GuiCmd::RequestReview {
                task: id.clone(),
                reviewer: "bob".into(),
            },
            GuiCmd::Approve {
                task: id.clone(),
                note: None,
            },
            GuiCmd::Complete { task: id.clone() },
            GuiCmd::Message {
                to: None,
                text: "ship it".into(),
                mode: None,
            },
        ] {
            c.run("boss", &cmd)
                .unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
        }

        let mut d = Data::open(home.clone(), "t").unwrap();
        d.refresh().unwrap();
        assert_eq!(
            d.state().task(&id).unwrap().status,
            rteams_core::Status::Done
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn policy_errors_come_back_as_messages() {
        let home = tmp("policy");
        let mut c = Commander::new(home.clone(), "t".into());
        c.run(
            "boss",
            &GuiCmd::CreateTeam {
                name: "t".into(),
                members: vec![("ada".into(), "dev".into()), ("bob".into(), "dev".into())],
            },
        )
        .unwrap();
        c.run(
            "boss",
            &GuiCmd::CreateTask {
                subject: "x".into(),
                description: "".into(),
                owner: Some("ada".into()),
                blocked_by: vec![],
            },
        )
        .unwrap();
        let e = c
            .run("bob", &GuiCmd::Start { task: "1".into() })
            .unwrap_err();
        assert!(e.contains("forbidden"), "{e}");
        let e = c.run(
            "nobody",
            &GuiCmd::Comment {
                task: "1".into(),
                text: "hi".into(),
            },
        );
        assert!(e.is_err());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn nothing_is_created_until_the_first_write() {
        let home = tmp("lazy");
        let _c = Commander::new(home.clone(), "t".into());
        assert!(!home.exists());
    }
}
