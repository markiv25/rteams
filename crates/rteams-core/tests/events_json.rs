//! Golden wire format. Changing these strings is a breaking change for stored logs.
use rteams_core::{Event, EventKind, Status};

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
fn golden_json() {
    assert_eq!(
        ev(EventKind::TaskTransitioned {
            task: "a".into(),
            from: Status::Pending,
            to: Status::InProgress
        }),
        r#"{"seq":3,"ts":1700,"actor":"echo","type":"task_transitioned","task":"a","from":"pending","to":"in_progress"}"#
    );
    assert_eq!(
        ev(EventKind::MessageSent {
            to: None,
            text: "hi".into(),
            mode: None,
            from_team: None
        }),
        r#"{"seq":3,"ts":1700,"actor":"echo","type":"message_sent","to":null,"text":"hi"}"#
    );
    assert_eq!(
        ev(EventKind::TaskBlocked {
            task: "a".into(),
            reason: "r".into(),
            from: Status::NeedsFix
        }),
        r#"{"seq":3,"ts":1700,"actor":"echo","type":"task_blocked","task":"a","reason":"r","from":"needs_fix"}"#
    );
    assert_eq!(
        ev(EventKind::TaskUpdated {
            task: "a".into(),
            subject: None,
            description: Some("d".into())
        }),
        r#"{"seq":3,"ts":1700,"actor":"echo","type":"task_updated","task":"a","description":"d"}"#
    );
}

#[test]
fn every_event_kind_round_trips() {
    let kinds = vec![
        EventKind::TeamCreated { name: "t".into() },
        EventKind::MemberAdded {
            name: "echo".into(),
            role: "developer".into(),
            runtime: None,
        },
        EventKind::TaskCreated {
            task: "a".into(),
            subject: "s".into(),
            description: "d".into(),
            owner: Some("echo".into()),
            blocked_by: vec!["b".into()],
        },
        EventKind::TaskDependenciesSet {
            task: "a".into(),
            blocked_by: vec![],
        },
        EventKind::TaskAssigned {
            task: "a".into(),
            owner: None,
        },
        EventKind::TaskUpdated {
            task: "a".into(),
            subject: Some("s".into()),
            description: None,
        },
        EventKind::TaskTransitioned {
            task: "a".into(),
            from: Status::Pending,
            to: Status::InProgress,
        },
        EventKind::TaskBlocked {
            task: "a".into(),
            reason: "r".into(),
            from: Status::Pending,
        },
        EventKind::TaskUnblocked {
            task: "a".into(),
            to: Status::Pending,
        },
        EventKind::TaskArchived {
            task: "a".into(),
            archived: true,
        },
        EventKind::TaskCommented {
            task: "a".into(),
            text: "x".into(),
        },
        EventKind::ReviewRequested {
            task: "a".into(),
            reviewer: "pixel".into(),
        },
        EventKind::ReviewApproved {
            task: "a".into(),
            note: None,
        },
        EventKind::ReviewChangesRequested {
            task: "a".into(),
            comment: "c".into(),
        },
        EventKind::MessageSent {
            to: Some("echo".into()),
            text: "x".into(),
            mode: Some("ask".into()),
            from_team: None,
        },
        EventKind::TeamConfigured {
            project_path: Some("/p".into()),
            lead_prompt: None,
        },
        EventKind::MemberUpdated {
            name: "echo".into(),
            role: Some("qa".into()),
            prompt: Some("p".into()),
            model: Some("m".into()),
            runtime: None,
            worktree: None,
            icon: None,
        },
        EventKind::MemberRemoved {
            name: "echo".into(),
            removed: true,
        },
        EventKind::ProcessRegistered {
            pid: 1,
            label: "l".into(),
            port: Some(80),
            url: None,
            command: None,
        },
        EventKind::ProcessStopped { pid: 1 },
    ];
    for kind in kinds {
        let e = Event {
            seq: 1,
            ts: 0,
            actor: "x".into(),
            kind,
        };
        let back: Event = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back, e);
    }
}

#[test]
fn task_created_without_blocked_by_still_parses() {
    // Logs written before dependencies existed must replay.
    let old = r#"{"seq":1,"ts":0,"actor":"x","type":"task_created","task":"a","subject":"s","description":"","owner":null}"#;
    let e: Event = serde_json::from_str(old).unwrap();
    assert_eq!(
        e.kind,
        EventKind::TaskCreated {
            task: "a".into(),
            subject: "s".into(),
            description: "".into(),
            owner: None,
            blocked_by: vec![],
        }
    );
}

#[test]
fn new_events_golden_and_old_lines_still_parse() {
    assert_eq!(
        ev(EventKind::TeamConfigured {
            project_path: Some("/p".into()),
            lead_prompt: None
        }),
        r#"{"seq":3,"ts":1700,"actor":"echo","type":"team_configured","project_path":"/p"}"#
    );
    assert_eq!(
        ev(EventKind::MemberUpdated {
            name: "x".into(),
            role: None,
            prompt: Some("p".into()),
            model: None,
            runtime: None,
            worktree: None,
            icon: None,
        }),
        r#"{"seq":3,"ts":1700,"actor":"echo","type":"member_updated","name":"x","prompt":"p"}"#
    );
    assert_eq!(
        ev(EventKind::MemberRemoved {
            name: "x".into(),
            removed: false
        }),
        r#"{"seq":3,"ts":1700,"actor":"echo","type":"member_removed","name":"x","removed":false}"#
    );
    assert_eq!(
        ev(EventKind::MessageSent {
            to: None,
            text: "hi".into(),
            mode: Some("do".into()),
            from_team: None
        }),
        r#"{"seq":3,"ts":1700,"actor":"echo","type":"message_sent","to":null,"text":"hi","mode":"do"}"#
    );
    // Old line without `mode` parses with mode = None.
    let old = r#"{"seq":3,"ts":1700,"actor":"echo","type":"message_sent","to":null,"text":"hi"}"#;
    let e: Event = serde_json::from_str(old).unwrap();
    assert_eq!(
        e.kind,
        EventKind::MessageSent {
            to: None,
            text: "hi".into(),
            mode: None,
            from_team: None
        }
    );
}

#[test]
fn old_domain_json_without_new_fields_parses() {
    use rteams_core::{Member, Message, Team};
    let t: Team =
        serde_json::from_str(r#"{"name":"t","members":[{"name":"a","role":"lead"}]}"#).unwrap();
    assert_eq!(t.project_path, None);
    assert_eq!(t.lead_prompt, None);
    let m: &Member = &t.members[0];
    assert!(m.prompt.is_none() && m.model.is_none() && !m.removed);
    let msg: Message =
        serde_json::from_str(r#"{"seq":1,"ts":2,"from":"a","scope":"broadcast","text":"x"}"#)
            .unwrap();
    assert_eq!(msg.mode, None);
    assert!(!serde_json::to_string(&msg).unwrap().contains("mode"));
}

#[test]
fn golden_json_wave3_events() {
    use rteams_core::Schedule;
    let cases: Vec<(EventKind, &str)> = vec![
        (
            EventKind::TaskClarificationSet {
                task: "a".into(),
                question: Some("why?".into()),
            },
            r#"{"seq":3,"ts":1700,"actor":"echo","type":"task_clarification_set","task":"a","question":"why?"}"#,
        ),
        (
            EventKind::TaskFileAttached {
                task: "a".into(),
                name: "n".into(),
                path: "/p".into(),
            },
            r#"{"seq":3,"ts":1700,"actor":"echo","type":"task_file_attached","task":"a","name":"n","path":"/p"}"#,
        ),
        (
            EventKind::TasksLinked {
                task: "a".into(),
                related: "b".into(),
                linked: true,
            },
            r#"{"seq":3,"ts":1700,"actor":"echo","type":"tasks_linked","task":"a","related":"b","linked":true}"#,
        ),
        (
            EventKind::ReviewStarted { task: "a".into() },
            r#"{"seq":3,"ts":1700,"actor":"echo","type":"review_started","task":"a"}"#,
        ),
        (
            EventKind::ColumnOrderSet {
                column: "todo".into(),
                order: vec!["b".into(), "a".into()],
            },
            r#"{"seq":3,"ts":1700,"actor":"echo","type":"column_order_set","column":"todo","order":["b","a"]}"#,
        ),
        (
            EventKind::TeamDeleted { deleted: true },
            r#"{"seq":3,"ts":1700,"actor":"echo","type":"team_deleted","deleted":true}"#,
        ),
        (
            EventKind::MessageReacted {
                message_seq: 2,
                emoji: "x".into(),
            },
            r#"{"seq":3,"ts":1700,"actor":"echo","type":"message_reacted","message_seq":2,"emoji":"x"}"#,
        ),
        (
            EventKind::ScheduleSet {
                schedule: Schedule {
                    id: "s".into(),
                    member: "echo".into(),
                    prompt: "p".into(),
                    every_minutes: 5,
                    enabled: true,
                    cron: None,
                    timezone: None,
                    warmup_minutes: None,
                },
            },
            r#"{"seq":3,"ts":1700,"actor":"echo","type":"schedule_set","schedule":{"id":"s","member":"echo","prompt":"p","every_minutes":5,"enabled":true}}"#,
        ),
        (
            EventKind::ScheduleRemoved { id: "s".into() },
            r#"{"seq":3,"ts":1700,"actor":"echo","type":"schedule_removed","id":"s"}"#,
        ),
        (
            EventKind::PermissionRequested {
                id: "p1".into(),
                member: "echo".into(),
                tool: "Bash".into(),
                input: serde_json::json!({"command":"ls"}),
            },
            r#"{"seq":3,"ts":1700,"actor":"echo","type":"permission_requested","id":"p1","member":"echo","tool":"Bash","input":{"command":"ls"}}"#,
        ),
        (
            EventKind::PermissionDecided {
                id: "p1".into(),
                allow: false,
                note: None,
            },
            r#"{"seq":3,"ts":1700,"actor":"echo","type":"permission_decided","id":"p1","allow":false,"note":null}"#,
        ),
        (
            EventKind::MemberAdded {
                name: "a".into(),
                role: "dev".into(),
                runtime: Some("codex".into()),
            },
            r#"{"seq":3,"ts":1700,"actor":"echo","type":"member_added","name":"a","role":"dev","runtime":"codex"}"#,
        ),
        (
            EventKind::MemberUpdated {
                name: "a".into(),
                role: None,
                prompt: None,
                model: None,
                runtime: Some("gemini".into()),
                worktree: Some(true),
                icon: None,
            },
            r#"{"seq":3,"ts":1700,"actor":"echo","type":"member_updated","name":"a","runtime":"gemini","worktree":true}"#,
        ),
        (
            EventKind::MessageSent {
                to: None,
                text: "hi".into(),
                mode: None,
                from_team: Some("other".into()),
            },
            r#"{"seq":3,"ts":1700,"actor":"echo","type":"message_sent","to":null,"text":"hi","from_team":"other"}"#,
        ),
    ];
    for (kind, golden) in cases {
        assert_eq!(ev(kind.clone()), golden);
        let back: Event = serde_json::from_str(golden).unwrap();
        assert_eq!(back.kind, kind);
    }
}

#[test]
fn old_lines_without_wave3_fields_parse() {
    let old = r#"{"seq":1,"ts":2,"actor":"a","type":"member_added","name":"x","role":"dev"}"#;
    let e: Event = serde_json::from_str(old).unwrap();
    assert_eq!(
        e.kind,
        EventKind::MemberAdded {
            name: "x".into(),
            role: "dev".into(),
            runtime: None
        }
    );
    let old = r#"{"seq":1,"ts":2,"actor":"a","type":"member_updated","name":"x","role":"qa"}"#;
    let e: Event = serde_json::from_str(old).unwrap();
    assert!(matches!(
        e.kind,
        EventKind::MemberUpdated {
            runtime: None,
            worktree: None,
            ..
        }
    ));
}

#[test]
fn old_domain_json_without_wave3_fields_parses() {
    use rteams_core::{Member, Message, Task, Team};
    let t: Team =
        serde_json::from_str(r#"{"name":"t","members":[{"name":"a","role":"lead"}]}"#).unwrap();
    assert!(!t.deleted && t.column_order.is_empty() && t.schedules.is_empty());
    let m: &Member = &t.members[0];
    assert!(m.runtime.is_none() && !m.worktree);
    let msg: Message =
        serde_json::from_str(r#"{"seq":1,"ts":2,"from":"a","scope":"broadcast","text":"x"}"#)
            .unwrap();
    assert!(msg.from_team.is_none() && msg.reactions.is_empty());
    let task: Task = serde_json::from_str(
        r#"{"id":"1","subject":"s","description":"","owner":null,"status":"pending","blocked_by":[],"reviewer":null,"blocked_from":null,"blocked_reason":null,"archived":false,"created_seq":1}"#,
    )
    .unwrap();
    assert!(task.clarification.is_none() && task.attachments.is_empty());
    assert!(task.related.is_empty() && !task.review_started);
}
