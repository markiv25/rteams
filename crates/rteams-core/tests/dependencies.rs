mod common;
use common::*;
use rteams_core::{briefing, Command, CoreError, Reason, Status};

fn set(f: &mut Fixture, task: &str, deps: &[&str]) -> Result<(), CoreError> {
    f.try_run(
        "lead",
        Command::SetDependencies {
            task: task.into(),
            blocked_by: deps.iter().map(|s| s.to_string()).collect(),
        },
    )
}

type Items = Vec<(String, Reason)>;

fn reasons(f: &Fixture, member: &str) -> (Items, Items) {
    let b = briefing(f.store.state(), member);
    let pick =
        |v: Vec<rteams_core::BriefingItem>| v.into_iter().map(|i| (i.task, i.reason)).collect();
    (pick(b.actionable), pick(b.awareness))
}

#[test]
fn relation_is_stored_one_way_and_derived_the_other() {
    let mut f = Fixture::new();
    f.task("a", Some("echo"));
    f.task("b", Some("echo"));
    f.task_after("c", Some("echo"), &["a", "b", "a"]); // duplicate collapses
    let s = f.store.state();
    assert_eq!(s.task("c").unwrap().blocked_by, ["a", "b"]);
    assert_eq!(s.dependents("a"), ["c"]);
    assert_eq!(s.dependents("c"), Vec::<&str>::new());

    set(&mut f, "c", &["b"]).unwrap();
    assert_eq!(f.store.state().task("c").unwrap().blocked_by, ["b"]);
    assert!(f.store.state().dependents("a").is_empty());
    set(&mut f, "c", &[]).unwrap();
    assert!(f.store.state().task("c").unwrap().blocked_by.is_empty());
}

#[test]
fn validation() {
    let mut f = Fixture::new();
    f.task("a", None);
    f.task("b", None);
    f.task("c", None);
    assert_eq!(
        set(&mut f, "a", &["nope"]),
        Err(CoreError::TaskNotFound("nope".into()))
    );
    assert_eq!(
        set(&mut f, "nope", &["a"]),
        Err(CoreError::TaskNotFound("nope".into()))
    );
    assert_eq!(
        f.try_run(
            "lead",
            Command::CreateTask {
                id: "d".into(),
                subject: "s".into(),
                description: "".into(),
                owner: None,
                blocked_by: vec!["nope".into()],
            }
        ),
        Err(CoreError::TaskNotFound("nope".into()))
    );
    assert!(
        f.store.state().task("d").is_none(),
        "failed create leaves nothing behind"
    );

    let cycle = |t: &str, d: &str| CoreError::DependencyCycle {
        task: t.into(),
        depends_on: d.into(),
    };
    assert_eq!(set(&mut f, "a", &["a"]), Err(cycle("a", "a")));
    set(&mut f, "a", &["b"]).unwrap();
    assert_eq!(set(&mut f, "b", &["a"]), Err(cycle("b", "a"))); // direct
    set(&mut f, "b", &["c"]).unwrap();
    assert_eq!(set(&mut f, "c", &["a"]), Err(cycle("c", "a"))); // transitive: a->b->c->a
                                                                // diamonds are fine
    f.task_after("top", None, &["a", "b"]);
    // a failed set changes nothing
    assert_eq!(
        f.store.state().task("c").unwrap().blocked_by,
        Vec::<String>::new()
    );
}

#[test]
fn unmet_dependencies_ignore_done_and_archived() {
    let mut f = Fixture::new();
    f.task("dep_open", Some("echo"));
    f.task("dep_done", Some("echo"));
    f.task("dep_archived", Some("echo"));
    f.task_after("t", Some("echo"), &["dep_open", "dep_done", "dep_archived"]);
    f.run("echo", transition("dep_done", Status::InProgress));
    f.run("echo", transition("dep_done", Status::Done));
    f.run(
        "lead",
        Command::ArchiveTask {
            task: "dep_archived".into(),
            archived: true,
        },
    );

    let s = f.store.state();
    assert_eq!(s.unmet_dependencies(s.task("t").unwrap()), ["dep_open"]);
}

#[test]
fn briefing_hides_start_until_dependencies_are_done() {
    let mut f = Fixture::new();
    f.task("first", Some("ember"));
    f.task_after("second", Some("echo"), &["first"]);

    // echo has nothing to start yet, but sees what it is waiting for
    let (act, aware) = reasons(&f, "echo");
    assert!(act.is_empty());
    assert_eq!(
        aware,
        [("second".to_string(), Reason::WaitingOnDependencies)]
    );
    // the dependency's owner is unaffected
    assert_eq!(
        reasons(&f, "ember").0,
        [("first".to_string(), Reason::Start)]
    );

    // not done yet (even in review) -> still waiting
    f.run("ember", transition("first", Status::InProgress));
    assert!(reasons(&f, "echo").0.is_empty());

    // dependency done -> becomes actionable, nothing moved automatically
    f.run("ember", transition("first", Status::Done));
    assert_eq!(f.status("second"), Status::Pending);
    let (act, aware) = reasons(&f, "echo");
    assert_eq!(act, [("second".to_string(), Reason::Start)]);
    assert!(aware.is_empty());

    // reopening the dependency puts it back to waiting
    f.run("ember", transition("first", Status::InProgress));
    assert_eq!(
        reasons(&f, "echo").1,
        [("second".to_string(), Reason::WaitingOnDependencies)]
    );
}

#[test]
fn dependencies_do_not_restrict_transitions_or_other_buckets() {
    let mut f = Fixture::new();
    f.task("first", Some("ember"));
    f.task_after("second", Some("echo"), &["first"]);
    // no enforcement in core: the relation only shapes the briefing
    f.run("echo", transition("second", Status::InProgress));
    assert_eq!(
        reasons(&f, "echo").0,
        [("second".to_string(), Reason::Resume)]
    );
}

#[test]
fn replay_restores_dependencies() {
    let mut f = Fixture::new();
    f.task("a", None);
    f.task_after("b", None, &["a"]);
    f.task("c", None);
    set(&mut f, "c", &["b"]).unwrap();
    let rebuilt = f.store.rebuild();
    assert_eq!(rebuilt.task("b").unwrap().blocked_by, ["a"]);
    assert_eq!(rebuilt.task("c").unwrap().blocked_by, ["b"]);
}
