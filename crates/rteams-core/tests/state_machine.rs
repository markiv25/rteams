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
                mode: None,
                to: Some("ghost".into()),
                text: "hi".into(),
                from_team: None
            }
        ),
        Err(CoreError::UnknownMember("ghost".into()))
    );
    assert_eq!(
        f.try_run(
            "lead",
            Command::AddMember {
                name: "echo".into(),
                role: "x".into(),
                runtime: None
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

#[test]
fn update_task_applies_and_validates() {
    let mut f = Fixture::new();
    f.task("a", Some("echo"));
    let upd = |s: Option<&str>, d: Option<&str>| Command::UpdateTask {
        task: "a".into(),
        subject: s.map(Into::into),
        description: d.map(Into::into),
    };
    f.run("echo", upd(Some("new title"), None));
    let t = f.store.state().task("a").unwrap();
    assert_eq!(
        (t.subject.as_str(), t.description.as_str()),
        ("new title", "")
    );
    f.run("echo", upd(None, Some("details")));
    let t = f.store.state().task("a").unwrap();
    assert_eq!(
        (t.subject.as_str(), t.description.as_str()),
        ("new title", "details")
    );
    assert_eq!(
        f.try_run("echo", upd(None, None)),
        Err(CoreError::EmptyUpdate)
    );
    assert_eq!(
        f.try_run("echo", upd(Some("  "), Some("x"))),
        Err(CoreError::EmptySubject)
    );
    assert_eq!(
        f.try_run(
            "echo",
            Command::UpdateTask {
                task: "zz".into(),
                subject: Some("s".into()),
                description: None
            }
        ),
        Err(CoreError::TaskNotFound("zz".into()))
    );
    // description may be cleared
    f.run("echo", upd(None, Some("")));
    assert_eq!(f.store.state().task("a").unwrap().description, "");
}

fn lead_cmds_ok(f: &mut Fixture) {
    f.run(
        "lead",
        Command::ConfigureTeam {
            project_path: Some("/proj".into()),
            lead_prompt: Some("be kind".into()),
        },
    );
    // None leaves unchanged.
    f.run(
        "lead",
        Command::ConfigureTeam {
            project_path: None,
            lead_prompt: Some("be brief".into()),
        },
    );
}

#[test]
fn configure_team() {
    let mut f = Fixture::new();
    lead_cmds_ok(&mut f);
    let t = f.store.state().team.clone().unwrap();
    assert_eq!(t.project_path.as_deref(), Some("/proj"));
    assert_eq!(t.lead_prompt.as_deref(), Some("be brief"));
    assert_eq!(
        f.try_run(
            "lead",
            Command::ConfigureTeam {
                project_path: None,
                lead_prompt: None
            }
        ),
        Err(CoreError::EmptyConfig)
    );
    let empty = rteams_core::MemStore::new();
    assert_eq!(
        empty.state().decide(Command::ConfigureTeam {
            project_path: Some("x".into()),
            lead_prompt: None
        }),
        Err(CoreError::TeamNotCreated)
    );
}

#[test]
fn update_member() {
    let mut f = Fixture::new();
    f.run(
        "lead",
        Command::UpdateMember {
            name: "echo".into(),
            role: Some("senior".into()),
            prompt: Some("p".into()),
            model: None,
            runtime: None,
            worktree: None,
            icon: None,
        },
    );
    let m = f
        .store
        .state()
        .team
        .as_ref()
        .unwrap()
        .members
        .iter()
        .find(|m| m.name == "echo")
        .cloned()
        .unwrap();
    assert_eq!(
        (m.role.as_str(), m.prompt.as_deref(), m.model),
        ("senior", Some("p"), None)
    );
    let upd = |name: &str, role: Option<&str>| Command::UpdateMember {
        name: name.into(),
        role: role.map(Into::into),
        prompt: None,
        model: Some("m".into()),
        runtime: None,
        worktree: None,
        icon: None,
    };
    assert_eq!(
        f.try_run("lead", upd("ghost", None)),
        Err(CoreError::UnknownMember("ghost".into()))
    );
    assert_eq!(
        f.try_run("lead", upd("echo", Some("  "))),
        Err(CoreError::EmptyRole)
    );
    assert_eq!(
        f.try_run(
            "lead",
            Command::UpdateMember {
                name: "echo".into(),
                role: None,
                prompt: None,
                model: None,
                runtime: None,
                worktree: None,
                icon: None,
            }
        ),
        Err(CoreError::EmptyMemberUpdate)
    );
    // The only lead cannot be demoted.
    assert_eq!(
        f.try_run("lead", upd("lead", Some("dev"))),
        Err(CoreError::LastLead("lead".into()))
    );
}

fn remove(name: &str, removed: bool) -> Command {
    Command::RemoveMember {
        name: name.into(),
        removed,
    }
}

#[test]
fn remove_and_restore_member() {
    let mut f = Fixture::new();
    f.task("a", Some("echo"));
    f.run("lead", remove("echo", true));
    assert!(!f.store.state().is_member("echo"));
    // Kept in the roster for history.
    let t = f.store.state().team.clone().unwrap();
    assert!(t.members.iter().any(|m| m.name == "echo" && m.removed));
    // Cannot be assigned, messaged, or re-added under the same name.
    assert_eq!(
        f.try_run(
            "lead",
            Command::AssignTask {
                task: "a".into(),
                owner: Some("echo".into())
            }
        ),
        Err(CoreError::UnknownMember("echo".into()))
    );
    assert_eq!(
        f.try_run(
            "lead",
            Command::SendMessage {
                mode: None,
                to: Some("echo".into()),
                text: "x".into(),
                from_team: None,
            }
        ),
        Err(CoreError::UnknownMember("echo".into()))
    );
    assert_eq!(
        f.try_run(
            "lead",
            Command::AddMember {
                name: "echo".into(),
                role: "x".into(),
                runtime: None
            }
        ),
        Err(CoreError::MemberExists("echo".into()))
    );
    assert_eq!(
        f.try_run("lead", remove("echo", true)),
        Err(CoreError::UnknownMember("echo".into()))
    );
    assert_eq!(
        f.try_run("lead", remove("ghost", false)),
        Err(CoreError::UnknownMember("ghost".into()))
    );
    f.run("lead", remove("echo", false));
    assert!(f.store.state().is_member("echo"));
    assert!(rteams_core::member_briefing(f.store.state(), "echo").is_some());
}

#[test]
fn removed_member_is_hidden_from_briefings() {
    let mut f = Fixture::new();
    f.run("lead", remove("echo", true));
    let s = f.store.state();
    assert!(rteams_core::member_briefing(s, "echo").is_none());
    let b = rteams_core::member_briefing(s, "pixel").unwrap();
    assert!(b.roster.iter().all(|m| m.name != "echo"));
    assert!(rteams_core::lead_briefing(s)
        .load
        .iter()
        .all(|l| l.member != "echo"));
}

#[test]
fn cannot_remove_last_lead_but_can_with_another() {
    let mut f = Fixture::new();
    assert_eq!(
        f.try_run("lead", remove("lead", true)),
        Err(CoreError::LastLead("lead".into()))
    );
    f.run(
        "lead",
        Command::UpdateMember {
            name: "echo".into(),
            role: Some("lead".into()),
            prompt: None,
            model: None,
            runtime: None,
            worktree: None,
            icon: None,
        },
    );
    f.run("lead", remove("lead", true));
    // echo is now the only lead.
    assert_eq!(
        f.try_run("echo", remove("echo", true)),
        Err(CoreError::LastLead("echo".into()))
    );
}

#[test]
fn message_mode_is_validated_and_carried() {
    let mut f = Fixture::new();
    let send = |mode: Option<&str>| Command::SendMessage {
        to: None,
        text: "x".into(),
        mode: mode.map(Into::into),
        from_team: None,
    };
    assert_eq!(
        f.try_run("echo", send(Some("shout"))),
        Err(CoreError::InvalidMode("shout".into()))
    );
    for m in ["do", "ask", "delegate"] {
        f.run("echo", send(Some(m)));
    }
    f.run("echo", send(None));
    let modes: Vec<_> = f
        .store
        .state()
        .messages
        .iter()
        .map(|m| m.mode.clone())
        .collect();
    assert_eq!(
        modes,
        vec![
            Some("do".into()),
            Some("ask".into()),
            Some("delegate".into()),
            None
        ]
    );
}

#[test]
fn member_icon_set_and_reset() {
    let mut f = Fixture::new();
    let icon = |f: &Fixture| {
        f.store
            .state()
            .team
            .as_ref()
            .unwrap()
            .members
            .iter()
            .find(|m| m.name == "echo")
            .unwrap()
            .icon
            .clone()
    };
    let set = |i: &str| Command::UpdateMember {
        name: "echo".into(),
        role: None,
        prompt: None,
        model: None,
        runtime: None,
        worktree: None,
        icon: Some(i.into()),
    };
    assert_eq!(icon(&f), None);
    f.run("lead", set(" 🚀 "));
    assert_eq!(icon(&f).as_deref(), Some("🚀"));
    // An icon alone is a valid update; the empty string resets to the default.
    f.run("lead", set(""));
    assert_eq!(icon(&f), None);
}
