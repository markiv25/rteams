//! Gap-wave additions: group chats, attachments, reviewers, restore/set/clear column,
//! heartbeats, launch/stop requests, member runtime options, wire format and old-log compat.
mod common;

use common::{transition, Fixture};
use rteams_core::{inbox, Attachment, Command, CoreError, Event, EventKind, Scope, State, Status};

fn att(n: &str) -> Attachment {
    Attachment {
        name: n.into(),
        path: format!("/tmp/{n}"),
    }
}

fn send(chat: Option<&str>, to: Option<&str>) -> Command {
    Command::SendMessage {
        to: to.map(Into::into),
        text: "hi".into(),
        mode: None,
        from_team: None,
        chat: chat.map(Into::into),
        attachments: vec![att("a.png")],
    }
}

#[test]
fn group_chat_lifecycle_and_messages() {
    let mut f = Fixture::new();
    f.run(
        "lead",
        Command::CreateGroupChat {
            id: "g1".into(),
            name: "Backend".into(),
            members: vec!["echo".into(), "pixel".into(), "echo".into()],
        },
    );
    let g = f.store.state().group("g1").unwrap().clone();
    assert_eq!(g.members, vec!["echo", "pixel"]);
    assert_eq!(g.created_by, "lead");
    assert!(matches!(
        f.try_run(
            "lead",
            Command::CreateGroupChat {
                id: "g1".into(),
                name: "x".into(),
                members: vec![]
            }
        ),
        Err(CoreError::GroupChatExists(_))
    ));
    assert!(matches!(
        f.try_run(
            "lead",
            Command::CreateGroupChat {
                id: "g2".into(),
                name: "x".into(),
                members: vec!["nobody".into()]
            }
        ),
        Err(CoreError::UnknownMember(_))
    ));
    f.run("lead", send(Some("g1"), None));
    let m = f.store.state().messages.last().unwrap().clone();
    assert_eq!(m.scope, Scope::Group { chat: "g1".into() });
    assert_eq!(m.attachments, vec![att("a.png")]);
    assert!(matches!(
        f.try_run("lead", send(Some("g1"), Some("echo"))),
        Err(CoreError::ChatWithRecipient)
    ));
    assert!(matches!(
        f.try_run("lead", send(Some("zz"), None)),
        Err(CoreError::GroupChatNotFound(_))
    ));
    // Inbox: only group members see it.
    assert_eq!(inbox(f.store.state(), "echo", 0).items.len(), 1);
    assert_eq!(inbox(f.store.state(), "ember", 0).items.len(), 0);
    f.run(
        "lead",
        Command::UpdateGroupChat {
            id: "g1".into(),
            name: Some("BE".into()),
            members: Some(vec!["ember".into()]),
        },
    );
    assert_eq!(f.store.state().group("g1").unwrap().name, "BE");
    assert_eq!(inbox(f.store.state(), "ember", 0).items.len(), 1);
    assert!(matches!(
        f.try_run(
            "lead",
            Command::UpdateGroupChat {
                id: "g1".into(),
                name: None,
                members: None
            }
        ),
        Err(CoreError::EmptyGroupUpdate)
    ));
    f.run("lead", Command::DeleteGroupChat { id: "g1".into() });
    assert!(f.store.state().group("g1").is_none());
    assert!(matches!(
        f.try_run("lead", Command::DeleteGroupChat { id: "g1".into() }),
        Err(CoreError::GroupChatNotFound(_))
    ));
}

#[test]
fn comment_attachments_validated() {
    let mut f = Fixture::new();
    f.task("1", Some("echo"));
    f.run(
        "echo",
        Command::Comment {
            task: "1".into(),
            text: "see".into(),
            attachments: vec![att("l.log")],
        },
    );
    assert_eq!(f.store.state().messages[0].attachments, vec![att("l.log")]);
    let bad = Attachment {
        name: " ".into(),
        path: "p".into(),
    };
    assert!(matches!(
        f.try_run(
            "echo",
            Command::Comment {
                task: "1".into(),
                text: "x".into(),
                attachments: vec![bad]
            }
        ),
        Err(CoreError::EmptyField(_))
    ));
}

#[test]
fn extra_reviewers() {
    let mut f = Fixture::new();
    f.task("1", Some("echo"));
    f.run("echo", transition("1", Status::InProgress));
    f.run(
        "echo",
        Command::RequestReview {
            task: "1".into(),
            reviewer: "pixel".into(),
        },
    );
    f.run(
        "lead",
        Command::AddReviewer {
            task: "1".into(),
            reviewer: "ember".into(),
        },
    );
    assert_eq!(f.store.state().task("1").unwrap().reviewers, vec!["ember"]);
    assert!(matches!(
        f.try_run(
            "lead",
            Command::AddReviewer {
                task: "1".into(),
                reviewer: "ember".into()
            }
        ),
        Err(CoreError::ReviewerExists { .. })
    ));
    assert!(matches!(
        f.try_run(
            "lead",
            Command::AddReviewer {
                task: "1".into(),
                reviewer: "pixel".into()
            }
        ),
        Err(CoreError::ReviewerExists { .. })
    ));
    assert_eq!(
        rteams_core::briefing(f.store.state(), "ember")
            .actionable
            .len(),
        1
    );
    f.run(
        "lead",
        Command::RemoveReviewer {
            task: "1".into(),
            reviewer: "ember".into(),
        },
    );
    assert!(matches!(
        f.try_run(
            "lead",
            Command::RemoveReviewer {
                task: "1".into(),
                reviewer: "ember".into()
            }
        ),
        Err(CoreError::ReviewerNotFound { .. })
    ));
}

#[test]
fn restore_set_column_and_clear_column() {
    let mut f = Fixture::new();
    f.task("1", Some("echo"));
    assert!(matches!(
        f.try_run("lead", Command::RestoreTask { task: "1".into() }),
        Err(CoreError::NotArchived(_))
    ));
    f.run(
        "lead",
        Command::ArchiveTask {
            task: "1".into(),
            archived: true,
        },
    );
    f.run("lead", Command::RestoreTask { task: "1".into() });
    assert!(!f.store.state().task("1").unwrap().archived);

    // pending -> done goes through in_progress.
    f.run(
        "lead",
        Command::SetColumn {
            task: "1".into(),
            column: "done".into(),
        },
    );
    assert_eq!(f.status("1"), Status::Done);
    assert!(matches!(
        f.try_run(
            "lead",
            Command::SetColumn {
                task: "1".into(),
                column: "pending".into()
            }
        ),
        Err(CoreError::NoPath { .. })
    ));
    assert!(matches!(
        f.try_run(
            "lead",
            Command::SetColumn {
                task: "1".into(),
                column: "blocked".into()
            }
        ),
        Err(CoreError::UseDedicatedCommand { .. })
    ));
    assert!(matches!(
        f.try_run(
            "lead",
            Command::SetColumn {
                task: "1".into(),
                column: "nope".into()
            }
        ),
        Err(CoreError::UnknownColumn(_))
    ));
    f.run(
        "lead",
        Command::SetColumn {
            task: "1".into(),
            column: "in_progress".into(),
        },
    );
    assert_eq!(f.status("1"), Status::InProgress);

    f.task("2", None);
    f.run(
        "lead",
        Command::SetColumn {
            task: "2".into(),
            column: "done".into(),
        },
    );
    assert!(matches!(
        f.try_run(
            "lead",
            Command::ClearColumn {
                column: "review".into()
            }
        ),
        Err(CoreError::ColumnNotClearable(_))
    ));
    f.run(
        "lead",
        Command::ClearColumn {
            column: "done".into(),
        },
    );
    assert!(f.store.state().task("2").unwrap().archived);
    assert!(!f.store.state().task("1").unwrap().archived);
    assert!(matches!(
        f.try_run(
            "lead",
            Command::ClearColumn {
                column: "done".into()
            }
        ),
        Err(CoreError::ColumnEmpty(_))
    ));
}

#[test]
fn heartbeat_launch_stop() {
    let mut f = Fixture::new();
    f.task("1", None);
    f.run(
        "echo",
        Command::Heartbeat {
            member: "echo".into(),
            status: "working".into(),
            task: Some("1".into()),
            note: Some("n".into()),
        },
    );
    let h = f.store.state().heartbeats.get("echo").unwrap().clone();
    assert_eq!(
        (h.status.as_str(), h.task.as_deref(), h.note.as_deref()),
        ("working", Some("1"), Some("n"))
    );
    assert!(h.ts > 0);
    assert!(matches!(
        f.try_run(
            "echo",
            Command::Heartbeat {
                member: "echo".into(),
                status: "dancing".into(),
                task: None,
                note: None
            }
        ),
        Err(CoreError::InvalidHeartbeat(_))
    ));
    assert!(matches!(
        f.try_run(
            "echo",
            Command::Heartbeat {
                member: "ghost".into(),
                status: "idle".into(),
                task: None,
                note: None
            }
        ),
        Err(CoreError::UnknownMember(_))
    ));
    f.run(
        "lead",
        Command::LaunchRequest {
            members: vec!["echo".into()],
        },
    );
    f.run("lead", Command::StopRequest { members: vec![] });
    assert!(matches!(
        f.try_run(
            "lead",
            Command::LaunchRequest {
                members: vec!["ghost".into()]
            }
        ),
        Err(CoreError::UnknownMember(_))
    ));
}

#[test]
fn member_options() {
    let mut f = Fixture::new();
    let upd = |effort: Option<&str>, fast, ctx, mcp: Option<Vec<&str>>| Command::UpdateMember {
        name: "echo".into(),
        role: None,
        prompt: None,
        model: None,
        runtime: None,
        worktree: None,
        icon: None,
        effort: effort.map(Into::into),
        fast,
        context_limit: ctx,
        mcp_allow: mcp.map(|v| v.into_iter().map(String::from).collect()),
    };
    f.run(
        "lead",
        upd(
            Some("high"),
            Some(true),
            Some(200_000),
            Some(vec!["gh", "gh", "db"]),
        ),
    );
    let m = f.store.state().team.clone().unwrap().members[1].clone();
    assert_eq!(m.effort.as_deref(), Some("high"));
    assert!(m.fast);
    assert_eq!(m.context_limit, Some(200_000));
    assert_eq!(m.mcp_allow, vec!["gh", "db"]);
    assert!(matches!(
        f.try_run("lead", upd(Some("extreme"), None, None, None)),
        Err(CoreError::InvalidEffort(_))
    ));
    assert!(matches!(
        f.try_run("lead", upd(None, None, None, None)),
        Err(CoreError::EmptyMemberUpdate)
    ));
    f.run("lead", upd(Some(""), Some(false), Some(0), Some(vec![])));
    let m = f.store.state().team.clone().unwrap().members[1].clone();
    assert_eq!(
        (m.effort, m.fast, m.context_limit, m.mcp_allow.len()),
        (None, false, None, 0)
    );
}

fn ev(kind: EventKind) -> String {
    serde_json::to_string(&Event {
        seq: 3,
        ts: 1700,
        actor: "echo".into(),
        kind,
    })
    .unwrap()
}

#[test]
fn golden_json_new_events() {
    assert_eq!(
        ev(EventKind::GroupChatCreated {
            id: "g".into(),
            name: "N".into(),
            members: vec!["a".into()]
        }),
        r#"{"seq":3,"ts":1700,"actor":"echo","type":"group_chat_created","id":"g","name":"N","members":["a"]}"#
    );
    assert_eq!(
        ev(EventKind::MessageSent {
            to: None,
            text: "x".into(),
            mode: None,
            from_team: None,
            chat: Some("g".into()),
            attachments: vec![att("a")]
        }),
        r#"{"seq":3,"ts":1700,"actor":"echo","type":"message_sent","to":null,"text":"x","chat":"g","attachments":[{"name":"a","path":"/tmp/a"}]}"#
    );
    assert_eq!(
        ev(EventKind::MemberHeartbeat {
            member: "echo".into(),
            status: "idle".into(),
            task: None,
            note: None
        }),
        r#"{"seq":3,"ts":1700,"actor":"echo","type":"member_heartbeat","member":"echo","status":"idle"}"#
    );
    assert_eq!(
        ev(EventKind::TaskColumnSet {
            task: "a".into(),
            from: Status::Pending,
            to: Status::Done
        }),
        r#"{"seq":3,"ts":1700,"actor":"echo","type":"task_column_set","task":"a","from":"pending","to":"done"}"#
    );
    assert_eq!(
        ev(EventKind::LaunchRequested {
            members: vec!["a".into()]
        }),
        r#"{"seq":3,"ts":1700,"actor":"echo","type":"launch_requested","members":["a"]}"#
    );
}

#[test]
fn old_format_events_still_parse() {
    let lines = [
        r#"{"seq":1,"ts":1,"actor":"u","type":"team_created","name":"t"}"#,
        r#"{"seq":2,"ts":2,"actor":"u","type":"member_added","name":"a","role":"lead"}"#,
        r#"{"seq":3,"ts":3,"actor":"u","type":"task_created","task":"1","subject":"s","description":"","owner":null,"blocked_by":[]}"#,
        r#"{"seq":4,"ts":4,"actor":"a","type":"task_commented","task":"1","text":"c"}"#,
        r#"{"seq":5,"ts":5,"actor":"a","type":"message_sent","to":null,"text":"hello"}"#,
        r#"{"seq":6,"ts":6,"actor":"a","type":"member_updated","name":"a","role":"lead"}"#,
    ];
    let events: Vec<Event> = lines
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let s = State::from_events(&events);
    assert_eq!(s.messages.len(), 2);
    assert_eq!(s.messages[1].scope, Scope::Broadcast);
    assert!(s.messages.iter().all(|m| m.attachments.is_empty()));
    assert!(s.team.as_ref().unwrap().group_chats.is_empty());
    // Round trip keeps old shape (no new keys when empty).
    for (l, e) in lines.iter().zip(&events) {
        assert_eq!(&serde_json::to_string(e).unwrap(), l);
    }
    // Old Message / Member / Task JSON deserializes.
    let m: rteams_core::Message =
        serde_json::from_str(r#"{"seq":1,"ts":1,"from":"a","scope":"broadcast","text":"x"}"#)
            .unwrap();
    assert!(m.attachments.is_empty());
    let g: rteams_core::Message = serde_json::from_str(
        r#"{"seq":1,"ts":1,"from":"a","scope":"group","chat":"g","text":"x"}"#,
    )
    .unwrap();
    assert_eq!(g.scope, Scope::Group { chat: "g".into() });
    let mem: rteams_core::Member = serde_json::from_str(r#"{"name":"a","role":"r"}"#).unwrap();
    assert!(mem.mcp_allow.is_empty() && !mem.fast && mem.effort.is_none());
}
