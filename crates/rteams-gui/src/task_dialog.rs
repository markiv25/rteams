//! Task dialog (centered modal with the full task: header, actions, description, related
//! tasks, logs, history, comments) and the Create Task dialog. Views only read a `View` and push
//! `Intent`s; the shell acts on them and calls `clear_inputs` / `clear` after a successful write.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Key, Modifiers, RichText};
use rteams_core::Status;

use crate::actions::GuiCmd;
use crate::data::TaskDetail;
use crate::forms::{member_combo, text_field};
use crate::git;
use crate::icons::{self, Icon};
use crate::runs::{self, RunEvent, RunKind};
use crate::theme;
use crate::view::{Intent, Intents, View};
use rteams_services::i18n::I18n;

/// Pill label and color for a status, as the original colors them.
pub(crate) fn status_pill(tr: &I18n, s: Status) -> (String, Color32) {
    let (key, c) = match s {
        Status::Pending => ("rteams.status.todo", theme::COL_TODO),
        Status::InProgress => ("team.activity.activeTasks.inProgress", theme::WARNING),
        Status::Review => ("rteams.status.review", theme::COL_REVIEW),
        Status::Approved => ("team.taskDetail.reviewStates.approved", theme::SUCCESS),
        Status::Done => ("rteams.status.done", theme::COL_DONE),
        Status::NeedsFix => ("team.taskDetail.reviewStates.needsFix", theme::ERROR),
        Status::Blocked => ("rteams.board.blocked", theme::WARNING),
    };
    (tr.t(key), c)
}

/// Which status-dependent actions are legal, per `Status::can_transition_to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Avail {
    start: bool,
    complete: bool,
    request_review: bool,
    /// Approve / request changes.
    review: bool,
    block: bool,
    unblock: bool,
}

fn available(status: Status) -> Avail {
    Avail {
        // Blocked -> InProgress is an unblock, not a start.
        start: status != Status::Blocked && status.can_transition_to(Status::InProgress),
        complete: status.can_transition_to(Status::Done),
        request_review: status.can_transition_to(Status::Review),
        review: status == Status::Review,
        block: status.can_transition_to(Status::Blocked),
        unblock: status == Status::Blocked,
    }
}

fn nonempty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Input state belonging to the open task; reset when another task opens.
#[derive(Debug, Default)]
struct Draft {
    comment: String,
    block_reason: String,
    changes: String,
    approve_note: String,
    owner: String,
    reviewer: String,
    /// Owner combo has been preselected from the task.
    seeded: bool,
    /// Title being edited (`Some` = edit mode).
    title_edit: Option<String>,
    /// Description being edited (`Some` = edit mode).
    desc_edit: Option<String>,
    /// Answer to the open clarification question.
    answer: String,
    /// Question for "Ask for clarification".
    question: String,
    /// Path typed into the attach field.
    attach_path: String,
    /// Task chosen in the "Link task" combo.
    link_pick: String,
    /// File whose diff is expanded.
    expanded: Option<String>,
    /// File waiting for the second click of Revert.
    revert_confirm: Option<String>,
    /// Files marked Keep (in memory only).
    kept: HashSet<String>,
    /// Run-file analysis, refreshed every few seconds.
    runs: Option<RunsCache>,
}

/// Interval between re-reads of run files and git diffs while the dialog is open.
const RUNS_REFRESH: Duration = Duration::from_secs(3);
const MAX_LOG_ROWS: usize = 200;
const MAX_FILES: usize = 60;
const MAX_DIFF_LINES: usize = 400;

#[derive(Debug)]
struct RunsCache {
    at: Instant,
    log: Vec<(String, RunEvent)>,
    files: Vec<FileChange>,
}

#[derive(Debug)]
struct FileChange {
    path: String,
    diff: Result<String, String>,
    added: usize,
    removed: usize,
}

/// Where run files and the repository live; set by the shell.
struct Env<'a> {
    home: &'a Path,
    team: &'a str,
    repo: Option<&'a Path>,
}

fn load_runs(env: &Env, task: &str) -> RunsCache {
    let log = runs::task_log(env.home, env.team, task);
    let files = match env.repo {
        Some(repo) => runs::changed_files(env.home, env.team, task)
            .into_iter()
            .take(MAX_FILES)
            .map(|path| {
                let diff = git::diff_file(repo, &path);
                let (added, removed) = diff.as_deref().map_or((0, 0), diff_counts);
                FileChange {
                    path,
                    diff,
                    added,
                    removed,
                }
            })
            .collect(),
        None => Vec::new(),
    };
    RunsCache {
        at: Instant::now(),
        log,
        files,
    }
}

/// `(+added, -removed)` lines of a unified diff, not counting the file headers.
fn diff_counts(diff: &str) -> (usize, usize) {
    let mut c = (0, 0);
    for l in diff.lines() {
        if l.starts_with('+') && !l.starts_with("+++") {
            c.0 += 1;
        } else if l.starts_with('-') && !l.starts_with("---") {
            c.1 += 1;
        }
    }
    c
}

/// File name part of a path, for attachments.
fn base_name(path: &str) -> String {
    Path::new(path.trim()).file_name().map_or_else(
        || path.trim().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn open_external(path: &str) {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(path).spawn();
}

/// Subject update, or `None` when blank or unchanged.
fn title_intent(task: &str, old: &str, new: &str) -> Option<Intent> {
    let new = new.trim();
    (!new.is_empty() && new != old).then(|| {
        Intent::Run(GuiCmd::UpdateTask {
            task: task.to_string(),
            subject: Some(new.to_string()),
            description: None,
        })
    })
}

/// Description update, or `None` when unchanged.
fn desc_intent(task: &str, old: &str, new: &str) -> Option<Intent> {
    let new = new.trim();
    (new != old.trim()).then(|| {
        Intent::Run(GuiCmd::UpdateTask {
            task: task.to_string(),
            subject: None,
            description: Some(new.to_string()),
        })
    })
}

/// Review badge for a history summary.
fn review_badge(tr: &I18n, summary: &str) -> Option<(String, Color32)> {
    let s = summary.trim_start().to_lowercase();
    if s.starts_with("approved") {
        Some((
            tr.t("team.taskDetail.reviewStates.approved"),
            theme::SUCCESS,
        ))
    } else if s.starts_with("requested review") {
        Some((
            tr.t("team.taskDetail.workflowTimeline.reviewRequested"),
            theme::INFO,
        ))
    } else if s.starts_with("requested changes") {
        Some((
            tr.t("team.taskDetail.workflowTimeline.changesRequested"),
            theme::ERROR,
        ))
    } else {
        None
    }
}

/// Save/Cancel row; returns (save, cancel).
fn save_cancel(ui: &mut egui::Ui, tr: &I18n) -> (bool, bool) {
    let mut r = (false, false);
    ui.horizontal(|ui| {
        r.0 = theme::solid(ui, &tr.t("common.actions.save"), theme::ACCENT).clicked();
        r.1 = ui.button(tr.t("common.actions.cancel")).clicked();
    });
    r
}

/// The task dialog. `show` is a no-op while closed.
#[derive(Debug, Default)]
pub struct TaskDialog {
    task: Option<String>,
    draft: Draft,
    home: PathBuf,
    team: String,
    repo: Option<PathBuf>,
}

impl TaskDialog {
    /// Where run files (`<home>/<team>/runs`) and the project repository live; the shell calls
    /// this whenever the team or project path changes.
    pub fn set_paths(&mut self, home: PathBuf, team: String, repo: Option<PathBuf>) {
        if self.home != home || self.team != team || self.repo != repo {
            self.draft.runs = None;
        }
        self.home = home;
        self.team = team;
        self.repo = repo;
    }

    /// Open on `id`; switching to a different task starts from empty inputs.
    pub fn open(&mut self, id: String) {
        if self.task.as_deref() != Some(id.as_str()) {
            self.draft = Draft::default();
        }
        self.task = Some(id);
    }

    pub fn close(&mut self) {
        self.task = None;
        self.draft = Draft::default();
    }

    pub fn is_open(&self) -> bool {
        self.task.is_some()
    }

    /// Empty all inputs (after a successful command), keeping the dialog open.
    pub fn clear_inputs(&mut self) {
        self.draft = Draft::default();
    }

    pub fn show(&mut self, ctx: &egui::Context, v: &View, out: &mut Intents) {
        let Some(id) = self.task.clone() else { return };
        let screen = ctx.content_rect();
        let width = (screen.width() * 0.92).min(1090.0);
        let detail = v.data.task_detail(&id);
        let draft = &mut self.draft;
        let env = Env {
            home: &self.home,
            team: &self.team,
            repo: self.repo.as_deref(),
        };

        let mut open = true;
        crate::forms::dialog(
            ctx,
            ("task_dialog", &id),
            "",
            width,
            &mut open,
            |ui| match &detail {
                Some(d) => body(ui, v, &env, d, draft, out),
                None => {
                    ui.label(v.tr.t_args("rteams.task.notFound", &[("id", &theme::short_id(&id))]));
                }
            },
        );
        if !open {
            out.push(Intent::CloseTask);
        }
    }
}

fn pad<R>(ui: &mut egui::Ui, f: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, f)
        .inner
}

/// Small rounded pill drawn in place (text `color` on a faint fill), 11px.
fn tiny_pill(ui: &mut egui::Ui, text: &str, color: Color32) {
    theme::pill(ui, text, color);
}

/// Collapsible section row: 28px icon tile, 15px semibold title, count and extra pills, chevron.
#[allow(clippy::too_many_arguments)]
fn sect(
    ui: &mut egui::Ui,
    id: &str,
    icon: Icon,
    title: &str,
    count: Option<usize>,
    extra: Option<(String, Color32)>,
    default_open: bool,
    add_right: impl FnOnce(&mut egui::Ui),
    body: impl FnOnce(&mut egui::Ui),
) {
    let cid = ui.make_persistent_id(("td_sect", id));
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        cid,
        default_open,
    );
    let open = state.is_open();
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::click());
    let a = if open {
        0.06
    } else if resp.hovered() {
        0.05
    } else {
        0.035
    };
    ui.painter().rect_filled(
        rect,
        egui::CornerRadius::same(10),
        Color32::from_white_alpha((a * 255.0) as u8),
    );
    let tile = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 12.0, rect.center().y - 14.0),
        egui::Vec2::splat(28.0),
    );
    ui.painter().rect_filled(
        tile,
        egui::CornerRadius::same(8),
        theme::tint(theme::ACCENT, 0.14),
    );
    icons::draw(
        ui.painter(),
        egui::Rect::from_center_size(tile.center(), egui::Vec2::splat(16.0)),
        icon,
        theme::ACCENT,
    );
    let g = ui.painter().layout_no_wrap(
        title.to_string(),
        egui::FontId::proportional(15.0),
        theme::TEXT,
    );
    let tx = tile.right() + 10.0;
    let mut x = tx + g.size().x + 10.0;
    ui.painter().galley(
        egui::pos2(tx, rect.center().y - g.size().y / 2.0),
        g,
        theme::TEXT,
    );
    let mut pills: Vec<(String, Color32)> = Vec::new();
    if let Some(n) = count {
        pills.push((n.to_string(), theme::TEXT_2));
    }
    if let Some(e) = extra {
        pills.push(e);
    }
    for (t, c) in pills {
        let g = ui
            .painter()
            .layout_no_wrap(t, egui::FontId::proportional(11.0), c);
        let r = egui::Rect::from_min_size(
            egui::pos2(x, rect.center().y - 9.0),
            egui::vec2(g.size().x + 14.0, 18.0),
        );
        ui.painter()
            .rect_filled(r, egui::CornerRadius::same(255), theme::tint(c, 0.15));
        ui.painter().galley(
            egui::pos2(r.left() + 7.0, r.center().y - g.size().y / 2.0),
            g,
            c,
        );
        x = r.right() + 6.0;
    }
    icons::draw(
        ui.painter(),
        egui::Rect::from_center_size(
            egui::pos2(rect.right() - 20.0, rect.center().y),
            egui::Vec2::splat(16.0),
        ),
        if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        },
        theme::TEXT_2,
    );
    let right = egui::Rect::from_min_max(
        egui::pos2(rect.right() - 200.0, rect.top()),
        egui::pos2(rect.right() - 40.0, rect.bottom()),
    );
    let mut rui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(right)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    add_right(&mut rui);
    if resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        state.toggle(ui);
    }
    state.show_body_unindented(ui, |ui| {
        ui.add_space(6.0);
        body(ui);
    });
    ui.add_space(6.0);
}

/// Draws the whole dialog body.
fn body(ui: &mut egui::Ui, v: &View, env: &Env, d: &TaskDetail, dr: &mut Draft, out: &mut Intents) {
    if dr
        .runs
        .as_ref()
        .is_none_or(|c| c.at.elapsed() > RUNS_REFRESH)
    {
        dr.runs = Some(load_runs(env, &d.id));
    }
    ui.ctx().request_repaint_after(RUNS_REFRESH);
    let core = v.state.task(&d.id);
    let (label, color) = status_pill(v.tr, d.status);
    ui.horizontal(|ui| {
        theme::pill(ui, format!("#{}", theme::short_id(&d.id)), theme::TEXT_2);
        theme::pill(ui, label, color);
        if d.archived {
            theme::pill(ui, v.tr.t("rteams.task.archived"), theme::MUTED);
        }
        if let Some(t) = core {
            if t.clarification.is_some() {
                tiny_pill(ui, &v.tr.t("rteams.board.awaitingAnswer"), theme::WARNING);
            }
        }
    });
    ui.add_space(6.0);
    title_row(ui, v.tr, d, dr, out);
    ui.add_space(6.0);
    meta_row(ui, v, d, out);
    ui.add_space(6.0);
    action_row(ui, v, d, dr, out);
    ui.add_space(10.0);
    theme::rule(ui);
    ui.add_space(10.0);

    let editing_desc = dr.desc_edit.is_some();
    let mut start_desc = false;
    sect(
        ui,
        "description",
        Icon::List,
        &v.tr.t("team.taskDetail.sections.description"),
        None,
        None,
        true,
        |ui| {
            if !editing_desc
                && theme::icon_button(ui, Icon::Edit, &v.tr.t("common.schedules.actions.edit"))
                    .clicked()
            {
                start_desc = true;
            }
        },
        |ui| {
            pad(ui, |ui| {
                if dr.desc_edit.is_some() {
                    desc_editor(ui, v.tr, d, dr, out);
                } else if d.description.trim().is_empty() {
                    ui.label(
                        RichText::new(v.tr.t("common.list.noDescription")).color(theme::MUTED),
                    );
                } else {
                    ui.add(egui::Label::new(&d.description).wrap());
                }
            });
        },
    );
    if start_desc {
        dr.desc_edit = Some(d.description.clone());
    }

    if let Some(banner) = core.and_then(|t| t.clarification.as_deref()) {
        clarification_banner(ui, v.tr, d, banner, dr, out);
        ui.add_space(6.0);
    }

    let linked: &[String] = core.map_or(&[], |t| t.related.as_slice());
    let n = d.blocked_by.len() + d.blocks.len() + linked.len();
    sect(
        ui,
        "related",
        Icon::Link,
        &v.tr.t("team.taskDetail.related.title"),
        Some(n),
        None,
        n > 0,
        |_| {},
        |ui| {
            pad(ui, |ui| {
                related(ui, v, d, out);
                linked_tasks(ui, v, d, linked, dr, out);
            });
        },
    );

    let atts = core.map_or(&[][..], |t| t.attachments.as_slice());
    sect(
        ui,
        "attachments",
        Icon::Paperclip,
        &v.tr.t("team.taskDetail.sections.attachments"),
        Some(atts.len()),
        None,
        !atts.is_empty(),
        |_| {},
        |ui| pad(ui, |ui| attachments(ui, v.tr, d, atts, dr, out)),
    );

    let nfiles = dr.runs.as_ref().map_or(0, |c| c.files.len());
    sect(
        ui,
        "changes",
        Icon::File,
        &v.tr.t("team.taskDetail.sections.changes"),
        Some(nfiles),
        None,
        false,
        |_| {},
        |ui| pad(ui, |ui| changes(ui, v.tr, env, d, dr, out)),
    );

    let lines: &[String] = d
        .owner
        .as_deref()
        .and_then(|o| v.agent(o))
        .map_or(&[], |a| a.last_lines.as_slice());
    let log_n = dr.runs.as_ref().map_or(0, |c| c.log.len());
    let log: Vec<(String, RunEvent)> = dr.runs.as_ref().map_or(Vec::new(), |c| c.log.clone());
    sect(
        ui,
        "logs",
        Icon::Terminal,
        &v.tr.t("team.taskDetail.sections.taskLogs"),
        (log_n > 0).then_some(log_n),
        None,
        false,
        |_| {},
        |ui| pad(ui, |ui| task_logs(ui, v, &log, lines)),
    );

    sect(
        ui,
        "history",
        Icon::History,
        &v.tr.t("team.taskDetail.sections.workflowHistory"),
        Some(d.history.len()),
        None,
        false,
        |_| {},
        |ui| {
            pad(ui, |ui| {
                for h in &d.history {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(format_ts(h.ts)).small().color(theme::MUTED));
                        ui.label(RichText::new(&h.actor).strong().color(v.color(&h.actor)));
                        if let Some((t, c)) = review_badge(v.tr, &h.summary) {
                            theme::pill(ui, t, c);
                        }
                        ui.add(egui::Label::new(&h.summary).wrap());
                    });
                }
            });
        },
    );

    sect(
        ui,
        "comments",
        Icon::MessageSquare,
        &v.tr.t("team.taskDetail.sections.comments"),
        Some(d.comments.len()),
        None,
        true,
        |_| {},
        |ui| {
            pad(ui, |ui| composer(ui, v.tr, d, dr, out));
            for c in &d.comments {
                comment_card(ui, v, c);
            }
        },
    );
    ui.add_space(8.0);
}

/// Title: click to edit; Enter saves, Esc cancels.
fn title_row(ui: &mut egui::Ui, tr: &I18n, d: &TaskDetail, dr: &mut Draft, out: &mut Intents) {
    let Some(text) = dr.title_edit.as_mut() else {
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::Label::new(RichText::new(&d.subject).size(20.0).strong()).wrap());
            if theme::icon_button(ui, Icon::Edit, &tr.t("rteams.task.clickToEdit")).clicked() {
                dr.title_edit = Some(d.subject.clone());
            }
        });
        return;
    };
    let id = egui::Id::unique(("td_title", &d.id));
    let focused = ui.memory(|m| m.has_focus(id));
    let (mut save, mut cancel) = (false, false);
    if focused {
        cancel = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        save = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
    }
    let r = ui.add(
        egui::TextEdit::singleline(text)
            .id(id)
            .font(egui::TextStyle::Heading)
            .desired_width(f32::INFINITY),
    );
    if !focused && !r.has_focus() && !save && !cancel {
        r.request_focus();
    }
    let (s2, c2) = save_cancel(ui, tr);
    if (save || s2) && !(cancel || c2) {
        if let Some(i) = title_intent(&d.id, &d.subject, text) {
            out.push(i);
        }
        dr.title_edit = None;
    } else if cancel || c2 {
        dr.title_edit = None;
    }
}

fn desc_editor(ui: &mut egui::Ui, tr: &I18n, d: &TaskDetail, dr: &mut Draft, out: &mut Intents) {
    let Some(text) = dr.desc_edit.as_mut() else {
        return;
    };
    let id = egui::Id::unique(("td_desc", &d.id));
    let focused = ui.memory(|m| m.has_focus(id));
    let cancel_key = focused && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
    ui.add(
        egui::TextEdit::multiline(text)
            .id(id)
            .desired_rows(5)
            .desired_width(f32::INFINITY),
    );
    let (save, cancel) = save_cancel(ui, tr);
    if save {
        if let Some(i) = desc_intent(&d.id, &d.description, text) {
            out.push(i);
        }
        dr.desc_edit = None;
    } else if cancel || cancel_key {
        dr.desc_edit = None;
    }
}

fn person(ui: &mut egui::Ui, v: &View, name: &str, prefix: &str) {
    let color = v.color(name);
    if !prefix.is_empty() {
        ui.label(RichText::new(prefix).color(theme::MUTED));
    }
    theme::avatar(ui, name, color, 22.0, None);
    ui.label(RichText::new(name).color(color));
}

fn meta_row(ui: &mut egui::Ui, v: &View, d: &TaskDetail, out: &mut Intents) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        match &d.owner {
            Some(o) => {
                let c = v.color(o);
                theme::avatar(ui, o, c, 22.0, None);
                theme::pill(ui, o.as_str(), c);
            }
            None => {
                ui.label(RichText::new(v.tr.t("team.taskDetail.unassigned")).color(theme::MUTED));
            }
        }
        if let Some(r) = &d.reviewer {
            person(ui, v, r, &v.tr.t("rteams.task.reviewerPrefix"));
        }
        if let Some(first) = d.history.first() {
            ui.label(RichText::new(&first.actor).color(theme::TEXT_2));
            icons::icon(ui, Icon::Clock, 13.0, theme::MUTED);
            ui.label(
                RichText::new(format_ts(first.ts))
                    .size(12.0)
                    .color(theme::MUTED),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = if d.archived {
                v.tr.t("team.kanban.trash.restore")
            } else {
                v.tr.t("common.taskContextMenu.archive")
            };
            if theme::pill_icon_button(ui, Some(Icon::Trash), &label, false).clicked() {
                out.push(Intent::Run(GuiCmd::Archive {
                    task: d.id.clone(),
                    archived: !d.archived,
                }));
            }
        });
    });
    if let Some(reason) = &d.blocked_reason {
        ui.label(
            RichText::new(v.tr.t_args("rteams.task.blockedReason", &[("reason", reason)]))
                .color(theme::WARNING),
        );
    }
}

fn input(ui: &mut egui::Ui, text: &mut String, hint: &str) {
    ui.add(
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .desired_width(180.0),
    );
}

/// Compact row of pill buttons for the transitions legal in the task's status, plus a `...`
/// menu with the actions that need input (assign, reviewers, notes, reasons, questions).
fn action_row(ui: &mut egui::Ui, v: &View, d: &TaskDetail, dr: &mut Draft, out: &mut Intents) {
    let id = d.id.clone();
    let av = available(d.status);
    let core = v.state.task(&id);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        let mut run = |c: GuiCmd| out.push(Intent::Run(c));
        if av.start
            && theme::pill_icon_button(
                ui,
                Some(Icon::Play),
                &v.tr.t("team.kanban.taskCard.start"),
                false,
            )
            .clicked()
        {
            run(GuiCmd::Start { task: id.clone() });
        }
        if av.complete
            && theme::pill_icon_button(
                ui,
                Some(Icon::Check),
                &v.tr.t("team.kanban.taskCard.complete"),
                false,
            )
            .clicked()
        {
            run(GuiCmd::Complete { task: id.clone() });
        }
        if av.unblock
            && theme::pill_icon_button(ui, None, &v.tr.t("rteams.task.unblock"), false).clicked()
        {
            run(GuiCmd::Unblock { task: id.clone() });
        }
        if av.review {
            if core.is_some_and(|t| t.review_started) {
                theme::pill(ui, v.tr.t("rteams.board.reviewing"), theme::COL_REVIEW);
            } else if theme::pill_icon_button(
                ui,
                Some(Icon::Eye),
                &v.tr.t("rteams.board.startReview"),
                false,
            )
            .clicked()
            {
                run(GuiCmd::StartReview { task: id.clone() });
            }
        }
        let more = theme::icon_button(ui, Icon::MoreHorizontal, "");
        egui::Popup::menu(&more)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_min_width(260.0);
                action_bar(ui, v, d, dr, out);
            });
    });
}

/// Input-driven actions (inside the `...` menu).
fn action_bar(ui: &mut egui::Ui, v: &View, d: &TaskDetail, dr: &mut Draft, out: &mut Intents) {
    let id = d.id.clone();
    let core = v.state.task(&id);
    let members: Vec<String> = v.members().iter().map(|m| m.name.clone()).collect();
    if !dr.seeded {
        dr.seeded = true;
        dr.owner = d.owner.clone().unwrap_or_default();
    }
    let av = available(d.status);
    let mut run = |c: GuiCmd| out.push(Intent::Run(c));
    ui.horizontal_wrapped(|ui| {
        member_combo(
            ui,
            ("td_owner", &id),
            &v.tr.t("team.taskDetail.unassigned"),
            &members,
            &mut dr.owner,
        );
        if ui.button(v.tr.t("rteams.task.assign")).clicked() {
            run(GuiCmd::SetOwner {
                task: id.clone(),
                owner: nonempty(&dr.owner),
            });
        }
    });
    if av.request_review {
        let reviewers: Vec<String> = members
            .iter()
            .filter(|m| d.owner.as_deref() != Some(m.as_str()))
            .cloned()
            .collect();
        ui.horizontal_wrapped(|ui| {
            member_combo(
                ui,
                ("td_reviewer", &id),
                &v.tr.t("rteams.task.reviewerPick"),
                &reviewers,
                &mut dr.reviewer,
            );
            if ui
                .add_enabled(
                    !dr.reviewer.is_empty(),
                    egui::Button::new(v.tr.t("team.kanban.taskCard.requestReview")),
                )
                .clicked()
            {
                run(GuiCmd::RequestReview {
                    task: id.clone(),
                    reviewer: dr.reviewer.clone(),
                });
            }
        });
    }
    if av.review {
        ui.horizontal_wrapped(|ui| {
            input(
                ui,
                &mut dr.approve_note,
                &v.tr.t("rteams.task.approvalNote"),
            );
            if theme::solid(ui, &v.tr.t("team.kanban.taskCard.approve"), theme::SUCCESS).clicked() {
                run(GuiCmd::Approve {
                    task: id.clone(),
                    note: nonempty(&dr.approve_note),
                });
            }
        });
        ui.horizontal_wrapped(|ui| {
            input(ui, &mut dr.changes, &v.tr.t("rteams.task.whatToChange"));
            let ready = !dr.changes.trim().is_empty();
            if ui
                .add_enabled(
                    ready,
                    egui::Button::new(v.tr.t("team.kanban.taskCard.requestChanges")),
                )
                .clicked()
            {
                run(GuiCmd::RequestChanges {
                    task: id.clone(),
                    comment: dr.changes.trim().to_string(),
                });
            }
        });
    }
    if av.block {
        ui.horizontal_wrapped(|ui| {
            input(ui, &mut dr.block_reason, &v.tr.t("rteams.task.reason"));
            let ready = !dr.block_reason.trim().is_empty();
            if ui
                .add_enabled(ready, egui::Button::new(v.tr.t("rteams.task.block")))
                .clicked()
            {
                run(GuiCmd::Block {
                    task: id.clone(),
                    reason: dr.block_reason.trim().to_string(),
                });
            }
        });
    }
    if core.is_some_and(|t| t.clarification.is_none()) {
        ui.horizontal_wrapped(|ui| {
            input(ui, &mut dr.question, &v.tr.t("rteams.task.questionHint"));
            let ready = !dr.question.trim().is_empty();
            if ui
                .add_enabled(
                    ready,
                    egui::Button::new(v.tr.t("rteams.task.askClarification")),
                )
                .clicked()
            {
                run(GuiCmd::SetClarification {
                    task: id.clone(),
                    question: Some(dr.question.trim().to_string()),
                });
            }
        });
    }
}

fn related(ui: &mut egui::Ui, v: &View, d: &TaskDetail, out: &mut Intents) {
    let groups = [
        (
            v.tr.t("team.organizations.editor.relationKind.dependsOn"),
            &d.blocked_by,
        ),
        (v.tr.t("team.taskDetail.related.blocks"), &d.blocks),
    ];
    for (title, ids) in groups {
        if ids.is_empty() {
            continue;
        }
        ui.label(RichText::new(title).small().color(theme::MUTED));
        for rid in ids {
            ui.horizontal_wrapped(|ui| {
                if ui.link(format!("#{}", theme::short_id(rid))).clicked() {
                    out.push(Intent::OpenTask(rid.clone()));
                }
                match v.state.task(rid) {
                    Some(t) => {
                        ui.add(egui::Label::new(&t.subject).wrap());
                        let (l, c) = status_pill(v.tr, t.status);
                        theme::pill(ui, l, c);
                        if d.waiting_on.contains(rid) {
                            theme::pill(ui, v.tr.t("rteams.task.waiting"), theme::WARNING);
                        }
                    }
                    None => {
                        ui.label(
                            RichText::new(v.tr.t("rteams.task.unknownTask")).color(theme::MUTED),
                        );
                    }
                }
            });
        }
    }
}

fn logs(ui: &mut egui::Ui, tr: &I18n, lines: &[String]) {
    if lines.is_empty() {
        ui.label(RichText::new(tr.t("rteams.task.noLogs")).color(theme::MUTED));
        return;
    }
    egui::Frame::new()
        .fill(theme::CODE_BG)
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let from = lines.len().saturating_sub(50);
            for l in &lines[from..] {
                ui.add(egui::Label::new(RichText::new(l).monospace().color(theme::TEXT_2)).wrap());
            }
        });
}

fn clarification_banner(
    ui: &mut egui::Ui,
    tr: &I18n,
    d: &TaskDetail,
    question: &str,
    dr: &mut Draft,
    out: &mut Intents,
) {
    egui::Frame::new()
        .fill(theme::tint(theme::WARNING, 0.12))
        .stroke(egui::Stroke::new(1.0, theme::tint(theme::WARNING, 0.5)))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width().max(0.0));
            ui.add(
                egui::Label::new(
                    RichText::new(
                        tr.t_args("rteams.task.waitingAnswer", &[("question", question)]),
                    )
                    .color(theme::WARNING),
                )
                .wrap(),
            );
            ui.horizontal_wrapped(|ui| {
                let w = (ui.available_width() - 90.0).clamp(60.0, 420.0);
                ui.add(
                    egui::TextEdit::singleline(&mut dr.answer)
                        .hint_text(tr.t("rteams.task.yourAnswer"))
                        .desired_width(w),
                );
                let ready = !dr.answer.trim().is_empty();
                if ui
                    .add_enabled(ready, egui::Button::new(tr.t("rteams.task.answer")))
                    .clicked()
                {
                    out.push(Intent::Run(GuiCmd::Comment {
                        task: d.id.clone(),
                        text: dr.answer.trim().to_string(),
                    }));
                    out.push(Intent::Run(GuiCmd::SetClarification {
                        task: d.id.clone(),
                        question: None,
                    }));
                    dr.answer.clear();
                }
            });
        });
}

/// Manually linked tasks (`Task.related`) with unlink, plus the "Link task" combo.
fn linked_tasks(
    ui: &mut egui::Ui,
    v: &View,
    d: &TaskDetail,
    linked: &[String],
    dr: &mut Draft,
    out: &mut Intents,
) {
    if !linked.is_empty() {
        ui.label(
            RichText::new(v.tr.t("rteams.task.related"))
                .small()
                .color(theme::MUTED),
        );
    }
    for rid in linked {
        ui.horizontal_wrapped(|ui| {
            if ui.link(format!("#{}", theme::short_id(rid))).clicked() {
                out.push(Intent::OpenTask(rid.clone()));
            }
            match v.state.task(rid) {
                Some(t) => {
                    ui.add(egui::Label::new(&t.subject).wrap());
                    let (l, c) = status_pill(v.tr, t.status);
                    theme::pill(ui, l, c);
                }
                None => {
                    ui.label(RichText::new(v.tr.t("rteams.task.unknownTask")).color(theme::MUTED));
                }
            }
            if theme::ghost(ui, &v.tr.t("rteams.task.unlink")).clicked() {
                out.push(Intent::Run(GuiCmd::LinkTasks {
                    task: d.id.clone(),
                    related: rid.clone(),
                    linked: false,
                }));
            }
        });
    }
    let candidates: Vec<(&str, &str)> = v
        .state
        .tasks
        .iter()
        .filter(|t| !t.archived && t.id != d.id && !linked.contains(&t.id))
        .map(|t| (t.id.as_str(), t.subject.as_str()))
        .collect();
    ui.horizontal_wrapped(|ui| {
        let shown = candidates
            .iter()
            .find(|(id, _)| *id == dr.link_pick)
            .map_or_else(
                || v.tr.t("rteams.task.linkPick").to_string(),
                |(id, s)| format!("#{} {}", theme::short_id(id), clip_chars(s, 30)),
            );
        egui::ComboBox::from_id_salt(("td_link", &d.id))
            .selected_text(shown)
            .width(220.0f32.min(ui.available_width().max(60.0)))
            .show_ui(ui, |ui| {
                for (id, subject) in &candidates {
                    let label = format!("#{} {}", theme::short_id(id), clip_chars(subject, 40));
                    ui.selectable_value(&mut dr.link_pick, id.to_string(), label);
                }
            });
        let ready = candidates.iter().any(|(id, _)| *id == dr.link_pick);
        if ui
            .add_enabled(ready, egui::Button::new(v.tr.t("rteams.task.link")))
            .clicked()
        {
            out.push(Intent::Run(GuiCmd::LinkTasks {
                task: d.id.clone(),
                related: std::mem::take(&mut dr.link_pick),
                linked: true,
            }));
        }
    });
}

fn clip_chars(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n).collect();
        t.push('\u{2026}');
        t
    }
}

fn attachments(
    ui: &mut egui::Ui,
    tr: &I18n,
    d: &TaskDetail,
    atts: &[rteams_core::Attachment],
    dr: &mut Draft,
    out: &mut Intents,
) {
    if atts.is_empty() {
        ui.label(RichText::new(tr.t("rteams.task.noAttachments")).color(theme::MUTED));
    }
    for a in atts {
        ui.horizontal_wrapped(|ui| {
            if ui
                .link(&a.name)
                .on_hover_text(tr.t_args("rteams.task.openPath", &[("path", &a.path)]))
                .clicked()
            {
                open_external(&a.path);
            }
            ui.label(RichText::new(&a.path).small().color(theme::MUTED));
        });
    }
    ui.horizontal_wrapped(|ui| {
        let w = (ui.available_width() - 100.0).clamp(60.0, 360.0);
        ui.add(
            egui::TextEdit::singleline(&mut dr.attach_path)
                .hint_text(tr.t("rteams.task.pathToFile"))
                .desired_width(w),
        );
        let ready = !dr.attach_path.trim().is_empty();
        if ui
            .add_enabled(ready, egui::Button::new(tr.t("rteams.task.attachFile")))
            .clicked()
        {
            let path = dr.attach_path.trim().to_string();
            out.push(Intent::Run(GuiCmd::AttachFile {
                task: d.id.clone(),
                name: base_name(&path),
                path,
            }));
            dr.attach_path.clear();
        }
    });
}

/// Task Logs: recorded run events attributed to the task, else the launcher's summarized lines.
fn task_logs(ui: &mut egui::Ui, v: &View, log: &[(String, RunEvent)], lines: &[String]) {
    if log.is_empty() {
        logs(ui, v.tr, lines);
        return;
    }
    let (mut tin, mut tout, mut cost) = (0u64, 0u64, 0f64);
    for (_, e) in log {
        if let RunKind::Result {
            tokens_in,
            tokens_out,
            cost_usd,
        } = e.kind
        {
            tin += tokens_in;
            tout += tokens_out;
            cost += cost_usd;
        }
    }
    ui.label(
        RichText::new(v.tr.t_args(
            "rteams.task.logSummary",
            &[
                ("events", &log.len().to_string()),
                ("tin", &tin.to_string()),
                ("tout", &tout.to_string()),
                ("cost", &format!("{cost:.2}")),
            ],
        ))
        .small()
        .color(theme::MUTED),
    );
    egui::ScrollArea::vertical()
        .id_salt("td_logs_scroll")
        .max_height(320.0)
        .auto_shrink([false, true])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            let from = log.len().saturating_sub(MAX_LOG_ROWS);
            for (member, e) in &log[from..] {
                log_row(ui, v, member, e);
            }
        });
}

fn log_row(ui: &mut egui::Ui, v: &View, member: &str, e: &RunEvent) {
    let (fill, fg, icon) = match &e.kind {
        RunKind::ToolUse { .. } => (
            theme::tint(Color32::from_rgb(0x78, 0x35, 0x0f), 0.2),
            theme::TOOL_FG,
            "\u{1F527}",
        ),
        RunKind::ToolResult { ok: true } => (
            theme::tint(theme::RESULT_FG, 0.06),
            theme::RESULT_FG,
            "\u{2714}",
        ),
        RunKind::ToolResult { ok: false } => {
            (theme::tint(theme::ERROR, 0.1), theme::ERROR_FG, "\u{2714}")
        }
        RunKind::Thinking => (theme::tint(theme::THINK_FG, 0.06), theme::THINK_FG, ""),
        RunKind::Result { .. } => (Color32::TRANSPARENT, theme::MUTED, ""),
        RunKind::Text => (Color32::TRANSPARENT, theme::TEXT, ""),
    };
    let body = match &e.kind {
        RunKind::Thinking => v.tr.t_args(
            "rteams.task.thinking",
            &[("text", &clip_chars(&e.text, 400))],
        ),
        RunKind::Result {
            tokens_in,
            tokens_out,
            cost_usd,
        } => v.tr.t_args(
            "rteams.task.runFinished",
            &[
                ("tin", &tokens_in.to_string()),
                ("tout", &tokens_out.to_string()),
                ("cost", &format!("{cost_usd:.2}")),
            ],
        ),
        RunKind::ToolResult { ok: false } => {
            v.tr.t_args("rteams.task.error", &[("text", &clip_chars(&e.text, 400))])
        }
        _ => clip_chars(&e.text, 400),
    };
    egui::Frame::new()
        .fill(fill)
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width().max(0.0));
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(member)
                        .size(11.0)
                        .strong()
                        .color(v.color(member)),
                );
                if !icon.is_empty() && !matches!(e.kind, RunKind::ToolResult { .. }) {
                    ui.label(RichText::new(icon).size(12.0).color(fg));
                }
                let mut t = RichText::new(body).size(12.0).color(fg);
                if matches!(e.kind, RunKind::ToolUse { .. }) {
                    t = t.monospace();
                }
                ui.add(egui::Label::new(t).wrap());
            });
        });
}

/// Changes: files the agents touched while on this task, with inline diff, Keep and Revert.
fn changes(
    ui: &mut egui::Ui,
    tr: &I18n,
    env: &Env,
    d: &TaskDetail,
    dr: &mut Draft,
    out: &mut Intents,
) {
    if env.repo.is_none() {
        ui.label(RichText::new(tr.t("rteams.task.noGit")).color(theme::MUTED));
        return;
    }
    let Some(cache) = dr.runs.as_ref() else {
        return;
    };
    if cache.files.is_empty() {
        ui.label(RichText::new(tr.t("rteams.task.noChangedFiles")).color(theme::MUTED));
        return;
    }
    let (ta, tdel) = cache
        .files
        .iter()
        .fold((0, 0), |a, f| (a.0 + f.added, a.1 + f.removed));
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(tr.t_count(
                "team.taskDetail.changes.fileCount",
                cache.files.len() as u64,
            ))
            .small()
            .color(theme::MUTED),
        );
        ui.label(RichText::new(format!("+{ta}")).small().color(DIFF_ADD_FG));
        ui.label(RichText::new(format!("-{tdel}")).small().color(DIFF_DEL_FG));
    });
    let mut toggle: Option<String> = None;
    let mut keep: Option<String> = None;
    let mut revert: Option<String> = None;
    let mut confirm: Option<String> = None;
    let mut cancel = false;
    for f in &cache.files {
        let kept = dr.kept.contains(&f.path);
        let open = dr.expanded.as_deref() == Some(f.path.as_str());
        ui.horizontal_wrapped(|ui| {
            let chevron = if open { "\u{23f7}" } else { "\u{23f5}" };
            let r = ui.add(
                egui::Label::new(
                    RichText::new(format!("{chevron} {}", f.path))
                        .monospace()
                        .size(12.5),
                )
                .sense(egui::Sense::click()),
            );
            if r.clicked() {
                toggle = Some(f.path.clone());
            }
            ui.label(
                RichText::new(format!("+{}", f.added))
                    .small()
                    .color(DIFF_ADD_FG),
            );
            ui.label(
                RichText::new(format!("-{}", f.removed))
                    .small()
                    .color(DIFF_DEL_FG),
            );
            if kept {
                theme::pill(ui, tr.t("rteams.task.kept"), theme::SUCCESS);
            }
            if dr.revert_confirm.as_deref() == Some(f.path.as_str()) {
                ui.label(RichText::new(tr.t("rteams.task.discardChange")).color(theme::WARNING));
                if theme::solid(ui, &tr.t("rteams.task.confirmRevert"), theme::ERROR).clicked() {
                    revert = Some(f.path.clone());
                }
                if ui.button(tr.t("common.actions.cancel")).clicked() {
                    cancel = true;
                }
            } else {
                if !kept && ui.button(tr.t("team.kanban.taskCard.keep")).clicked() {
                    keep = Some(f.path.clone());
                }
                if ui.button(tr.t("rteams.task.revert")).clicked() {
                    confirm = Some(f.path.clone());
                }
            }
        });
        if open {
            diff_view(ui, tr, &f.diff);
        }
    }
    if let Some(p) = toggle {
        dr.expanded = if dr.expanded.as_deref() == Some(p.as_str()) {
            None
        } else {
            Some(p)
        };
    }
    if let Some(p) = keep {
        dr.kept.insert(p);
    }
    if let Some(p) = confirm {
        dr.revert_confirm = Some(p);
    }
    if cancel {
        dr.revert_confirm = None;
    }
    if let Some(p) = revert {
        dr.revert_confirm = None;
        dr.runs = None;
        out.push(Intent::RevertFile {
            task: d.id.clone(),
            path: p,
        });
    }
}

const DIFF_ADD_FG: Color32 = Color32::from_rgb(0x4a, 0xde, 0x80);
const DIFF_DEL_FG: Color32 = Color32::from_rgb(0xf8, 0x71, 0x71);

fn diff_view(ui: &mut egui::Ui, tr: &I18n, diff: &Result<String, String>) {
    egui::Frame::new()
        .fill(theme::CODE_BG)
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width().max(0.0));
            let text = match diff {
                Ok(t) if t.trim().is_empty() => {
                    ui.label(RichText::new(tr.t("rteams.task.noDifferences")).color(theme::MUTED));
                    return;
                }
                Ok(t) => t,
                Err(e) => {
                    ui.label(RichText::new(e).color(theme::ERROR_FG));
                    return;
                }
            };
            let mut shown = 0;
            for l in text.lines() {
                if l.starts_with("diff --git")
                    || l.starts_with("index ")
                    || l.starts_with("new file")
                    || l.starts_with("---")
                    || l.starts_with("+++")
                {
                    continue;
                }
                if shown >= MAX_DIFF_LINES {
                    ui.label(
                        RichText::new(tr.t("rteams.task.diffTruncated"))
                            .small()
                            .color(theme::MUTED),
                    );
                    break;
                }
                shown += 1;
                let (fill, fg) = if l.starts_with('+') {
                    (
                        theme::tint(Color32::from_rgb(0x22, 0xc5, 0x5e), 0.15),
                        DIFF_ADD_FG,
                    )
                } else if l.starts_with('-') {
                    (
                        theme::tint(Color32::from_rgb(0xef, 0x44, 0x44), 0.15),
                        DIFF_DEL_FG,
                    )
                } else if l.starts_with("@@") {
                    (Color32::TRANSPARENT, theme::INFO)
                } else {
                    (Color32::TRANSPARENT, theme::TEXT_2)
                };
                egui::Frame::new().fill(fill).show(ui, |ui| {
                    ui.set_min_width(ui.available_width().max(0.0));
                    ui.add(
                        egui::Label::new(RichText::new(l).monospace().size(11.5).color(fg)).wrap(),
                    );
                });
            }
        });
}

fn composer(ui: &mut egui::Ui, tr: &I18n, d: &TaskDetail, dr: &mut Draft, out: &mut Intents) {
    let id = egui::Id::unique(("td_comment", &d.id));
    let mut send = false;
    if ui.memory(|m| m.has_focus(id)) {
        send = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
    }
    egui::Frame::new()
        .fill(theme::CARD)
        .stroke(egui::Stroke::new(1.0, theme::hairline(0.14)))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut dr.comment)
                    .id(id)
                    .hint_text(tr.t("team.taskComments.placeholder"))
                    .desired_rows(3)
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(4.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if indigo_button(ui, Icon::Send, &tr.t("team.taskComments.comment")).clicked() {
                    send = true;
                }
            });
        });
    if send && !dr.comment.trim().is_empty() {
        out.push(Intent::Run(GuiCmd::Comment {
            task: d.id.clone(),
            text: dr.comment.trim().to_string(),
        }));
    }
    ui.add_space(8.0);
}

/// Filled indigo pill button with a leading icon (the original's `Comment` button).
fn indigo_button(ui: &mut egui::Ui, icon: Icon, text: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(13.0),
        Color32::WHITE,
    );
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 44.0, 30.0),
        egui::Sense::click(),
    );
    let fill = if resp.hovered() {
        Color32::from_rgb(0x58, 0x4f, 0xe8)
    } else {
        Color32::from_rgb(0x4f, 0x46, 0xe5)
    };
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(255), fill);
    icons::draw(
        ui.painter(),
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + 18.0, rect.center().y),
            egui::Vec2::splat(14.0),
        ),
        icon,
        Color32::WHITE,
    );
    ui.painter().galley(
        egui::pos2(rect.left() + 32.0, rect.center().y - galley.size().y / 2.0),
        galley,
        Color32::WHITE,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// 26px round copy button (two overlapping rounded squares).
fn copy_button(ui: &mut egui::Ui, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::Vec2::splat(26.0), egui::Sense::click());
    let c = if resp.hovered() {
        theme::TEXT
    } else {
        theme::MUTED
    };
    if resp.hovered() {
        ui.painter()
            .circle_filled(rect.center(), 13.0, Color32::from_rgb(0x25, 0x27, 0x33));
    }
    let st = egui::Stroke::new(1.3, c);
    let a = egui::Rect::from_min_size(
        rect.center() + egui::vec2(-5.5, -2.5),
        egui::Vec2::splat(8.0),
    );
    let b = egui::Rect::from_min_size(
        rect.center() + egui::vec2(-2.5, -5.5),
        egui::Vec2::splat(8.0),
    );
    ui.painter()
        .rect_stroke(a, egui::CornerRadius::same(2), st, egui::StrokeKind::Middle);
    ui.painter()
        .rect_stroke(b, egui::CornerRadius::same(2), st, egui::StrokeKind::Middle);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tip)
}

fn comment_card(ui: &mut egui::Ui, v: &View, c: &crate::data::CommentView) {
    let color = v.color(&c.from);
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(8, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                theme::avatar(ui, &c.from, color, 24.0, None);
                theme::pill(ui, c.from.as_str(), color);
                ui.label(
                    RichText::new(format_ts(c.ts))
                        .size(12.0)
                        .color(theme::MUTED),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if copy_button(ui, &v.tr.t("common.actions.copyToClipboard")).clicked() {
                        ui.ctx().copy_text(c.text.clone());
                    }
                });
            });
            ui.add_space(4.0);
            ui.add(egui::Label::new(&c.text).wrap().selectable(true));
        });
    theme::rule(ui);
}

/// Event timestamps are unix milliseconds. Rendered in UTC.
fn format_ts(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
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

// ---------------------------------------------------------------------------------------------

/// The Create Task modal. The shell calls `clear` after a successful create; Cancel / X / Esc
/// close it locally.
#[derive(Debug, Default)]
pub struct CreateTaskDialog {
    open: bool,
    subject: String,
    description: String,
    assignee: String,
    blocked_by: Vec<String>,
    start_now: bool,
    prompt: String,
    blocked_search: String,
    show_optional: bool,
}

impl CreateTaskDialog {
    /// The intent for Create, or `None` while the subject is blank.
    fn submit(&self) -> Option<Intent> {
        let subject = self.subject.trim();
        if subject.is_empty() {
            return None;
        }
        Some(Intent::CreateTask {
            cmd: GuiCmd::CreateTask {
                subject: subject.to_string(),
                description: self.description.trim().to_string(),
                owner: nonempty(&self.assignee),
                blocked_by: self.blocked_by.clone(),
            },
            start: self.start_now,
            prompt: nonempty(&self.prompt),
        })
    }

    pub fn open(&mut self, owner: Option<String>) {
        *self = CreateTaskDialog {
            open: true,
            assignee: owner.unwrap_or_default(),
            ..Default::default()
        };
    }

    /// Reset the form and close.
    pub fn clear(&mut self) {
        *self = CreateTaskDialog::default();
    }

    pub fn show(&mut self, ctx: &egui::Context, v: &View, out: &mut Intents) {
        if !self.open {
            return;
        }
        let width = (ctx.content_rect().width() * 0.92).min(580.0);
        let members: Vec<String> = v.members().iter().map(|m| m.name.clone()).collect();
        let open_tasks: Vec<_> = v
            .state
            .tasks
            .iter()
            .filter(|t| !t.archived && t.status != Status::Done)
            .collect();
        let mut open = true;
        let cancel = std::cell::Cell::new(false);
        let form = std::cell::RefCell::new(std::mem::take(self));
        let title = v.tr.t("team.tasks.createTask.title");
        crate::forms::dialog_with_footer(
            ctx,
            "create_task_dialog",
            &title,
            width,
            &mut open,
            |ui| {
                let mut f = form.borrow_mut();
                ui.label(
                    RichText::new(
                        "The task will be created in the team's tasks/ directory and appear on the Kanban board.",
                    )
                    .size(12.0)
                    .color(theme::MUTED),
                );
                ui.add_space(10.0);
                field_label(ui, &v.tr.t("team.tasks.createTask.subject"));
                text_field(ui, &mut f.subject, "What needs to be done?", 1);
                ui.add_space(8.0);
                field_label(ui, &v.tr.t("team.taskDetail.sections.description"));
                text_field(
                    ui,
                    &mut f.description,
                    "Task details (supports markdown)",
                    4,
                );
                ui.add_space(8.0);
                field_label(ui, &v.tr.t("team.tasks.createTask.assignee"));
                member_combo(
                    ui,
                    "ct_assignee",
                    &v.tr.t("team.taskDetail.unassigned"),
                    &members,
                    &mut f.assignee,
                );
                ui.add_space(8.0);
                field_label(ui, &v.tr.t("team.taskDetail.related.blockedBy"));
                blocked_picker(ui, v, &open_tasks, &mut f);
                ui.add_space(8.0);
                let label = if f.show_optional {
                    "Hide optional fields"
                } else {
                    "Show optional fields"
                };
                if ui
                    .add(
                        egui::Label::new(RichText::new(label).color(theme::ACCENT))
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    f.show_optional = !f.show_optional;
                }
                if f.show_optional || !f.prompt.is_empty() {
                    ui.add_space(6.0);
                    field_label(ui, &v.tr.t("team.tasks.createTask.promptOptional"));
                    text_field(ui, &mut f.prompt, &v.tr.t("rteams.task.promptHint"), 3);
                }
                ui.add_space(8.0);
                let mut start = f.start_now;
                if ui
                    .checkbox(&mut start, v.tr.t("team.tasks.createTask.startImmediately"))
                    .changed()
                {
                    f.start_now = start;
                }
            },
            |ui| {
                let f = form.borrow();
                let ready = !f.subject.trim().is_empty();
                let create = ui.add_enabled(
                    ready,
                    egui::Button::new(
                        RichText::new(v.tr.t("team.tasks.createTask.create"))
                            .strong()
                            .color(Color32::WHITE),
                    )
                    .fill(Color32::from_rgb(0x4f, 0x46, 0xe5))
                    .corner_radius(255)
                    .min_size(egui::vec2(80.0, 30.0)),
                );
                if create.clicked() {
                    if let Some(i) = f.submit() {
                        out.push(i);
                    }
                }
                if theme::pill_button(ui, &v.tr.t("team.tasks.createTask.cancel"), false).clicked()
                {
                    cancel.set(true);
                }
            },
        );
        *self = form.into_inner();
        if cancel.get() || !open {
            self.open = false;
        }
    }
}

fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).strong().color(theme::TEXT_2));
    ui.add_space(2.0);
}

/// Searchable multi-select for the blocking tasks: chips for the picks, a popup with a search box.
fn blocked_picker(
    ui: &mut egui::Ui,
    v: &View,
    tasks: &[&rteams_core::Task],
    f: &mut CreateTaskDialog,
) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        let picked = f.blocked_by.clone();
        for id in &picked {
            let subject = tasks
                .iter()
                .find(|t| &t.id == id)
                .map_or(String::new(), |t| clip_chars(&t.subject, 28));
            let text = format!("#{} {}  \u{00D7}", theme::short_id(id), subject);
            if theme::pill_button(ui, &text, true).clicked() {
                f.blocked_by.retain(|x| x != id);
            }
        }
        let btn = theme::pill_icon_button(
            ui,
            Some(Icon::Search),
            if tasks.is_empty() {
                "No open tasks"
            } else {
                "Search tasks..."
            },
            false,
        );
        egui::Popup::menu(&btn)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_min_width(320.0);
                text_field(ui, &mut f.blocked_search, "Search tasks...", 1);
                ui.add_space(4.0);
                let q = f.blocked_search.trim().to_lowercase();
                egui::ScrollArea::vertical()
                    .id_salt("ct_blocked")
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for t in tasks {
                            let text = format!(
                                "#{} {}",
                                theme::short_id(&t.id),
                                clip_chars(&t.subject, 44)
                            );
                            if !q.is_empty() && !text.to_lowercase().contains(&q) {
                                continue;
                            }
                            let on = f.blocked_by.contains(&t.id);
                            let icon = on.then_some(Icon::Check);
                            if theme::menu_item(ui, icon, &text).clicked() {
                                if on {
                                    f.blocked_by.retain(|x| x != &t.id);
                                } else {
                                    f.blocked_by.push(t.id.clone());
                                }
                            }
                        }
                    });
            });
    });
    let _ = v;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::{fixture, render};

    #[test]
    fn availability_follows_lifecycle() {
        use Status::*;
        for s in Status::ALL {
            let a = available(s);
            assert_eq!(
                a.start,
                matches!(s, Pending | NeedsFix | Done),
                "start {s:?}"
            );
            assert_eq!(
                a.complete,
                matches!(s, InProgress | Approved),
                "complete {s:?}"
            );
            assert_eq!(a.request_review, s == InProgress, "request_review {s:?}");
            assert_eq!(a.review, s == Review, "review {s:?}");
            assert_eq!(
                a.block,
                matches!(s, Pending | InProgress | NeedsFix),
                "block {s:?}"
            );
            assert_eq!(a.unblock, s == Blocked, "unblock {s:?}");
        }
    }

    #[test]
    fn draft_resets_on_task_change_only() {
        let mut d = TaskDialog::default();
        assert!(!d.is_open());
        d.open("1".into());
        d.draft.comment = "hi".into();
        d.open("1".into());
        assert_eq!(d.draft.comment, "hi");
        d.open("2".into());
        assert!(d.draft.comment.is_empty());
        d.draft.reviewer = "bob".into();
        d.clear_inputs();
        assert!(d.is_open() && d.draft.reviewer.is_empty());
        d.close();
        assert!(!d.is_open());
    }

    #[test]
    fn create_emits_start_and_prompt() {
        let mut c = CreateTaskDialog::default();
        assert!(c.submit().is_none());
        c.open(Some("ada".into()));
        c.subject = " Do it ".into();
        c.start_now = true;
        c.prompt = "  go  ".into();
        match c.submit() {
            Some(Intent::CreateTask { cmd, start, prompt }) => {
                assert!(start);
                assert_eq!(prompt.as_deref(), Some("go"));
                assert!(
                    matches!(cmd, GuiCmd::CreateTask { ref subject, ref owner, .. }
                    if subject == "Do it" && owner.as_deref() == Some("ada"))
                );
            }
            other => panic!("{other:?}"),
        }
        c.prompt = "  ".into();
        c.start_now = false;
        assert!(matches!(
            c.submit(),
            Some(Intent::CreateTask {
                start: false,
                prompt: None,
                ..
            })
        ));
    }

    #[test]
    fn edit_intents_and_badges() {
        assert!(title_intent("1", "a", "a ").is_none());
        assert!(title_intent("1", "a", " ").is_none());
        assert!(title_intent("1", "a", "b").is_some());
        assert!(desc_intent("1", "x", "x").is_none());
        assert!(desc_intent("1", "x", "y").is_some());
        assert_eq!(
            review_badge(crate::view::testing::tr(), "approved 12")
                .unwrap()
                .0,
            "Approved"
        );
        assert_eq!(
            review_badge(crate::view::testing::tr(), "requested review of 1 from b")
                .unwrap()
                .0,
            "Review requested"
        );
        assert_eq!(
            review_badge(crate::view::testing::tr(), "requested changes on 1: x")
                .unwrap()
                .0,
            "Changes requested"
        );
        assert!(review_badge(crate::view::testing::tr(), "started 1").is_none());
    }

    #[test]
    fn edit_mode_renders_headless() {
        let (home, data) = fixture("taskedit");
        let view = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let id = data.state().tasks[0].id.clone();
        let mut dialog = TaskDialog::default();
        dialog.open(id);
        dialog.draft.title_edit = Some("New".into());
        dialog.draft.desc_edit = Some("Desc".into());
        render(|ui| dialog.show(ui.ctx(), &view, &mut Intents::default()));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn timestamps_render_as_utc() {
        assert_eq!(format_ts(0), "1970-01-01 00:00");
        assert_eq!(format_ts(951_782_400_000), "2000-02-29 00:00");
    }

    #[test]
    fn dialogs_render_headless() {
        let (home, data) = fixture("taskdlg");
        let view = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let ids: Vec<String> = data.state().tasks.iter().map(|t| t.id.clone()).collect();
        for id in ids {
            let mut dialog = TaskDialog::default();
            dialog.open(id);
            let mut intents = Intents::default();
            render(|ui| dialog.show(ui.ctx(), &view, &mut intents));
        }
        let mut dialog = TaskDialog::default();
        dialog.open("nope".into());
        render(|ui| dialog.show(ui.ctx(), &view, &mut Intents::default()));

        let mut create = CreateTaskDialog::default();
        create.open(Some("ada".into()));
        assert!(create.open);
        render(|ui| create.show(ui.ctx(), &view, &mut Intents::default()));
        create.clear();
        assert!(!create.open);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn diff_counts_and_names() {
        let d = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n+more\n ctx\n";
        assert_eq!(diff_counts(d), (2, 1));
        assert_eq!(base_name("/a/b/c.txt"), "c.txt");
        assert_eq!(clip_chars("abcdef", 3), "abc\u{2026}");
    }

    #[test]
    fn new_sections_render_headless() {
        let (home, data) = fixture("tasknew");
        // A repo with an edited file and a run that edited it while on task 1.
        let repo = home.join("proj");
        git::init_repo(&repo).unwrap();
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git::initial_commit(&repo).unwrap();
        std::fs::write(repo.join("a.txt"), "two\n").unwrap();
        let runs_dir = home.join("t/runs/ada");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let line = |name: &str, input: serde_json::Value| {
            serde_json::json!({"type":"assistant","message":{"content":[
                {"type":"tool_use","name":name,"input":input}]}})
            .to_string()
        };
        let path = repo.join("a.txt").to_string_lossy().into_owned();
        std::fs::write(
            runs_dir.join("1.jsonl"),
            [
                line("mcp__rteams__task_start", serde_json::json!({"task":"1"})),
                line("Edit", serde_json::json!({"file_path": path})),
                "{\"type\":\"result\",\"usage\":{\"input_tokens\":3,\"output_tokens\":4}}".into(),
            ]
            .join("\n"),
        )
        .unwrap();
        let view = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let id = data.state().tasks[0].id.clone();
        for repo in [Some(repo.clone()), None] {
            let mut dialog = TaskDialog::default();
            dialog.set_paths(home.clone(), "t".into(), repo);
            dialog.open(id.clone());
            dialog.draft.expanded = Some(path.clone());
            dialog.draft.revert_confirm = Some(path.clone());
            dialog.draft.attach_path = "/tmp/x.txt".into();
            let mut intents = Intents::default();
            render(|ui| dialog.show(ui.ctx(), &view, &mut intents));
            let cache = dialog.draft.runs.as_ref().unwrap();
            assert_eq!(cache.log.len(), 3);
            assert_eq!(cache.files.len(), usize::from(dialog.repo.is_some()));
            if dialog.repo.is_some() {
                assert_eq!((cache.files[0].added, cache.files[0].removed), (1, 1));
            }
        }
        let _ = std::fs::remove_dir_all(home);
    }
}
