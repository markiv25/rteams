//! Wave 3 commands: clarification, attachments, links, review start, column order, team delete,
//! reactions, schedules, permissions, runtime/worktree.
mod common;

use common::{transition, Fixture};
use rteams_core::{Command, CoreError, Schedule, State, Status};
use serde_json::json;

fn sched(id: &str, member: &str, mins: u32) -> Schedule {
    Schedule {
        id: id.into(),
        member: member.into(),
        prompt: "check in".into(),
        every_minutes: mins,
        enabled: true,
        cron: None,
        timezone: None,
        warmup_minutes: None,
    }
}

#[test]
fn clarification_set_and_cleared() {
    let mut f = Fixture::new();
    f.task("1", Some("echo"));
    f.run(
        "echo",
        Command::SetClarification {
            task: "1".into(),
            question: Some("which db?".into()),
        },
    );
    assert_eq!(
        f.store.state().task("1").unwrap().clarification.as_deref(),
        Some("which db?")
    );
    f.run(
        "lead",
        Command::SetClarification {
            task: "1".into(),
            question: None,
        },
    );
    assert!(f.store.state().task("1").unwrap().clarification.is_none());
    assert_eq!(
        f.try_run(
            "echo",
            Command::SetClarification {
                task: "1".into(),
                question: Some("  ".into())
            }
        ),
        Err(CoreError::EmptyField("clarification question"))
    );
    assert_eq!(
        f.try_run(
            "echo",
            Command::SetClarification {
                task: "nope".into(),
                question: None
            }
        ),
        Err(CoreError::TaskNotFound("nope".into()))
    );
}

#[test]
fn attach_file_appends_and_validates() {
    let mut f = Fixture::new();
    f.task("1", None);
    f.run(
        "echo",
        Command::AttachFile {
            task: "1".into(),
            name: "spec.md".into(),
            path: "/tmp/spec.md".into(),
        },
    );
    let t = f.store.state().task("1").unwrap();
    assert_eq!(t.attachments.len(), 1);
    assert_eq!(t.attachments[0].name, "spec.md");
    assert!(matches!(
        f.try_run(
            "echo",
            Command::AttachFile {
                task: "1".into(),
                name: "".into(),
                path: "/p".into()
            }
        ),
        Err(CoreError::EmptyField(_))
    ));
    assert!(matches!(
        f.try_run(
            "echo",
            Command::AttachFile {
                task: "1".into(),
                name: "n".into(),
                path: " ".into()
            }
        ),
        Err(CoreError::EmptyField(_))
    ));
}

#[test]
fn link_is_symmetric_and_unlink_removes_both_sides() {
    let mut f = Fixture::new();
    f.task("1", None);
    f.task("2", None);
    let link = |linked| Command::LinkTasks {
        task: "1".into(),
        related: "2".into(),
        linked,
    };
    f.run("lead", link(true));
    f.run("lead", link(true)); // idempotent: no duplicates
    assert_eq!(f.store.state().task("1").unwrap().related, vec!["2"]);
    assert_eq!(f.store.state().task("2").unwrap().related, vec!["1"]);
    f.run("lead", link(false));
    assert!(f.store.state().task("1").unwrap().related.is_empty());
    assert!(f.store.state().task("2").unwrap().related.is_empty());
    assert!(matches!(
        f.try_run(
            "lead",
            Command::LinkTasks {
                task: "1".into(),
                related: "1".into(),
                linked: true
            }
        ),
        Err(CoreError::InvalidLink { .. })
    ));
    assert_eq!(
        f.try_run(
            "lead",
            Command::LinkTasks {
                task: "1".into(),
                related: "9".into(),
                linked: true
            }
        ),
        Err(CoreError::TaskNotFound("9".into()))
    );
}

#[test]
fn review_start_requires_review_and_resets_on_new_request() {
    let mut f = Fixture::new();
    f.task("1", Some("echo"));
    let start = || Command::StartReview { task: "1".into() };
    assert!(matches!(
        f.try_run("pixel", start()),
        Err(CoreError::NotInReview { .. })
    ));
    f.run("echo", transition("1", Status::InProgress));
    f.run(
        "echo",
        Command::RequestReview {
            task: "1".into(),
            reviewer: "pixel".into(),
        },
    );
    assert!(!f.store.state().task("1").unwrap().review_started);
    f.run("pixel", start());
    assert!(f.store.state().task("1").unwrap().review_started);
    f.run(
        "pixel",
        Command::RequestChanges {
            task: "1".into(),
            comment: "no".into(),
        },
    );
    assert!(!f.store.state().task("1").unwrap().review_started);
    assert!(matches!(
        f.try_run("pixel", start()),
        Err(CoreError::NotInReview { .. })
    ));
}

#[test]
fn column_order_validates_ids_and_column() {
    let mut f = Fixture::new();
    f.task("1", None);
    f.task("2", None);
    f.run(
        "lead",
        Command::SetColumnOrder {
            column: "pending".into(),
            order: vec!["2".into(), "1".into(), "2".into()],
        },
    );
    assert_eq!(
        f.store.state().team.as_ref().unwrap().column_order["pending"],
        vec!["2", "1"]
    );
    assert_eq!(
        f.try_run(
            "lead",
            Command::SetColumnOrder {
                column: "pending".into(),
                order: vec!["9".into()]
            }
        ),
        Err(CoreError::TaskNotFound("9".into()))
    );
    assert_eq!(
        f.try_run(
            "lead",
            Command::SetColumnOrder {
                column: "bogus".into(),
                order: vec![]
            }
        ),
        Err(CoreError::UnknownColumn("bogus".into()))
    );
}

#[test]
fn deleted_team_rejects_everything_but_restore() {
    let mut f = Fixture::new();
    f.task("1", None);
    f.run("lead", Command::DeleteTeam { deleted: true });
    assert!(f.store.state().team.as_ref().unwrap().deleted);
    assert_eq!(
        f.try_run(
            "lead",
            Command::SendMessage {
                to: None,
                text: "x".into(),
                mode: None,
                from_team: None
            }
        ),
        Err(CoreError::TeamDeleted)
    );
    assert_eq!(
        f.try_run("lead", Command::DeleteTeam { deleted: true }),
        Err(CoreError::TeamDeleted)
    );
    assert_eq!(
        f.try_run("lead", transition("1", Status::InProgress)),
        Err(CoreError::TeamDeleted)
    );
    f.run("lead", Command::DeleteTeam { deleted: false });
    assert!(!f.store.state().team.as_ref().unwrap().deleted);
    f.run("lead", transition("1", Status::InProgress));
    // Replay reproduces the flag.
    let rebuilt = State::from_events(f.store.events());
    assert!(!rebuilt.team.unwrap().deleted);
}

#[test]
fn delete_needs_a_team() {
    let mut s = rteams_core::MemStore::new();
    assert_eq!(
        s.execute("a", 1, Command::DeleteTeam { deleted: true })
            .unwrap_err(),
        CoreError::TeamNotCreated
    );
}

#[test]
fn reactions_toggle_per_actor_and_emoji() {
    let mut f = Fixture::new();
    f.run(
        "echo",
        Command::SendMessage {
            to: None,
            text: "hi".into(),
            mode: None,
            from_team: None,
        },
    );
    let seq = f.store.state().messages[0].seq;
    let react = |e: &str| Command::React {
        message_seq: seq,
        emoji: e.into(),
    };
    f.run("pixel", react("+1"));
    f.run("ember", react("+1"));
    f.run("pixel", react("tada"));
    let m = &f.store.state().messages[0];
    assert_eq!(m.reactions.len(), 3);
    f.run("pixel", react("+1")); // toggles off
    let m = &f.store.state().messages[0];
    assert_eq!(m.reactions.len(), 2);
    assert!(!m
        .reactions
        .iter()
        .any(|r| r.from == "pixel" && r.emoji == "+1"));
    assert_eq!(
        f.try_run(
            "pixel",
            Command::React {
                message_seq: 999,
                emoji: "x".into()
            }
        ),
        Err(CoreError::MessageNotFound(999))
    );
    assert!(matches!(
        f.try_run("pixel", react(" ")),
        Err(CoreError::EmptyField(_))
    ));
}

#[test]
fn from_team_is_carried_on_the_message() {
    let mut f = Fixture::new();
    f.run(
        "outsider",
        Command::SendMessage {
            to: Some("echo".into()),
            text: "hello".into(),
            mode: None,
            from_team: Some("other".into()),
        },
    );
    assert_eq!(
        f.store.state().messages[0].from_team.as_deref(),
        Some("other")
    );
    assert!(matches!(
        f.try_run(
            "outsider",
            Command::SendMessage {
                to: Some("ghost".into()),
                text: "x".into(),
                mode: None,
                from_team: Some("other".into())
            }
        ),
        Err(CoreError::UnknownMember(_))
    ));
}

#[test]
fn schedules_upsert_remove_and_validate() {
    let mut f = Fixture::new();
    f.run(
        "lead",
        Command::SetSchedule {
            schedule: sched("s1", "echo", 5),
        },
    );
    f.run(
        "lead",
        Command::SetSchedule {
            schedule: sched("s2", "pixel", 1),
        },
    );
    let mut changed = sched("s1", "echo", 10);
    changed.enabled = false;
    f.run("lead", Command::SetSchedule { schedule: changed });
    let sc = &f.store.state().team.as_ref().unwrap().schedules;
    assert_eq!(sc.len(), 2, "ids stay unique");
    assert_eq!((sc[0].every_minutes, sc[0].enabled), (10, false));
    assert_eq!(
        f.try_run(
            "lead",
            Command::SetSchedule {
                schedule: sched("s3", "echo", 0)
            }
        ),
        Err(CoreError::InvalidInterval)
    );
    assert_eq!(
        f.try_run(
            "lead",
            Command::SetSchedule {
                schedule: sched("s3", "ghost", 5)
            }
        ),
        Err(CoreError::UnknownMember("ghost".into()))
    );
    assert!(matches!(
        f.try_run(
            "lead",
            Command::SetSchedule {
                schedule: sched("", "echo", 5)
            }
        ),
        Err(CoreError::EmptyField(_))
    ));
    f.run("lead", Command::RemoveSchedule { id: "s1".into() });
    assert_eq!(f.store.state().team.as_ref().unwrap().schedules.len(), 1);
    assert_eq!(
        f.try_run("lead", Command::RemoveSchedule { id: "s1".into() }),
        Err(CoreError::ScheduleNotFound("s1".into()))
    );
}

#[test]
fn permissions_request_then_decide_once() {
    let mut f = Fixture::new();
    let req = |id: &str, member: &str| Command::RequestPermission {
        id: id.into(),
        member: member.into(),
        tool: "Bash".into(),
        input: json!({ "command": "ls" }),
    };
    f.run("echo", req("echo-1", "echo"));
    let p = &f.store.state().permissions[0];
    assert_eq!(
        (p.decided, p.input.clone()),
        (None, json!({ "command": "ls" }))
    );
    assert_eq!(
        f.try_run("echo", req("echo-1", "echo")),
        Err(CoreError::PermissionExists("echo-1".into()))
    );
    assert_eq!(
        f.try_run("x", req("g-1", "ghost")),
        Err(CoreError::UnknownMember("ghost".into()))
    );
    f.run(
        "lead",
        Command::DecidePermission {
            id: "echo-1".into(),
            allow: false,
            note: Some("no".into()),
        },
    );
    let p = &f.store.state().permissions[0];
    assert_eq!((p.decided, p.note.as_deref()), (Some(false), Some("no")));
    assert_eq!(
        f.try_run(
            "lead",
            Command::DecidePermission {
                id: "echo-1".into(),
                allow: true,
                note: None
            }
        ),
        Err(CoreError::PermissionDecided("echo-1".into()))
    );
    assert_eq!(
        f.try_run(
            "lead",
            Command::DecidePermission {
                id: "zzz".into(),
                allow: true,
                note: None
            }
        ),
        Err(CoreError::PermissionNotFound("zzz".into()))
    );
}

#[test]
fn member_runtime_and_worktree() {
    let mut f = Fixture::new();
    f.run(
        "lead",
        Command::AddMember {
            name: "nova".into(),
            role: "dev".into(),
            runtime: Some("codex".into()),
        },
    );
    assert_eq!(
        f.try_run(
            "lead",
            Command::AddMember {
                name: "bad".into(),
                role: "dev".into(),
                runtime: Some(" ".into())
            }
        ),
        Err(CoreError::EmptyRuntime)
    );
    let upd = |runtime: Option<&str>, worktree: Option<bool>| Command::UpdateMember {
        name: "nova".into(),
        role: None,
        prompt: None,
        model: None,
        runtime: runtime.map(Into::into),
        worktree,
        icon: None,
    };
    f.run("lead", upd(Some("gemini"), Some(true)));
    let m = f
        .store
        .state()
        .team
        .as_ref()
        .unwrap()
        .members
        .iter()
        .find(|m| m.name == "nova")
        .unwrap();
    assert_eq!((m.runtime.as_deref(), m.worktree), (Some("gemini"), true));
    f.run("lead", upd(None, Some(false)));
    assert_eq!(
        f.try_run("lead", upd(Some(""), None)),
        Err(CoreError::EmptyRuntime)
    );
    assert_eq!(
        f.try_run("lead", upd(None, None)),
        Err(CoreError::EmptyMemberUpdate)
    );
}
