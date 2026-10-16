#![allow(dead_code)]
use rteams_core::{Command, MemStore, Status};

/// Store with team "t" and members lead, echo, pixel, ember. Timestamps count up from 1000.
pub struct Fixture {
    pub store: MemStore,
    clock: i64,
}

impl Fixture {
    pub fn new() -> Self {
        let mut f = Fixture {
            store: MemStore::new(),
            clock: 1000,
        };
        f.run("lead", Command::CreateTeam { name: "t".into() });
        for (n, r) in [
            ("lead", "lead"),
            ("echo", "developer"),
            ("pixel", "reviewer"),
            ("ember", "qa"),
        ] {
            f.run(
                "lead",
                Command::AddMember {
                    name: n.into(),
                    role: r.into(),
                    runtime: None,
                },
            );
        }
        f
    }

    pub fn run(&mut self, actor: &str, cmd: Command) {
        self.try_run(actor, cmd).unwrap();
    }

    pub fn try_run(&mut self, actor: &str, cmd: Command) -> Result<(), rteams_core::CoreError> {
        self.clock += 1;
        self.store.execute(actor, self.clock, cmd).map(|_| ())
    }

    pub fn task(&mut self, id: &str, owner: Option<&str>) {
        self.run(
            "lead",
            Command::CreateTask {
                id: id.into(),
                subject: format!("subject {id}"),
                description: String::new(),
                owner: owner.map(Into::into),
                blocked_by: vec![],
            },
        );
    }

    pub fn status(&self, id: &str) -> Status {
        self.store.state().task(id).unwrap().status
    }
}

pub fn transition(task: &str, to: Status) -> Command {
    Command::Transition {
        task: task.into(),
        to,
    }
}

impl Fixture {
    pub fn task_after(&mut self, id: &str, owner: Option<&str>, blocked_by: &[&str]) {
        self.run(
            "lead",
            Command::CreateTask {
                id: id.into(),
                subject: format!("subject {id}"),
                description: String::new(),
                owner: owner.map(Into::into),
                blocked_by: blocked_by.iter().map(|s| s.to_string()).collect(),
            },
        );
    }
}
