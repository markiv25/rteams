//! Dashboard screen (the original's first tab): header with quick actions, running teams,
//! recent projects and a summary of every team. Heavy scans (team states, git, Claude / Codex
//! session discovery) run on worker threads and are polled in `tick`.
//!
//! Shell contract:
//! * call `set_running(launched, provisioning)` whenever the set of teams whose agents the GUI
//!   runs changes (other teams are detected from recent `sessions.json` / run files);
//! * poll `take_draft()` each frame: a `TeamDraft` from the template picker or the import
//!   dialog that the shell turns into commands (the folder already exists);

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use rteams_services::paths::AppPaths;
use rteams_services::projects::recent::{self, Sources};
use rteams_services::projects::{folder, RecentProject};
use rteams_services::team_import::TeamDraft;

use crate::icons::Icon;
use crate::theme;
use crate::ui::import_team_dialog::ImportTeamDialog;
use crate::ui::template_picker::TemplatePicker;
use crate::ui::{FeatureCtx, Screen, ShellRequest, TabId};

pub mod kit;
pub mod project_picker;
pub mod recent_projects;
pub mod running_teams;

use recent_projects::{Action, RecentSection};
use running_teams::{RunStatus, RunningCandidate, TaskCounts};

const TEAMS_REFRESH: Duration = Duration::from_secs(4);
const RECENT_TTL: Duration = Duration::from_secs(60);
/// How long an unfinished scan shows skeletons before the empty state takes over.
const LOAD_GRACE: Duration = Duration::from_millis(2500);

pub struct Dashboard {
    recent_section: RecentSection,
    recent: Vec<RecentProject>,
    recent_at: Option<Instant>,
    recent_rx: Option<Receiver<Vec<RecentProject>>>,
    candidates: Vec<RunningCandidate>,
    teams_at: Option<Instant>,
    teams_rx: Option<Receiver<Vec<RunningCandidate>>>,
    launched: HashSet<String>,
    provisioning: HashSet<String>,
    sources: Sources,
    new_name: String,
    create_open: bool,
    template: TemplatePicker,
    import: ImportTeamDialog,
    draft: Option<TeamDraft>,
}

impl Default for Dashboard {
    fn default() -> Self {
        Dashboard {
            recent_section: RecentSection::new(),
            recent: Vec::new(),
            recent_at: None,
            recent_rx: None,
            candidates: Vec::new(),
            teams_at: None,
            teams_rx: None,
            launched: HashSet::new(),
            provisioning: HashSet::new(),
            sources: Sources::from_env(),
            new_name: String::new(),
            create_open: false,
            template: TemplatePicker::default(),
            import: ImportTeamDialog::default(),
            draft: None,
        }
    }
}

impl Dashboard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Where Claude / Codex sessions are looked up (tests point this at temp dirs).
    #[cfg(test)]
    pub fn with_sources(mut self, sources: Sources) -> Self {
        self.sources = sources;
        self
    }

    /// Teams whose agents this app runs / is launching right now.
    pub fn set_running(&mut self, launched: HashSet<String>, provisioning: HashSet<String>) {
        if launched != self.launched || provisioning != self.provisioning {
            self.launched = launched;
            self.provisioning = provisioning;
            self.teams_at = None; // refresh now
        }
    }

    /// A confirmed draft from the template picker or import dialog.
    pub fn take_draft(&mut self) -> Option<TeamDraft> {
        self.draft.take()
    }

    /// Force a rescan of teams and projects (e.g. after a team was created or deleted).
    pub fn invalidate(&mut self) {
        self.teams_at = None;
        self.recent_at = None;
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.teams_rx {
            if let Ok(v) = rx.try_recv() {
                self.candidates = v;
                self.teams_rx = None;
            }
        }
        if let Some(rx) = &self.recent_rx {
            if let Ok(v) = rx.try_recv() {
                self.recent = v;
                self.recent_rx = None;
            }
        }
    }

    fn start_jobs(&mut self, ctx: &egui::Context, home: &std::path::Path) {
        let now = Instant::now();
        if self.teams_rx.is_none() && self.teams_at.is_none_or(|t| now - t >= TEAMS_REFRESH) {
            self.teams_at = Some(now);
            let (tx, rx) = channel();
            self.teams_rx = Some(rx);
            let (home, l, p, ctx) = (
                home.to_path_buf(),
                self.launched.clone(),
                self.provisioning.clone(),
                ctx.clone(),
            );
            std::thread::spawn(move || {
                let v = running_teams::collect_candidates(&home, &l, &p, recent_projects::now_ms());
                let _ = tx.send(v);
                ctx.request_repaint();
            });
        }
        if self.recent_rx.is_none() && self.recent_at.is_none_or(|t| now - t >= RECENT_TTL) {
            self.recent_at = Some(now);
            let (tx, rx) = channel();
            self.recent_rx = Some(rx);
            let (paths, src, ctx) = (AppPaths::new(home), self.sources.clone(), ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(recent::collect(&paths, &src));
                ctx.request_repaint();
            });
        }
    }

    fn handle(&mut self, c: &mut FeatureCtx, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::CreateTeamAt(p) => self.template.open(Some(p), None),
                Action::Opened(p) => {
                    let _ = recent::touch(&AppPaths::new(c.home), &p, recent_projects::now_ms());
                    self.recent_at = None;
                }
                Action::SelectFolder => {
                    if let Some(p) = project_picker::pick_folder(c.tr) {
                        match folder::validate(&p) {
                            Ok(_) => {
                                let _ = recent::touch(
                                    &AppPaths::new(c.home),
                                    &p,
                                    recent_projects::now_ms(),
                                );
                                self.recent_at = None;
                                self.template.open(Some(p), None);
                            }
                            Err(e) => c.req.push(ShellRequest::Toast(e)),
                        }
                    }
                }
            }
        }
    }

    fn header(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) -> Vec<Action> {
        let mut actions = Vec::new();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 10.0);
            ui.label(RichText::new("Dashboard").size(22.0).strong());
            ui.add_space(12.0);
            let create = egui::Button::new(
                RichText::new(c.tr.t("team.list.actions.createTeam"))
                    .strong()
                    .color(egui::Color32::WHITE),
            )
            .fill(theme::ACCENT)
            .corner_radius(egui::CornerRadius::same(255))
            .min_size(egui::vec2(0.0, 30.0));
            if ui.add(create).clicked() {
                self.create_open = true;
            }
            if theme::pill_icon_button(
                ui,
                Some(Icon::Kanban),
                &c.tr.t("team.templates.label"),
                false,
            )
            .clicked()
            {
                self.template.open(None, None);
            }
            if theme::pill_icon_button(
                ui,
                Some(Icon::UserPlus),
                &c.tr.t("team.list.actions.importTeam"),
                false,
            )
            .clicked()
            {
                self.import.open();
            }
            if theme::pill_icon_button(
                ui,
                Some(Icon::Folder),
                &c.tr.t("dashboard.recentProjects.selectFolder"),
                false,
            )
            .clicked()
            {
                actions.push(Action::SelectFolder);
            }
        });
        ui.add_space(14.0);
        actions
    }

    /// Four summary tiles across the full width.
    fn stats(&self, ui: &mut egui::Ui, c: &FeatureCtx) {
        let live: Vec<&RunningCandidate> = self.candidates.iter().filter(|t| !t.deleted).collect();
        let running = live
            .iter()
            .filter(|t| t.status != RunStatus::Offline)
            .count();
        let sum = |f: fn(&TaskCounts) -> usize| live.iter().map(|t| f(&t.counts)).sum::<usize>();
        let tiles = [
            (
                c.tr.t("common.layout.menu.teams"),
                live.len(),
                theme::ACCENT,
            ),
            (c.tr.t("team.runningTeams.title"), running, theme::SUCCESS),
            (
                "Active tasks".to_string(),
                sum(|n| n.in_progress),
                theme::WARNING,
            ),
            ("Pending".to_string(), sum(|n| n.pending), theme::INFO),
            ("Done".to_string(), sum(|n| n.completed), theme::COL_DONE),
        ];
        let min_w = ((ui.available_width() - 4.0 * kit::GAP) / 5.0).max(150.0);
        kit::grid(ui, min_w, &tiles, |ui, w, (label, n, col)| {
            kit::tile(ui, label, w, 78.0, false, |ui| {
                ui.label(RichText::new(label).size(13.0).color(theme::TEXT_2));
                ui.label(RichText::new(n.to_string()).size(24.0).strong().color(*col));
            });
        });
    }

    fn all_teams(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        let teams: Vec<RunningCandidate> = self
            .candidates
            .iter()
            .filter(|t| !t.deleted)
            .cloned()
            .collect();
        kit::heading(
            ui,
            Icon::Users,
            &c.tr.t("common.layout.menu.teams"),
            Some(teams.len()),
        );
        ui.add_space(8.0);
        if teams.is_empty() {
            if self.teams_rx.is_some() && self.teams_at.is_none_or(|t| t.elapsed() < LOAD_GRACE) {
                kit::skeleton_tiles(ui, 3, 300.0, 110.0, &c.tr.t("team.list.loading"));
            } else {
                kit::empty_state(
                    ui,
                    Icon::Users,
                    &c.tr.t("team.list.empty.title"),
                    &c.tr.t("team.list.empty.description"),
                );
            }
            return;
        }
        let mut open: Option<String> = None;
        let mut launch: Option<String> = None;
        kit::grid(ui, 300.0, &teams, |ui, w, t| {
            let col = running_teams::status_color(t.status);
            let r = kit::tile(ui, ("dash-team", &t.dir), w, 112.0, false, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 4.0, col);
                    ui.add(
                        egui::Label::new(RichText::new(&t.display_name).size(15.0).strong())
                            .truncate(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        theme::pill(ui, c.tr.t(running_teams::status_key(t.status)), col);
                    });
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    theme::pill(ui, format!("{} pending", t.counts.pending), theme::TEXT_2);
                    theme::pill(
                        ui,
                        format!("{} active", t.counts.in_progress),
                        theme::WARNING,
                    );
                    theme::pill(ui, format!("{} done", t.counts.completed), theme::COL_DONE);
                });
                if t.status == RunStatus::Offline {
                    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                        if theme::pill_icon_button(
                            ui,
                            Some(Icon::Play),
                            &c.tr.t("team.list.actions.launchTeam"),
                            false,
                        )
                        .clicked()
                        {
                            launch = Some(t.dir.clone());
                        }
                    });
                }
            });
            if r.clicked() {
                open = Some(t.dir.clone());
            }
        });
        if let Some(d) = launch {
            c.req.push(ShellRequest::LaunchTeam(d));
        } else if let Some(d) = open {
            c.req.push(ShellRequest::OpenTeam(d));
        }
    }
}

impl Screen for Dashboard {
    fn id(&self) -> TabId {
        TabId::Dashboard
    }

    fn title_key(&self) -> &'static str {
        "team.list.title"
    }

    fn tick(&mut self, ctx: &egui::Context, c: &mut FeatureCtx) {
        self.poll();
        self.start_jobs(ctx, c.home);
        ctx.request_repaint_after(TEAMS_REFRESH);
    }

    fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        // Also correct when `tick` was not called (headless tests, first frame).
        self.poll();
        self.start_jobs(ui.ctx(), c.home);
        let mut actions = self.header(ui, c);
        let ctx0 = ui.ctx().clone();
        if let Some(n) =
            kit::create_team_dialog(&ctx0, c.tr, &mut self.create_open, &mut self.new_name)
        {
            c.req.push(ShellRequest::OpenTeam(n));
            self.teams_at = None;
        }
        let running = running_teams::build_running_teams(self.candidates.clone());
        let counts: HashMap<String, TaskCounts> = self
            .candidates
            .iter()
            .map(|t| (t.dir.clone(), t.counts))
            .collect();
        // Attach teams to recent projects by project path when the history did not.
        let recent: Vec<RecentProject> = self
            .recent
            .iter()
            .cloned()
            .map(|mut p| {
                if p.team.is_none() {
                    p.team = self
                        .candidates
                        .iter()
                        .find(|t| t.project_path.as_deref() == Some(p.path.as_str()) && !t.deleted)
                        .map(|t| t.dir.clone());
                }
                p
            })
            .collect();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.stats(ui, c);
                ui.add_space(10.0);
                running_teams::show(ui, c, &running);
                // Never a perpetual spinner: after the grace period an unfinished scan reads as
                // "no recent projects" and fills in when the worker lands.
                let loading = self.recent_rx.is_some()
                    && recent.is_empty()
                    && self.recent_at.is_none_or(|t| t.elapsed() < LOAD_GRACE);
                actions.extend(self.recent_section.show(ui, c, &recent, &counts, loading));
                ui.add_space(14.0);
                self.all_teams(ui, c);
                ui.add_space(24.0);
            });
        self.handle(c, actions);

        let ctx = ui.ctx().clone();
        let home: PathBuf = c.home.to_path_buf();
        if let Some(d) = self.template.show(&ctx, c.tr, &home, &recent) {
            self.draft = Some(d);
        }
        if let Some(d) = self.import.show(&ctx, c.tr, &home, &recent) {
            self.draft = Some(d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{testing, Intents};

    fn run_frames(d: &mut Dashboard, home: &std::path::Path, req: &mut Vec<ShellRequest>) {
        let tr = rteams_services::i18n::I18n::default();
        let mut out = Intents::default();
        testing::render(|ui| {
            let mut c = FeatureCtx {
                home,
                team: None,
                view: None,
                out: &mut out,
                tr: &tr,
                req,
            };
            d.tick(&ui.ctx().clone(), &mut c);
            d.show(ui, &mut c);
        });
    }

    #[test]
    fn dashboard_renders_with_teams_and_projects() {
        let (home, _data) = testing::fixture("dashboard");
        let mut d = Dashboard::new().with_sources(Sources::default());
        d.set_running(["t".to_string()].into(), HashSet::new());
        let mut req = Vec::new();
        run_frames(&mut d, &home, &mut req);
        // Let the workers finish, then draw again with data present.
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(20));
            d.poll();
            if d.teams_rx.is_none() && d.recent_rx.is_none() {
                break;
            }
        }
        assert_eq!(d.candidates.len(), 1);
        assert_eq!(d.candidates[0].status, RunStatus::Active);
        run_frames(&mut d, &home, &mut req);
        assert!(req.is_empty());
        assert!(d.take_draft().is_none());
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn empty_home_renders() {
        let home = std::env::temp_dir().join(format!("rteams-dash-empty-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let mut d = Dashboard::new().with_sources(Sources::default());
        let mut req = Vec::new();
        run_frames(&mut d, &home, &mut req);
        let _ = std::fs::remove_dir_all(home);
    }

    /// Renders the recent-projects grid at several widths into `$RTEAMS_SHOTS_OUT`
    /// (`dash_recent_<w>.png`): `cargo test -p rteams-gui dashboard_recent_sizes -- --ignored`.
    #[test]
    #[ignore]
    fn dashboard_recent_sizes() {
        use rteams_services::projects::RecentProject;
        let out = std::path::PathBuf::from(std::env::var("RTEAMS_SHOTS_OUT").expect("out dir"));
        std::fs::create_dir_all(&out).unwrap();
        let now = recent_projects::now_ms();
        let mk =
            |label: &str, path: &str, ago: i64, prov: &[&str], team: Option<&str>| RecentProject {
                identity: path.into(),
                path: path.into(),
                label: label.into(),
                last_activity_ms: now - ago,
                providers: prov.iter().map(|s| s.to_string()).collect(),
                exists: true,
                team: team.map(Into::into),
            };
        let items = vec![
            mk(
                "Desktop",
                "/Users/me/Projects",
                180_000,
                &["claude", "codex"],
                None,
            ),
            mk(
                "rteams",
                "/Users/me/Projects/rteams",
                300_000,
                &["claude"],
                Some("demo"),
            ),
            mk(
                "tmp.CdL5SLbpRR",
                "/private/var/folders/tc/8yhmrrx108g1r63w6rnc1jt8000/T/tmp.CdL5SLbpRR",
                50_400_000,
                &["claude", "codex", "rteams"],
                None,
            ),
        ];
        let counts = HashMap::from([(
            "demo".to_string(),
            TaskCounts {
                pending: 2,
                in_progress: 1,
                completed: 5,
            },
        )]);
        let tr = rteams_services::i18n::I18n::english();
        for w in [1100.0, 720.0] {
            let mut section = recent_projects::RecentSection::new();
            let mut req = Vec::new();
            let mut intents = crate::view::Intents::default();
            let home = std::env::temp_dir();
            let mut h = egui_kittest::Harness::builder()
                .with_size(egui::vec2(w, 700.0))
                .wgpu()
                .build_ui(|ui| {
                    crate::theme::apply(ui.ctx());
                    let mut c = FeatureCtx {
                        home: &home,
                        team: None,
                        view: None,
                        out: &mut intents,
                        tr: &tr,
                        req: &mut req,
                    };
                    egui::Frame::new().inner_margin(20).show(ui, |ui| {
                        section.show(ui, &mut c, &items, &counts, false);
                    });
                });
            h.run_steps(4);
            h.render()
                .unwrap()
                .save(out.join(format!("dash_recent_{}.png", w as u32)))
                .unwrap();
        }
    }
}
