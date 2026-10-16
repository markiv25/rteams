//! Screens added in the port's second phase (dashboard, usage, editor, terminal, ...).
//! The shell owns this file: every screen gets a `FeatureCtx` and reports what it needs the
//! shell to do as `ShellRequest`s, so screens never edit `Intent` / `View`.

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::view::{Intents, View};

pub mod close_guard;
pub mod dashboard;
pub mod editor;
pub mod ext;
pub mod graph;
pub mod i18n_widgets;
pub mod import_team_dialog;
pub mod log_stream;
pub mod news_button;
pub mod news_dialog;
pub mod orgs;
pub mod queue_list;
pub mod recovery_settings;
pub mod schedules;
pub mod sessions;
pub mod settings;
pub mod task_folders;
pub mod template_picker;
pub mod terminal;
pub mod tmux_banner;
pub mod trigger_settings;
pub mod update_banner;
pub mod update_dialog;
pub mod usage;
pub mod viz;
pub mod work_sync_panel;

/// Top-level screens that live in the tab bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TabId {
    Dashboard,
    Usage,
    Editor,
    /// Team graph (agent-graph).
    Graph,
    /// Extensions: MCP servers, plugins, skills, API keys.
    Extensions,
    /// Scheduled prompts (cron).
    Schedules,
    /// Organization map canvas.
    OrgMap,
    /// Claude / Codex session history viewer.
    Sessions,
    /// App settings.
    Settings,
}

/// Something a screen needs the shell to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellRequest {
    OpenTab(TabId),
    /// Switch to a team (directory name under the home).
    OpenTeam(String),
    Toast(String),
    /// Launch every agent of this team.
    LaunchTeam(String),
    OpenEditor {
        root: PathBuf,
        file: Option<PathBuf>,
    },
    OpenTerminal {
        cwd: PathBuf,
    },
}

/// What every screen gets each frame.
pub struct FeatureCtx<'a> {
    pub home: &'a Path,
    /// Current team directory, if one is open.
    pub team: Option<&'a str>,
    /// The open team's view and intent sink, for screens that act on the open team (the
    /// later org / work-sync screens); phase-one screens only read `home` / `team`.
    #[allow(dead_code)]
    pub view: Option<&'a View<'a>>,
    #[allow(dead_code)]
    pub out: &'a mut Intents,
    pub tr: &'a rteams_services::i18n::I18n,
    pub req: &'a mut Vec<ShellRequest>,
}

/// A tab screen.
pub trait Screen {
    fn id(&self) -> TabId;
    /// i18n key of the tab title.
    fn title_key(&self) -> &'static str;
    /// Background polling, called every frame even when the tab is hidden.
    fn tick(&mut self, _ctx: &egui::Context, _c: &mut FeatureCtx) {}
    fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx);
}

/// Unsaved work that should stop the app from closing silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseBlocker {
    pub owner: String,
    pub detail: String,
}

pub trait CloseParticipant {
    fn blockers(&self) -> Vec<CloseBlocker>;
    fn save_all(&mut self) -> Result<(), String>;
}
