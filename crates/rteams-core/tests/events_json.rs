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
            text: "hi".into()
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
}

#[test]
fn every_event_kind_round_trips() {
    let kinds = vec![
        EventKind::TeamCreated { name: "t".into() },
        EventKind::MemberAdded {
            name: "echo".into(),
            role: "developer".into(),
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
