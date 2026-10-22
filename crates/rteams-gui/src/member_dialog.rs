//! Member detail dialog (centered modal): header with avatar, role and status, action buttons,
//! and Tasks / Logs / Messages tabs. Views only push `Intent`s; the shell acts on them.

use std::path::PathBuf;

use eframe::egui::{self, Color32, RichText};
use rteams_core::{Event, EventKind, Scope, Status};

use crate::actions::GuiCmd;
use crate::launcher::AgentStatus;
use crate::providers::{Probe, RuntimeInfo};
use crate::runs::{self, RunEvent, RunKind, SessionSummary};
use crate::runtimes;
use crate::team::status_chip;
use crate::theme::{self, Presence};
use crate::ui::launch_dialog::{parse_limit, EFFORTS};
use crate::ui::mcp_policy::McpPolicy;
use crate::view::{Intent, Intents, View};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Tasks,
    Logs,
    Messages,
    Stats,
    Sessions,
    Changes,
    Edit,
}

/// Form state of the Edit tab, prefilled from the roster when a member is opened.
#[derive(Debug, Default)]
struct EditState {
    /// Member the fields were loaded for.
    loaded: Option<String>,
    role: String,
    model: String,
    prompt: String,
    /// Runtime id; `None` = the default (Claude Code).
    runtime: Option<String>,
    worktree: bool,
    /// Picked avatar icon; empty = the default from `names::default_icon`.
    icon: String,
    /// Saved launch settings: effort (`None` = default), fast mode, context-limit text.
    effort: Option<String>,
    fast: bool,
    limit: String,
    confirm_remove: bool,
}

const MODELS: [&str; 4] = ["default", "sonnet", "opus", "haiku"];

impl EditState {
    fn load(&mut self, v: &View, name: &str) {
        if self.loaded.as_deref() == Some(name) {
            return;
        }
        let m = v.members().iter().find(|m| m.name == name);
        self.loaded = Some(name.to_string());
        self.role = m.map(|m| m.role.clone()).unwrap_or_default();
        self.model = m.and_then(|m| m.model.clone()).unwrap_or_default();
        self.prompt = m.and_then(|m| m.prompt.clone()).unwrap_or_default();
        self.runtime = m.and_then(|m| m.runtime.clone());
        self.worktree = m.is_some_and(|m| m.worktree);
        self.icon = m.and_then(|m| m.icon.clone()).unwrap_or_default();
        self.effort = m.and_then(|m| m.effort.clone());
        self.fast = m.is_some_and(|m| m.fast);
        self.limit = m
            .and_then(|m| m.context_limit)
            .map_or_else(String::new, |n| n.to_string());
        self.confirm_remove = false;
    }

    /// The `UpdateMember` for what differs from the roster, or `None` when nothing changed.
    fn update(&self, v: &View, name: &str) -> Option<GuiCmd> {
        let m = v.members().iter().find(|m| m.name == name)?;
        let role = self.role.trim();
        let model = self.model.trim();
        let prompt = self.prompt.trim();
        let role = (!role.is_empty() && role != m.role).then(|| role.to_string());
        let model = (model != m.model.as_deref().unwrap_or("")).then(|| model.to_string());
        let prompt =
            (prompt != m.prompt.as_deref().unwrap_or("").trim()).then(|| prompt.to_string());
        let icon = (self.icon != m.icon.as_deref().unwrap_or("")).then(|| self.icon.clone());
        if role.is_none() && model.is_none() && prompt.is_none() && icon.is_none() {
            return None;
        }
        Some(GuiCmd::UpdateMember {
            name: name.to_string(),
            role,
            prompt,
            model,
            icon,
        })
    }
}

impl EditState {
    /// The `SetMemberLaunch` for changed effort / fast / context limit, or `None` (also when the
    /// limit text is invalid).
    fn launch_cmd(&self, v: &View, name: &str) -> Option<GuiCmd> {
        let m = v.members().iter().find(|m| m.name == name)?;
        let limit = parse_limit(&self.limit).ok()?;
        let effort = (self.effort != m.effort).then(|| self.effort.clone().unwrap_or_default());
        let fast = (self.fast != m.fast).then_some(self.fast);
        let context_limit = (limit != m.context_limit).then(|| limit.unwrap_or(0));
        if effort.is_none() && fast.is_none() && context_limit.is_none() {
            return None;
        }
        Some(GuiCmd::SetMemberLaunch {
            name: name.to_string(),
            effort,
            fast,
            context_limit,
        })
    }

    /// The `SetMemberRuntime` for a changed runtime / worktree flag, or `None`.
    fn runtime_cmd(&self, v: &View, name: &str) -> Option<GuiCmd> {
        let m = v.members().iter().find(|m| m.name == name)?;
        let runtime = (self.runtime != m.runtime).then(|| self.runtime.clone().unwrap_or_default());
        let worktree = (self.worktree != m.worktree).then_some(self.worktree);
        if runtime.is_none() && worktree.is_none() {
            return None;
        }
        Some(GuiCmd::SetMemberRuntime {
            name: name.to_string(),
            runtime,
            worktree,
        })
    }
}

/// Facts derived from the event log for one member.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EventCounts {
    pub tasks_done: usize,
    pub reviews_done: usize,
    pub comments: usize,
    pub messages: usize,
}

/// Counts of what `name` did, from the events they authored.
pub fn event_counts(events: &[Event], name: &str) -> EventCounts {
    let mut c = EventCounts::default();
    for e in events.iter().filter(|e| e.actor == name) {
        match &e.kind {
            EventKind::TaskTransitioned {
                to: Status::Done | Status::Approved,
                ..
            } => c.tasks_done += 1,
            EventKind::ReviewApproved { .. } | EventKind::ReviewChangesRequested { .. } => {
                c.reviews_done += 1
            }
            EventKind::TaskCommented { .. } => c.comments += 1,
            EventKind::MessageSent { .. } => c.messages += 1,
            _ => {}
        }
    }
    c
}

/// Time of the member's last authored event (unix ms).
pub fn last_activity(events: &[Event], name: &str) -> Option<i64> {
    events
        .iter()
        .filter(|e| e.actor == name)
        .map(|e| e.ts)
        .max()
}

/// `YYYY-MM-DD HH:MM` (UTC) from unix millis.
pub fn date_time(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
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
        rem / 3600,
        rem % 3600 / 60
    )
}

/// Compact token count: `950`, `12.3k`, `1.2M`.
pub fn compact(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

/// Session list and the open transcript, loaded lazily and dropped on tab or member change.
#[derive(Debug, Default)]
struct Panels {
    home: PathBuf,
    team: String,
    sessions: Option<Vec<SessionSummary>>,
    stats: Option<runs::Stats>,
    open_run: Option<(PathBuf, Vec<RunEvent>)>,
    preview: Option<(Vec<String>, Vec<String>)>,
    repo: Option<PathBuf>,
    /// Files this member changed across their tasks, computed on a worker thread.
    changes: Option<Vec<crate::changes::AgentFile>>,
    changes_job: Option<crate::changes::Job<Vec<crate::changes::AgentFile>>>,
    /// Paths the Changes tab asked the review dialog to open.
    review_req: Option<Vec<String>>,
}

impl Panels {
    fn reset(&mut self) {
        self.sessions = None;
        self.stats = None;
        self.open_run = None;
        self.preview = None;
        self.changes = None;
        self.changes_job = None;
        self.review_req = None;
    }
}

/// The member dialog. `show` is a no-op while closed.
#[derive(Debug, Default)]
pub struct MemberDialog {
    member: Option<String>,
    tab: Tab,
    edit: EditState,
    panels: Panels,
    probe: Probe,
    mcp: McpPolicy,
    review: crate::ui::review::ReviewDialog,
}

fn status_pill(tr: &rteams_services::i18n::I18n, s: Status) -> (String, Color32) {
    crate::task_dialog::status_pill(tr, s)
}

impl MemberDialog {
    /// The shell tells the dialog where runs are recorded (call on start and team switch).
    pub fn set_paths(&mut self, home: PathBuf, team: String) {
        if self.panels.home != home || self.panels.team != team {
            self.panels.home = home;
            self.panels.team = team;
            self.panels.reset();
        }
    }

    /// The project repository the Changes tab diffs against.
    pub fn set_repo(&mut self, repo: Option<PathBuf>) {
        if self.panels.repo != repo {
            self.panels.repo = repo;
            self.panels.changes = None;
            self.panels.changes_job = None;
        }
    }

    pub fn open(&mut self, name: String) {
        if self.member.as_deref() != Some(name.as_str()) {
            self.tab = Tab::Tasks;
            self.panels.reset();
        }
        self.member = Some(name);
    }

    #[cfg(test)]
    pub fn show_edit_tab_for_tests(&mut self) {
        self.tab = Tab::Edit;
    }

    pub fn close(&mut self) {
        self.member = None;
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.member.is_some()
    }

    pub fn show(&mut self, ctx: &egui::Context, v: &View, out: &mut Intents) {
        let Some(name) = self.member.clone() else {
            return;
        };
        let screen = ctx.content_rect();
        let width = (screen.width() * 0.92).clamp(1.0, 900.0);
        self.edit.load(v, &name);
        self.probe.ensure(ctx);
        let infos = self.probe.get().unwrap_or_default();
        let tab = &mut self.tab;
        let edit = &mut self.edit;
        let panels = &mut self.panels;
        let mcp = &mut self.mcp;
        let mut open = true;
        let mut close = false;
        crate::forms::dialog(ctx, ("member_dialog", &name), "", width, &mut open, |ui| {
            close = body(ui, v, &name, tab, edit, panels, mcp, &infos, out);
        });
        if close || !open {
            self.close();
        }
        if let Some(paths) = self.panels.review_req.take() {
            self.review.open(paths);
        }
        if let Some(repo) = self.panels.repo.as_deref() {
            let task = format!("agent-{name}");
            let rc = crate::ui::review::ReviewCtx {
                home: &self.panels.home,
                team: &self.panels.team,
                repo,
                task: &task,
            };
            let was_open = self.review.is_open();
            self.review.show(ctx, v, &rc, out);
            if was_open && self.review.is_open() {
                // Rejected hunks change the working tree; recount on the next draw.
                self.panels.changes = None;
            }
        }
    }
}

/// Draws the dialog; returns true when it should close.
#[allow(clippy::too_many_arguments)]
fn body(
    ui: &mut egui::Ui,
    v: &View,
    name: &str,
    tab: &mut Tab,
    edit: &mut EditState,
    panels: &mut Panels,
    mcp: &mut McpPolicy,
    infos: &[RuntimeInfo],
    out: &mut Intents,
) -> bool {
    let mut close = false;
    let role = v
        .members()
        .iter()
        .find(|m| m.name == name)
        .map_or("member", |m| m.role.as_str());
    let is_human = Some(name) == v.actor;
    let removed = v.members().iter().any(|m| m.name == name && m.removed);
    let agent = v.agent(name);
    let running = agent.is_some_and(|a| matches!(a.status, AgentStatus::Running { .. }));
    let (chip, chip_color) = if removed {
        (
            v.tr.t("team.memberDraft.removed").to_lowercase(),
            theme::MUTED,
        )
    } else if is_human {
        (
            v.tr.t("team.messages.chats.you").to_lowercase(),
            theme::ACCENT,
        )
    } else {
        status_chip(agent.map(|a| &a.status))
    };
    let color = v.color(name);
    let presence = (!is_human).then_some(if running {
        Presence::Online
    } else {
        Presence::Offline
    });

    ui.horizontal(|ui| {
        theme::avatar(ui, name, color, 48.0, presence);
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(name).size(20.0).strong().color(color));
                theme::pill(ui, chip, chip_color);
                if !is_human {
                    if let Some((d, tip)) = crate::team::member_diag(ui, v, name) {
                        theme::pill(ui, v.tr.t(d.key()), d.color()).on_hover_text(tip);
                    }
                }
            });
            let rt = v
                .members()
                .iter()
                .find(|m| m.name == name)
                .and_then(|m| m.runtime.as_deref());
            let label = runtimes::get(rt).map_or("Claude Code", |r| r.label());
            ui.label(RichText::new(format!("{label} \u{b7} {role}")).color(theme::TEXT_2));
            if !is_human && !running {
                if let Some(ts) = last_activity(v.data.events(), name) {
                    ui.label(
                        RichText::new(
                            v.tr.t_args("rteams.member.idleSince", &[("time", &theme::hhmm(ts))]),
                        )
                        .small()
                        .color(theme::MUTED),
                    );
                }
            }
        });
    });
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        if theme::solid(ui, &v.tr.t("team.members.detail.sendMessage"), theme::INFO).clicked() {
            out.push(Intent::OpenChat(Some(name.to_string())));
            close = true;
        }
        if !removed
            && ui
                .button(v.tr.t("team.members.detail.assignTask"))
                .clicked()
        {
            out.push(Intent::NewTask {
                owner: Some(name.to_string()),
            });
        }
        if !is_human && !removed {
            if running {
                if theme::solid(ui, &v.tr.t("team.detail.actions.stop"), theme::ERROR).clicked() {
                    out.push(Intent::Stop(name.to_string()));
                }
                if ui.button(v.tr.t("team.members.detail.restart")).clicked() {
                    out.push(Intent::Restart(name.to_string()));
                }
            } else if theme::solid(ui, &v.tr.t("team.detail.actions.launch"), theme::SUCCESS)
                .clicked()
            {
                out.push(Intent::Launch(name.to_string()));
            }
        }
    });
    ui.add_space(8.0);

    let tasks: Vec<_> = v
        .state
        .tasks
        .iter()
        .filter(|t| !t.archived && t.owner.as_deref() == Some(name))
        .collect();
    let msgs: Vec<_> = v
        .state
        .messages
        .iter()
        .filter(|m| match &m.scope {
            Scope::Direct { to } => m.from == name || to == name,
            _ => false,
        })
        .collect();
    let lines: &[String] = agent.map_or(&[], |a| a.last_lines.as_slice());
    ui.horizontal(|ui| {
        for (t, label, n) in [
            (Tab::Tasks, v.tr.t("common.tasksPanel.title"), tasks.len()),
            (
                Tab::Logs,
                v.tr.t("common.layout.sections.claudeLogs"),
                lines.len(),
            ),
            (Tab::Messages, v.tr.t("team.messages.title"), msgs.len()),
            (Tab::Stats, v.tr.t("rteams.member.stats"), 0),
            (Tab::Sessions, v.tr.t("common.layout.sections.sessions"), 0),
            (Tab::Changes, v.tr.t("team.taskDetail.changes.title"), 0),
            (Tab::Edit, v.tr.t("common.schedules.actions.edit"), 0),
        ] {
            let text = if matches!(t, Tab::Edit | Tab::Stats | Tab::Sessions | Tab::Changes) {
                label.clone()
            } else {
                format!("{label} ({n})")
            };
            if ui.selectable_label(*tab == t, text).clicked() {
                *tab = t;
            }
        }
    });
    theme::rule(ui);
    ui.add_space(6.0);

    match *tab {
        Tab::Tasks => {
            if tasks.is_empty() {
                ui.label(RichText::new(v.tr.t("rteams.member.noTasks")).color(theme::MUTED));
            }
            for t in tasks {
                ui.horizontal_wrapped(|ui| {
                    let (l, c) = status_pill(v.tr, t.status);
                    let r = ui.add(
                        egui::Label::new(
                            RichText::new(format!("#{}", theme::short_id(&t.id)))
                                .color(theme::TEXT_2),
                        )
                        .sense(egui::Sense::click()),
                    );
                    let s = ui.add(egui::Label::new(&t.subject).sense(egui::Sense::click()));
                    theme::pill(ui, l, c);
                    if r.clicked() || s.clicked() {
                        out.push(Intent::OpenTask(t.id.clone()));
                    }
                });
            }
        }
        Tab::Logs => {
            if lines.is_empty() {
                ui.label(RichText::new(v.tr.t("rteams.task.noLogs")).color(theme::MUTED));
            } else {
                egui::Frame::new()
                    .fill(theme::CODE_BG)
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width().max(1.0));
                        let from = lines.len().saturating_sub(200);
                        for l in &lines[from..] {
                            ui.add(
                                egui::Label::new(RichText::new(l).monospace().color(theme::TEXT_2))
                                    .wrap(),
                            );
                        }
                    });
            }
        }
        Tab::Edit => edit_tab(ui, v, name, is_human, removed, edit, mcp, infos, out),
        Tab::Stats => stats_tab(ui, v, name, panels),
        Tab::Sessions => sessions_tab(ui, v.tr, name, panels),
        Tab::Changes => changes_tab(ui, v.tr, name, panels),
        Tab::Messages => {
            if msgs.is_empty() {
                ui.label(
                    RichText::new(v.tr.t("team.activity.timeline.noMessages")).color(theme::MUTED),
                );
            }
            for m in msgs {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(theme::hhmm(m.ts)).small().color(theme::MUTED));
                    ui.label(RichText::new(&m.from).strong().color(v.color(&m.from)));
                    ui.add(egui::Label::new(&m.text).wrap());
                });
            }
        }
    }
    close
}

fn stat_row(ui: &mut egui::Ui, label: String, value: String) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(theme::TEXT_2));
        ui.label(RichText::new(value).strong());
    });
}

/// Every file this member changed across their tasks (from their run logs), with +/- counts.
/// Computed on a worker thread; a row opens the review dialog on that file.
fn changes_tab(
    ui: &mut egui::Ui,
    tr: &rteams_services::i18n::I18n,
    name: &str,
    panels: &mut Panels,
) {
    let Some(repo) = panels.repo.clone() else {
        ui.label(RichText::new(tr.t("rteams.task.noGit")).color(theme::MUTED));
        return;
    };
    if let Some(job) = &panels.changes_job {
        if let Some(done) = job.take() {
            panels.changes = Some(done);
            panels.changes_job = None;
        }
    }
    if panels.changes.is_none() && panels.changes_job.is_none() {
        let (home, team, member) = (panels.home.clone(), panels.team.clone(), name.to_string());
        panels.changes_job = Some(crate::changes::Job::spawn(ui.ctx(), move || {
            crate::changes::agent_changes(&home, &team, &repo, &member)
        }));
    }
    let Some(files) = panels.changes.clone() else {
        ui.label(RichText::new(tr.t("team.taskDetail.changes.loading")).color(theme::MUTED));
        return;
    };
    if files.is_empty() {
        ui.label(
            RichText::new(tr.t("team.taskDetail.changes.empty.noFileChangesRecorded"))
                .color(theme::MUTED),
        );
        return;
    }
    let add: usize = files.iter().map(|f| f.added).sum();
    let del: usize = files.iter().map(|f| f.removed).sum();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(tr.t_count("team.taskDetail.changes.fileCount", files.len() as u64))
                .small()
                .color(theme::MUTED),
        );
        ui.label(
            RichText::new(format!("+{add}"))
                .small()
                .color(theme::SUCCESS),
        );
        ui.label(RichText::new(format!("-{del}")).small().color(theme::ERROR));
        if theme::pill_button(ui, &tr.t("team.taskDetail.changes.reviewDiff"), true).clicked() {
            panels.review_req = Some(files.iter().map(|f| f.path.clone()).collect());
        }
        if ui
            .small_button(tr.t("team.taskDetail.changes.refreshShort"))
            .clicked()
        {
            panels.changes = None;
        }
    });
    ui.add_space(4.0);
    for f in &files {
        ui.horizontal_wrapped(|ui| {
            let r = ui.add(
                egui::Label::new(RichText::new(&f.path).monospace()).sense(egui::Sense::click()),
            );
            ui.label(
                RichText::new(format!("+{}", f.added))
                    .small()
                    .color(theme::SUCCESS),
            );
            ui.label(
                RichText::new(format!("-{}", f.removed))
                    .small()
                    .color(theme::ERROR),
            );
            let ids: Vec<String> = f
                .tasks
                .iter()
                .map(|t| format!("#{}", theme::short_id(t)))
                .collect();
            ui.label(RichText::new(ids.join(" ")).small().color(theme::MUTED));
            if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                panels.review_req = Some(vec![f.path.clone()]);
            }
        });
    }
}

fn stats_tab(ui: &mut egui::Ui, v: &View, name: &str, panels: &mut Panels) {
    let stats = *panels
        .stats
        .get_or_insert_with(|| runs::member_stats(&panels.home, &panels.team, name));
    let c = event_counts(v.data.events(), name);
    let (latest, raw) = panels.preview.get_or_insert_with(|| {
        (
            crate::member_preview::latest_activity(&panels.home, &panels.team, name),
            crate::member_preview::runtime_tail(&panels.home, &panels.team, name, 20),
        )
    });
    if !latest.is_empty() {
        ui.label(RichText::new("Latest activity").strong());
        for l in latest.iter() {
            ui.label(RichText::new(l).monospace().color(theme::TEXT_2));
        }
        egui::CollapsingHeader::new("Runtime log tail")
            .id_salt("member_runtime_tail")
            .show(ui, |ui| {
                for l in raw.iter() {
                    let t: String = l.chars().take(300).collect();
                    ui.label(RichText::new(t).monospace().small().color(theme::MUTED));
                }
            });
        ui.add_space(8.0);
    }
    ui.label(RichText::new(v.tr.t("rteams.member.runs")).strong());
    stat_row(
        ui,
        v.tr.t("rteams.member.agentRuns"),
        stats.runs.to_string(),
    );
    stat_row(
        ui,
        v.tr.t("team.members.stats.toolCalls"),
        stats.tool_calls.to_string(),
    );
    stat_row(
        ui,
        v.tr.t("rteams.member.tokensIn"),
        compact(stats.tokens_in),
    );
    stat_row(
        ui,
        v.tr.t("rteams.member.tokensOut"),
        compact(stats.tokens_out),
    );
    ui.add_space(8.0);
    ui.label(RichText::new(v.tr.t("team.agentGraph.activityHud.activity")).strong());
    stat_row(
        ui,
        v.tr.t("rteams.member.tasksDone"),
        c.tasks_done.to_string(),
    );
    stat_row(
        ui,
        v.tr.t("rteams.member.reviewsDone"),
        c.reviews_done.to_string(),
    );
    stat_row(
        ui,
        v.tr.t("team.taskDetail.sections.comments"),
        c.comments.to_string(),
    );
    stat_row(ui, v.tr.t("team.messages.title"), c.messages.to_string());
}

fn transcript_row(ui: &mut egui::Ui, e: &RunEvent) {
    let (tag, color) = match &e.kind {
        RunKind::Thinking => ("thinking", theme::THINK_FG),
        RunKind::ToolUse { name, .. } => (name.as_str(), theme::TOOL_FG),
        RunKind::ToolResult { ok: true } => ("result", theme::RESULT_FG),
        RunKind::ToolResult { ok: false } => ("error", theme::ERROR_FG),
        RunKind::Result { .. } => ("done", theme::RESULT_FG),
        RunKind::Text => ("text", theme::TEXT_2),
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(tag).monospace().strong().color(color));
        let text: String = e.text.chars().take(400).collect();
        ui.add(egui::Label::new(RichText::new(text).monospace().color(theme::TEXT_2)).wrap());
    });
}

fn sessions_tab(
    ui: &mut egui::Ui,
    tr: &rteams_services::i18n::I18n,
    name: &str,
    panels: &mut Panels,
) {
    let sessions = panels
        .sessions
        .get_or_insert_with(|| runs::sessions(&panels.home, &panels.team, name))
        .clone();
    if sessions.is_empty() {
        ui.label(RichText::new(tr.t("rteams.member.noSessions")).color(theme::MUTED));
        return;
    }
    for s in &sessions {
        let open = panels.open_run.as_ref().is_some_and(|(p, _)| *p == s.path);
        let head = format!(
            "{} {}   {}",
            if open { "\u{23f7}" } else { "\u{25b6}" },
            date_time(s.started_ms),
            tr.t_args(
                "rteams.member.sessionHead",
                &[
                    ("events", &s.events.to_string()),
                    ("tools", &s.tool_calls.to_string()),
                    ("tin", &compact(s.tokens_in)),
                    ("tout", &compact(s.tokens_out)),
                ],
            )
        );
        if ui
            .add(egui::Label::new(RichText::new(head)).sense(egui::Sense::click()))
            .clicked()
        {
            panels.open_run = if open {
                None
            } else {
                Some((s.path.clone(), runs::read_run(&s.path)))
            };
        }
        if let Some((p, events)) = &panels.open_run {
            if *p == s.path {
                egui::Frame::new()
                    .fill(theme::CODE_BG)
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width().max(1.0));
                        if events.is_empty() {
                            ui.label(
                                RichText::new(tr.t("rteams.member.emptyRun")).color(theme::MUTED),
                            );
                        }
                        for e in events.iter().take(300) {
                            transcript_row(ui, e);
                        }
                        if events.len() > 300 {
                            ui.label(
                                RichText::new(tr.t_args(
                                    "rteams.member.more",
                                    &[("n", &(events.len() - 300).to_string())],
                                ))
                                .color(theme::MUTED),
                            );
                        }
                    });
            }
        }
        ui.add_space(4.0);
    }
}

/// Avatar icon choices: "Default" (the name's own icon) plus `names::PICKER`, wrapped in rows.
fn icon_picker(ui: &mut egui::Ui, v: &View, name: &str, icon: &mut String) {
    ui.label(RichText::new(v.tr.t("rteams.member.icon")).color(theme::TEXT_2));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        let default = format!(
            "{}  {}",
            crate::names::default_icon(name),
            v.tr.t("rteams.member.iconDefault")
        );
        if ui.selectable_label(icon.is_empty(), default).clicked() {
            icon.clear();
        }
        for &choice in crate::names::PICKER {
            let picked = icon == choice;
            let text = RichText::new(choice).size(18.0);
            if ui
                .add_sized([30.0, 30.0], egui::Button::selectable(picked, text))
                .clicked()
            {
                *icon = choice.to_string();
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn edit_tab(
    ui: &mut egui::Ui,
    v: &View,
    name: &str,
    is_human: bool,
    removed: bool,
    edit: &mut EditState,
    mcp: &mut McpPolicy,
    infos: &[RuntimeInfo],
    out: &mut Intents,
) {
    let can_write = v.actor.is_some();
    icon_picker(ui, v, name, &mut edit.icon);
    ui.add_space(6.0);
    ui.label(RichText::new(v.tr.t("rteams.addMember.role")).color(theme::TEXT_2));
    ui.add(egui::TextEdit::singleline(&mut edit.role).desired_width(f32::INFINITY));
    ui.add_space(6.0);
    ui.label(
        RichText::new(v.tr.t("team.terminalWorkspace.settingsRuntimeTitle")).color(theme::TEXT_2),
    );
    let all = runtimes::all();
    let default_id = all.first().map(|r| r.id());
    let current_owned: Option<String> = edit
        .runtime
        .clone()
        .or_else(|| default_id.map(str::to_string));
    let current = current_owned.as_deref();
    let label_of = |id: &str| -> String {
        let label = all
            .iter()
            .find(|r| r.id() == id)
            .map_or(id.to_string(), |r| r.label().to_string());
        let missing = infos.iter().any(|i| i.id == id && !i.installed());
        if missing {
            format!("{label} ({})", v.tr.t("rteams.member.notInstalled"))
        } else {
            label
        }
    };
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("member-runtime")
            .selected_text(current.map_or("Claude Code".to_string(), label_of))
            .show_ui(ui, |ui| {
                for r in &all {
                    let value = (Some(r.id()) != default_id).then(|| r.id().to_string());
                    if ui
                        .selectable_label(current == Some(r.id()), label_of(r.id()))
                        .clicked()
                    {
                        edit.runtime = value;
                    }
                }
            });
        ui.checkbox(&mut edit.worktree, v.tr.t("rteams.member.worktree"));
    });
    ui.add_space(6.0);
    ui.label(RichText::new(v.tr.t("common.tokens.model")).color(theme::TEXT_2));
    let models: Vec<(String, String)> = match infos.iter().find(|i| Some(i.id.as_str()) == current)
    {
        Some(i) if !i.models.is_empty() => i
            .models
            .iter()
            .map(|m| (m.id.clone(), m.label.clone()))
            .collect(),
        _ => MODELS
            .iter()
            .map(|m| (m.to_string(), m.to_string()))
            .collect(),
    };
    ui.horizontal(|ui| {
        let shown = if edit.model.is_empty() {
            v.tr.t("rteams.member.modelDefault")
        } else {
            models
                .iter()
                .find(|(id, _)| *id == edit.model)
                .map_or(edit.model.clone(), |(_, l)| l.clone())
        };
        egui::ComboBox::from_id_salt("member-model")
            .selected_text(shown)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(edit.model.is_empty(), v.tr.t("rteams.member.modelDefault"))
                    .clicked()
                {
                    edit.model.clear();
                }
                for (id, label) in &models {
                    if id == "default" {
                        continue;
                    }
                    if ui.selectable_label(edit.model == *id, label).clicked() {
                        edit.model = id.clone();
                    }
                }
            });
        ui.add(
            egui::TextEdit::singleline(&mut edit.model)
                .hint_text(v.tr.t("rteams.member.modelName"))
                .desired_width(ui.available_width().clamp(40.0, 220.0)),
        );
    });
    ui.add_space(6.0);
    ui.label(RichText::new(v.tr.t("rteams.member.standingPrompt")).color(theme::TEXT_2));
    ui.add(
        egui::TextEdit::multiline(&mut edit.prompt)
            .hint_text(v.tr.t("rteams.member.standingHint"))
            .desired_rows(4)
            .desired_width(f32::INFINITY),
    );
    ui.add_space(6.0);
    ui.label(RichText::new(v.tr.t("team.effortLevel.label")).color(theme::TEXT_2));
    ui.horizontal(|ui| {
        if ui
            .selectable_label(edit.effort.is_none(), v.tr.t("rteams.launch.effortDefault"))
            .clicked()
        {
            edit.effort = None;
        }
        for e in EFFORTS {
            let picked = edit.effort.as_deref() == Some(e);
            if ui
                .selectable_label(picked, v.tr.t(&format!("rteams.launch.effort.{e}")))
                .clicked()
            {
                edit.effort = Some(e.to_string());
            }
        }
    });
    ui.checkbox(
        &mut edit.fast,
        v.tr.t("team.modelSelector.fastMode.optionalLabel"),
    )
    .on_hover_text(v.tr.t("rteams.launch.fastHint"));
    ui.label(RichText::new(v.tr.t("rteams.launch.limit")).color(theme::TEXT_2));
    ui.add(
        egui::TextEdit::singleline(&mut edit.limit)
            .hint_text(v.tr.t("rteams.launch.limitHint"))
            .desired_width(180.0),
    );
    let bad_limit = parse_limit(&edit.limit).is_err();
    if bad_limit {
        ui.label(RichText::new(v.tr.t("rteams.launch.badLimit")).color(theme::ERROR));
    }
    ui.add_space(8.0);
    let update = edit.update(v, name);
    let rt_cmd = edit.runtime_cmd(v, name);
    let launch_cmd = edit.launch_cmd(v, name);
    ui.horizontal(|ui| {
        let ready = can_write && (update.is_some() || rt_cmd.is_some() || launch_cmd.is_some());
        if ui
            .add_enabled_ui(ready, |ui| {
                theme::solid(ui, &v.tr.t("common.actions.save"), theme::INFO)
            })
            .inner
            .clicked()
        {
            for cmd in update.into_iter().chain(rt_cmd).chain(launch_cmd) {
                out.push(Intent::Run(cmd));
            }
        }
    });
    if is_human {
        return;
    }
    ui.add_space(10.0);
    theme::rule(ui);
    ui.add_space(8.0);
    if !removed {
        let m = v.members().iter().find(|m| m.name == name);
        mcp.load(
            name,
            m.and_then(|m| m.runtime.as_deref()),
            m.map_or(&[], |m| m.mcp_allow.as_slice()),
            v.team()
                .and_then(|t| t.project_path.as_deref())
                .filter(|p| !p.trim().is_empty())
                .map(std::path::PathBuf::from),
        );
        if let Some(allow) = mcp.show(ui, v.tr, can_write) {
            out.push(Intent::Run(GuiCmd::SetMemberMcp {
                name: name.to_string(),
                allow,
            }));
        }
        ui.add_space(10.0);
        theme::rule(ui);
        ui.add_space(8.0);
    }
    if removed {
        if theme::solid(ui, &v.tr.t("team.kanban.trash.restore"), theme::SUCCESS).clicked() {
            out.push(Intent::Run(GuiCmd::RemoveMember {
                name: name.to_string(),
                removed: false,
            }));
        }
    } else if !edit.confirm_remove {
        let b = egui::Button::new(
            RichText::new(v.tr.t("rteams.member.removeFromTeam")).color(theme::ERROR),
        )
        .stroke(egui::Stroke::new(1.0, theme::tint(theme::ERROR, 0.5)));
        if ui.add_enabled(can_write, b).clicked() {
            edit.confirm_remove = true;
        }
    } else {
        ui.label(
            RichText::new(v.tr.t_args("rteams.member.removeConfirm", &[("name", name)]))
                .color(theme::ERROR),
        );
        ui.horizontal(|ui| {
            if theme::solid(ui, &v.tr.t("rteams.member.confirmRemove"), theme::ERROR).clicked() {
                out.push(Intent::Stop(name.to_string()));
                out.push(Intent::Run(GuiCmd::RemoveMember {
                    name: name.to_string(),
                    removed: true,
                }));
                edit.confirm_remove = false;
            }
            if ui.button(v.tr.t("common.actions.cancel")).clicked() {
                edit.confirm_remove = false;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::{fixture, render};

    #[test]
    fn date_and_compact() {
        assert_eq!(date_time(0), "1970-01-01 00:00");
        assert_eq!(date_time(1_709_210_096_000), "2024-02-29 12:34");
        assert_eq!(compact(950), "950");
        assert_eq!(compact(12_300), "12.3k");
        assert_eq!(compact(1_200_000), "1.2M");
    }

    #[test]
    fn counts_and_idle_from_events() {
        let (home, data) = fixture("membercounts");
        let c = event_counts(data.events(), "boss");
        assert!(c.messages >= 2 && c.comments >= 1, "{c:?}");
        assert_eq!(
            event_counts(data.events(), "nobody"),
            EventCounts::default()
        );
        assert!(last_activity(data.events(), "boss").is_some());
        assert!(last_activity(data.events(), "nobody").is_none());
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn runtime_cmd_diff() {
        let (home, data) = fixture("memberrt");
        let view = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let mut e = EditState::default();
        e.load(&view, "ada");
        assert!(e.runtime_cmd(&view, "ada").is_none());
        e.worktree = true;
        e.runtime = Some("codex".into());
        assert_eq!(
            e.runtime_cmd(&view, "ada"),
            Some(GuiCmd::SetMemberRuntime {
                name: "ada".into(),
                runtime: Some("codex".into()),
                worktree: Some(true),
            })
        );
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn open_close() {
        let mut d = MemberDialog::default();
        assert!(!d.is_open());
        d.open("ada".into());
        d.tab = Tab::Logs;
        d.open("ada".into());
        assert_eq!(d.tab, Tab::Logs);
        d.open("bob".into());
        assert_eq!(d.tab, Tab::Tasks);
        d.close();
        assert!(!d.is_open());
    }

    #[test]
    fn renders_headless() {
        let (home, data) = fixture("memberdlg");
        let view = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        for name in ["ada", "boss", "nobody"] {
            for tab in [
                Tab::Tasks,
                Tab::Logs,
                Tab::Messages,
                Tab::Stats,
                Tab::Sessions,
                Tab::Changes,
                Tab::Edit,
            ] {
                let mut d = MemberDialog::default();
                d.open(name.into());
                d.tab = tab;
                render(|ui| d.show(ui.ctx(), &view, &mut Intents::default()));
            }
        }
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn edit_states_render_and_diff() {
        let (home, data) = fixture("memberedit");
        let view = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let mut d = MemberDialog::default();
        d.open("ada".into());
        d.tab = Tab::Edit;
        render(|ui| d.show(ui.ctx(), &view, &mut Intents::default()));
        assert_eq!(d.edit.role, "dev");
        assert!(d.edit.update(&view, "ada").is_none());
        d.edit.model = "opus".into();
        match d.edit.update(&view, "ada") {
            Some(GuiCmd::UpdateMember {
                role: None,
                prompt: None,
                model: Some(m),
                ..
            }) => assert_eq!(m, "opus"),
            other => panic!("{other:?}"),
        }
        // Launch settings: prefilled from the member, diffed into SetMemberLaunch.
        assert!(d.edit.launch_cmd(&view, "ada").is_none());
        d.edit.effort = Some("high".into());
        d.edit.fast = true;
        d.edit.limit = "200,000".into();
        assert_eq!(
            d.edit.launch_cmd(&view, "ada"),
            Some(GuiCmd::SetMemberLaunch {
                name: "ada".into(),
                effort: Some("high".into()),
                fast: Some(true),
                context_limit: Some(200_000),
            })
        );
        d.edit.limit = "abc".into();
        assert!(d.edit.launch_cmd(&view, "ada").is_none());
        render(|ui| d.show(ui.ctx(), &view, &mut Intents::default()));
        d.edit.confirm_remove = true;
        render(|ui| d.show(ui.ctx(), &view, &mut Intents::default()));
        let _ = std::fs::remove_dir_all(home);
    }
}
