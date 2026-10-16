mod common;
use common::*;
use rteams_core::{Command, CoreError, EventKind, Status};

fn review_cmd(task: &str) -> Command {
    Command::RequestReview {
        task: task.into(),
        reviewer: "pixel".into(),
    }
}

#[test]
fn full_review_path() {
    let mut f = Fixture::new();
    f.task("a", Some("echo"));
    assert_eq!(f.status("a"), Status::Pending);
    f.run("echo", transition("a", Status::InProgress));
    f.run("echo", review_cmd("a"));
    assert_eq!(f.status("a"), Status::Review);
    f.run(
        "pixel",
        Command::ApproveReview {
            task: "a".into(),
            note: Some("lgtm".into()),
        },
    );
    assert_eq!(f.status("a"), Status::Approved);
    f.run("echo", transition("a", Status::Done));
    assert_eq!(f.status("a"), Status::Done);
}

#[test]
fn no_review_path_and_reopen() {
    let mut f = Fixture::new();
    f.task("a", Some("echo"));
    f.run("echo", transition("a", Status::InProgress));
    f.run("echo", transition("a", Status::Done));
    f.run("echo", transition("a", Status::InProgress)); // reopen
    assert_eq!(f.status("a"), Status::InProgress);
}

#[test]
fn changes_requested_loops_back_through_needs_fix() {
    let mut f = Fixture::new();
    f.task("a", Some("echo"));
    f.run("echo", transition("a", Status::InProgress));
    f.run("echo", review_cmd("a"));
    f.run(
        "pixel",
        Command::RequestChanges {
            task: "a".into(),
            comment: "fix x".into(),
        },
    );
    assert_eq!(f.status("a"), Status::NeedsFix);
    f.run("echo", transition("a", Status::InProgress));
    f.run("echo", review_cmd("a"));
    assert_eq!(f.status("a"), Status::Review);
}

#[test]
fn block_and_unblock_restore_prior_status() {
    for (start, steps) in [
        (Status::Pending, vec![]),
        (Status::InProgress, vec![Status::InProgress]),
    ] {
        let mut f = Fixture::new();
        f.task("a", Some("echo"));
        for s in steps {
            f.run("echo", transition("a", s));
        }
        f.run(
            "echo",
            Command::Block {
                task: "a".into(),
                reason: "waiting".into(),
            },
        );
        assert_eq!(f.status("a"), Status::Blocked);
        f.run("echo", Command::Unblock { task: "a".into() });
        assert_eq!(f.status("a"), start);
    }
    // needs_fix survives a block too
    let mut f = Fixture::new();
    f.task("a", Some("echo"));
    f.run("echo", transition("a", Status::InProgress));
    f.run("echo", review_cmd("a"));
    f.run(
        "pixel",
        Command::RequestChanges {
            task: "a".into(),
            comment: "x".into(),
        },
    );
    f.run(
        "echo",
        Command::Block {
            task: "a".into(),
            reason: "r".into(),
        },
    );
    f.run("echo", Command::Unblock { task: "a".into() });
    assert_eq!(f.status("a"), Status::NeedsFix);
}

fn illegal(from: Status, to: Status, task: &str) -> CoreError {
    CoreError::IllegalTransition {
        task: task.into(),
        from,
        to,
    }
}

#[test]
fn illegal_transitions_are_rejected_with_typed_errors() {
    let mut f = Fixture::new();
    f.task("a", Some("echo"));
    // pending can't be completed, reviewed, approved or sent back
    assert_eq!(
        f.try_run("echo", transition("a", Status::Done)),
        Err(illegal(Status::Pending, Status::Done, "a"))
    );
    assert_eq!(
        f.try_run("echo", review_cmd("a")),
        Err(illegal(Status::Pending, Status::Review, "a"))
    );
    assert_eq!(
        f.try_run(
            "pixel",
            Command::ApproveReview {
                task: "a".into(),
                note: None
            }
        ),
        Err(illegal(Status::Pending, Status::Approved, "a"))
    );
    assert_eq!(
        f.try_run(
            "pixel",
            Command::RequestChanges {
                task: "a".into(),
                comment: "x".into()
            }
        ),
        Err(illegal(Status::Pending, Status::NeedsFix, "a"))
    );
    // failed commands leave no trace
    assert_eq!(f.status("a"), Status::Pending);

    // in review: can't complete without approval; can't re-start
    f.run("echo", transition("a", Status::InProgress));
    f.run("echo", review_cmd("a"));
    assert_eq!(
        f.try_run("echo", transition("a", Status::Done)),
        Err(illegal(Status::Review, Status::Done, "a"))
    );
    assert_eq!(
        f.try_run("echo", transition("a", Status::InProgress)),
        Err(illegal(Status::Review, Status::InProgress, "a"))
    );
    assert_eq!(
        f.try_run("echo", review_cmd("a")),
        Err(illegal(Status::Review, Status::Review, "a"))
    );
    assert_eq!(
        f.try_run(
            "echo",
            Command::Block {
                task: "a".into(),
                reason: "r".into()
            }
        ),
        Err(illegal(Status::Review, Status::Blocked, "a"))
    );

    // done can only be reopened
    f.run(
        "pixel",
        Command::ApproveReview {
            task: "a".into(),
            note: None,
        },
    );
    f.run("echo", transition("a", Status::Done));
    assert_eq!(
        f.try_run("echo", transition("a", Status::Pending)),
        Err(illegal(Status::Done, Status::Pending, "a"))
    );
}

#[test]
fn plain_transition_refuses_states_with_dedicated_commands() {
    let mut f = Fixture::new();
    f.task("a", Some("echo"));
    f.run("echo", transition("a", Status::InProgress));
    for to in [
        Status::Review,
        Status::Approved,
        Status::NeedsFix,
        Status::Blocked,
    ] {
        assert_eq!(
            f.try_run("echo", transition("a", to)),
            Err(CoreError::UseDedicatedCommand {
                task: "a".into(),
                to
            })
        );
    }
}

#[test]
fn unblock_rules() {
    let mut f = Fixture::new();
    f.task("a", Some("echo"));
    assert_eq!(
        f.try_run("echo", Command::Unblock { task: "a".into() }),
        Err(CoreError::NotBlocked("a".into()))
    );
    f.run(
        "echo",
        Command::Block {
            task: "a".into(),
            reason: "r".into(),
        },
    );
    // can't leave Blocked by a plain transition
    assert_eq!(
        f.try_run("echo", transition("a", Status::InProgress)),
        Err(illegal(Status::Blocked, Status::InProgress, "a"))
    );
}

#[test]
fn lookups_and_membership_are_validated() {
    let mut f = Fixture::new();
    assert_eq!(
        f.try_run("echo", transition("nope", Status::InProgress)),
        Err(CoreError::TaskNotFound("nope".into()))
    );
    f.task("a", None);
    assert_eq!(
        f.try_run(
            "lead",
            Command::CreateTask {
                id: "a".into(),
                subject: "s".into(),
                description: "".into(),
                owner: None,
                blocked_by: vec![],
            }
        ),
        Err(CoreError::TaskExists("a".into()))
    );
    assert_eq!(
        f.try_run(
            "lead",
            Command::AssignTask {
                task: "a".into(),
                owner: Some("ghost".into())
            }
        ),
        Err(CoreError::UnknownMember("ghost".into()))
    );
    assert_eq!(
        f.try_run(
            "echo",
            Command::SendMessage {
                to: Some("ghost".into()),
                text: "hi".into()
            }
        ),
        Err(CoreError::UnknownMember("ghost".into()))
    );
    assert_eq!(
        f.try_run(
            "lead",
            Command::AddMember {
                name: "echo".into(),
                role: "x".into()
            }
        ),
        Err(CoreError::MemberExists("echo".into()))
    );
    assert_eq!(
        f.try_run(
            "lead",
            Command::CreateTeam {
                name: "again".into()
            }
        ),
        Err(CoreError::TeamExists)
    );
}

#[test]
fn process_rules() {
    let mut f = Fixture::new();
    let reg = |pid| Command::RegisterProcess {
        pid,
        label: "dev".into(),
        port: Some(3000),
        url: None,
        command: None,
    };
    f.run("echo", reg(42));
    assert_eq!(
        f.try_run("echo", reg(42)),
        Err(CoreError::ProcessRunning(42))
    );
    assert_eq!(
        f.try_run("echo", Command::StopProcess { pid: 7 }),
        Err(CoreError::ProcessNotFound(7))
    );
    f.run("echo", Command::StopProcess { pid: 42 });
    assert_eq!(
        f.try_run("echo", Command::StopProcess { pid: 42 }),
        Err(CoreError::ProcessNotFound(42))
    );
    f.run("echo", reg(42)); // pid reuse after stop is fine
}

#[test]
fn replay_matches_incremental_state_and_seq_is_gapless() {
    let mut f = Fixture::new();
    f.task("a", Some("echo"));
    f.run("echo", transition("a", Status::InProgress));
    f.run(
        "echo",
        Command::Comment {
            task: "a".into(),
            text: "hi".into(),
        },
    );
    f.run("echo", review_cmd("a"));
    let seqs: Vec<u64> = f.store.events().iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (1..=seqs.len() as u64).collect::<Vec<_>>());

    let rebuilt = f.store.rebuild();
    assert_eq!(rebuilt.tasks, f.store.state().tasks);
    assert_eq!(rebuilt.messages, f.store.state().messages);

    // round-trip through JSON, as a persistent store would
    let json = serde_json::to_string(f.store.events()).unwrap();
    let back: Vec<rteams_core::Event> = serde_json::from_str(&json).unwrap();
    assert_eq!(back, f.store.events());
    let from_disk = rteams_core::MemStore::from_events(back);
    assert_eq!(from_disk.state().tasks, f.store.state().tasks);
}

#[test]
fn transition_event_records_from_and_to() {
    let mut f = Fixture::new();
    f.task("a", Some("echo"));
    f.run("echo", transition("a", Status::InProgress));
    let last = f.store.events().last().unwrap();
    assert_eq!(
        last.kind,
        EventKind::TaskTransitioned {
            task: "a".into(),
            from: Status::Pending,
            to: Status::InProgress
        }
    );
}
