//! App shell, laid out like agent-teams' team screen: a 40px tab bar on top, the Messages
//! column on the left, and the team view (header, members, kanban, logs) in the main column.
//! Views only emit `Intent`s; this file is the one place that runs commands, opens dialogs
//! and drives the launcher.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use eframe::egui::{self, RichText};

use crate::actions::{Commander, GuiCmd};
use crate::approvals::Approvals;
use crate::board::{self, BoardFilter};
use crate::data::Data;
use crate::forms;
use crate::git;
use crate::icons::{self, Icon};
use crate::launcher::{AgentSpec, AgentStatus, LaunchConfig, Mode, Permissions, Supervisor};
use crate::logs::Logs;
use crate::member_dialog::MemberDialog;
use crate::messages::Messages;
use crate::notifications::NotificationCenter;
use crate::notify;
use crate::providers::Providers;
use crate::settings::{normalize_perm, SettingsDialog, PERM_ASK, PERM_EDITS};
use crate::task_dialog::{CreateTaskDialog, TaskDialog};
use crate::team;
use crate::teams::Teams;
use crate::theme;
use crate::ui::activity;
use crate::ui::close_guard::{CloseDecision, CloseGuard, StaticBlockers};
use crate::ui::dashboard::Dashboard;
use crate::ui::editor::EditorScreen;
use crate::ui::ext::store_tabs::ExtensionsScreen;
use crate::ui::graph::GraphScreen;
use crate::ui::launch_dialog::{LaunchDialog, LaunchOptions};
use crate::ui::log_stream::LogStream;
use crate::ui::news_button::NewsButton;
use crate::ui::onboarding::Onboarding;
use crate::ui::orgs::map_tab::OrgMapScreen;
use crate::ui::palette::{self, Palette};
use crate::ui::queue_list::QueueList;
use crate::ui::schedules::Schedules;
use crate::ui::sessions::SessionsScreen;
use crate::ui::settings::permissions::ToolApproval;
use crate::ui::settings::SettingsScreen;
use crate::ui::task_folders::TaskFoldersPanel;
use crate::ui::task_sidebar::TaskSidebar;
use crate::ui::terminal::TerminalDrawer;
use crate::ui::tmux_banner::TmuxBanner;
use crate::ui::update_banner::UpdateBanner;
use crate::ui::usage::UsageScreen;
use crate::ui::work_sync_panel::WorkSyncPanel;
use crate::ui::{CloseParticipant, FeatureCtx, Screen, ShellRequest, TabId};
use crate::view::{Intent, Intents, View};
use crate::wake::WakeScheduler;
use rteams_services::config::AppConfig;
use rteams_services::ext::apikeys;
use rteams_services::http_server::{Handle as HttpHandle, HttpServer};
use rteams_services::i18n::I18n;
use rteams_services::recovery::scheduler as recovery;
use rteams_services::schedule::{history, runner};
use rteams_services::team_import::TeamDraft;
use rteams_services::trust;
use rteams_services::worksync::nudge::WorkSync;

/// How often the log is checked for new events. Cheap: it is one `read` past the last offset.
const POLL: Duration = Duration::from_millis(500);

/// Agents running at once, at most; further wakes wait their turn.
const MAX_CONCURRENT: usize = 3;

/// Kick-off prompt; the token-frugal rules live in the launcher's system prompt.
const PROMPT: &str = "Work your rteams queue: take your first actionable task and finish it.";

pub struct App {
    home: PathBuf,
    team: String,
    /// `None` until the team's log exists (an agent or the create screen makes it).
    data: Option<Data>,
    open_error: Option<String>,
    commander: Commander,
    supervisor: Supervisor,
    /// Where launched agents run.
    workdir: String,
    /// `--as`; otherwise the team lead acts.
    actor_override: Option<String>,
    /// Last refresh error; the previous board stays on screen.
    error: Option<String>,
    /// Outcome of the last action: Ok(label) or Err(message).
    last: Option<Result<String, String>>,
    messages: Messages,
    filter: BoardFilter,
    logs: Logs,
    task_dialog: TaskDialog,
    create_task: CreateTaskDialog,
    create_team: forms::CreateTeam,
    teams: Teams,
    add_member: forms::AddMemberDialog,
    member_dialog: MemberDialog,
    /// Which top tab is showing.
    tab: Tab,
    /// On-demand tabs the user opened (closable), in opening order.
    open_tabs: Vec<Tab>,
    /// Teams listed in the team tab's menu, refreshed each time the menu opens.
    team_menu: Vec<crate::teams::TeamCard>,
    /// Tab the strip last scrolled into view; a new selection scrolls once, then the user can.
    tab_scrolled: Option<Tab>,
    /// Language list expanded inside the overflow menu.
    lang_open: bool,
    /// Last toast text shown and when it first appeared (for auto-dismiss).
    toast_seen: Option<(String, std::time::Instant)>,
    /// Teams whose Team section was collapsed once because the roster is big.
    collapse_seeded: HashSet<String>,
    providers: Providers,
    i18n: I18n,
    dashboard: Dashboard,
    usage: UsageScreen,
    editor: EditorScreen,
    terminal: TerminalDrawer,
    /// The user confirmed closing; let the next close request through.
    allow_close: bool,
    config: AppConfig,
    graph: GraphScreen,
    ext: ExtensionsScreen,
    schedules: Schedules,
    queue: QueueList,
    work_sync: WorkSyncPanel,
    log_stream: LogStream,
    /// Logs window opened from the Messages sidebar's `Logs` strip.
    show_logs: bool,
    org_map: OrgMapScreen,
    settings_screen: SettingsScreen,
    sessions: SessionsScreen,
    folders: TaskFoldersPanel,
    tmux: TmuxBanner,
    guard: CloseGuard,
    update: UpdateBanner,
    /// First-run setup wizard (no teams yet).
    onboarding: Onboarding,
    news: NewsButton,
    /// Local HTTP API, running while Settings > Advanced enables it.
    http: Option<HttpHandle>,
    /// Last time schedules / recovery (15 s) and work-sync (60 s) were evaluated.
    last_automation: i64,
    last_work_sync: i64,
    /// Members seen running last frame, to notice runs that just ended.
    was_running: HashSet<String>,
    /// Wakes agents when the log gives them work (messages, assignments, reviews...).
    wake: WakeScheduler,
    settings: SettingsDialog,
    /// Log position up to which OS notifications have been shown.
    notified: u64,
    approvals: Approvals,
    /// Permission requests already announced (so each notifies once).
    approvals_seen: HashSet<String>,
    center: NotificationCenter,
    /// `skip` | `edits` | `ask`, saved per team in gui-state.json.
    perm: String,
    /// Git branch of the project path, refreshed when settings open or git changes.
    branch: Option<String>,
    window_title: String,
    /// Command palette / global search.
    palette: Palette,
    /// Palette candidates, rebuilt every couple of seconds while it is open.
    pal_items: Vec<palette::Item>,
    pal_acts: Vec<PAct>,
    pal_built: Option<std::time::Instant>,
    /// Global task sidebar (right).
    task_sidebar: TaskSidebar,
    /// Second pane (right) of the split view, with its own active tab.
    split: Option<Tab>,
    /// Launch dialog (member picker + options).
    launch_dialog: LaunchDialog,
    /// Options picked in the launch dialog, applied to the launches it starts.
    launch_opts: Option<LaunchOptions>,
    /// Log position up to which LaunchRequested / StopRequested events were handled.
    launch_cursor: u64,
}

/// Message mode string -> launcher mode (unknown or none = do).
fn mode_of(m: Option<&str>) -> Mode {
    match m {
        Some("ask") => Mode::Ask,
        Some("delegate") => Mode::Delegate,
        _ => Mode::Do,
    }
}

/// Settings choice -> launcher policy. Default is skip-all, as the user chose (same as the
/// original app).
fn permissions_of(perm: &str) -> Permissions {
    match normalize_perm(perm) {
        PERM_EDITS => Permissions::EditsOnly,
        PERM_ASK => Permissions::Manual,
        _ => Permissions::SkipAll,
    }
}

fn launch_config(home: &Path, team: &str) -> LaunchConfig {
    let mut cfg = LaunchConfig::new(home.to_path_buf(), team);
    cfg.permissions = permissions_of(&load_perm(home, team));
    cfg
}

fn team_project(state: &rteams_core::State) -> Option<PathBuf> {
    let p = state.team.as_ref()?.project_path.clone()?;
    (!p.trim().is_empty()).then(|| PathBuf::from(p.trim()))
}

fn gui_state_path(home: &Path, team: &str) -> PathBuf {
    home.join(team).join("gui-state.json")
}

/// Saved permission choice for `team` (`skip` when unset).
fn load_gui_key(home: &Path, team: &str, key: &str) -> Option<String> {
    std::fs::read_to_string(gui_state_path(home, team))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v[key].as_str().map(String::from))
}

fn load_perm(home: &Path, team: &str) -> String {
    load_gui_key(home, team, "permissions")
        // No per-team choice yet: Settings > Permissions (app-wide; default skip).
        .unwrap_or_else(|| ToolApproval::from_extra(&AppConfig::load(home).extra).default)
}

/// Save the permission choice, keeping the file's other keys. Best effort.
fn save_perm(home: &Path, team: &str, perm: &str) {
    save_gui_key(home, team, "permissions", perm);
}

/// The team's saved extra CLI flags (raw text); empty when none.
fn load_extra_args(home: &Path, team: &str) -> String {
    load_gui_key(home, team, "extra_cli_args").unwrap_or_default()
}

/// Set one string key in the team's GUI state file, keeping the other keys. Best effort.
fn save_gui_key(home: &Path, team: &str, key: &str, value: &str) {
    let path = gui_state_path(home, team);
    let mut v = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    v[key] = serde_json::Value::String(value.into());
    let _ = std::fs::write(path, v.to_string());
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Top-level tabs, as in the original's tab bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Tab {
    Dashboard,
    Team,
    Teams,
    Usage,
    Editor,
    Graph,
    Extensions,
    Schedules,
    OrgMap,
    Sessions,
    Settings,
    Providers,
}

/// What a palette row does when picked.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PAct {
    OpenTab(Tab),
    NewTask,
    NewTeam,
    LaunchAll,
    StopAll,
    ToggleTaskSidebar,
    ToggleSplit,
    SwitchTeam(String),
    Task(String),
    Member(String),
    Chat(Option<String>),
    Logs,
}

/// Right-click menu choices of a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TabAct {
    Close,
    CloseOthers,
    CloseRight,
    MoveToPane,
    CloseSplit,
}

/// Move `item` so it sits before the element at `slot` (0..=len, counted before the move).
fn move_to_slot<T: PartialEq + Copy>(v: &mut Vec<T>, item: T, slot: usize) {
    let Some(from) = v.iter().position(|x| *x == item) else {
        return;
    };
    v.remove(from);
    let at = if slot > from { slot - 1 } else { slot };
    v.insert(at.min(v.len()), item);
}

impl Tab {
    /// Tabs that can show in the split pane: screens drawn by `phase2`.
    fn splittable(self) -> bool {
        matches!(
            self,
            Tab::Dashboard
                | Tab::Usage
                | Tab::Editor
                | Tab::Graph
                | Tab::Extensions
                | Tab::Schedules
                | Tab::OrgMap
                | Tab::Sessions
                | Tab::Settings
        )
    }

    /// On-demand tabs the user can close; Dashboard, the team and Teams are always there.
    fn closable(self) -> bool {
        !matches!(self, Tab::Dashboard | Tab::Team | Tab::Teams)
    }

    fn icon(self) -> Icon {
        match self {
            Tab::Dashboard => Icon::LayoutDashboard,
            Tab::Team | Tab::Teams => Icon::Users,
            Tab::Usage => Icon::ChartBar,
            Tab::Editor => Icon::Code,
            Tab::Graph => Icon::Network,
            Tab::Extensions => Icon::Puzzle,
            Tab::Schedules => Icon::Clock,
            Tab::OrgMap => Icon::Sitemap,
            Tab::Sessions => Icon::History,
            Tab::Settings => Icon::Settings,
            Tab::Providers => Icon::Cpu,
        }
    }
}

impl From<TabId> for Tab {
    fn from(t: TabId) -> Self {
        match t {
            TabId::Dashboard => Tab::Dashboard,
            TabId::Usage => Tab::Usage,
            TabId::Editor => Tab::Editor,
            TabId::Graph => Tab::Graph,
            TabId::Extensions => Tab::Extensions,
            TabId::Schedules => Tab::Schedules,
            TabId::OrgMap => Tab::OrgMap,
            TabId::Sessions => Tab::Sessions,
            TabId::Settings => Tab::Settings,
        }
    }
}

pub fn window_title(team: &str) -> String {
    format!("rteams \u{2014} {team}")
}

/// Default name offered for the lead when creating a team.
fn default_lead(actor: Option<&str>) -> String {
    actor
        .map(String::from)
        .or_else(|| std::env::var("USER").ok().filter(|u| !u.is_empty()))
        .unwrap_or_else(|| "boss".into())
}

impl App {
    pub fn new(cc_ctx: &egui::Context, home: PathBuf, team: String, actor: Option<String>) -> Self {
        theme::apply(cc_ctx);
        egui_extras::install_image_loaders(cc_ctx);
        let config = AppConfig::load(&home);
        let home_for_screens = home.clone();
        apikeys::init_home(&home);
        crate::provider_cfg::set_home(&home);
        history::interrupt_stale(&home, &team, now_ms());
        let workdir = std::env::current_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        let mut app = App {
            commander: Commander::new(home.clone(), team.clone()),
            supervisor: Supervisor::new(launch_config(&home, &team)),
            create_team: forms::CreateTeam::new(default_lead(actor.as_deref())),
            window_title: window_title(&team),
            home,
            team,
            data: None,
            open_error: None,
            workdir,
            actor_override: actor,
            error: None,
            last: None,
            messages: Messages::default(),
            filter: BoardFilter::default(),
            logs: Logs::default(),
            task_dialog: TaskDialog::default(),
            create_task: CreateTaskDialog::default(),
            teams: Teams::default(),
            add_member: forms::AddMemberDialog::default(),
            member_dialog: MemberDialog::default(),
            tab: Tab::Team,
            open_tabs: Vec::new(),
            team_menu: Vec::new(),
            tab_scrolled: None,
            lang_open: false,
            toast_seen: None,
            collapse_seeded: HashSet::new(),
            providers: Providers::default(),
            i18n: I18n::from_pref(&config.general.locale),
            dashboard: Dashboard::new(),
            usage: UsageScreen::new(&home_for_screens),
            editor: EditorScreen::new(home_for_screens.clone()),
            terminal: TerminalDrawer::new(),
            allow_close: false,
            config: config.clone(),
            graph: GraphScreen::new(),
            ext: ExtensionsScreen::new(),
            schedules: Schedules::new(),
            queue: QueueList::default(),
            work_sync: WorkSyncPanel,
            log_stream: LogStream::new(),
            show_logs: false,
            org_map: OrgMapScreen::new(),
            settings_screen: SettingsScreen::new(),
            sessions: SessionsScreen::new(),
            folders: TaskFoldersPanel::default(),
            tmux: TmuxBanner::default(),
            guard: CloseGuard::default(),
            update: UpdateBanner::default(),
            onboarding: Onboarding::default(),
            news: NewsButton::default(),
            http: None,
            last_automation: 0,
            last_work_sync: 0,
            was_running: HashSet::new(),
            wake: WakeScheduler::default(),
            settings: SettingsDialog::default(),
            notified: 0,
            approvals: Approvals::default(),
            approvals_seen: HashSet::new(),
            center: NotificationCenter::default(),
            perm: "skip".into(),
            branch: None,
            palette: Palette::default(),
            pal_items: Vec::new(),
            pal_acts: Vec::new(),
            pal_built: None,
            task_sidebar: TaskSidebar::default(),
            split: None,
            launch_dialog: LaunchDialog::default(),
            launch_opts: None,
            launch_cursor: 0,
        };
        app.try_open();
        app.sync_http();
        if Onboarding::needed(&app.home) {
            app.onboarding.start();
        }
        app
    }

    fn try_open(&mut self) {
        match Data::open(self.home.clone(), &self.team) {
            Ok(d) => {
                // History before the GUI opened never wakes anyone.
                self.wake = WakeScheduler::with_cursor(d.seq());
                self.notified = d.seq();
                self.launch_cursor = d.seq();
                self.messages
                    .load_cursors(gui_state_path(&self.home, &self.team));
                self.messages.set_home(self.home.clone(), self.team.clone());
                self.center.load(&self.home, &self.team);
                self.logs.load_history(&self.home, &self.team);
                self.member_dialog
                    .set_paths(self.home.clone(), self.team.clone());
                // Requests pending before the GUI opened show in the sheet, not as toasts.
                self.approvals_seen = d.state().permissions.iter().map(|p| p.id.clone()).collect();
                self.perm = load_perm(&self.home, &self.team);
                self.launch_dialog.extra = load_extra_args(&self.home, &self.team);
                self.supervisor.set_permissions(permissions_of(&self.perm));
                self.data = Some(d);
                self.sync_repo();
                self.open_error = None;
            }
            Err(e) => self.open_error = Some(e),
        }
    }

    /// The team's project path, when set and non-empty.
    fn project(&self) -> Option<PathBuf> {
        team_project(self.data.as_ref()?.state())
    }

    /// Point the task dialog at the project repo and refresh the branch label.
    fn sync_repo(&mut self) {
        let repo = self.project();
        self.branch = repo.as_deref().and_then(git::current_branch);
        self.member_dialog.set_repo(repo.clone());
        self.task_dialog
            .set_paths(self.home.clone(), self.team.clone(), repo);
    }

    /// Run a git operation in the project path and refresh the branch.
    fn git_op(&mut self, op: fn(&Path) -> Result<(), String>, done: &str) {
        self.last = Some(match self.project() {
            Some(repo) => op(&repo).map(|_| done.to_string()),
            None => Err(self.i18n.t("rteams.toast.setProjectPath")),
        });
        self.sync_repo();
    }

    /// Who the GUI acts as: `--as`, else the first lead on the roster.
    fn actor(&self) -> Option<String> {
        if let Some(a) = &self.actor_override {
            return Some(a.clone());
        }
        let team = self.data.as_ref()?.state().team.as_ref()?;
        team.members
            .iter()
            .find(|m| m.role == "lead")
            .map(|m| m.name.clone())
    }

    fn run(&mut self, actor: &str, cmd: &GuiCmd) -> bool {
        self.run_value(actor, cmd).is_some()
    }

    /// Run `cmd`, record the outcome for the tab bar, and return the tool result on success.
    fn run_value(&mut self, actor: &str, cmd: &GuiCmd) -> Option<serde_json::Value> {
        let r = self.commander.run(actor, cmd);
        self.last = Some(
            r.as_ref()
                .map(|_| cmd.done_label(&self.i18n))
                .map_err(Clone::clone),
        );
        let v = r.ok()?;
        if let Some(d) = &mut self.data {
            let _ = d.refresh();
        }
        Some(v)
    }

    /// Launch triggered by automation (nudge, retry, schedule): only within the concurrency
    /// cap. Returns false when deferred; the caller retries on a later tick.
    fn auto_launch(&mut self, member: &str, prompt: String, mode: Mode) -> bool {
        if self.supervisor.running_count() >= MAX_CONCURRENT {
            return false;
        }
        self.launch_with(member, prompt, mode);
        true
    }

    fn launch(&mut self, member: &str) {
        self.launch_with(member, PROMPT.into(), Mode::Do);
    }

    /// Launch `member` with the team's settings: work folder (the team's project path, else
    /// the header's workdir), the member's model, and standing instructions (lead prompt plus
    /// the member's own prompt).
    fn launch_with(&mut self, member: &str, prompt: String, mode: Mode) {
        let Some(team) = self.data.as_ref().and_then(|d| d.state().team.as_ref()) else {
            return;
        };
        let Some(m) = team.members.iter().find(|m| m.name == member && !m.removed) else {
            return;
        };
        let nonempty = |s: &Option<String>| s.clone().filter(|s| !s.trim().is_empty());
        let instructions: Vec<String> = [nonempty(&team.lead_prompt), nonempty(&m.prompt)]
            .into_iter()
            .flatten()
            .collect();
        let project = nonempty(&team.project_path);
        // Headless runs never show Claude's trust prompt, so accept it for the project here.
        if let Some(p) = &project {
            let _ = trust::ensure_claude_trusted(Path::new(p));
        }
        let workdir = project
            .clone()
            .unwrap_or_else(|| self.workdir.trim().into());
        // Agents that run: everyone but the human (an AI lead counts).
        let me = self.actor();
        let agents = team
            .members
            .iter()
            .filter(|x| !x.removed && Some(&x.name) != me.as_ref())
            .count();
        let spec = AgentSpec {
            member: member.to_string(),
            role: m.role.clone(),
            workdir: PathBuf::from(workdir),
            prompt,
            mode,
            model: nonempty(&m.model).or_else(|| {
                // Settings > CLI Runtime default model for this runtime.
                let rt = m
                    .runtime
                    .as_deref()
                    .filter(|r| !r.is_empty())
                    .unwrap_or("claude");
                self.config
                    .extra
                    .get("runtimes")?
                    .get(rt)?
                    .get("model")?
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .map(String::from)
            }),
            instructions: (!instructions.is_empty()).then(|| instructions.join("\n")),
            runtime: nonempty(&m.runtime),
            worktree: m.worktree,
            project: project.clone().map(PathBuf::from),
            solo: agents == 1,
            env: {
                let mut env = apikeys::env_for_project(
                    &self.home,
                    m.runtime
                        .as_deref()
                        .filter(|r| !r.is_empty())
                        .unwrap_or("claude"),
                    project.as_deref().map(Path::new),
                );
                // Approval timeout for `Ask me` mode (read by the rteams MCP server).
                let ms = ToolApproval::from_extra(&self.config.extra).timeout_ms();
                env.push(("RTEAMS_PERMISSION_TIMEOUT_MS".into(), ms.to_string()));
                let rt = m
                    .runtime
                    .as_deref()
                    .filter(|r| !r.is_empty())
                    .unwrap_or("claude");
                if let Some(n) = crate::provider_cfg::max_output(
                    &AppConfig::load(&self.home).extra,
                    rt,
                    m.model.as_deref(),
                ) {
                    env.push(("CLAUDE_CODE_MAX_OUTPUT_TOKENS".into(), n.to_string()));
                }
                env
            },
            effort: self
                .launch_opts
                .as_ref()
                .map_or_else(|| m.effort.clone(), |o| o.effort.clone()),
            fast: self.launch_opts.as_ref().map_or(m.fast, |o| o.fast),
            context_limit: crate::provider_cfg::effective_context(
                &AppConfig::load(&self.home).extra,
                m.runtime
                    .as_deref()
                    .filter(|r| !r.is_empty())
                    .unwrap_or("claude"),
                m.model.as_deref(),
                self.launch_opts
                    .as_ref()
                    .map_or(m.context_limit, |o| o.context_limit),
            ),
            permissions: self.launch_opts.as_ref().map(|o| o.permissions.clone()),
            mcp_allow: m.mcp_allow.clone(),
            // This launch's flags, else the team's saved ones (protected flags re-checked).
            extra_args: self.launch_opts.as_ref().map_or_else(
                || {
                    crate::launcher::validate_extra_args(
                        &load_extra_args(&self.home, &self.team),
                        None,
                    )
                    .unwrap_or_default()
                },
                |o| o.extra_args.clone(),
            ),
            ..Default::default()
        };
        self.last = Some(
            self.supervisor
                .launch(spec)
                .map(|_| {
                    self.i18n
                        .t_args("rteams.toast.launched", &[("member", member)])
                })
                .map_err(|e| format!("{member}: {e}")),
        );
    }

    /// Members whose agent is running right now.
    fn running_members(&self) -> Vec<String> {
        self.supervisor
            .agents()
            .into_iter()
            .filter(|a| matches!(a.status, AgentStatus::Running { .. }))
            .map(|a| a.member)
            .collect()
    }

    /// Start the launches the dialog confirmed, with its options.
    fn launch_with_options(&mut self, o: LaunchOptions) {
        let running = self.running_members();
        save_gui_key(&self.home, &self.team, "extra_cli_args", &o.extra_raw);
        self.launch_opts = Some(o.clone());
        for m in o.members.iter().filter(|m| !running.contains(m)) {
            self.launch(m);
        }
        self.launch_opts = None;
    }

    /// Launch / stop requests agents recorded in the log (`team_launch` / `team_stop`).
    fn tick_launch_requests(&mut self) {
        let Some(d) = &self.data else { return };
        if d.seq() <= self.launch_cursor {
            return;
        }
        let (mut launch, mut stop) = (Vec::new(), Vec::new());
        for e in d.events().iter().filter(|e| e.seq > self.launch_cursor) {
            match &e.kind {
                rteams_core::EventKind::LaunchRequested { members } => {
                    launch.extend(members.iter().cloned())
                }
                rteams_core::EventKind::StopRequested { members } => {
                    stop.extend(members.iter().cloned())
                }
                _ => {}
            }
        }
        self.launch_cursor = d.seq();
        let all = self.agent_names();
        let running = self.running_members();
        for m in launch {
            if all.contains(&m) && !running.contains(&m) {
                self.launch(&m);
            }
        }
        for m in stop {
            recovery::cancel(&self.home, &self.team, &m);
            self.supervisor.stop(&m);
        }
    }

    /// Every roster member except the human.
    fn agent_names(&self) -> Vec<String> {
        let me = self.actor();
        self.data
            .as_ref()
            .and_then(|d| d.state().team.as_ref())
            .map(|t| {
                t.members
                    .iter()
                    .filter(|m| !m.removed && Some(&m.name) != me.as_ref())
                    .map(|m| m.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// OS notifications for what happened since the last frame (not for our own actions).
    fn tick_notifications(&mut self, me: Option<&str>) {
        let (Some(data), Some(me)) = (&self.data, me) else {
            return;
        };
        let mut notes = Vec::new();
        if data.seq() > self.notified {
            notes = notify::notifications(data.events(), self.notified, me);
            self.notified = data.seq();
        }
        notes.extend(notify::approval_notifications(
            data.state(),
            &mut self.approvals_seen,
            now_ms(),
        ));
        for n in notes {
            if self.center.push_note(&n) {
                notify::notify(&n.title, &n.body);
            }
        }
    }

    /// Feed new log events to the scheduler and launch whoever is due.
    fn tick_wakes(&mut self) {
        let Some(data) = &self.data else { return };
        let now = now_ms();
        let names = self.agent_names();
        self.wake.observe(data.events(), data.state(), &names, now);
        let running: Vec<String> = self
            .supervisor
            .agents()
            .into_iter()
            .filter(|a| matches!(a.status, AgentStatus::Running { .. }))
            .map(|a| a.member)
            .collect();
        let orders = self.wake.due(
            data.state(),
            &running,
            MAX_CONCURRENT,
            self.supervisor.running_count(),
            now,
        );
        for o in orders {
            self.launch_with(&o.member, o.prompt, mode_of(o.mode.as_deref()));
        }
    }

    fn apply(&mut self, intents: Intents, actor: Option<&str>) {
        for intent in intents.0 {
            match intent {
                Intent::Run(cmd) => {
                    let Some(actor) = actor else {
                        self.last = Some(Err(self.i18n.t("rteams.toast.noLead")));
                        continue;
                    };
                    if self.run(actor, &cmd) {
                        match &cmd {
                            GuiCmd::CreateTask { .. } => self.create_task.clear(),
                            // The wake scheduler sees the message and wakes its recipients.
                            GuiCmd::Message { .. } => self.messages.clear_composer(),
                            GuiCmd::AddMember { .. } => self.add_member.clear(),
                            // A new project path changes the repo the task dialog diffs.
                            GuiCmd::ConfigureTeam { .. } => self.sync_repo(),
                            _ => self.task_dialog.clear_inputs(),
                        }
                    }
                }
                Intent::CreateTask { cmd, start, prompt } => {
                    let Some(actor) = actor else {
                        self.last = Some(Err(self.i18n.t("rteams.toast.noLead")));
                        continue;
                    };
                    let owner = match &cmd {
                        GuiCmd::CreateTask { owner, .. } => owner.clone(),
                        _ => None,
                    };
                    let Some(v) = self.run_value(actor, &cmd) else {
                        continue;
                    };
                    self.create_task.clear();
                    let id = v["task"]["id"].as_str().unwrap_or_default().to_string();
                    if start && !id.is_empty() {
                        self.run(actor, &GuiCmd::Start { task: id.clone() });
                    }
                    // Instructions reach the owner as a direct message, which the wake
                    // scheduler folds into the owner's next wake prompt.
                    if let (Some(owner), Some(text)) = (owner, prompt) {
                        let text = format!("About task #{id}: {text}");
                        self.run(
                            actor,
                            &GuiCmd::Message {
                                to: Some(owner),
                                text,
                                mode: Some("do".into()),
                            },
                        );
                    }
                }
                Intent::OpenTask(id) => self.task_dialog.open(id),
                Intent::CloseTask => self.task_dialog.close(),
                Intent::NewTask { owner } => self.create_task.open(owner),
                // Clicking a chat row shows that conversation (full-screen chat), as in the
                // original.
                Intent::OpenChat(chat) => {
                    self.messages.open_chat(chat);
                    self.messages.set_full_screen(true);
                }
                Intent::Launch(m) => self.launch_dialog.open(&[m]),
                Intent::Stop(m) => {
                    recovery::cancel(&self.home, &self.team, &m);
                    self.supervisor.stop(&m);
                }
                Intent::Restart(m) => {
                    if let Err(e) = self.supervisor.restart(&m) {
                        self.last = Some(Err(e));
                    }
                }
                Intent::LaunchAll => {
                    let running = self.running_members();
                    let names: Vec<String> = self
                        .agent_names()
                        .into_iter()
                        .filter(|m| !running.contains(m))
                        .collect();
                    if !names.is_empty() {
                        self.launch_dialog.open(&names);
                    }
                }
                Intent::StopAll => {
                    for m in self.agent_names() {
                        self.supervisor.stop(&m);
                    }
                }
                Intent::SwitchTeam(team) => self.switch_team(team),
                Intent::OpenApprovals => self.approvals.open(),
                Intent::OpenProviders => self.tab = Tab::Providers,
                Intent::OpenNotifications => self.center.open(),
                Intent::CrossTeamSend { team, to, text } => {
                    let Some(actor) = actor else { continue };
                    let cmd = GuiCmd::CrossTeamMessage {
                        to,
                        text,
                        from_team: self.team.clone(),
                    };
                    let r = Commander::new(self.home.clone(), team.clone()).run(actor, &cmd);
                    self.last = Some(
                        r.map(|_| self.i18n.t_args("rteams.toast.sentTo", &[("team", &team)])),
                    );
                    if matches!(self.last, Some(Ok(_))) {
                        self.messages.clear_composer();
                    }
                }
                Intent::RevertFile { path, .. } => {
                    self.last = Some(match self.project() {
                        Some(repo) => git::revert_file(&repo, &path).map(|_| {
                            self.i18n
                                .t_args("rteams.toast.reverted", &[("path", &path)])
                        }),
                        None => Err(self.i18n.t("rteams.toast.noProjectPath")),
                    });
                }
                Intent::GitInit => {
                    self.git_op(git::init_repo, &self.i18n.t("rteams.toast.gitInitialized"))
                }
                Intent::GitInitialCommit => self.git_op(
                    git::initial_commit,
                    &self.i18n.t("rteams.toast.initialCommit"),
                ),
                Intent::AddMember => {
                    let taken = self.member_names();
                    self.add_member.open(taken.iter().map(String::as_str));
                }
                Intent::OpenMember(name) => self.member_dialog.open(name),
            }
        }
    }

    /// Second-phase screens: tick them, draw the terminal drawer, draw the active screen if it
    /// is one of theirs (returns true), and resolve what they asked the shell to do.
    fn phase2(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, actor: Option<&str>) -> bool {
        let mut out = Intents::default();
        let mut req = Vec::new();
        let agents = self.supervisor.agents();
        let view = self
            .data
            .as_ref()
            .filter(|d| d.state().team.is_some())
            .map(|d| View {
                state: d.state(),
                data: d,
                actor,
                agents: &agents,
                tr: &self.i18n,
            });
        let launched: HashSet<String> = (self.supervisor.running_count() > 0)
            .then(|| self.team.clone())
            .into_iter()
            .collect();
        self.dashboard.set_running(launched, HashSet::new());
        {
            let mut c = FeatureCtx {
                home: &self.home,
                team: Some(&self.team),
                view: view.as_ref(),
                out: &mut out,
                tr: &self.i18n,
                req: &mut req,
            };
            self.queue.dispatch_idle(&mut c, now_ms());
            self.dashboard.tick(ctx, &mut c);
            self.usage.tick(ctx, &mut c);
            self.editor.tick(ctx, &mut c);
            self.settings_screen.tick(ctx, &mut c);
            self.sessions.tick(ctx, &mut c);
            self.terminal.show(ui, &mut c);
        }
        let mine = self.tab.splittable();
        let split = self.split.filter(|t| t.splittable());
        let split_titles: Vec<(Tab, String)> = [Tab::Dashboard]
            .into_iter()
            .chain(self.open_tabs.iter().copied())
            .filter(|t| t.splittable())
            .map(|t| (t, self.tab_title(t, &self.team)))
            .collect();
        let split_title = split.map(|t| self.tab_title(t, &self.team));
        let mut split_pick: Option<Tab> = None;
        let mut split_close = false;
        let tab_now = self.tab;
        // Draws the screen of `tab` (one at a time per frame, so widget ids never clash).
        let mut draw = |ui: &mut egui::Ui, tab: Tab, c: &mut FeatureCtx| {
            if tab == Tab::from(self.dashboard.id()) {
                self.update.show(ui, c);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.news.show(ui, c)
                    });
                });
                self.tmux.show(ui, c);
                self.dashboard.show(ui, c);
            } else if tab == Tab::from(self.usage.id()) {
                self.usage.show(ui, c);
            } else if tab == Tab::from(self.editor.id()) {
                self.editor.show(ui, c);
            } else if tab == Tab::from(self.graph.id()) {
                self.graph.show(ui, c);
            } else if tab == Tab::from(self.ext.id()) {
                self.ext.show(ui, c);
            } else if tab == Tab::from(self.schedules.id()) {
                self.schedules.show(ui, c);
            } else if tab == Tab::from(self.org_map.id()) {
                self.org_map.show(ui, c);
            } else if tab == Tab::from(self.sessions.id()) {
                self.sessions.show(ui, c);
            } else if tab == Tab::from(self.settings_screen.id()) {
                self.settings_screen.show(ui, c);
            }
        };
        let screen_w = ui.available_width();
        if let Some(st) = split {
            egui::Panel::right("split_pane")
                .default_size((screen_w * 0.45).clamp(280.0, 900.0))
                .size_range(240.0..=(screen_w * 0.8).max(241.0))
                .frame(
                    egui::Frame::new()
                        .fill(theme::SURFACE)
                        .stroke(egui::Stroke::new(1.0, theme::hairline(0.1)))
                        .inner_margin(egui::Margin::same(12)),
                )
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let title = split_title.clone().unwrap_or_default();
                        let btn = theme::pill_icon_button(ui, Some(st.icon()), &title, true);
                        egui::Popup::menu(&btn).width(220.0).show(|ui| {
                            for (t, label) in &split_titles {
                                if theme::menu_item(ui, Some(t.icon()), label).clicked() {
                                    split_pick = Some(*t);
                                    ui.close();
                                }
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::icon_button(
                                ui,
                                Icon::X,
                                &self.i18n.t("rteams.shell.tab.closeSplit"),
                            )
                            .clicked()
                            {
                                split_close = true;
                            }
                        });
                    });
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical()
                        .id_salt("split_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let mut c = FeatureCtx {
                                home: &self.home,
                                team: Some(&self.team),
                                view: view.as_ref(),
                                out: &mut out,
                                tr: &self.i18n,
                                req: &mut req,
                            };
                            draw(ui, st, &mut c);
                        });
                });
        }
        if mine {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(theme::SURFACE)
                        .inner_margin(egui::Margin::same(16)),
                )
                .show(ui, |ui| {
                    let mut c = FeatureCtx {
                        home: &self.home,
                        team: Some(&self.team),
                        view: view.as_ref(),
                        out: &mut out,
                        tr: &self.i18n,
                        req: &mut req,
                    };
                    draw(ui, tab_now, &mut c);
                });
        }
        if let Some(t) = split_pick {
            self.move_to_pane(t);
        }
        if split_close {
            self.split = None;
        }
        if let Some(d) = self.dashboard.take_draft() {
            self.create_from_draft(d);
        }
        let outbox = self.schedules.take_outbox();
        for s in &outbox.save {
            out.push(Intent::Run(GuiCmd::set_schedule(s)));
        }
        for s in outbox.run_now {
            self.fire_schedule(&s.id, &s.member, s.prompt.clone(), now_ms());
        }
        self.tick_automation(&agents);
        if self.settings_screen.config_changed() {
            self.config = AppConfig::load(&self.home);
            self.i18n = I18n::from_pref(&self.config.general.locale);
            self.sync_http();
            // Settings > Permissions is the app-wide default for launches.
            let mode = ToolApproval::from_extra(&self.config.extra).default;
            if mode != self.perm {
                self.perm = mode;
                save_perm(&self.home, &self.team, &self.perm);
                self.supervisor.set_permissions(permissions_of(&self.perm));
            }
        }
        if self.onboarding.is_open() {
            let home = self.home.clone();
            self.onboarding.show(ctx, &self.i18n, &home);
        }
        if let Some(d) = self.onboarding.take_draft() {
            self.create_from_draft(d);
        }
        self.apply(out, actor);
        for r in req {
            self.handle_request(r, actor);
        }
        mine
    }

    /// Start or stop the local HTTP API to match Settings > Advanced.
    fn sync_http(&mut self) {
        let want = self.config.general.server_enabled;
        let port_changed = self
            .http
            .as_ref()
            .is_some_and(|h| h.port() != self.config.general.server_port);
        if !want || port_changed {
            if let Some(h) = self.http.take() {
                h.stop();
            }
        }
        if want && self.http.is_none() {
            if HttpServer::ensure_token(&mut self.config) {
                let _ = self.config.save(&self.home);
            }
            self.http = match HttpServer::start(&self.home, &self.config) {
                Ok(h) => {
                    self.last = Some(Ok(self
                        .i18n
                        .t_args("rteams.toast.httpOn", &[("url", &h.url())])));
                    Some(h)
                }
                Err(e) => {
                    self.last = Some(Err(self
                        .i18n
                        .t_args("rteams.toast.httpError", &[("error", &e.to_string())])));
                    None
                }
            };
        }
    }

    /// Launch a scheduled run and record it so the same fire never repeats.
    fn fire_schedule(&mut self, id: &str, member: &str, prompt: String, scheduled_for: i64) {
        let _ = history::append(
            &self.home,
            &self.team,
            &history::RunRecord::new(id, member, scheduled_for),
        );
        self.launch_with(member, prompt, Mode::Do);
    }

    /// Schedules and failure recovery every 15 s, work-sync every 60 s, and failure /
    /// success bookkeeping for runs that just ended.
    fn tick_automation(&mut self, agents: &[crate::launcher::AgentView]) {
        let now = now_ms();
        let running: HashSet<String> = agents
            .iter()
            .filter(|a| matches!(a.status, AgentStatus::Running { .. }))
            .map(|a| a.member.clone())
            .collect();
        for a in agents
            .iter()
            .filter(|a| self.was_running.contains(&a.member))
        {
            match &a.status {
                AgentStatus::Running { .. } => {}
                AgentStatus::Exited { code: Some(0) } => {
                    recovery::note_success(&self.home, &self.team, &a.member)
                }
                // Stopped by the user (no code): not a failure.
                AgentStatus::Exited { code: None } => {}
                status => {
                    let exit_code = match status {
                        AgentStatus::Exited { code } => *code,
                        _ => None,
                    };
                    let sig = recovery::FailureSignal {
                        member: a.member.clone(),
                        observed_at: now,
                        detail: a.last_error.clone().unwrap_or_default(),
                        status_code: None,
                        exit_code,
                        rate_limited_until: a.rate_limited_until,
                    };
                    recovery::observe_failure(&self.home, &self.team, &sig, &self.config.recovery);
                }
            }
        }
        self.was_running = running.clone();

        let due_automation = now - self.last_automation >= 15_000;
        let due_sync = now - self.last_work_sync >= 60_000;
        if !due_automation && !due_sync {
            return;
        }
        // Cloned only when something is due (not every frame).
        let Some(state) = self.data.as_ref().map(|d| d.state().clone()) else {
            return;
        };
        if due_automation {
            self.last_automation = now;
            let schedules = state
                .team
                .as_ref()
                .map(|t| t.schedules.clone())
                .unwrap_or_default();
            let ev = runner::evaluate(
                &schedules,
                &history::last_scheduled(&self.home, &self.team),
                now,
            );
            for f in ev.fires {
                // Over the cap: not recorded, so the same fire is retried next tick.
                if self.supervisor.running_count() < MAX_CONCURRENT {
                    self.fire_schedule(&f.schedule_id, &f.member, f.prompt, f.scheduled_for);
                }
            }
            for (id, at) in ev.interrupted {
                let member = schedules
                    .iter()
                    .find(|s| s.id == id)
                    .map(|s| s.member.clone())
                    .unwrap_or_default();
                let mut rec = history::RunRecord::new(&id, &member, at);
                rec.status = history::RunStatus::Interrupted;
                let _ = history::append(&self.home, &self.team, &rec);
            }
            for a in recovery::due_actions(&self.home, &self.team, now) {
                if !running.contains(&a.member) && self.auto_launch(&a.member, a.prompt, Mode::Do) {
                    recovery::mark_dispatched(&self.home, &self.team, &a.job_id);
                }
            }
        }
        if due_sync {
            self.last_work_sync = now;
            // Only agents already started in this session (by you or an event): opening the app
            // must never wake agents on its own.
            let started: HashSet<String> = agents.iter().map(|a| a.member.clone()).collect();
            let eligible: Vec<String> = self
                .agent_names()
                .into_iter()
                .filter(|m| started.contains(m))
                .collect();
            let running: Vec<String> = running.into_iter().collect();
            let nudges =
                WorkSync::new(&self.home, &self.team).nudges(&state, &eligible, &running, now);
            for n in nudges {
                let prompt = crate::wake::build_prompt(
                    &state,
                    &n.member,
                    std::slice::from_ref(&n.reason),
                    None,
                );
                self.auto_launch(&n.member, prompt, Mode::Do);
            }
        }
    }

    fn handle_request(&mut self, r: ShellRequest, actor: Option<&str>) {
        match r {
            ShellRequest::OpenTab(t) => self.open_tab(t.into()),
            ShellRequest::OpenTeam(team) => {
                self.switch_team(team);
                self.tab = Tab::Team;
            }
            ShellRequest::Toast(m) => self.last = Some(Ok(m)),
            ShellRequest::LaunchTeam(team) => {
                self.switch_team(team);
                self.tab = Tab::Team;
                self.apply(Intents(vec![Intent::LaunchAll]), actor);
            }
            ShellRequest::OpenEditor { root, file } => {
                self.editor.open_root(root, file);
                self.open_tab(Tab::Editor);
            }
            ShellRequest::OpenTerminal { cwd } => self.terminal.open(cwd),
        }
    }

    /// Create a team from a template or import: roster, lead prompt, project path, then each
    /// member's standing prompt and runtime.
    fn create_from_draft(&mut self, d: TeamDraft) {
        let lead = self
            .actor_override
            .clone()
            .unwrap_or_else(|| default_lead(None));
        self.switch_team(d.team_name.clone());
        let members = d
            .members
            .iter()
            .map(|m| (m.name.clone(), m.role.clone()))
            .collect();
        if !self.run(
            &lead,
            &GuiCmd::CreateTeam {
                name: d.team_name.clone(),
                members,
            },
        ) {
            return;
        }
        self.actor_override.get_or_insert(lead.clone());
        self.try_open();
        let lead_prompt = Some(d.lead_prompt.clone()).filter(|p| !p.trim().is_empty());
        if d.project_path.is_some() || lead_prompt.is_some() {
            self.run(
                &lead,
                &GuiCmd::ConfigureTeam {
                    project_path: d.project_path.clone(),
                    lead_prompt,
                },
            );
        }
        for m in &d.members {
            if !m.prompt.trim().is_empty() {
                self.run(
                    &lead,
                    &GuiCmd::UpdateMember {
                        name: m.name.clone(),
                        role: None,
                        prompt: Some(m.prompt.clone()),
                        model: None,
                        icon: None,
                    },
                );
            }
            if !m.runtime.is_empty() && m.runtime != "claude" {
                self.run(
                    &lead,
                    &GuiCmd::SetMemberRuntime {
                        name: m.name.clone(),
                        runtime: Some(m.runtime.clone()),
                        worktree: None,
                    },
                );
            }
        }
        self.sync_repo();
        self.dashboard.invalidate();
        self.teams.invalidate();
        self.tab = Tab::Team;
    }

    /// Keep the window open while the editor has unsaved work or terminals are running, and
    /// ask what to do (shared close guard).
    fn close_guard(&mut self, ctx: &egui::Context) {
        let mut shells = StaticBlockers::new(
            &self.i18n.t("common.terminal.title"),
            self.terminal.running_tabs(),
            &self.i18n.t("rteams.close.runningTerminals"),
        );
        let decision = {
            let mut parts: [&mut dyn CloseParticipant; 2] = [&mut self.editor, &mut shells];
            if ctx.input(|i| i.viewport().close_requested())
                && !self.allow_close
                && self.guard.request(&mut parts) == CloseDecision::Pending
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
            self.guard.show(ctx, &mut parts, &self.i18n)
        };
        if decision == CloseDecision::Quit && !self.allow_close {
            self.settings_screen.flush();
            self.quit(ctx);
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.allow_close = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Point every component at another team under the same home.
    fn switch_team(&mut self, team: String) {
        if team == self.team {
            self.tab = Tab::Team;
            return;
        }
        self.supervisor = Supervisor::new(launch_config(&self.home, &team));
        self.commander = Commander::new(self.home.clone(), team.clone());
        self.team = team;
        self.data = None;
        self.messages = Messages::default();
        self.filter = BoardFilter::default();
        self.logs = Logs::default();
        self.task_dialog = TaskDialog::default();
        self.create_task = CreateTaskDialog::default();
        self.member_dialog = MemberDialog::default();
        self.wake = WakeScheduler::default();
        self.settings = SettingsDialog::default();
        self.approvals = Approvals::default();
        self.center = NotificationCenter::default();
        self.tab = Tab::Team;
        self.try_open();
    }

    fn create_screen(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.set_max_width(520.0);
                let team = self.team.clone();
                if let Some(cmd) = self.create_team.show(ui, &self.i18n, &team) {
                    let lead = self.create_team.lead.trim().to_string();
                    if self.run(&lead, &cmd) {
                        self.actor_override.get_or_insert(lead);
                        self.try_open();
                    }
                }
                ui.add_space(8.0);
                if let Some(Err(e)) = &self.last {
                    ui.colored_label(theme::ERROR, e);
                }
                ui.add_space(16.0);
                if theme::ghost(
                    ui,
                    &format!("◀ {}", self.i18n.t("rteams.createTeam.backToTeams")),
                )
                .clicked()
                {
                    self.tab = Tab::Teams;
                }
            });
        });
    }

    /// Show `t` as the active tab, adding it to the closable tabs if it is an on-demand one.
    fn open_tab(&mut self, t: Tab) {
        if t.closable() && !self.open_tabs.contains(&t) {
            self.open_tabs.push(t);
        }
        if t == Tab::Teams {
            self.teams.invalidate();
        }
        // A screen can only be drawn once: selecting the split pane's tab takes it over.
        if self.split == Some(t) {
            self.split = None;
        }
        self.tab = t;
    }

    fn close_tab(&mut self, t: Tab) {
        self.open_tabs.retain(|x| *x != t);
        if self.tab == t {
            self.tab = Tab::Team;
        }
        if self.split == Some(t) {
            self.split = None;
        }
    }

    /// Show `t` in the split pane (opening it if needed); the main pane moves off it.
    fn move_to_pane(&mut self, t: Tab) {
        if !t.splittable() {
            return;
        }
        if t.closable() && !self.open_tabs.contains(&t) {
            self.open_tabs.push(t);
        }
        if self.tab == t {
            self.tab = Tab::Team;
        }
        self.split = Some(t);
    }

    fn apply_tab_act(&mut self, t: Tab, act: TabAct) {
        match act {
            TabAct::Close => self.close_tab(t),
            TabAct::CloseOthers => {
                let others: Vec<Tab> = self.open_tabs.iter().copied().filter(|x| *x != t).collect();
                for o in others {
                    self.close_tab(o);
                }
            }
            TabAct::CloseRight => {
                if let Some(i) = self.open_tabs.iter().position(|x| *x == t) {
                    let right: Vec<Tab> = self.open_tabs[i + 1..].to_vec();
                    for o in right {
                        self.close_tab(o);
                    }
                }
            }
            TabAct::MoveToPane => self.move_to_pane(t),
            TabAct::CloseSplit => self.split = None,
        }
    }

    /// Rebuild the palette's candidates: actions, tasks, members, messages, comments, logs.
    fn build_palette(&mut self) {
        use palette::{Group, Item};
        let tr = self.i18n.clone();
        let mut items: Vec<Item> = Vec::new();
        let mut acts: Vec<PAct> = Vec::new();
        let mut add =
            |g: Group, icon: Icon, label: String, detail: String, hay: String, a: PAct| {
                items.push(Item {
                    group: g,
                    icon,
                    label,
                    detail,
                    haystack: hay,
                });
                acts.push(a);
            };
        let open = tr.t("common.actions.open");
        for t in [
            Tab::Team,
            Tab::Dashboard,
            Tab::Teams,
            Tab::Usage,
            Tab::Graph,
            Tab::OrgMap,
            Tab::Sessions,
            Tab::Extensions,
            Tab::Schedules,
            Tab::Providers,
            Tab::Settings,
            Tab::Editor,
        ] {
            let title = self.tab_title(t, &self.team);
            add(
                Group::Action,
                t.icon(),
                format!("{open} {title}"),
                String::new(),
                String::new(),
                PAct::OpenTab(t),
            );
        }
        let act = |k: &str| tr.t(k);
        add(
            Group::Action,
            Icon::Plus,
            act("rteams.shell.act.newTask"),
            String::new(),
            String::new(),
            PAct::NewTask,
        );
        add(
            Group::Action,
            Icon::Users,
            act("rteams.shell.act.newTeam"),
            String::new(),
            String::new(),
            PAct::NewTeam,
        );
        add(
            Group::Action,
            Icon::Play,
            act("rteams.shell.act.launch"),
            String::new(),
            String::new(),
            PAct::LaunchAll,
        );
        add(
            Group::Action,
            Icon::Square,
            act("rteams.shell.act.stop"),
            String::new(),
            String::new(),
            PAct::StopAll,
        );
        add(
            Group::Action,
            Icon::List,
            act("rteams.shell.act.taskSidebar"),
            String::new(),
            String::new(),
            PAct::ToggleTaskSidebar,
        );
        add(
            Group::Action,
            Icon::Columns,
            act("rteams.shell.act.split"),
            String::new(),
            String::new(),
            PAct::ToggleSplit,
        );
        for c in crate::teams::scan(&self.home)
            .into_iter()
            .filter(|c| !c.deleted)
        {
            if c.dir != self.team {
                let name = c.name.clone().unwrap_or_else(|| c.dir.clone());
                add(
                    Group::Action,
                    Icon::Users,
                    format!("{} {name}", act("rteams.shell.act.switchTeam")),
                    String::new(),
                    String::new(),
                    PAct::SwitchTeam(c.dir),
                );
            }
        }
        if let Some(d) = &self.data {
            let st = d.state();
            let me = self.actor();
            for t in st.tasks.iter().filter(|t| !t.archived) {
                let (status, _) = crate::task_dialog::status_pill(&tr, t.status);
                let detail = match &t.owner {
                    Some(o) => format!("{status} \u{00b7} {o}"),
                    None => status,
                };
                add(
                    Group::Task,
                    Icon::Kanban,
                    format!("#{} {}", t.id, t.subject),
                    detail,
                    t.description.clone(),
                    PAct::Task(t.id.clone()),
                );
            }
            if let Some(team) = &st.team {
                for m in team.members.iter().filter(|m| !m.removed) {
                    add(
                        Group::Member,
                        Icon::Users,
                        m.name.clone(),
                        m.role.clone(),
                        String::new(),
                        PAct::Member(m.name.clone()),
                    );
                }
            }
            let skip = st.messages.len().saturating_sub(600);
            for m in st.messages.iter().skip(skip).rev() {
                match &m.scope {
                    rteams_core::Scope::Task { task } => add(
                        Group::Comment,
                        Icon::MessageSquare,
                        format!("#{task} \u{00b7} {}", m.from),
                        m.text.clone(),
                        m.text.clone(),
                        PAct::Task(task.clone()),
                    ),
                    rteams_core::Scope::Direct { to } => {
                        let other = if Some(m.from.as_str()) == me.as_deref() {
                            to.clone()
                        } else {
                            m.from.clone()
                        };
                        add(
                            Group::Message,
                            Icon::MessageSquare,
                            format!("{} > {to}", m.from),
                            m.text.clone(),
                            m.text.clone(),
                            PAct::Chat(Some(other)),
                        )
                    }
                    _ => add(
                        Group::Message,
                        Icon::MessageSquare,
                        m.from.clone(),
                        m.text.clone(),
                        m.text.clone(),
                        PAct::Chat(None),
                    ),
                }
            }
        }
        for a in self.supervisor.agents() {
            for l in a.last_lines.iter().rev().take(200) {
                add(
                    Group::Log,
                    Icon::Terminal,
                    a.member.clone(),
                    l.clone(),
                    l.clone(),
                    PAct::Logs,
                );
            }
        }
        self.pal_items = items;
        self.pal_acts = acts;
        self.pal_built = Some(std::time::Instant::now());
    }

    fn run_palette_act(&mut self, a: PAct, actor: Option<&str>) {
        match a {
            PAct::OpenTab(t) => self.open_tab(t),
            PAct::NewTask => {
                self.tab = Tab::Team;
                self.create_task.open(None);
            }
            PAct::NewTeam => self.open_tab(Tab::Teams),
            PAct::LaunchAll => self.apply(Intents(vec![Intent::LaunchAll]), actor),
            PAct::StopAll => self.apply(Intents(vec![Intent::StopAll]), actor),
            PAct::ToggleTaskSidebar => self.task_sidebar.toggle(),
            PAct::ToggleSplit => {
                if self.split.is_some() {
                    self.split = None;
                } else {
                    let pick = self
                        .open_tabs
                        .iter()
                        .copied()
                        .find(|t| t.splittable() && *t != self.tab)
                        .unwrap_or(Tab::Usage);
                    self.move_to_pane(pick);
                }
            }
            PAct::SwitchTeam(d) => self.switch_team(d),
            PAct::Task(id) => {
                self.tab = Tab::Team;
                self.task_dialog.open(id);
            }
            PAct::Member(n) => {
                self.tab = Tab::Team;
                self.member_dialog.open(n);
            }
            PAct::Chat(c) => {
                self.tab = Tab::Team;
                self.apply(Intents(vec![Intent::OpenChat(c)]), actor);
            }
            PAct::Logs => {
                self.tab = Tab::Team;
                self.show_logs = true;
            }
        }
    }

    fn tab_title(&self, t: Tab, team_title: &str) -> String {
        let tr = |k: &str| self.i18n.t(k);
        match t {
            // The catalog has no plain "Dashboard" string (its title key reads "Select Team").
            Tab::Dashboard => "Dashboard".to_string(),
            Tab::Team => team_title.to_string(),
            Tab::Teams => tr("common.layout.menu.teams"),
            Tab::Usage => tr(self.usage.title_key()),
            Tab::Editor => tr(self.editor.title_key()),
            Tab::Graph => tr(self.graph.title_key()),
            Tab::Extensions => tr(self.ext.title_key()),
            Tab::Schedules => tr(self.schedules.title_key()),
            Tab::OrgMap => tr(self.org_map.title_key()),
            Tab::Sessions => tr(self.sessions.title_key()),
            Tab::Settings => tr(self.settings_screen.title_key()),
            Tab::Providers => tr("settings.runtimeProvider.tabs.providers"),
        }
    }

    /// Transient messages (last action, error, queued wakes) as a card in the bottom right
    /// instead of text crammed into the tab bar. The last-action message fades after 6 s.
    fn toasts(&mut self, ctx: &egui::Context) {
        const TTL: Duration = Duration::from_secs(6);
        let last = self.last.as_ref().map(|r| match r {
            Ok(m) => m.clone(),
            Err(e) => e.clone(),
        });
        match (&last, &self.toast_seen) {
            (Some(m), Some((seen, _))) if m == seen => {}
            (Some(m), _) => self.toast_seen = Some((m.clone(), std::time::Instant::now())),
            (None, _) => self.toast_seen = None,
        }
        if self
            .toast_seen
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() > TTL)
        {
            self.last = None;
            self.toast_seen = None;
        }
        let mut lines: Vec<(String, egui::Color32)> = Vec::new();
        if let Some(e) = &self.error {
            lines.push((e.clone(), theme::ERROR));
        }
        let queued: Vec<String> = self.wake.pending().into_iter().map(|(m, _)| m).collect();
        if !queued.is_empty() {
            lines.push((
                self.i18n
                    .t_args("rteams.status.queued", &[("members", &queued.join(", "))]),
                theme::WARNING,
            ));
        }
        match &self.last {
            Some(Ok(m)) => lines.push((m.clone(), theme::SUCCESS)),
            Some(Err(e)) => lines.push((e.clone(), theme::ERROR)),
            None => {}
        }
        if lines.is_empty() {
            return;
        }
        let mut dismiss = false;
        egui::Area::new(egui::Id::unique("toasts"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -16.0])
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(theme::RAISED)
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_max_width(420.0);
                        for (text, color) in &lines {
                            ui.horizontal_top(|ui| {
                                ui.label(RichText::new(text).color(*color).size(13.0));
                            });
                        }
                        if self.last.is_some()
                            && ui
                                .add(
                                    egui::Label::new(
                                        RichText::new(self.i18n.t("common.actions.close"))
                                            .size(12.0)
                                            .color(theme::MUTED),
                                    )
                                    .sense(egui::Sense::click()),
                                )
                                .clicked()
                        {
                            dismiss = true;
                        }
                    });
            });
        if dismiss {
            self.last = None;
            self.toast_seen = None;
        }
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    /// Draws the 44px tab strip: team pill, Dashboard, Teams, the on-demand tabs the user
    /// opened, and icon buttons at the right (new team, notifications, overflow menu, terminal).
    fn tab_bar(&mut self, ui: &mut egui::Ui, title: &str, actor: Option<&str>) {
        let mut select: Option<Tab> = None;
        let mut close: Option<Tab> = None;
        let mut open: Option<Tab> = None;
        let mut switch: Option<String> = None;
        let mut terminal = false;
        let mut new_team = false;
        let mut bell = false;
        let mut tab_act: Option<(Tab, TabAct)> = None;
        let mut drop_to: Option<(Tab, usize)> = None;
        let mut toggle_sidebar = false;
        let mut toggle_split = false;
        let mut open_palette = false;
        let sidebar_open = self.task_sidebar.is_open();
        let split_open = self.split.is_some();
        let mut lang = self.i18n.clone();
        let mut locale_changed = false;
        let unread = self.center.unread();
        let term_open = self.terminal.is_open();
        let tr = self.i18n.clone();
        let titles: Vec<(Tab, String)> = [Tab::Dashboard, Tab::Teams]
            .into_iter()
            .chain(self.open_tabs.iter().copied())
            .map(|t| (t, self.tab_title(t, title)))
            .collect();
        let overflow: Vec<(Tab, String)> = [
            Tab::Usage,
            Tab::Graph,
            Tab::OrgMap,
            Tab::Sessions,
            Tab::Extensions,
            Tab::Schedules,
            Tab::Providers,
            Tab::Settings,
            Tab::Editor,
        ]
        .into_iter()
        .map(|t| (t, self.tab_title(t, title)))
        .collect();
        let current = self.tab;
        let team_dir = self.team.clone();
        let hint = match actor {
            Some(a) => tr.t_args("rteams.status.actingAs", &[("name", a)]),
            None => tr.t("rteams.status.noLead"),
        };
        let home = self.home.clone();
        egui::Panel::top("tabs")
            .exact_size(44.0)
            .frame(
                egui::Frame::new()
                    .fill(theme::SIDEBAR)
                    .inner_margin(egui::Margin::symmetric(8, 0)),
            )
            .show(ui, |ui| {
                let rect = ui.available_rect_before_wrap();
                ui.painter().hline(
                    rect.x_range(),
                    rect.bottom() - 0.5,
                    egui::Stroke::new(1.0, theme::hairline(0.07)),
                );
                let right_w = 232.0;
                let left_rect = egui::Rect::from_min_max(
                    rect.min,
                    egui::pos2((rect.right() - right_w).max(rect.left()), rect.bottom()),
                );
                let right_rect = egui::Rect::from_min_max(
                    egui::pos2((rect.right() - right_w).max(rect.left()), rect.top()),
                    rect.max,
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(left_rect), |ui| {
                    // A plain mouse wheel only scrolls vertically; turn it sideways over the
                    // strip so tabs past the right edge stay reachable.
                    let wheel = ui.input(|i| i.smooth_scroll_delta);
                    let over_strip = ui.rect_contains_pointer(left_rect);
                    egui::ScrollArea::horizontal()
                        .id_salt("tab-scroll")
                        .scroll_bar_visibility(
                            egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded,
                        )
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if over_strip && wheel.x == 0.0 && wheel.y != 0.0 {
                                ui.scroll_with_delta(egui::vec2(wheel.y, 0.0));
                            }
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                // Team pill with the team switcher menu.
                                let c = chip(
                                    ui,
                                    current == Tab::Team,
                                    Tab::Team.icon(),
                                    title,
                                    190.0,
                                    false,
                                    true,
                                );
                                c.main.clone().on_hover_text(&hint);
                                if c.main.clicked() {
                                    select = Some(Tab::Team);
                                }
                                if let Some(chev) = &c.chevron {
                                    if chev.clicked() {
                                        self.team_menu = crate::teams::scan(&home)
                                            .into_iter()
                                            .filter(|t| !t.deleted)
                                            .collect();
                                    }
                                    egui::Popup::menu(chev).width(240.0).show(|ui| {
                                        ui.set_min_width(220.0);
                                        for t in &self.team_menu {
                                            let name =
                                                t.name.clone().unwrap_or_else(|| t.dir.clone());
                                            let label = if t.dir == team_dir {
                                                format!("{name}  \u{2022}")
                                            } else {
                                                name
                                            };
                                            if theme::menu_item(ui, Some(Icon::Users), &label)
                                                .clicked()
                                            {
                                                switch = Some(t.dir.clone());
                                                ui.close();
                                            }
                                        }
                                        theme::rule(ui);
                                        if theme::menu_item(
                                            ui,
                                            Some(Icon::Plus),
                                            &tr.t("common.list.actions.createTeam"),
                                        )
                                        .clicked()
                                        {
                                            new_team = true;
                                            ui.close();
                                        }
                                    });
                                }
                                let mut closable_rects: Vec<(Tab, egui::Rect)> = Vec::new();
                                let mut dragging: Option<Tab> = None;
                                for (t, label) in &titles {
                                    let c = chip(
                                        ui,
                                        current == *t,
                                        t.icon(),
                                        label,
                                        0.0,
                                        t.closable(),
                                        false,
                                    );
                                    if c.main.clicked() {
                                        select = Some(*t);
                                    }
                                    if current == *t && self.tab_scrolled != Some(current) {
                                        c.main.scroll_to_me(None);
                                    }
                                    if c.main.middle_clicked() && t.closable() {
                                        close = Some(*t);
                                    }
                                    if c.close.as_ref().is_some_and(|x| x.clicked()) {
                                        close = Some(*t);
                                    }
                                    if t.closable() {
                                        closable_rects.push((*t, c.main.rect));
                                        if c.main.dragged() {
                                            dragging = Some(*t);
                                        }
                                        if c.main.drag_stopped() {
                                            if let Some(p) = c.main.interact_pointer_pos() {
                                                let slot =
                                                    closable_rects_slot(&closable_rects, p.x);
                                                drop_to = Some((*t, slot));
                                            }
                                        }
                                    }
                                    c.main.context_menu(|ui| {
                                        ui.set_min_width(200.0);
                                        let tk = |k: &str| tr.t(k);
                                        if t.closable()
                                            && theme::menu_item(
                                                ui,
                                                Some(Icon::X),
                                                &tk("common.actions.close"),
                                            )
                                            .clicked()
                                        {
                                            tab_act = Some((*t, TabAct::Close));
                                            ui.close();
                                        }
                                        if t.closable()
                                            && theme::menu_item(
                                                ui,
                                                None,
                                                &tk("rteams.shell.tab.closeOthers"),
                                            )
                                            .clicked()
                                        {
                                            tab_act = Some((*t, TabAct::CloseOthers));
                                            ui.close();
                                        }
                                        if t.closable()
                                            && theme::menu_item(
                                                ui,
                                                None,
                                                &tk("rteams.shell.tab.closeRight"),
                                            )
                                            .clicked()
                                        {
                                            tab_act = Some((*t, TabAct::CloseRight));
                                            ui.close();
                                        }
                                        if t.splittable()
                                            && theme::menu_item(
                                                ui,
                                                Some(Icon::Columns),
                                                &tk("rteams.shell.tab.moveToPane"),
                                            )
                                            .clicked()
                                        {
                                            tab_act = Some((*t, TabAct::MoveToPane));
                                            ui.close();
                                        }
                                        if split_open
                                            && theme::menu_item(
                                                ui,
                                                None,
                                                &tk("rteams.shell.tab.closeSplit"),
                                            )
                                            .clicked()
                                        {
                                            tab_act = Some((*t, TabAct::CloseSplit));
                                            ui.close();
                                        }
                                    });
                                }
                                if let (Some(d), Some(p)) =
                                    (dragging, ui.input(|i| i.pointer.interact_pos()))
                                {
                                    let slot = closable_rects_slot(&closable_rects, p.x);
                                    if let Some((_, r)) = closable_rects
                                        .get(slot.min(closable_rects.len().saturating_sub(1)))
                                    {
                                        let x = if slot >= closable_rects.len() {
                                            r.right() + 2.0
                                        } else {
                                            r.left() - 2.0
                                        };
                                        ui.painter().vline(
                                            x,
                                            r.y_range(),
                                            egui::Stroke::new(2.0, theme::ACCENT),
                                        );
                                    }
                                    let _ = d;
                                }
                            });
                        });
                });
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(right_rect)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let term_tip = format!("{} (Ctrl+`)", tr.t("common.terminal.title"));
                        if theme::icon_button_active(ui, Icon::Terminal, &term_tip, term_open)
                            .clicked()
                        {
                            terminal = true;
                        }
                        let more = theme::icon_button(
                            ui,
                            Icon::MoreHorizontal,
                            &tr.t("common.actions.moreActions"),
                        );
                        egui::Popup::menu(&more).width(240.0).show(|ui| {
                            ui.set_min_width(220.0);
                            for (t, label) in &overflow {
                                if theme::menu_item(ui, Some(t.icon()), label).clicked() {
                                    open = Some(*t);
                                    ui.close();
                                }
                            }
                            theme::rule(ui);
                            let label = tr.t("settings.general.appLanguage.label");
                            let chevron_label = format!("{label}  \u{00b7}  {}", lang.locale);
                            if theme::menu_item(ui, Some(Icon::Globe), &chevron_label).clicked() {
                                self.lang_open = !self.lang_open;
                            }
                            if self.lang_open && crate::ui::i18n_widgets::locale_list(ui, &mut lang)
                            {
                                locale_changed = true;
                                ui.close();
                            }
                        });
                        let bell_resp =
                            theme::icon_button(ui, Icon::Bell, &tr.t("common.notifications.title"));
                        if unread > 0 {
                            badge(
                                ui,
                                bell_resp.rect.right_top() + egui::vec2(-4.0, 6.0),
                                unread,
                            );
                        }
                        if bell_resp.clicked() {
                            bell = true;
                        }
                        if theme::icon_button(
                            ui,
                            Icon::Plus,
                            &tr.t("common.list.actions.createTeam"),
                        )
                        .clicked()
                        {
                            new_team = true;
                        }
                        if theme::icon_button_active(
                            ui,
                            Icon::Columns,
                            &tr.t("rteams.shell.act.split"),
                            split_open,
                        )
                        .clicked()
                        {
                            toggle_split = true;
                        }
                        if theme::icon_button_active(
                            ui,
                            Icon::List,
                            &tr.t("rteams.shell.act.taskSidebar"),
                            sidebar_open,
                        )
                        .clicked()
                        {
                            toggle_sidebar = true;
                        }
                        if theme::icon_button(
                            ui,
                            Icon::Search,
                            &format!("{} (Ctrl+K)", tr.t("rteams.shell.palette.title")),
                        )
                        .clicked()
                        {
                            open_palette = true;
                        }
                    },
                );
            });
        self.tab_scrolled = Some(current);
        if terminal {
            self.terminal.toggle();
        }
        if open_palette {
            self.palette.open(false);
            self.pal_built = None;
        }
        if toggle_sidebar {
            self.task_sidebar.toggle();
        }
        if toggle_split {
            self.run_palette_act(PAct::ToggleSplit, actor);
        }
        if let Some((t, slot)) = drop_to {
            move_to_slot(&mut self.open_tabs, t, slot);
        }
        if let Some((t, act)) = tab_act {
            self.apply_tab_act(t, act);
        }
        if bell {
            self.center.open();
        }
        if new_team {
            self.open_tab(Tab::Teams);
        }
        if let Some(dir) = switch {
            self.switch_team(dir);
        }
        if let Some(t) = open {
            self.open_tab(t);
        }
        if let Some(t) = close {
            self.close_tab(t);
        }
        if let Some(t) = select {
            self.open_tab(t);
        }
        if locale_changed {
            let mut cfg = AppConfig::load(&self.home);
            cfg.general.locale = lang.locale.clone();
            let _ = cfg.save(&self.home);
            self.i18n = lang;
        }
    }
}

impl App {
    /// Every member name, removed ones included (their names stay reserved).
    fn member_names(&self) -> Vec<String> {
        self.data
            .as_ref()
            .and_then(|d| d.state().team.as_ref())
            .map(|t| t.members.iter().map(|m| m.name.clone()).collect())
            .unwrap_or_default()
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Poll the log twice a second while focused; in the background every 2 s is enough
        // (input and agent output still repaint immediately).
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        ctx.request_repaint_after(if focused { POLL } else { POLL * 4 });
        self.supervisor.poll();
        self.close_guard(&ctx);
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Backtick)) {
            self.terminal.toggle();
        }

        let (k, shift) = ctx.input(|i| {
            (
                i.modifiers.command && i.key_pressed(egui::Key::K),
                i.modifiers.shift,
            )
        });
        if k {
            self.palette.open(shift);
            self.pal_built = None;
        }

        if self.data.is_none() {
            self.try_open();
        }
        if let Some(data) = &mut self.data {
            self.error = data.refresh().err();
            // Picked member icons, read by every avatar via `names::icon_for`.
            let icons = data
                .state()
                .team
                .iter()
                .flat_map(|t| &t.members)
                .filter_map(|m| Some((m.name.clone(), m.icon.clone()?)))
                .collect();
            crate::names::publish(&ctx, icons);
        }
        let title = self
            .data
            .as_ref()
            .and_then(|d| d.team_name())
            .unwrap_or_else(|| self.team.clone());
        let wanted = window_title(&title);
        if wanted != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(wanted.clone()));
            self.window_title = wanted;
        }
        let actor = self.actor();
        if self.tab.closable() && !self.open_tabs.contains(&self.tab) {
            self.open_tabs.push(self.tab);
        }
        self.tick_launch_requests();
        self.tab_bar(ui, &title, actor.as_deref());
        self.toasts(&ctx);
        if let Some(p) = self
            .task_sidebar
            .show(ui, &self.home, &self.team, &self.i18n)
        {
            self.switch_team(p.team);
            self.task_dialog.open(p.task);
        }
        if self.palette.is_open() {
            if self
                .pal_built
                .is_none_or(|t| t.elapsed() > Duration::from_secs(2))
            {
                self.build_palette();
            }
            if let Some(i) = self.palette.show(&ctx, &self.i18n, &self.pal_items) {
                if let Some(a) = self.pal_acts.get(i).cloned() {
                    self.run_palette_act(a, actor.as_deref());
                }
            }
        }
        self.center.show(&ctx, &self.i18n, &mut Intents::default());
        if self.phase2(ui, &ctx, actor.as_deref()) {
            return;
        }

        if self.tab == Tab::Providers {
            let agents = self.supervisor.agents();
            let mut recovery_changed = false;
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(theme::SURFACE)
                        .inner_margin(egui::Margin::same(20)),
                )
                .show(ui, |ui| match &self.data {
                    Some(data) => {
                        let v = View {
                            state: data.state(),
                            data,
                            actor: actor.as_deref(),
                            agents: &agents,
                            tr: &self.i18n,
                        };
                        self.providers.show(ui, &v);
                        ui.add_space(16.0);
                        theme::section(
                            ui,
                            "recovery",
                            "🔁",
                            &self.i18n.t("rteams.sections.failureRecovery"),
                            None,
                            false,
                            |_| {},
                            |ui| {
                                if crate::ui::recovery_settings::show_tr(
                                    ui,
                                    &mut self.config,
                                    &self.i18n,
                                ) {
                                    recovery_changed = true;
                                }
                            },
                        );
                    }
                    None => {
                        ui.label(RichText::new(self.i18n.t("rteams.noTeam")).color(theme::MUTED));
                    }
                });
            if let Some(cmd) = self.providers.take_login() {
                // The terminal has no write-a-line API: open it in the project and put the
                // sign-in command on the clipboard to paste.
                let cwd = self
                    .project()
                    .unwrap_or_else(|| PathBuf::from(self.workdir.trim()));
                self.terminal.open(cwd);
                ctx.copy_text(cmd.clone());
                self.last = Some(Ok(format!("{cmd}  (copied: paste it in the terminal)")));
            }
            if recovery_changed {
                let mut cfg = AppConfig::load(&self.home);
                cfg.recovery = self.config.recovery.clone();
                let _ = cfg.save(&self.home);
            }
            return;
        }

        if self.tab == Tab::Teams {
            let mut intents = Intents::default();
            let org_reqs: Vec<ShellRequest> = Vec::new();
            let (home, team) = (self.home.clone(), self.team.clone());
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(theme::SURFACE)
                        .inner_margin(egui::Margin::same(20)),
                )
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("teams_tab")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            self.teams.show(ui, &self.i18n, &home, &team, &mut intents);
                        });
                });
            if let Some(crate::teams::TeamsAction::OpenOrgMap) = self.teams.take_action() {
                self.open_tab(Tab::OrgMap);
            }
            if let Some(d) = self.teams.take_draft() {
                self.create_from_draft(d);
            }
            if let (Some((dir, name)), Some(a)) = (self.teams.take_rename_request(), &actor) {
                let r = Commander::new(home.clone(), dir.clone())
                    .run(a, &GuiCmd::RenameTeam { name: name.clone() });
                self.last = Some(r.map(|_| {
                    self.i18n
                        .t_args("rteams.toast.renamedTeam", &[("dir", &name)])
                }));
                self.teams.invalidate();
            }
            if let Some(dir) = self.teams.take_purge_request() {
                let r = rteams_store::FileStore::delete_team_permanently(&home, &dir)
                    .map_err(|e| e.to_string());
                self.last = Some(r.map(|_| format!("Deleted {dir} forever")));
                self.teams.invalidate();
            }
            if let (Some((dir, deleted)), Some(a)) = (self.teams.take_delete_request(), &actor) {
                let r = Commander::new(home.clone(), dir.clone())
                    .run(a, &GuiCmd::DeleteTeam { deleted });
                self.last = Some(r.map(|_| {
                    self.i18n.t_args(
                        if deleted {
                            "rteams.toast.deletedTeam"
                        } else {
                            "rteams.toast.restoredTeam"
                        },
                        &[("dir", &dir)],
                    )
                }));
                self.teams.invalidate();
            }
            self.apply(intents, actor.as_deref());
            for r in org_reqs {
                self.handle_request(r, actor.as_deref());
            }
            return;
        }
        let has_team = self.data.as_ref().is_some_and(|d| d.state().team.is_some());
        if !has_team {
            self.create_screen(ui);
            return;
        }

        self.tick_wakes();
        self.tick_notifications(actor.as_deref());
        let agents = self.supervisor.agents();
        let mut intents = Intents::default();
        let mut open_settings = false;
        let mut team_reqs: Vec<ShellRequest> = Vec::new();
        let data = self.data.as_ref().expect("checked above");
        let v = View {
            state: data.state(),
            data,
            actor: actor.as_deref(),
            agents: &agents,
            tr: &self.i18n,
        };

        let width = ui.available_width();
        egui::Panel::left("messages")
            .default_size((width * 0.25).clamp(280.0, 420.0))
            .size_range(240.0..=520.0)
            .frame(
                egui::Frame::new()
                    .fill(theme::SIDEBAR)
                    .inner_margin(egui::Margin::symmetric(12, 10)),
            )
            .show(ui, |ui| self.messages.sidebar(ui, &v, &mut intents));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::symmetric(18, 12)),
            )
            .show(ui, |ui| {
                if self.messages.full_screen() {
                    self.messages.chat(ui, &v, &mut intents);
                    return;
                }
                let viewport_h = ui.available_height();
                egui::ScrollArea::vertical()
                    .id_salt("main")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let content_top = ui.cursor().top();
                        let t = |k: &str| self.i18n.t(k);
                        ui.horizontal(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if theme::pill_icon_button(
                                        ui,
                                        Some(Icon::Network),
                                        &t("team.detail.actions.visualize"),
                                        true,
                                    )
                                    .clicked()
                                    {
                                        team_reqs.push(ShellRequest::OpenTab(TabId::Graph));
                                    }
                                    if theme::pill_icon_button(
                                        ui,
                                        Some(Icon::ChartBar),
                                        &t("team.detail.actions.usage"),
                                        false,
                                    )
                                    .clicked()
                                    {
                                        team_reqs.push(ShellRequest::OpenTab(TabId::Usage));
                                    }
                                    if let Some(root) = team_project(v.state).or_else(|| {
                                        Some(PathBuf::from(&self.workdir)).filter(|p| p.is_dir())
                                    }) {
                                        if theme::pill_icon_button(
                                            ui,
                                            Some(Icon::Code),
                                            &t("team.detail.actions.editCode"),
                                            false,
                                        )
                                        .clicked()
                                        {
                                            team_reqs.push(ShellRequest::OpenEditor {
                                                root,
                                                file: None,
                                            });
                                        }
                                    }
                                },
                            );
                        });
                        let unread = self.center.unread();
                        if team::header(ui, &v, &mut self.workdir, unread, &mut intents) {
                            open_settings = true;
                        }
                        ui.add_space(6.0);
                        // A big roster would push the board below the fold: start collapsed.
                        let roster = v
                            .state
                            .team
                            .as_ref()
                            .map_or(0, |t| t.members.iter().filter(|m| !m.removed).count());
                        if roster > 3 && self.collapse_seeded.insert(self.team.clone()) {
                            let id = ui.make_persistent_id(("section", "team"));
                            egui::collapsing_header::CollapsingState::load_with_default_open(
                                ui.ctx(),
                                id,
                                false,
                            )
                            .store(ui.ctx());
                        }
                        team::members(ui, &v, &mut intents);
                        theme::section(
                            ui,
                            "folders",
                            "📁",
                            &self.i18n.t("common.taskFolders.title"),
                            None,
                            false,
                            |_| {},
                            |ui| {
                                let mut c = FeatureCtx {
                                    home: &self.home,
                                    team: Some(&self.team),
                                    view: Some(&v),
                                    out: &mut intents,
                                    tr: &self.i18n,
                                    req: &mut team_reqs,
                                };
                                self.folders.show(ui, &mut c);
                            },
                        );
                        self.filter.folder = self.folders.folder_filter();
                        let repo = team_project(v.state);
                        self.filter
                            .changes
                            .set_paths(&self.home, &self.team, repo.as_deref());
                        let open_tasks = v.state.tasks.iter().filter(|t| !t.archived).count();
                        theme::section(
                            ui,
                            "kanban",
                            "📊",
                            &self.i18n.t("common.layout.sections.kanban"),
                            Some(open_tasks),
                            true,
                            |_| {},
                            |ui| {
                                let w = ui.available_width().max(0.0);
                                // The board fills what is left of the viewport.
                                let used = ui.cursor().top() - content_top;
                                let board_height = (viewport_h - used - 8.0).max(460.0);
                                ui.allocate_ui(egui::vec2(w, board_height), |ui| {
                                    board::show(ui, &v, &mut self.filter, &mut intents);
                                });
                            },
                        );
                        team::processes(ui, &v, &mut intents);
                        self.logs.show(ui, &v);
                        let mut c = FeatureCtx {
                            home: &self.home,
                            team: Some(&self.team),
                            view: Some(&v),
                            out: &mut intents,
                            tr: &self.i18n,
                            req: &mut team_reqs,
                        };
                        theme::section(
                            ui,
                            "activity",
                            "📜",
                            &self.i18n.t("rteams.shell.activity.title"),
                            None,
                            true,
                            |_| {},
                            |ui| activity::show(ui, &v),
                        );
                        theme::section(
                            ui,
                            "queue",
                            "📋",
                            &self.i18n.t("rteams.sections.messageQueue"),
                            None,
                            false,
                            |_| {},
                            |ui| self.queue.show(ui, &mut c),
                        );
                        theme::section(
                            ui,
                            "worksync",
                            "🔁",
                            &self.i18n.t("rteams.sections.workSync"),
                            None,
                            false,
                            |_| {},
                            |ui| self.work_sync.show(ui, &mut c),
                        );
                        theme::section(
                            ui,
                            "livelog",
                            "📜",
                            &self.i18n.t("rteams.sections.liveLog"),
                            None,
                            false,
                            |_| {},
                            |ui| self.log_stream.show(ui, &mut c),
                        );
                    });
            });

        if self.messages.take_open_logs() {
            self.show_logs = true;
        }
        let logs_title = self.i18n.t("team.claudeLogs.logsTitle");
        theme::dialog(
            &ctx,
            "logs_window",
            &logs_title,
            960.0,
            &mut self.show_logs,
            |ui| self.logs.show(ui, &v),
        );
        self.task_dialog.show(&ctx, &v, &mut intents);
        self.create_task.show(&ctx, &v, &mut intents);
        self.add_member.show(&ctx, &self.i18n, &mut intents);
        self.member_dialog.show(&ctx, &v, &mut intents);
        if open_settings {
            self.branch = team_project(v.state)
                .as_deref()
                .and_then(git::current_branch);
            self.settings.open(&v, &self.perm);
        }
        self.settings.show(
            &ctx,
            &v,
            self.branch.as_deref(),
            &mut self.center,
            &mut intents,
        );
        self.approvals.show(&ctx, &v, &mut intents);
        let launched = self.launch_dialog.show(&ctx, &v, &mut intents);
        if let Some(p) = self.settings.take_permissions_choice() {
            self.perm = normalize_perm(&p).to_string();
            save_perm(&self.home, &self.team, &self.perm);
            self.supervisor.set_permissions(permissions_of(&self.perm));
        }

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && self.task_dialog.is_open() {
            intents.push(Intent::CloseTask);
        }
        self.apply(intents, actor.as_deref());
        if let Some(o) = launched {
            self.launch_with_options(o);
        }
        for r in team_reqs {
            self.handle_request(r, actor.as_deref());
        }
    }
}

struct Chip {
    main: egui::Response,
    close: Option<egui::Response>,
    chevron: Option<egui::Response>,
}

/// One tab: icon, label, optional chevron (menu) and close button. Active = raised pill.
fn chip(
    ui: &mut egui::Ui,
    active: bool,
    icon: Icon,
    label: &str,
    min_w: f32,
    closable: bool,
    chevron: bool,
) -> Chip {
    let label: String = if label.chars().count() > 28 {
        format!("{}\u{2026}", label.chars().take(27).collect::<String>())
    } else {
        label.to_string()
    };
    let font = egui::TextStyle::Body.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(label, font, egui::Color32::PLACEHOLDER);
    let extra = if closable || chevron { 26.0 } else { 0.0 };
    let w = (14.0 + 16.0 + 8.0 + galley.size().x + extra + 12.0).max(min_w);
    let sense = if closable {
        egui::Sense::click_and_drag()
    } else {
        egui::Sense::click()
    };
    let (rect, main) = ui.allocate_exact_size(egui::vec2(w, 32.0), sense);
    let side = |ui: &egui::Ui, id: &str| {
        let r = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 20.0, rect.center().y),
            egui::vec2(22.0, 22.0),
        );
        (r, ui.interact(r, main.id.with(id), egui::Sense::click()))
    };
    let close = closable.then(|| side(ui, "close"));
    let chev = chevron.then(|| side(ui, "chevron"));
    if ui.is_rect_visible(rect) {
        let hovered = main.hovered() || close.as_ref().is_some_and(|c| c.1.hovered());
        let (fill, stroke) = if active {
            (theme::RAISED, theme::hairline(0.12))
        } else if hovered {
            (
                theme::tint(egui::Color32::WHITE, 0.04),
                egui::Color32::TRANSPARENT,
            )
        } else {
            (egui::Color32::TRANSPARENT, egui::Color32::TRANSPARENT)
        };
        let radius = egui::CornerRadius::same(9);
        ui.painter().rect_filled(rect, radius, fill);
        ui.painter().rect_stroke(
            rect,
            radius,
            egui::Stroke::new(1.0, stroke),
            egui::StrokeKind::Inside,
        );
        let fg = if active {
            theme::TEXT
        } else if hovered {
            theme::TEXT_2
        } else {
            theme::MUTED
        };
        icons::draw(
            ui.painter(),
            egui::Rect::from_center_size(
                egui::pos2(rect.left() + 14.0 + 8.0, rect.center().y),
                egui::vec2(16.0, 16.0),
            ),
            icon,
            fg,
        );
        ui.painter().galley(
            egui::pos2(
                rect.left() + 14.0 + 16.0 + 8.0,
                rect.center().y - galley.size().y / 2.0,
            ),
            galley,
            fg,
        );
        for (r, resp, glyph) in [
            close.as_ref().map(|(r, x)| (*r, x, Icon::X)),
            chev.as_ref().map(|(r, x)| (*r, x, Icon::ChevronDown)),
        ]
        .into_iter()
        .flatten()
        {
            if resp.hovered() {
                ui.painter().rect_filled(
                    r,
                    egui::CornerRadius::same(6),
                    theme::tint(egui::Color32::WHITE, 0.08),
                );
            }
            icons::draw(
                ui.painter(),
                egui::Rect::from_center_size(r.center(), egui::vec2(12.0, 12.0)),
                glyph,
                fg,
            );
        }
    }
    Chip {
        main,
        close: close.map(|c| c.1),
        chevron: chev.map(|c| c.1),
    }
}

/// Insertion slot for a tab dropped at pointer `x`: the number of tab centres left of it.
fn closable_rects_slot(rects: &[(Tab, egui::Rect)], x: f32) -> usize {
    rects.iter().filter(|(_, r)| r.center().x < x).count()
}

/// Small red count badge centred at `at` (e.g. on the bell).
fn badge(ui: &egui::Ui, at: egui::Pos2, n: usize) {
    let text = if n > 99 {
        "99+".to_string()
    } else {
        n.to_string()
    };
    let font = egui::FontId::proportional(10.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text, font, egui::Color32::WHITE);
    let w = (galley.size().x + 8.0).max(14.0);
    let rect = egui::Rect::from_center_size(at, egui::vec2(w, 14.0));
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(255), theme::ERROR);
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        egui::Color32::WHITE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(app: &mut App) {
        let mut frame = eframe::Frame::_new_kittest();
        for (w, h) in [(1600.0, 1000.0), (900.0, 600.0), (320.0, 240.0)] {
            let ctx = egui::Context::default();
            for _ in 0..3 {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(w, h),
                    )),
                    ..Default::default()
                };
                let mut out = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
                out.textures_delta.clear();
            }
        }
    }

    #[test]
    fn tab_reorder_and_close_actions() {
        let mut v = vec![1, 2, 3, 4];
        move_to_slot(&mut v, 1, 3);
        assert_eq!(v, [2, 3, 1, 4]);
        move_to_slot(&mut v, 4, 0);
        assert_eq!(v, [4, 2, 3, 1]);
        move_to_slot(&mut v, 2, 2);
        assert_eq!(v, [4, 2, 3, 1]);
        let ctx = egui::Context::default();
        let mut app = App::new(
            &ctx,
            std::env::temp_dir().join("rteams-gui-tabs-none"),
            "t".into(),
            None,
        );
        for t in [Tab::Usage, Tab::Graph, Tab::Sessions] {
            app.open_tab(t);
        }
        app.apply_tab_act(Tab::Graph, TabAct::CloseRight);
        assert_eq!(app.open_tabs, [Tab::Usage, Tab::Graph]);
        app.apply_tab_act(Tab::Usage, TabAct::MoveToPane);
        assert_eq!(app.split, Some(Tab::Usage));
        app.apply_tab_act(Tab::Graph, TabAct::CloseOthers);
        assert_eq!(app.open_tabs, [Tab::Graph]);
        assert_eq!(app.split, None);
    }

    #[test]
    fn every_screen_renders_headless() {
        let home = std::env::temp_dir().join(format!("rteams-gui-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let ctx = egui::Context::default();

        // Missing team: create screen.
        let mut app = App::new(&ctx, home.clone(), "t".into(), None);
        frames(&mut app);

        let (_h, _d) = crate::view::testing::fixture("app-fixture");
        let mut app = App::new(&ctx, _h.clone(), "t".into(), None);
        frames(&mut app);
        app.task_dialog.open("3".into());
        app.create_task.open(Some("ada".into()));
        frames(&mut app);
        app.task_dialog.close();
        app.create_task.clear();
        app.member_dialog.open("ada".into());
        app.add_member.open(["ada"]);
        frames(&mut app);
        app.member_dialog.close();
        app.add_member.clear();
        app.messages.open_chat(Some("ada".into()));
        frames(&mut app);
        app.tab = Tab::Teams;
        frames(&mut app);
        app.tab = Tab::Providers;
        frames(&mut app);
        for t in [
            Tab::Dashboard,
            Tab::Usage,
            Tab::Editor,
            Tab::Graph,
            Tab::Extensions,
            Tab::Schedules,
            Tab::OrgMap,
            Tab::Sessions,
            Tab::Settings,
        ] {
            app.tab = t;
            frames(&mut app);
        }
        // Shell chrome: split pane, task sidebar, palette, global search.
        app.tab = Tab::Team;
        app.move_to_pane(Tab::Usage);
        app.task_sidebar.toggle();
        frames(&mut app);
        app.palette.open(false);
        app.palette.set_query("task");
        frames(&mut app);
        app.palette.open(true);
        app.palette.set_query("hello");
        frames(&mut app);
        app.palette.close();
        app.split = None;
        app.task_sidebar.toggle();
        app.terminal.toggle();
        frames(&mut app);
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&_h);
    }
}

/// Offscreen screenshots of every screen for UI review against the original's screenshots.
/// `RTEAMS_SHOTS_HOME=<home with team "demo"> RTEAMS_SHOTS_OUT=<dir> cargo test -p rteams-gui
/// screenshots -- --ignored` (`RTEAMS_SHOTS_TEAM` picks another team)
#[cfg(test)]
mod screenshots {
    use super::*;

    fn shoot(out: &Path, name: &str, setup: impl Fn(&mut App)) {
        shoot_sized(out, name, egui::vec2(1600.0, 1000.0), setup);
    }

    fn shoot_sized(out: &Path, name: &str, size: egui::Vec2, setup: impl Fn(&mut App)) {
        let home = PathBuf::from(std::env::var("RTEAMS_SHOTS_HOME").expect("RTEAMS_SHOTS_HOME"));
        let team = std::env::var("RTEAMS_SHOTS_TEAM").unwrap_or_else(|_| "demo".into());
        let mut h = egui_kittest::Harness::builder()
            .with_size(size)
            .with_accessibility_check(false)
            .wgpu()
            .build_eframe(|cc| App::new(&cc.egui_ctx, home.clone(), team.clone(), None));
        // Live agent text can hold any emoji; draw tofu like the real app instead of panicking.
        h.ctx
            .set_missing_glyph_policy(egui::epaint::text::MissingGlyphPolicy::Tofu);
        setup(h.state_mut());
        h.run_steps(6);
        let img = h.render().expect("render");
        img.save(out.join(format!("{name}.png"))).expect("save png");
    }

    #[test]
    #[ignore]
    fn screenshots() {
        let out = PathBuf::from(std::env::var("RTEAMS_SHOTS_OUT").expect("RTEAMS_SHOTS_OUT"));
        std::fs::create_dir_all(&out).unwrap();
        for (name, tab) in [
            ("team", Tab::Team),
            ("dashboard", Tab::Dashboard),
            ("teams", Tab::Teams),
            ("usage", Tab::Usage),
            ("editor", Tab::Editor),
            ("graph", Tab::Graph),
            ("extensions", Tab::Extensions),
            ("schedules", Tab::Schedules),
            ("orgmap", Tab::OrgMap),
            ("sessions", Tab::Sessions),
            ("settings", Tab::Settings),
            ("providers", Tab::Providers),
        ] {
            shoot(&out, name, |a| a.open_tab(tab));
        }
        shoot(&out, "team_tabs", |a| {
            for t in [
                Tab::Usage,
                Tab::Graph,
                Tab::OrgMap,
                Tab::Sessions,
                Tab::Extensions,
            ] {
                a.open_tab(t);
            }
            a.open_tab(Tab::Team);
        });
        // Every tab open in a laptop-width window: the strip must scroll, not overlap.
        shoot_sized(&out, "tabs_overflow", egui::vec2(1100.0, 700.0), |a| {
            for t in [
                Tab::Usage,
                Tab::Graph,
                Tab::OrgMap,
                Tab::Sessions,
                Tab::Extensions,
                Tab::Schedules,
                Tab::Providers,
                Tab::Settings,
            ] {
                a.open_tab(t);
            }
        });
        shoot_sized(
            &out,
            "tabs_overflow_narrow",
            egui::vec2(720.0, 500.0),
            |a| {
                for t in [
                    Tab::Extensions,
                    Tab::Schedules,
                    Tab::OrgMap,
                    Tab::Sessions,
                    Tab::Settings,
                ] {
                    a.open_tab(t);
                }
                a.open_tab(Tab::Team);
            },
        );
        // The team's own first task / first non-lead member, so any shots home works.
        let first_task = |a: &mut App| {
            a.try_open();
            a.data
                .as_ref()
                .and_then(|d| d.state().tasks.first().map(|t| t.id.clone()))
        };
        let first_member = |a: &mut App| {
            a.try_open();
            a.data
                .as_ref()
                .and_then(|d| d.state().team.as_ref())
                .and_then(|t| {
                    t.members
                        .iter()
                        .find(|m| m.role != "lead")
                        .map(|m| m.name.clone())
                })
        };
        shoot(&out, "task_dialog", |a| {
            let id = first_task(a).unwrap_or_else(|| "11".into());
            a.task_dialog.open(id)
        });
        shoot(&out, "chat_full", |a| {
            a.messages.open_chat(None);
            a.messages.toggle_full_screen_for_tests();
        });
        shoot(&out, "member_dialog", |a| {
            let name = first_member(a).unwrap_or_else(|| "alice".into());
            a.member_dialog.open(name)
        });
        shoot(&out, "member_edit", |a| {
            let name = first_member(a).unwrap_or_else(|| "alice".into());
            a.member_dialog.open(name);
            a.member_dialog.show_edit_tab_for_tests();
        });
        shoot(&out, "create_task", |a| a.create_task.open(None));
        shoot(&out, "palette", |a| a.palette.open(false));
        shoot(&out, "palette_search", |a| {
            a.palette.open(true);
            a.palette.set_query("task");
        });
        shoot(&out, "split_panes", |a| {
            a.open_tab(Tab::Usage);
            a.move_to_pane(Tab::Graph);
        });
        shoot(&out, "task_sidebar", |a| a.task_sidebar.toggle());
        shoot(&out, "inbox", |a| a.messages.open_inbox_for_tests());
        shoot(&out, "add_member", |a| {
            a.try_open();
            let taken = a.member_names();
            a.add_member.open(taken.iter().map(String::as_str))
        });
    }
}
