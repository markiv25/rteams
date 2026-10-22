mod common;
use common::*;
use rteams_core::{briefing, inbox, kanban, process_list, Command, Reason, Scope, Status};

fn ids(cards: &[rteams_core::Card]) -> Vec<&str> {
    cards.iter().map(|c| c.id.as_str()).collect()
}

fn review(f: &mut Fixture, task: &str, reviewer: &str) {
    f.run(
        "echo",
        Command::RequestReview {
            task: task.into(),
            reviewer: reviewer.into(),
        },
    );
}

#[test]
fn kanban_is_derived_from_status() {
    let mut f = Fixture::new();
    for id in [
        "pending", "wip", "rev", "appr", "done", "fix", "blk_wip", "arch",
    ] {
        f.task(id, Some("echo"));
    }
    for id in ["wip", "rev", "appr", "done", "fix", "blk_wip", "arch"] {
        f.run("echo", transition(id, Status::InProgress));
    }
    review(&mut f, "rev", "pixel");
    review(&mut f, "appr", "pixel");
    review(&mut f, "fix", "pixel");
    f.run(
        "pixel",
        Command::ApproveReview {
            task: "appr".into(),
            note: None,
        },
    );
    f.run(
        "pixel",
        Command::RequestChanges {
            task: "fix".into(),
            comment: "x".into(),
        },
    );
    f.run("echo", transition("done", Status::Done));
    f.run(
        "echo",
        Command::Block {
            task: "blk_wip".into(),
            reason: "r".into(),
        },
    );
    f.run(
        "lead",
        Command::ArchiveTask {
            task: "arch".into(),
            archived: true,
        },
    );

    let b = kanban(f.store.state());
    assert_eq!(ids(&b.todo), ["pending", "fix"]); // needs_fix lands in todo
    assert_eq!(ids(&b.in_progress), ["wip", "blk_wip"]); // blocked stays where it was
    assert_eq!(ids(&b.review), ["rev"]);
    assert_eq!(ids(&b.approved), ["appr"]);
    assert_eq!(ids(&b.done), ["done"]);
    // archived task is gone; cards keep the real status for badges
    assert!(!b.todo.iter().chain(&b.in_progress).any(|c| c.id == "arch"));
    assert_eq!(b.todo[1].status, Status::NeedsFix);
    assert_eq!(b.in_progress[1].status, Status::Blocked);

    // unarchive brings it back; no stored column to repair
    f.run(
        "lead",
        Command::ArchiveTask {
            task: "arch".into(),
            archived: false,
        },
    );
    assert_eq!(
        ids(&kanban(f.store.state()).in_progress),
        ["wip", "blk_wip", "arch"]
    );
}

#[test]
fn briefing_splits_actionable_from_awareness_in_priority_order() {
    let mut f = Fixture::new();
    // echo owns one task in every state, created in reverse priority order
    for id in [
        "start", "finish", "fix", "resume", "waiting", "blocked", "done",
    ] {
        f.task(id, Some("echo"));
    }
    f.task("unowned", None);
    f.task("others", Some("ember"));
    for id in ["finish", "fix", "resume", "waiting", "blocked", "done"] {
        f.run("echo", transition(id, Status::InProgress));
    }
    for id in ["finish", "fix", "waiting"] {
        review(&mut f, id, "pixel");
    }
    f.run(
        "pixel",
        Command::ApproveReview {
            task: "finish".into(),
            note: None,
        },
    );
    f.run(
        "pixel",
        Command::RequestChanges {
            task: "fix".into(),
            comment: "x".into(),
        },
    );
    f.run(
        "echo",
        Command::Block {
            task: "blocked".into(),
            reason: "r".into(),
        },
    );
    f.run("echo", transition("done", Status::Done));

    let b = briefing(f.store.state(), "echo");
    let act: Vec<_> = b
        .actionable
        .iter()
        .map(|i| (i.task.as_str(), i.reason))
        .collect();
    assert_eq!(
        act,
        [
            ("resume", Reason::Resume),
            ("fix", Reason::Fix),
            ("finish", Reason::Finish),
            ("start", Reason::Start)
        ]
    );
    let aware: Vec<_> = b
        .awareness
        .iter()
        .map(|i| (i.task.as_str(), i.reason))
        .collect();
    assert_eq!(
        aware,
        [
            ("waiting", Reason::AwaitingReview),
            ("blocked", Reason::Blocked)
        ]
    );

    // nothing for a member with no work; unowned tasks are nobody's briefing
    let lead = briefing(f.store.state(), "lead");
    assert!(lead.actionable.is_empty() && lead.awareness.is_empty());
}

#[test]
fn briefing_for_reviewer_and_archived_tasks() {
    let mut f = Fixture::new();
    f.task("a", Some("echo"));
    f.task("b", Some("echo"));
    f.run("echo", transition("a", Status::InProgress));
    f.run("echo", transition("b", Status::InProgress));
    review(&mut f, "a", "pixel");
    review(&mut f, "b", "pixel");
    f.run(
        "lead",
        Command::ArchiveTask {
            task: "b".into(),
            archived: true,
        },
    );

    let p = briefing(f.store.state(), "pixel");
    assert_eq!(p.actionable.len(), 1);
    assert_eq!(p.actionable[0].task, "a");
    assert_eq!(p.actionable[0].reason, Reason::Review);
    assert!(p.awareness.is_empty());

    // once approved the reviewer is done with it
    f.run(
        "pixel",
        Command::ApproveReview {
            task: "a".into(),
            note: None,
        },
    );
    assert!(briefing(f.store.state(), "pixel").actionable.is_empty());
    assert_eq!(
        briefing(f.store.state(), "echo").actionable[0].reason,
        Reason::Finish
    );
}

#[test]
fn inbox_unifies_direct_broadcast_and_task_comments_with_cursor() {
    let mut f = Fixture::new();
    f.task("mine", Some("echo"));
    f.task("theirs", Some("ember"));
    f.run(
        "lead",
        Command::SendMessage {
            mode: None,
            to: Some("echo".into()),
            text: "direct to echo".into(),
            from_team: None,
            attachments: vec![],
            chat: None,
        },
    ); // seq 7
    f.run(
        "lead",
        Command::SendMessage {
            mode: None,
            to: Some("ember".into()),
            text: "direct to ember".into(),
            from_team: None,
            attachments: vec![],
            chat: None,
        },
    );
    f.run(
        "lead",
        Command::SendMessage {
            mode: None,
            to: None,
            text: "broadcast".into(),
            from_team: None,
            attachments: vec![],
            chat: None,
        },
    );
    f.run(
        "echo",
        Command::SendMessage {
            mode: None,
            to: None,
            text: "my own broadcast".into(),
            from_team: None,
            attachments: vec![],
            chat: None,
        },
    );
    f.run(
        "pixel",
        Command::Comment {
            task: "mine".into(),
            text: "comment on mine".into(),
            attachments: vec![],
        },
    );
    f.run(
        "pixel",
        Command::Comment {
            task: "theirs".into(),
            text: "comment on theirs".into(),
            attachments: vec![],
        },
    );
    f.run(
        "echo",
        Command::Comment {
            task: "mine".into(),
            text: "my own comment".into(),
            attachments: vec![],
        },
    );

    let i = inbox(f.store.state(), "echo", 0);
    let texts: Vec<_> = i.items.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(texts, ["direct to echo", "broadcast", "comment on mine"]);
    assert_eq!(i.items[0].scope, Scope::Direct { to: "echo".into() });
    assert_eq!(i.items[1].scope, Scope::Broadcast);
    assert_eq!(
        i.items[2].scope,
        Scope::Task {
            task: "mine".into()
        }
    );
    assert_eq!(i.items[0].from, "lead");

    // cursor: nothing new -> same cursor, empty
    let again = inbox(f.store.state(), "echo", i.next_cursor);
    assert!(again.items.is_empty());
    assert_eq!(again.next_cursor, i.next_cursor);

    // then only new items show up
    f.run(
        "lead",
        Command::SendMessage {
            mode: None,
            to: Some("echo".into()),
            text: "later".into(),
            from_team: None,
            attachments: vec![],
            chat: None,
        },
    );
    let later = inbox(f.store.state(), "echo", i.next_cursor);
    assert_eq!(later.items.len(), 1);
    assert_eq!(later.items[0].text, "later");
    assert!(later.next_cursor > i.next_cursor);

    // ember sees its own direct + broadcast + comment on its task, not echo's
    let e: Vec<_> = inbox(f.store.state(), "ember", 0)
        .items
        .into_iter()
        .map(|m| m.text)
        .collect();
    assert_eq!(
        e,
        [
            "direct to ember",
            "broadcast",
            "my own broadcast",
            "comment on theirs"
        ]
    );
}

#[test]
fn review_feedback_flows_into_owner_inbox_as_task_comments() {
    let mut f = Fixture::new();
    f.task("a", Some("echo"));
    f.run("echo", transition("a", Status::InProgress));
    review(&mut f, "a", "pixel");
    f.run(
        "pixel",
        Command::RequestChanges {
            task: "a".into(),
            comment: "rename foo".into(),
        },
    );
    let i = inbox(f.store.state(), "echo", 0);
    assert_eq!(i.items.len(), 1);
    assert_eq!(i.items[0].text, "rename foo");
    assert_eq!(i.items[0].scope, Scope::Task { task: "a".into() });

    // approval without a note adds no message; with a note it does
    f.run("echo", transition("a", Status::InProgress));
    review(&mut f, "a", "pixel");
    f.run(
        "pixel",
        Command::ApproveReview {
            task: "a".into(),
            note: Some("nice".into()),
        },
    );
    assert_eq!(
        inbox(f.store.state(), "echo", i.next_cursor).items[0].text,
        "nice"
    );

    // reviewer sees the owner's follow-up comments too
    f.run(
        "echo",
        Command::Comment {
            task: "a".into(),
            text: "thanks".into(),
            attachments: vec![],
        },
    );
    let p: Vec<_> = inbox(f.store.state(), "pixel", 0)
        .items
        .into_iter()
        .map(|m| m.text)
        .collect();
    assert_eq!(p, ["thanks"]);
}

#[test]
fn process_list_shows_running_only_in_registration_order() {
    let mut f = Fixture::new();
    let reg = |pid: u32, label: &str| Command::RegisterProcess {
        pid,
        label: label.into(),
        port: Some(3000 + pid as u16),
        url: Some(format!("http://localhost:{}", 3000 + pid)),
        command: Some("pnpm dev".into()),
    };
    f.run("echo", reg(30, "web"));
    f.run("ember", reg(10, "db"));
    f.run("echo", reg(20, "worker"));
    f.run("echo", Command::StopProcess { pid: 10 });

    let list = process_list(f.store.state());
    let labels: Vec<_> = list
        .iter()
        .map(|p| (p.pid, p.label.as_str(), p.owner.as_str()))
        .collect();
    assert_eq!(labels, [(30, "web", "echo"), (20, "worker", "echo")]);
    assert_eq!(list[0].port, Some(3030));
    assert_eq!(list[0].url.as_deref(), Some("http://localhost:3030"));

    // re-registering a stopped pid moves it to the end under the new owner
    f.run("ember", reg(10, "db2"));
    let last = process_list(f.store.state()).pop().unwrap();
    assert_eq!(
        (last.pid, last.label.as_str(), last.owner.as_str()),
        (10, "db2", "ember")
    );
}

#[test]
fn current_task_state_projection() {
    let mut f = Fixture::new();
    f.task("a", None);
    f.run(
        "lead",
        Command::AssignTask {
            task: "a".into(),
            owner: Some("echo".into()),
        },
    );
    f.run("echo", transition("a", Status::InProgress));
    f.run(
        "echo",
        Command::Block {
            task: "a".into(),
            reason: "waiting on atlas".into(),
        },
    );
    let t = f.store.state().task("a").unwrap().clone();
    assert_eq!(t.owner.as_deref(), Some("echo"));
    assert_eq!(t.status, Status::Blocked);
    assert_eq!(t.blocked_from, Some(Status::InProgress));
    assert_eq!(t.blocked_reason.as_deref(), Some("waiting on atlas"));
    f.run("echo", Command::Unblock { task: "a".into() });
    let t = f.store.state().task("a").unwrap();
    assert_eq!(
        (t.status, t.blocked_from, t.blocked_reason.as_deref()),
        (Status::InProgress, None, None)
    );
    // unassign
    f.run(
        "lead",
        Command::AssignTask {
            task: "a".into(),
            owner: None,
        },
    );
    assert_eq!(f.store.state().task("a").unwrap().owner, None);
}
