mod common;
use common::*;
use rteams_core::{lead_briefing, member_briefing, Command, LeadReason, Status};

fn reasons(items: &[rteams_core::LeadItem]) -> Vec<(&str, LeadReason)> {
    items.iter().map(|i| (i.task.as_str(), i.reason)).collect()
}

#[test]
fn lead_queue_splits_unassigned_and_blocked_from_review_and_approved() {
    let mut f = Fixture::new();
    f.task("loose", None);
    f.task("parked", Some("echo"));
    f.task("in-review", Some("echo"));
    f.task("approved", Some("echo"));
    f.task("busy", Some("echo"));
    f.task("finished", Some("echo"));
    f.task("hidden", None);
    f.run("echo", transition("parked", Status::InProgress));
    f.run(
        "echo",
        Command::Block {
            task: "parked".into(),
            reason: "waiting".into(),
        },
    );
    for id in ["in-review", "approved", "busy", "finished"] {
        f.run("echo", transition(id, Status::InProgress));
    }
    for id in ["in-review", "approved"] {
        f.run(
            "echo",
            Command::RequestReview {
                task: id.into(),
                reviewer: "pixel".into(),
            },
        );
    }
    f.run(
        "pixel",
        Command::ApproveReview {
            task: "approved".into(),
            note: None,
        },
    );
    f.run("echo", transition("finished", Status::Done));
    f.run(
        "lead",
        Command::ArchiveTask {
            task: "hidden".into(),
            archived: true,
        },
    );

    let b = lead_briefing(f.store.state());
    assert_eq!(
        reasons(&b.actionable),
        [
            ("loose", LeadReason::Unassigned),
            ("parked", LeadReason::Blocked)
        ]
    );
    assert_eq!(
        reasons(&b.awareness),
        [
            ("in-review", LeadReason::InReview),
            ("approved", LeadReason::Approved)
        ]
    );
    assert_eq!(b.awareness[0].reviewer.as_deref(), Some("pixel"));

    // plain in-progress work and done work never reach the lead's queue; archived is gone
    let all: Vec<_> = b.actionable.iter().chain(&b.awareness).collect();
    assert!(all
        .iter()
        .all(|i| !["busy", "finished", "hidden"].contains(&i.task.as_str())));

    assert_eq!(b.counts.todo, 1); // loose
    assert_eq!(b.counts.in_progress, 2); // parked (blocked stays in its source column) + busy
    assert_eq!(
        (b.counts.review, b.counts.approved, b.counts.done),
        (1, 1, 1)
    );
}

#[test]
fn lead_load_has_one_row_per_member_in_roster_order() {
    let mut f = Fixture::new();
    f.task("a", Some("echo"));
    f.task("b", Some("echo"));
    f.task("c", Some("echo"));
    f.task("d", Some("pixel"));
    f.run("echo", transition("a", Status::InProgress));
    f.run("echo", transition("b", Status::InProgress));
    f.run(
        "echo",
        Command::Block {
            task: "b".into(),
            reason: "r".into(),
        },
    );

    let b = lead_briefing(f.store.state());
    let names: Vec<_> = b.load.iter().map(|l| l.member.as_str()).collect();
    assert_eq!(names, ["lead", "echo", "pixel", "ember"]);
    let echo = &b.load[1];
    assert_eq!(echo.role, "developer");
    assert_eq!(
        (echo.in_progress, echo.todo, echo.review, echo.blocked),
        (1, 1, 0, 1)
    );
    assert_eq!(b.load[2].todo, 1);
    assert_eq!(b.load[3].in_progress + b.load[3].todo, 0);
}

#[test]
fn lead_briefing_on_an_empty_store_is_empty() {
    let b = lead_briefing(&rteams_core::State::default());
    assert!(b.actionable.is_empty() && b.awareness.is_empty() && b.load.is_empty());
}

#[test]
fn member_briefing_bootstraps_without_listing_work_items() {
    let mut f = Fixture::new();
    f.task("one", Some("echo"));
    f.task("two", Some("echo"));
    f.run("echo", transition("two", Status::InProgress));
    f.run(
        "echo",
        Command::RegisterProcess {
            pid: 42,
            label: "dev".into(),
            port: None,
            url: None,
            command: None,
        },
    );

    let b = member_briefing(f.store.state(), "echo").unwrap();
    assert_eq!(b.team, "t");
    assert_eq!(
        (b.you.name.as_str(), b.you.role.as_str()),
        ("echo", "developer")
    );
    assert_eq!(b.leads, ["lead"]);
    assert_eq!(b.roster.len(), 4);
    assert_eq!((b.queue.actionable, b.queue.awareness), (2, 0));
    assert_eq!(b.processes.len(), 1);
}

#[test]
fn member_briefing_is_none_without_team_or_member() {
    assert!(member_briefing(&rteams_core::State::default(), "echo").is_none());
    let f = Fixture::new();
    assert!(member_briefing(f.store.state(), "ghost").is_none());
}
