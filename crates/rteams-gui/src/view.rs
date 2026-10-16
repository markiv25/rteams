//! The contract between the app shell and the screen modules (board, task dialog, messages,
//! team section, logs). Views read a `View` and push `Intent`s; only `app.rs` acts on them, so
//! views never touch the store, the commander or the launcher directly.

use rteams_core::{Member, State, Team};

use crate::actions::GuiCmd;
use crate::data::Data;
use crate::launcher::AgentView;

/// Read-only inputs for one frame.
pub struct View<'a> {
    pub state: &'a State,
    pub data: &'a Data,
    /// Who the human acts as (the lead by default); `None` = cannot write.
    pub actor: Option<&'a str>,
    /// Launcher status and captured output, one per launched member.
    pub agents: &'a [AgentView],
    /// Translator for the active UI language.
    pub tr: &'a rteams_services::i18n::I18n,
}

impl<'a> View<'a> {
    pub fn team(&self) -> Option<&'a Team> {
        self.state.team.as_ref()
    }

    pub fn members(&self) -> &'a [Member] {
        self.team().map_or(&[], |t| t.members.as_slice())
    }

    /// Roster minus the human and removed members: the members that run as agents.
    pub fn agent_members(&self) -> Vec<&'a Member> {
        self.members()
            .iter()
            .filter(|m| !m.removed && Some(m.name.as_str()) != self.actor)
            .collect()
    }

    pub fn agent(&self, member: &str) -> Option<&'a AgentView> {
        self.agents.iter().find(|a| a.member == member)
    }

    pub fn color(&self, name: &str) -> eframe::egui::Color32 {
        crate::theme::member_color(self.team(), name)
    }
}

/// Something a view asks the shell to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// Run a write as the current actor.
    Run(GuiCmd),
    /// Open the task dialog for this task id.
    OpenTask(String),
    /// Close the task dialog.
    CloseTask,
    /// Open the create-task dialog, optionally preassigned.
    NewTask {
        owner: Option<String>,
    },
    /// Select a chat: `None` = Group chat (broadcasts), `Some(member)` = direct chat.
    OpenChat(Option<String>),
    /// Launch / stop one agent, or every agent member.
    Launch(String),
    Stop(String),
    /// Kill and relaunch with the same settings.
    Restart(String),
    LaunchAll,
    StopAll,
    /// Create a task; then optionally start it and wake its owner with `prompt`.
    CreateTask {
        cmd: GuiCmd,
        start: bool,
        prompt: Option<String>,
    },
    /// Open the member detail dialog.
    OpenMember(String),
    /// Open the add-member dialog.
    AddMember,
    /// Switch to another team under the same home (Teams list).
    SwitchTeam(String),
    /// Show pending tool-approval requests.
    OpenApprovals,
    /// Show the notification centre.
    OpenNotifications,
    /// Send a message to a member of another team under the same home.
    CrossTeamSend {
        team: String,
        to: Option<String>,
        text: String,
    },
    /// Discard an agent's change to `path` (made while working `task`).
    RevertFile {
        task: String,
        path: String,
    },
    /// `git init` in the team's project path / initial commit there.
    GitInit,
    GitInitialCommit,
}

/// Collected intents for one frame.
#[derive(Debug, Default)]
pub struct Intents(pub Vec<Intent>);

impl Intents {
    pub fn push(&mut self, i: Intent) {
        self.0.push(i);
    }
}

#[cfg(test)]
pub mod testing {
    //! Headless rendering for view tests: build a real team on disk, then draw a view at
    //! several window sizes. Catches egui panics (e.g. negative widths) without a window.

    use std::path::PathBuf;

    use eframe::egui;

    use crate::actions::{Commander, GuiCmd};
    use crate::data::Data;

    /// English translator for tests (assertions on English text hold).
    pub fn tr() -> &'static rteams_services::i18n::I18n {
        static T: std::sync::OnceLock<rteams_services::i18n::I18n> = std::sync::OnceLock::new();
        T.get_or_init(rteams_services::i18n::I18n::english)
    }

    /// A team `t` with lead `boss`, members `ada` and `bob`, and tasks in several states.
    pub fn fixture(tag: &str) -> (PathBuf, Data) {
        let home = std::env::temp_dir().join(format!("rteams-gui-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let mut c = Commander::new(home.clone(), "t".into());
        let run = |c: &mut Commander, cmd: GuiCmd| {
            c.run("boss", &cmd)
                .unwrap_or_else(|e| panic!("{cmd:?}: {e}"))
        };
        run(
            &mut c,
            GuiCmd::CreateTeam {
                name: "t".into(),
                members: vec![("ada".into(), "dev".into()), ("bob".into(), "dev".into())],
            },
        );
        for (i, owner) in ["ada", "bob", "ada", "bob"].iter().enumerate() {
            run(
                &mut c,
                GuiCmd::CreateTask {
                    subject: format!("task number {i} with a fairly long subject line"),
                    description: "some **markdown** description".into(),
                    owner: Some(owner.to_string()),
                    blocked_by: if i == 3 { vec!["1".into()] } else { vec![] },
                },
            );
        }
        run(&mut c, GuiCmd::Start { task: "2".into() });
        run(&mut c, GuiCmd::Start { task: "3".into() });
        run(
            &mut c,
            GuiCmd::RequestReview {
                task: "3".into(),
                reviewer: "bob".into(),
            },
        );
        run(
            &mut c,
            GuiCmd::Comment {
                task: "3".into(),
                text: "please look".into(),
            },
        );
        run(
            &mut c,
            GuiCmd::Message {
                to: None,
                text: "hello team".into(),
                mode: None,
            },
        );
        run(
            &mut c,
            GuiCmd::Message {
                to: Some("ada".into()),
                text: "take task 1".into(),
                mode: None,
            },
        );
        let mut d = Data::open(home.clone(), "t").unwrap();
        d.refresh().unwrap();
        (home, d)
    }

    /// Draw `f` for a few frames at 1600x1000, 900x600 and 320x240.
    pub fn render(mut f: impl FnMut(&mut egui::Ui)) {
        for (w, h) in [(1600.0, 1000.0), (900.0, 600.0), (320.0, 240.0)] {
            let ctx = egui::Context::default();
            crate::theme::apply(&ctx);
            for _ in 0..3 {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(w, h),
                    )),
                    ..Default::default()
                };
                let mut out = ctx.run_ui(input, |ui| {
                    egui::CentralPanel::default().show(ui, |ui| f(ui));
                });
                out.textures_delta.clear();
            }
        }
    }
}
