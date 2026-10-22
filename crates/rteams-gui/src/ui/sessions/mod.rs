//! Sessions screen (TabId::Sessions): browse every Claude Code, Codex and rteams-run session on
//! disk. Three panes (projects | sessions | viewer), a conversation / waterfall / report viewer,
//! full-text search and notification triggers. All scanning, parsing and searching runs on
//! worker threads; the UI only draws what they deliver.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use rteams_services::config::AppConfig;
use rteams_services::i18n::I18n;
use rteams_services::sessions::chunks::{load_session, Session};
use rteams_services::sessions::metadata_index::SessionMeta;
use rteams_services::sessions::paths::Roots;
use rteams_services::sessions::report::{self, Report};
use rteams_services::sessions::scanner::{self, ScanResult};
use rteams_services::sessions::search::{self, SearchCache, SearchJob};
use rteams_services::sessions::triggers::{self, TriggerHit, TriggerRule, TriggerWatcher};
use rteams_services::sessions::worktrees::{group_by_repo, RepoGroup};

use crate::theme;
use crate::ui::{FeatureCtx, Screen, ShellRequest, TabId};

pub mod context_tab;
pub mod project_list;
pub mod report_tab;
pub mod search_bar;
pub mod session_list;
pub mod viewer;
pub mod waterfall;

use session_list::ListAction;

const RESCAN_EVERY: Duration = Duration::from_secs(30);
const WATCH_EVERY: Duration = Duration::from_secs(15);

// ---- shared formatting ----

/// Catalog text for `key`, or `default` when the catalog has no such key.
pub(crate) fn tt(tr: &I18n, key: &str, default: &str) -> String {
    if tr.has(key) {
        tr.t(key)
    } else {
        default.to_string()
    }
}

pub(crate) fn fmt_dur(ms: i64) -> String {
    let s = ms / 1000;
    if ms < 1000 {
        format!("{ms}ms")
    } else if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3600, s % 3600 / 60)
    }
}

pub(crate) fn fmt_cost(c: f64) -> String {
    if c > 0.0 && c < 0.01 {
        format!("${c:.4}")
    } else {
        format!("${c:.2}")
    }
}

pub(crate) fn fmt_tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.2}M", n as f64 / 1e6),
    }
}

/// `YYYY-MM-DD HH:MM` (UTC).
pub(crate) fn fmt_date(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let secs = ms.div_euclid(1000).rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs / 3600,
        secs % 3600 / 60
    )
}

/// Relative time ("5m ago") from `now` back to `then`; dates for anything older than a week.
pub(crate) fn fmt_ago(now: i64, then: i64) -> String {
    if then <= 0 {
        return String::new();
    }
    let d = (now - then).max(0) / 1000;
    match d {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m ago", d / 60),
        3600..=86_399 => format!("{}h ago", d / 3600),
        86_400..=604_799 => format!("{}d ago", d / 86_400),
        _ => fmt_date(then)[..10].to_string(),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

// ---- background jobs ----

/// One result delivered from a worker thread.
struct Job<T> {
    rx: Receiver<T>,
}

impl<T: Send + 'static> Job<T> {
    fn spawn(ctx: &egui::Context, f: impl FnOnce() -> T + Send + 'static) -> Job<T> {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f());
            ctx.request_repaint();
        });
        Job { rx }
    }

    fn poll(&self) -> Option<T> {
        self.rx.try_recv().ok()
    }
}

/// A finished scan with derived grouping.
struct Scanned {
    scan: ScanResult,
    groups: Vec<RepoGroup>,
    /// Project id of each session, parallel to `scan.sessions`.
    project_of: Vec<String>,
}

fn do_scan(roots: &Roots) -> Scanned {
    let scan = scanner::scan(roots);
    let mut canon = std::collections::HashMap::new();
    let project_of = scan
        .sessions
        .iter()
        .map(|s| scanner::project_id(s, &mut canon))
        .collect();
    let groups = group_by_repo(&scan.projects);
    Scanned {
        scan,
        groups,
        project_of,
    }
}

type LoadResult = Result<(Session, Report), String>;

struct Loaded {
    id: String,
    mtime_ms: i64,
    session: Session,
    report: Report,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewerTab {
    #[default]
    Conversation,
    Waterfall,
    Report,
    Context,
}

pub struct SessionsScreen {
    /// Test hook: explicit Claude / Codex dirs instead of `~/.claude` / `~/.codex`.
    override_dirs: Option<(Option<PathBuf>, Option<PathBuf>)>,
    scanned: Option<Arc<Scanned>>,
    scan_job: Option<Job<Scanned>>,
    last_scan: Option<Instant>,
    scan_error_free: bool,
    projects: project_list::ProjectListState,
    selected_project: Option<String>,
    selected_session: Option<String>,
    show_hidden: bool,
    cfg: AppConfig,
    loaded: Option<Loaded>,
    load_job: Option<(String, Job<LoadResult>)>,
    load_error: Option<String>,
    viewer: viewer::ViewerState,
    tab: ViewerTab,
    context: Option<context_tab::ContextData>,
    search: search_bar::SearchState,
    search_job: Option<SearchJob>,
    search_wake: Arc<Mutex<bool>>,
    search_cache: Arc<Mutex<SearchCache>>,
    watcher: Arc<Mutex<TriggerWatcher>>,
    watch_job: Option<Job<Vec<TriggerHit>>>,
    last_watch: Option<Instant>,
    rules: Option<Vec<TriggerRule>>,
    triggers_open: bool,
    /// Most recent trigger hits (newest last).
    pub recent_hits: Vec<TriggerHit>,
}

impl Default for SessionsScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionsScreen {
    pub fn new() -> Self {
        SessionsScreen {
            override_dirs: None,
            scanned: None,
            scan_job: None,
            last_scan: None,
            scan_error_free: true,
            projects: Default::default(),
            selected_project: None,
            selected_session: None,
            show_hidden: false,
            cfg: AppConfig::default(),
            loaded: None,
            load_job: None,
            load_error: None,
            viewer: Default::default(),
            tab: ViewerTab::Conversation,
            context: None,
            search: Default::default(),
            search_job: None,
            search_wake: Arc::default(),
            search_cache: Arc::default(),
            watcher: Arc::new(Mutex::new(TriggerWatcher::new(now_ms()))),
            watch_job: None,
            last_watch: None,
            rules: None,
            triggers_open: false,
            recent_hits: Vec::new(),
        }
    }

    /// Scan these directories instead of the real `~/.claude` / `~/.codex` (tests).
    #[cfg(test)]
    pub fn with_dirs(claude: Option<PathBuf>, codex: Option<PathBuf>) -> Self {
        SessionsScreen {
            override_dirs: Some((claude, codex)),
            ..Self::new()
        }
    }

    fn roots(&self, home: &std::path::Path) -> Roots {
        match &self.override_dirs {
            Some((c, x)) => Roots::new(home, c.clone(), x.clone()),
            None => Roots::system(home),
        }
    }

    fn start_scan(&mut self, ctx: &egui::Context, home: &std::path::Path) {
        if self.scan_job.is_some() {
            return;
        }
        let roots = self.roots(home);
        self.cfg = AppConfig::load(home);
        self.scan_job = Some(Job::spawn(ctx, move || do_scan(&roots)));
        self.last_scan = Some(Instant::now());
    }

    fn start_load(&mut self, ctx: &egui::Context, meta: &SessionMeta) {
        let path = PathBuf::from(&meta.path);
        let source = meta.source;
        let id = meta.id.clone();
        self.load_error = None;
        self.load_job = Some((
            id,
            Job::spawn(ctx, move || {
                load_session(&path, source)
                    .map(|s| {
                        let r = report::build(&s);
                        (s, r)
                    })
                    .map_err(|e| e.to_string())
            }),
        ));
    }

    fn select_session(&mut self, ctx: &egui::Context, id: &str) {
        self.selected_session = Some(id.to_string());
        self.viewer.reset();
        self.loaded = None;
        let meta = self
            .scanned
            .as_ref()
            .and_then(|s| s.scan.sessions.iter().find(|m| m.id == id).cloned());
        if let Some(m) = meta {
            self.start_load(ctx, &m);
        }
    }

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        if let Some(sc) = self.scan_job.as_ref().and_then(Job::poll) {
            self.scan_job = None;
            self.scan_error_free = true;
            let sc = Arc::new(sc);
            // reload the open session when its file changed
            let reload = self.selected_session.as_ref().and_then(|id| {
                let m = sc.scan.sessions.iter().find(|m| &m.id == id)?;
                let stale = self
                    .loaded
                    .as_ref()
                    .is_some_and(|l| l.id == *id && l.mtime_ms != m.mtime_ms);
                stale.then(|| m.clone())
            });
            self.scanned = Some(sc);
            if let Some(m) = reload {
                self.start_load(ctx, &m);
            }
        }
        let done = self
            .load_job
            .as_ref()
            .and_then(|(id, j)| j.poll().map(|r| (id.clone(), r)));
        if let Some((id, res)) = done {
            self.load_job = None;
            match res {
                Ok((session, report)) => {
                    let mtime_ms = self
                        .scanned
                        .as_ref()
                        .and_then(|s| s.scan.sessions.iter().find(|m| m.id == id))
                        .map_or(0, |m| m.mtime_ms);
                    self.loaded = Some(Loaded {
                        id,
                        mtime_ms,
                        session,
                        report,
                    });
                }
                Err(e) => self.load_error = Some(e),
            }
        }
        if let Some(job) = &self.search_job {
            if let Some(hits) = job.try_take() {
                self.search.results = hits;
                self.search.searching = false;
                self.search_job = None;
            }
        }
    }

    fn run_search(&mut self, ctx: &egui::Context, q: String) {
        let Some(sc) = self.scanned.clone() else {
            return;
        };
        self.search.ran_query = q.clone();
        self.search.searching = true;
        self.search.results.clear();
        let sessions = sc.scan.sessions.clone();
        let ctx = ctx.clone();
        self.search_job = Some(search::spawn(
            sessions,
            q,
            search::MAX_RESULTS,
            self.search_cache.clone(),
            move || ctx.request_repaint(),
        ));
    }

    fn toggle_list(&mut self, home: &std::path::Path, id: &str, pin: bool) {
        // Re-read the config so edits made elsewhere are not overwritten.
        let mut cfg = AppConfig::load(home);
        let list = if pin {
            &mut cfg.pinned_sessions
        } else {
            &mut cfg.hidden_sessions
        };
        if let Some(i) = list.iter().position(|x| x == id) {
            list.remove(i);
        } else {
            list.push(id.to_string());
        }
        let _ = cfg.save(home);
        self.cfg = cfg;
    }

    /// Hide (`hide`) or unhide every session in the current list at once.
    fn set_hidden_bulk(&mut self, home: &std::path::Path, ids: &[String], hide: bool) {
        let mut cfg = AppConfig::load(home);
        for id in ids {
            let pos = cfg.hidden_sessions.iter().position(|x| x == id);
            match (hide, pos) {
                (true, None) => cfg.hidden_sessions.push(id.clone()),
                (false, Some(i)) => {
                    cfg.hidden_sessions.remove(i);
                }
                _ => {}
            }
        }
        let _ = cfg.save(home);
        self.cfg = cfg;
    }

    fn display_list(&self) -> Vec<&SessionMeta> {
        let Some(sc) = &self.scanned else {
            return Vec::new();
        };
        let all: Vec<&SessionMeta> = match &self.selected_project {
            None => sc.scan.sessions.iter().collect(),
            Some(p) => sc
                .scan
                .sessions
                .iter()
                .zip(&sc.project_of)
                .filter(|(_, id)| *id == p)
                .map(|(s, _)| s)
                .collect(),
        };
        scanner::order_for_display(
            all,
            &self.cfg.pinned_sessions,
            &self.cfg.hidden_sessions,
            self.show_hidden,
        )
    }

    fn header(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        let ctx = ui.ctx().clone();
        let tr = c.tr;
        ui.horizontal(|ui| {
            ui.heading(tt(tr, "common.sessions.title", "Sessions"));
            ui.add_space(12.0);
            match search_bar::show_bar(ui, tr, &mut self.search) {
                search_bar::SearchAction::Run(q) => self.run_search(&ctx, q),
                search_bar::SearchAction::Clear => {
                    if let Some(j) = self.search_job.take() {
                        j.cancel();
                    }
                    self.search.clear();
                }
                search_bar::SearchAction::None => {}
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let busy = self.scan_job.is_some();
                if ui
                    .add_enabled(
                        !busy,
                        egui::Button::new(tt(tr, "sessions.refresh", "Refresh")),
                    )
                    .clicked()
                {
                    self.start_scan(&ctx, c.home);
                }
                if ui
                    .button(tt(
                        tr,
                        "settings.notificationTriggers.title",
                        "Notification Triggers",
                    ))
                    .clicked()
                {
                    self.triggers_open = !self.triggers_open;
                    if self.triggers_open {
                        self.rules = Some(triggers::load_rules(c.home));
                    }
                }
                if busy {
                    ui.spinner();
                }
            });
        });
    }

    fn viewer_pane(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        let tr = c.tr;
        let Some(sel) = self.selected_session.clone() else {
            ui.add_space(20.0);
            crate::ui::dashboard::kit::empty_state(
                ui,
                crate::icons::Icon::History,
                &tt(
                    tr,
                    "common.sessions.empty.selectProject",
                    "Select a session to view it",
                ),
                "",
            );
            return;
        };
        if self.loaded.as_ref().is_none_or(|l| l.id != sel) {
            if let Some(e) = &self.load_error {
                ui.colored_label(
                    theme::ERROR,
                    format!(
                        "{}: {e}",
                        tt(tr, "common.sessions.failedToLoad", "Failed to load session")
                    ),
                );
            } else {
                crate::ui::dashboard::kit::skeleton_tiles(
                    ui,
                    1,
                    ui.available_width(),
                    120.0,
                    &tt(tr, "common.sessions.loading", "Loading session..."),
                );
            }
            return;
        }
        let meta = self
            .scanned
            .as_ref()
            .and_then(|s| s.scan.sessions.iter().find(|m| m.id == sel))
            .cloned();
        let pinned = self.cfg.pinned_sessions.contains(&sel);
        let hidden = self.cfg.hidden_sessions.contains(&sel);
        let mut toggle: Option<bool> = None;
        let mut copy = false;
        {
            let Some(l) = self.loaded.as_ref() else {
                return;
            };
            let s = &l.session;
            ui.horizontal_wrapped(|ui| {
                let title = s
                    .chunks
                    .first()
                    .and_then(|ch| ch.prompt.as_deref())
                    .unwrap_or(&s.id)
                    .replace('\n', " ");
                let mut t: String = title.chars().take(90).collect();
                if title.chars().count() > 90 {
                    t.push('\u{2026}');
                }
                ui.label(RichText::new(t).strong().size(15.0));
            });
            ui.horizontal_wrapped(|ui| {
                theme::pill(ui, s.source.as_str(), theme::ACCENT);
                let mut bits = vec![theme::short_id(&s.id)];
                if let Some(cwd) = &s.cwd {
                    bits.push(cwd.clone());
                }
                if let Some(b) = &s.branch {
                    bits.push(b.clone());
                }
                if !s.model.is_empty() {
                    bits.push(s.model.clone());
                }
                if let Some(d) = s.duration_ms() {
                    bits.push(fmt_dur(d));
                }
                bits.push(fmt_cost(l.report.cost.total));
                ui.label(
                    RichText::new(bits.join(" \u{00B7} "))
                        .small()
                        .color(theme::MUTED),
                );
            });
            if let Some(m) = &meta {
                let _ = m;
            }
        }
        ui.horizontal(|ui| {
            for (tab, key, d) in [
                (
                    ViewerTab::Conversation,
                    "sessions.tab.conversation",
                    "Conversation",
                ),
                (ViewerTab::Waterfall, "sessions.tab.waterfall", "Waterfall"),
                (ViewerTab::Report, "sessions.tab.report", "Session report"),
                (ViewerTab::Context, "sessions.tab.context", "Context"),
            ] {
                if ui
                    .selectable_label(self.tab == tab, tt(tr, key, d))
                    .clicked()
                {
                    self.tab = tab;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let hide = if hidden {
                    tt(tr, "common.sessions.actions.unhide", "Unhide")
                } else {
                    tt(tr, "common.sessions.actions.hide", "Hide")
                };
                if theme::ghost(ui, &hide).clicked() {
                    toggle = Some(false);
                }
                let pin = if pinned {
                    tt(tr, "common.sessions.actions.unpin", "Unpin")
                } else {
                    tt(tr, "common.sessions.actions.pin", "Pin")
                };
                if theme::ghost(ui, &pin).clicked() {
                    toggle = Some(true);
                }
                if self.tab == ViewerTab::Conversation {
                    if theme::ghost(ui, &tt(tr, "sessions.collapseAll", "Collapse all")).clicked() {
                        self.viewer.collapse_all();
                    }
                    if theme::ghost(ui, &tt(tr, "sessions.expandTools", "Expand tools")).clicked() {
                        if let Some(l) = &self.loaded {
                            self.viewer.expand_all_tools(&l.session);
                        }
                    }
                    ui.checkbox(
                        &mut self.viewer.show_thinking,
                        tt(tr, "sessions.showThinking", "Thinking"),
                    );
                }
            });
        });
        theme::rule(ui);
        ui.add_space(4.0);
        let Some(l) = self.loaded.as_ref() else {
            return;
        };
        match self.tab {
            ViewerTab::Conversation => viewer::show(ui, tr, &mut self.viewer, &l.session),
            ViewerTab::Waterfall => {
                if let Some(chunk) = waterfall::show(ui, tr, &l.session) {
                    self.viewer.scroll_to = Some(chunk);
                    self.tab = ViewerTab::Conversation;
                }
            }
            ViewerTab::Report => copy = report_tab::show(ui, tr, &l.report),
            ViewerTab::Context => {
                if self.context.as_ref().map(|d| d.session_id.as_str())
                    != Some(l.session.id.as_str())
                {
                    let ov = self
                        .cfg
                        .extra
                        .get("claude_root_override")
                        .and_then(|v| v.as_str());
                    let base = rteams_services::claude_root::resolve_system(ov).path;
                    self.context = Some(context_tab::build(
                        &l.session,
                        &base,
                        rteams_services::trust::claude_json_path().as_deref(),
                    ));
                }
                if let Some(d) = &self.context {
                    context_tab::show(ui, tr, d);
                }
            }
        }
        if copy {
            ui.ctx().copy_text(report::to_markdown(&l.report));
            c.req.push(ShellRequest::Toast(tt(
                tr,
                "sessions.reportCopied",
                "Report copied",
            )));
        }
        if let Some(pin) = toggle {
            self.toggle_list(c.home, &sel, pin);
        }
    }
}

impl Screen for SessionsScreen {
    fn id(&self) -> TabId {
        TabId::Sessions
    }

    fn title_key(&self) -> &'static str {
        "common.sessions.title"
    }

    fn tick(&mut self, ctx: &egui::Context, c: &mut FeatureCtx) {
        self.poll_jobs(ctx);
        // Notification triggers: poll session files in the background.
        if let Some(hits) = self.watch_job.as_ref().and_then(Job::poll) {
            self.watch_job = None;
            for h in hits.iter().take(5) {
                crate::notify::notify(&h.title, &h.body);
                c.req.push(ShellRequest::Toast(h.title.clone()));
            }
            self.recent_hits.extend(hits);
            let n = self.recent_hits.len();
            if n > 50 {
                self.recent_hits.drain(..n - 50);
            }
        }
        let due = self.last_watch.is_none_or(|t| t.elapsed() >= WATCH_EVERY);
        if self.watch_job.is_none() && due {
            self.last_watch = Some(Instant::now());
            let rules = triggers::load_rules(c.home);
            if rules.iter().any(|r| r.enabled) {
                let roots = self.roots(c.home);
                let w = self.watcher.clone();
                self.watch_job = Some(Job::spawn(ctx, move || match w.lock() {
                    Ok(mut g) => g.poll(&roots, &rules, now_ms()),
                    Err(_) => Vec::new(),
                }));
            }
        }
        let _ = &self.search_wake;
    }

    fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        let ctx = ui.ctx().clone();
        self.poll_jobs(&ctx);
        let rescan = self.last_scan.is_none_or(|t| t.elapsed() >= RESCAN_EVERY);
        if rescan {
            self.start_scan(&ctx, c.home);
        }
        ctx.request_repaint_after(Duration::from_secs(5));
        let tr = c.tr;
        let now = now_ms();

        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| self.header(ui, c));
        theme::rule(ui);

        let Some(sc) = self.scanned.clone() else {
            ui.add_space(20.0);
            crate::ui::dashboard::kit::skeleton_tiles(
                ui,
                3,
                300.0,
                96.0,
                &tt(tr, "sessions.scanning", "Scanning sessions..."),
            );
            return;
        };

        if self.triggers_open {
            let mut open = true;
            let mut rules = self
                .rules
                .take()
                .unwrap_or_else(|| triggers::load_rules(c.home));
            let mut changed = false;
            egui::Window::new(tt(
                tr,
                "settings.notificationTriggers.title",
                "Notification Triggers",
            ))
            .open(&mut open)
            .default_width(520.0)
            .vscroll(true)
            .show(ui.ctx(), |ui| {
                let sample = self.loaded.as_ref().map(|l| &l.session);
                changed =
                    crate::ui::trigger_settings::show_with_session(ui, &mut rules, tr, sample);
            });
            if changed {
                let _ = triggers::save_rules(c.home, &rules);
            }
            self.rules = Some(rules);
            self.triggers_open = open;
        }

        let width = ui.available_width();
        egui::Panel::left("sessions_projects_panel")
            .default_size((width * 0.2).clamp(180.0, 300.0))
            .size_range(140.0..=420.0)
            .frame(
                egui::Frame::new()
                    .fill(theme::SIDEBAR)
                    .inner_margin(egui::Margin::symmetric(10, 8)),
            )
            .show(ui, |ui| {
                project_list::show(
                    ui,
                    tr,
                    &mut self.projects,
                    &sc.groups,
                    sc.scan.sessions.len(),
                    &mut self.selected_project,
                    now,
                );
            });

        let mut actions = Vec::new();
        if !self.search.active() {
            egui::Panel::left("sessions_list_panel")
                .default_size((width * 0.28).clamp(240.0, 420.0))
                .size_range(200.0..=520.0)
                .frame(
                    egui::Frame::new()
                        .fill(theme::SURFACE)
                        .inner_margin(egui::Margin::symmetric(8, 8)),
                )
                .show(ui, |ui| {
                    let list = self.display_list();
                    let mut show_hidden = self.show_hidden;
                    actions = session_list::show(
                        ui,
                        tr,
                        &list,
                        self.selected_session.as_deref(),
                        &self.cfg.pinned_sessions,
                        &self.cfg.hidden_sessions,
                        &mut show_hidden,
                        now,
                    );
                    self.show_hidden = show_hidden;
                });
        }

        let mut open_hit: Option<String> = None;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::symmetric(14, 8)),
            )
            .show(ui, |ui| {
                if self.search.active() {
                    if let Some(i) = search_bar::show_results(ui, tr, &self.search) {
                        open_hit = self.search.results.get(i).map(|h| h.session_id.clone());
                    }
                } else {
                    self.viewer_pane(ui, c);
                }
            });

        if let Some(id) = open_hit {
            // jump to the session's project and open it
            if let Some(p) = sc
                .scan
                .sessions
                .iter()
                .zip(&sc.project_of)
                .find(|(s, _)| s.id == id)
                .map(|(_, p)| p.clone())
            {
                self.selected_project = Some(p);
            }
            if let Some(j) = self.search_job.take() {
                j.cancel();
            }
            self.search.clear();
            self.select_session(&ctx, &id);
        }
        for a in actions {
            match a {
                ListAction::Select(id) => self.select_session(&ctx, &id),
                ListAction::TogglePin(id) => self.toggle_list(c.home, &id, true),
                ListAction::ToggleHide(id) => self.toggle_list(c.home, &id, false),
                ListAction::SetHiddenAll(hide) => {
                    let ids: Vec<String> =
                        self.display_list().iter().map(|s| s.id.clone()).collect();
                    self.set_hidden_bulk(c.home, &ids, hide);
                }
            }
        }
        let _ = self.scan_error_free;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::render;
    use crate::view::Intents;

    #[test]
    fn formatting() {
        assert_eq!(fmt_dur(500), "500ms");
        assert_eq!(fmt_dur(65_000), "1m 5s");
        assert_eq!(fmt_dur(3_700_000), "1h 1m");
        assert_eq!(fmt_tokens(1500), "1.5k");
        assert_eq!(fmt_tokens(2_500_000), "2.50M");
        assert_eq!(fmt_cost(0.005), "$0.0050");
        assert_eq!(fmt_cost(1.234), "$1.23");
        assert_eq!(fmt_date(0), "1970-01-01 00:00");
        assert_eq!(fmt_date(1_740_823_200_000), "2025-03-01 10:00");
        assert_eq!(fmt_ago(1_000_000, 1_000_000 - 120_000), "2m ago");
    }

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let claude = d.path().join("claude");
        let proj = claude.join("projects/-work-proj");
        std::fs::create_dir_all(proj.join("s1/subagents")).unwrap();
        let main = [
            r#"{"type":"user","uuid":"u1","sessionId":"s1","cwd":"/work/proj","gitBranch":"main","timestamp":"2025-03-01T10:00:00.000Z","message":{"role":"user","content":"Fix the login bug please"}}"#,
            r#"{"type":"assistant","uuid":"a1","timestamp":"2025-03-01T10:00:02.000Z","message":{"id":"m1","model":"claude-sonnet-4-5","content":[{"type":"thinking","thinking":"look"},{"type":"text","text":"Let me **read** it."},{"type":"tool_use","id":"t1","name":"Task","input":{"description":"explore","prompt":"look around"}}],"usage":{"input_tokens":100,"output_tokens":20}}}"#,
            r#"{"type":"user","uuid":"u2","timestamp":"2025-03-01T10:00:20.000Z","toolUseResult":{"agentId":"abc"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"done"}]}}"#,
            r#"{"type":"assistant","uuid":"a2","timestamp":"2025-03-01T10:00:25.000Z","message":{"id":"m2","model":"claude-sonnet-4-5","content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"cargo test"}}],"usage":{"input_tokens":10,"output_tokens":5}}}"#,
            r#"{"type":"user","uuid":"u3","timestamp":"2025-03-01T10:00:30.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t2","content":"error: failed","is_error":true}]}}"#,
        ]
        .join("\n");
        std::fs::write(proj.join("s1.jsonl"), main).unwrap();
        std::fs::write(
            proj.join("s1/subagents/agent-abc.jsonl"),
            [
                r#"{"type":"user","isSidechain":true,"timestamp":"2025-03-01T10:00:03Z","message":{"role":"user","content":"look around"}}"#,
                r#"{"type":"assistant","isSidechain":true,"timestamp":"2025-03-01T10:00:10Z","message":{"id":"sm","model":"claude-haiku-4-5","content":[{"type":"tool_use","id":"x","name":"Grep","input":{"pattern":"foo"}}],"usage":{"input_tokens":5,"output_tokens":1}}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        std::fs::write(
            proj.join("s2.jsonl"),
            r#"{"type":"user","uuid":"u1","sessionId":"s2","cwd":"/work/proj","timestamp":"2025-03-02T10:00:00.000Z","message":{"role":"user","content":"second session"}}"#,
        )
        .unwrap();
        let home = d.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        (d, home)
    }

    fn frame(s: &mut SessionsScreen, home: &std::path::Path, tr: &I18n) {
        render(|ui| {
            let mut out = Intents::default();
            let mut req = Vec::new();
            let mut c = FeatureCtx {
                home,
                team: None,
                view: None,
                out: &mut out,
                tr,
                req: &mut req,
            };
            s.tick(ui.ctx(), &mut c);
            s.show(ui, &mut c);
        });
    }

    fn settle(
        s: &mut SessionsScreen,
        home: &std::path::Path,
        tr: &I18n,
        until: impl Fn(&SessionsScreen) -> bool,
    ) {
        for _ in 0..300 {
            let ctx = egui::Context::default();
            s.poll_jobs(&ctx);
            if until(s) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = (home, tr);
        panic!("timed out");
    }

    #[test]
    fn scans_selects_and_renders_every_tab() {
        let (d, home) = fixture();
        let tr = I18n::default();
        let mut s = SessionsScreen::with_dirs(Some(d.path().join("claude")), None);
        frame(&mut s, &home, &tr);
        settle(&mut s, &home, &tr, |s| s.scanned.is_some());
        frame(&mut s, &home, &tr);
        let sc = s.scanned.clone().unwrap();
        assert_eq!(sc.scan.sessions.len(), 2);
        assert_eq!(sc.groups.len(), 1);
        // newest first
        assert_eq!(s.display_list()[0].id, "s2");
        s.select_session(&egui::Context::default(), "s1");
        settle(&mut s, &home, &tr, |s| s.loaded.is_some());
        let l = s.loaded.as_ref().unwrap();
        assert_eq!(l.session.subagents.len(), 1);
        assert_eq!(l.report.errors.len(), 1);
        for tab in [
            ViewerTab::Conversation,
            ViewerTab::Waterfall,
            ViewerTab::Report,
            ViewerTab::Context,
        ] {
            s.tab = tab;
            frame(&mut s, &home, &tr);
        }
        // open the subagent card
        let mut st = viewer::ViewerState::default();
        st.open.insert("c0.s2".into());
        s.viewer = st;
        s.tab = ViewerTab::Conversation;
        frame(&mut s, &home, &tr);
    }

    #[test]
    fn pin_and_hide_persist_to_config_and_reorder() {
        let (d, home) = fixture();
        let tr = I18n::default();
        let mut s = SessionsScreen::with_dirs(Some(d.path().join("claude")), None);
        frame(&mut s, &home, &tr);
        settle(&mut s, &home, &tr, |s| s.scanned.is_some());
        s.toggle_list(&home, "s1", true);
        assert_eq!(s.display_list()[0].id, "s1", "pinned first");
        assert_eq!(
            AppConfig::load(&home).pinned_sessions,
            vec!["s1".to_string()]
        );
        s.toggle_list(&home, "s2", false);
        assert!(s.display_list().iter().all(|m| m.id != "s2"));
        s.show_hidden = true;
        assert!(s.display_list().iter().any(|m| m.id == "s2"));
        s.toggle_list(&home, "s2", false);
        assert!(AppConfig::load(&home).hidden_sessions.is_empty());
        frame(&mut s, &home, &tr);
    }

    #[test]
    fn bulk_hide_and_unhide_persist() {
        let (d, home) = fixture();
        let tr = I18n::default();
        let mut s = SessionsScreen::with_dirs(Some(d.path().join("claude")), None);
        frame(&mut s, &home, &tr);
        settle(&mut s, &home, &tr, |s| s.scanned.is_some());
        let ids: Vec<String> = s.display_list().iter().map(|m| m.id.clone()).collect();
        assert_eq!(ids.len(), 2);
        s.set_hidden_bulk(&home, &ids, true);
        assert_eq!(AppConfig::load(&home).hidden_sessions.len(), 2);
        assert!(s.display_list().is_empty());
        s.set_hidden_bulk(&home, &ids, true);
        assert_eq!(AppConfig::load(&home).hidden_sessions.len(), 2, "no dupes");
        s.set_hidden_bulk(&home, &ids, false);
        assert!(AppConfig::load(&home).hidden_sessions.is_empty());
    }

    #[test]
    fn search_flow_and_open_hit() {
        let (d, home) = fixture();
        let tr = I18n::default();
        let mut s = SessionsScreen::with_dirs(Some(d.path().join("claude")), None);
        frame(&mut s, &home, &tr);
        settle(&mut s, &home, &tr, |s| s.scanned.is_some());
        s.search.query = "LOGIN".into();
        s.run_search(&egui::Context::default(), "LOGIN".into());
        settle(&mut s, &home, &tr, |s| !s.search.searching);
        assert!(s.search.results.iter().any(|h| h.session_id == "s1"));
        frame(&mut s, &home, &tr);
    }

    #[test]
    fn trigger_window_renders_and_saves() {
        let (d, home) = fixture();
        let tr = I18n::default();
        let mut s = SessionsScreen::with_dirs(Some(d.path().join("claude")), None);
        frame(&mut s, &home, &tr);
        settle(&mut s, &home, &tr, |s| s.scanned.is_some());
        s.triggers_open = true;
        frame(&mut s, &home, &tr);
        assert!(s.rules.is_some());
    }

    #[test]
    fn tick_without_enabled_rules_spawns_nothing() {
        let (d, home) = fixture();
        let tr = I18n::default();
        let mut s = SessionsScreen::with_dirs(Some(d.path().join("claude")), None);
        let ctx = egui::Context::default();
        let mut out = Intents::default();
        let mut req = Vec::new();
        let mut c = FeatureCtx {
            home: &home,
            team: None,
            view: None,
            out: &mut out,
            tr: &tr,
            req: &mut req,
        };
        s.tick(&ctx, &mut c);
        assert!(s.watch_job.is_none());
        // enable a rule: a job starts
        let mut rules = triggers::load_rules(&home);
        rules[0].enabled = true;
        triggers::save_rules(&home, &rules).unwrap();
        s.last_watch = None;
        s.tick(&ctx, &mut c);
        assert!(s.watch_job.is_some());
    }
}
