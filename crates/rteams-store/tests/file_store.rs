use std::fs::OpenOptions;
use std::io::Write;
use std::thread;

use rteams_core::{briefing, inbox, kanban, Command, CoreError, MemStore, Scope, State, Status};
use rteams_store::{FileStore, Store, StoreError};
use tempfile::TempDir;

fn setup(store: &mut impl Store) {
    store
        .execute("lead", 1, Command::CreateTeam { name: "t".into() })
        .unwrap();
    for (n, r) in [
        ("lead", "lead"),
        ("echo", "developer"),
        ("pixel", "reviewer"),
    ] {
        store
            .execute(
                "lead",
                2,
                Command::AddMember {
                    name: n.into(),
                    role: r.into(),
                },
            )
            .unwrap();
    }
}

fn script() -> Vec<(&'static str, Command)> {
    let t = |id: &str| id.to_string();
    vec![
        (
            "lead",
            Command::CreateTask {
                id: t("a"),
                subject: "A".into(),
                description: "d".into(),
                owner: Some("echo".into()),
                blocked_by: vec![],
            },
        ),
        (
            "lead",
            Command::CreateTask {
                id: t("b"),
                subject: "B".into(),
                description: "d".into(),
                owner: None,
                blocked_by: vec![],
            },
        ),
        (
            "echo",
            Command::Transition {
                task: t("a"),
                to: Status::InProgress,
            },
        ),
        (
            "echo",
            Command::Comment {
                task: t("a"),
                text: "working".into(),
            },
        ),
        (
            "echo",
            Command::RequestReview {
                task: t("a"),
                reviewer: "pixel".into(),
            },
        ),
        (
            "pixel",
            Command::RequestChanges {
                task: t("a"),
                comment: "fix it".into(),
            },
        ),
        (
            "lead",
            Command::SendMessage {
                to: Some("echo".into()),
                text: "hi".into(),
            },
        ),
        (
            "lead",
            Command::SendMessage {
                to: None,
                text: "all".into(),
            },
        ),
        (
            "lead",
            Command::Block {
                task: t("b"),
                reason: "wait".into(),
            },
        ),
        (
            "echo",
            Command::RegisterProcess {
                pid: 7,
                label: "srv".into(),
                port: Some(80),
                url: None,
                command: None,
            },
        ),
    ]
}

fn run_script(store: &mut impl Store) {
    for (i, (actor, cmd)) in script().into_iter().enumerate() {
        store.execute(actor, 100 + i as i64, cmd).unwrap();
    }
}

#[test]
fn matches_memstore_exactly() {
    let dir = TempDir::new().unwrap();
    let mut file = FileStore::open(dir.path(), "t").unwrap();
    let mut mem = MemStore::new();
    setup(&mut file);
    setup(&mut mem);
    run_script(&mut file);
    run_script(&mut mem);
    assert_eq!(file.events().unwrap(), mem.events());
    assert_eq!(kanban(file.state().unwrap()), kanban(mem.state()));
    assert_eq!(
        briefing(file.state().unwrap(), "echo"),
        briefing(mem.state(), "echo")
    );
    assert_eq!(
        inbox(file.state().unwrap(), "echo", 0),
        inbox(mem.state(), "echo", 0)
    );
}

#[test]
fn state_survives_restart_and_equals_rebuild() {
    let dir = TempDir::new().unwrap();
    let before = {
        let mut s = FileStore::open(dir.path(), "t").unwrap();
        setup(&mut s);
        run_script(&mut s);
        s.events().unwrap().to_vec()
    };
    let mut s = FileStore::open(dir.path(), "t").unwrap();
    assert_eq!(s.events().unwrap(), &before[..]);
    let rebuilt = State::from_events(&before);
    let st = s.state().unwrap();
    assert_eq!(st.tasks, rebuilt.tasks);
    assert_eq!(st.messages, rebuilt.messages);
    assert_eq!(st.processes, rebuilt.processes);
    assert_eq!(st.task("a").unwrap().status, Status::NeedsFix);
    // and it keeps appending with the right seq
    let e = s
        .execute(
            "echo",
            999,
            Command::Transition {
                task: "a".into(),
                to: Status::InProgress,
            },
        )
        .unwrap();
    assert_eq!(e.seq, before.len() as u64 + 1);
}

#[test]
fn rejected_command_appends_nothing() {
    let dir = TempDir::new().unwrap();
    let mut s = FileStore::open(dir.path(), "t").unwrap();
    setup(&mut s);
    let n = s.events().unwrap().len();
    let err = s
        .execute(
            "x",
            5,
            Command::Comment {
                task: "nope".into(),
                text: "x".into(),
            },
        )
        .unwrap_err();
    assert!(matches!(err, StoreError::Core(CoreError::TaskNotFound(_))));
    assert_eq!(s.events().unwrap().len(), n);
    assert_eq!(
        FileStore::open(dir.path(), "t")
            .unwrap()
            .events()
            .unwrap()
            .len(),
        n
    );
}

#[test]
fn second_handle_sees_writes_from_first() {
    let dir = TempDir::new().unwrap();
    let mut a = FileStore::open(dir.path(), "t").unwrap();
    let mut b = FileStore::open(dir.path(), "t").unwrap();
    setup(&mut a);
    assert_eq!(b.state().unwrap().team.as_ref().unwrap().members.len(), 3);
    // b writes after a's events; a catches up on its next write instead of reusing a stale seq
    let eb = b
        .execute(
            "lead",
            10,
            Command::SendMessage {
                to: None,
                text: "from b".into(),
            },
        )
        .unwrap();
    let ea = a
        .execute(
            "lead",
            11,
            Command::SendMessage {
                to: None,
                text: "from a".into(),
            },
        )
        .unwrap();
    assert_eq!(ea.seq, eb.seq + 1);
}

#[test]
fn concurrent_writers_keep_seq_gapless() {
    let dir = TempDir::new().unwrap();
    {
        let mut s = FileStore::open(dir.path(), "t").unwrap();
        setup(&mut s);
    }
    let base = 4u64;
    let (writers, per) = (4u64, 25u64);
    let handles: Vec<_> = (0..writers)
        .map(|w| {
            let root = dir.path().to_path_buf();
            thread::spawn(move || {
                let mut s = FileStore::open(&root, "t").unwrap();
                for i in 0..per {
                    s.execute(
                        "lead",
                        0,
                        Command::SendMessage {
                            to: None,
                            text: format!("{w}-{i}"),
                        },
                    )
                    .unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let mut s = FileStore::open(dir.path(), "t").unwrap();
    let events = s.events().unwrap();
    assert_eq!(events.len() as u64, base + writers * per);
    for (i, e) in events.iter().enumerate() {
        assert_eq!(e.seq, i as u64 + 1);
    }
    let msgs = s
        .state()
        .unwrap()
        .messages
        .iter()
        .filter(|m| m.scope == Scope::Broadcast)
        .count();
    assert_eq!(msgs as u64, writers * per);
}

#[test]
fn torn_tail_is_ignored_then_repaired() {
    let dir = TempDir::new().unwrap();
    let log = {
        let mut s = FileStore::open(dir.path(), "t").unwrap();
        setup(&mut s);
        s.dir().join("events.jsonl")
    };
    let len_ok = std::fs::metadata(&log).unwrap().len();
    // simulate a crash mid-append
    OpenOptions::new()
        .append(true)
        .open(&log)
        .unwrap()
        .write_all(br#"{"seq":5,"ts":1,"act"#)
        .unwrap();

    let mut s = FileStore::open(dir.path(), "t").unwrap();
    assert_eq!(s.events().unwrap().len(), 4);
    assert!(
        std::fs::metadata(&log).unwrap().len() > len_ok,
        "readers do not modify the file"
    );

    let e = s
        .execute(
            "lead",
            9,
            Command::SendMessage {
                to: None,
                text: "after crash".into(),
            },
        )
        .unwrap();
    assert_eq!(e.seq, 5);
    let again = FileStore::open(dir.path(), "t")
        .unwrap()
        .events()
        .unwrap()
        .to_vec();
    assert_eq!(again.len(), 5);
}

#[test]
fn corrupt_complete_line_is_an_error() {
    let dir = TempDir::new().unwrap();
    let log = {
        let mut s = FileStore::open(dir.path(), "t").unwrap();
        setup(&mut s);
        s.dir().join("events.jsonl")
    };
    OpenOptions::new()
        .append(true)
        .open(&log)
        .unwrap()
        .write_all(b"not json\n")
        .unwrap();
    match FileStore::open(dir.path(), "t") {
        Err(StoreError::Corrupt { line, .. }) => assert_eq!(line, 5),
        other => panic!("expected Corrupt, got {:?}", other.map(|_| ())),
    }
}

#[test]
fn command_id_is_idempotent_across_handles_and_restart() {
    let dir = TempDir::new().unwrap();
    let mut a = FileStore::open(dir.path(), "t").unwrap();
    setup(&mut a);
    let mk = || Command::CreateTask {
        id: "x".into(),
        subject: "S".into(),
        description: "".into(),
        owner: None,
        blocked_by: vec![],
    };

    let first = a.execute_with_id("lead", 10, "cmd-1", mk()).unwrap();
    assert!(!first.replayed);
    // a retry would normally fail with TaskExists; with the same id it returns the original event
    let retry = a.execute_with_id("lead", 11, "cmd-1", mk()).unwrap();
    assert!(retry.replayed);
    assert_eq!(retry.event, first.event);

    let mut b = FileStore::open(dir.path(), "t").unwrap();
    let retry2 = b.execute_with_id("lead", 12, "cmd-1", mk()).unwrap();
    assert!(retry2.replayed);
    assert_eq!(retry2.event.seq, first.event.seq);
    assert_eq!(b.events().unwrap().len() as u64, first.event.seq);

    // a different id is a new command and hits normal validation
    let err = b.execute_with_id("lead", 13, "cmd-2", mk()).unwrap_err();
    assert!(matches!(err, StoreError::Core(CoreError::TaskExists(_))));
}

#[test]
fn team_names_cannot_escape_root() {
    let dir = TempDir::new().unwrap();
    for bad in [
        "",
        ".",
        "..",
        "../x",
        "a/b",
        ".hidden",
        "a b",
        &"x".repeat(65),
    ] {
        assert!(
            matches!(
                FileStore::open(dir.path(), bad),
                Err(StoreError::InvalidTeamName(_))
            ),
            "{bad:?}"
        );
    }
    assert!(FileStore::open(dir.path(), "ai-team_1.v2").is_ok());
}

#[test]
fn teams_are_isolated() {
    let dir = TempDir::new().unwrap();
    let mut a = FileStore::open(dir.path(), "a").unwrap();
    let mut b = FileStore::open(dir.path(), "b").unwrap();
    a.execute("x", 1, Command::CreateTeam { name: "a".into() })
        .unwrap();
    assert!(b.state().unwrap().team.is_none());
}

fn read_snap(dir: &std::path::Path, name: &str) -> serde_json::Value {
    let p = dir.join("t").join("tasks").join(format!("{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[test]
fn snapshots_match_projected_state() {
    let dir = TempDir::new().unwrap();
    let mut s = FileStore::open(dir.path(), "t").unwrap();
    setup(&mut s);
    run_script(&mut s);
    let st = s.state().unwrap().clone();
    for t in &st.tasks {
        let v = read_snap(dir.path(), &t.id);
        assert_eq!(v["id"], t.id.as_str());
        assert_eq!(v["display_id"], t.id.as_str());
        assert_eq!(v["status"], serde_json::to_value(t.status).unwrap());
        assert_eq!(v["owner"], serde_json::to_value(&t.owner).unwrap());
        let want: Vec<_> = st
            .messages
            .iter()
            .filter(|m| m.scope == Scope::Task { task: t.id.clone() })
            .collect();
        assert_eq!(v["comments"].as_array().unwrap().len(), want.len());
    }
    // task "a" got a comment and a review bounce: 2 task-scoped messages
    assert_eq!(
        read_snap(dir.path(), "a")["comments"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(read_snap(dir.path(), "a")["status"], "needs_fix");
    assert_eq!(read_snap(dir.path(), "b")["status"], "blocked");
}

#[test]
fn snapshots_are_never_read_back_and_can_be_regenerated() {
    let dir = TempDir::new().unwrap();
    {
        let mut s = FileStore::open(dir.path(), "t").unwrap();
        setup(&mut s);
        run_script(&mut s);
    }
    let tasks_dir = dir.path().join("t").join("tasks");
    std::fs::write(tasks_dir.join("a.json"), "garbage").unwrap();
    std::fs::remove_file(tasks_dir.join("b.json")).unwrap();
    // state is unaffected by tampered snapshots
    let mut s = FileStore::open(dir.path(), "t").unwrap();
    assert_eq!(
        s.state().unwrap().task("a").unwrap().status,
        Status::NeedsFix
    );
    s.rewrite_snapshots().unwrap();
    assert_eq!(read_snap(dir.path(), "a")["status"], "needs_fix");
    assert_eq!(read_snap(dir.path(), "b")["status"], "blocked");
}

#[test]
fn snapshot_names_are_safe_and_collisions_fall_back_to_full_id() {
    let dir = TempDir::new().unwrap();
    let mut s = FileStore::open(dir.path(), "t").unwrap();
    setup(&mut s);
    let mk = |id: &str| Command::CreateTask {
        id: id.into(),
        subject: "S".into(),
        description: "".into(),
        owner: None,
        blocked_by: vec![],
    };
    s.execute("lead", 3, mk("../../evil")).unwrap();
    s.execute("lead", 4, mk("a1b2c3d4-0001-x")).unwrap();
    s.execute("lead", 5, mk("a1b2c3d4-0002-x")).unwrap();
    s.execute(
        "lead",
        6,
        Command::Comment {
            task: "a1b2c3d4-0002-x".into(),
            text: "hi".into(),
        },
    )
    .unwrap();

    let tasks_dir = dir.path().join("t").join("tasks");
    assert!(!dir.path().join("evil.json").exists());
    assert!(
        tasks_dir.join("__.__.json").exists()
            || std::fs::read_dir(&tasks_dir).unwrap().all(|e| {
                let n = e.unwrap().file_name().into_string().unwrap();
                !n.contains('/') && !n.starts_with('.')
            })
    );
    // second same-prefix task falls back to its full (sanitized) id
    assert!(tasks_dir.join("a1b2c3d4-0002-x.json").exists());
    assert_eq!(
        read_snap(dir.path(), "a1b2c3d4-0002-x")["comments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // no temp files left behind
    for e in std::fs::read_dir(&tasks_dir).unwrap() {
        assert!(e.unwrap().file_name().to_string_lossy().ends_with(".json"));
    }
}
