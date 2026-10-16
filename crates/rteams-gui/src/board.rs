//! Kanban board, styled after agent-teams: five flat columns (TODO / IN PROGRESS / REVIEW on the
//! first row, DONE / APPROVED on the second), each with a colored glow, an uppercase header with
//! a count, and flat cards separated by a hairline. Hovering a card reveals quick actions.

use std::collections::HashMap;

use eframe::egui::{
    self, text::LayoutJob, Align, Color32, FontId, Layout as ELayout, Margin, Rect, RichText,
    Sense, Shape, Stroke, Ui, Vec2,
};
use rteams_core::{Scope, Status, Task};

use crate::actions::GuiCmd;
use crate::icons::{self, Icon};
use crate::theme::{self, short_id};
use crate::view::{Intent, Intents, View};

const MIN_COL_W: f32 = 200.0;
const MIN_ROW_H: f32 = 340.0;
const CARD_MIN_H: f32 = 66.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// Most recently touched first (last comment or creation).
    #[default]
    Updated,
    /// Newest created first.
    Created,
    /// By owner name, unassigned last.
    Owner,
    /// The team's saved column order (drag cards within a column to change it).
    Manual,
}

impl Sort {
    const ALL: [Sort; 4] = [Sort::Updated, Sort::Created, Sort::Owner, Sort::Manual];

    fn label_key(self) -> &'static str {
        match self {
            Sort::Updated => "rteams.board.sort.updated",
            Sort::Created => "rteams.board.sort.created",
            Sort::Owner => "rteams.board.sort.owner",
            Sort::Manual => "rteams.board.sort.manual",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    /// 3 + 2 grid.
    #[default]
    Grid,
    /// All visible columns side by side.
    Columns,
}

#[derive(Debug, Default)]
pub struct BoardFilter {
    pub search: String,
    pub owner: Option<String>,
    pub sort: Sort,
    pub layout: Layout,
    /// Columns hidden from the board, indexed like `Col::ALL`.
    pub hidden: [bool; 5],
    /// Show the trash (archived tasks) instead of the board.
    pub show_trash: bool,
    /// Only these task ids (the selected task folder), when set.
    pub folder: Option<std::collections::HashSet<String>>,
}

impl BoardFilter {
    /// Number of active filters (owner and hidden columns) for the toolbar badge.
    pub fn active_count(&self) -> usize {
        usize::from(self.owner.is_some()) + self.hidden.iter().filter(|h| **h).count()
    }
}

/// Columns that are not hidden, in display order.
pub fn visible_columns(f: &BoardFilter) -> Vec<Col> {
    Col::ALL
        .into_iter()
        .filter(|c| !f.hidden[col_index(*c)])
        .collect()
}

/// Archived tasks only, newest first.
pub fn trash_list(tasks: &[Task]) -> Vec<&Task> {
    let mut v: Vec<&Task> = tasks.iter().filter(|t| t.archived).collect();
    v.sort_by_key(|t| std::cmp::Reverse(t.created_seq));
    v
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Col {
    Todo,
    InProgress,
    Review,
    Done,
    Approved,
}

impl Col {
    const ALL: [Col; 5] = [
        Col::Todo,
        Col::InProgress,
        Col::Review,
        Col::Done,
        Col::Approved,
    ];
    const ROW1: [Col; 3] = [Col::Todo, Col::InProgress, Col::Review];
    const ROW2: [Col; 2] = [Col::Done, Col::Approved];

    fn title_key(self) -> &'static str {
        match self {
            Col::Todo => "team.kanban.columns.todo",
            Col::InProgress => "team.kanban.columns.inProgress",
            Col::Review => "team.kanban.columns.review",
            Col::Done => "team.kanban.columns.done",
            Col::Approved => "team.kanban.columns.approved",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Col::Todo => Icon::List,
            Col::InProgress => Icon::Play,
            Col::Review => Icon::Eye,
            Col::Done => Icon::Check,
            Col::Approved => Icon::Shield,
        }
    }

    fn color(self) -> Color32 {
        match self {
            Col::Todo => theme::COL_TODO,
            Col::InProgress => theme::COL_IN_PROGRESS,
            Col::Review => theme::COL_REVIEW,
            Col::Done => theme::COL_DONE,
            Col::Approved => theme::COL_APPROVED,
        }
    }

    /// Key of this column in `Team::column_order`: its primary status in snake_case.
    pub fn order_key(self) -> &'static str {
        match self {
            Col::Todo => "pending",
            Col::InProgress => "in_progress",
            Col::Review => "review",
            Col::Done => "done",
            Col::Approved => "approved",
        }
    }

    fn has_add(self) -> bool {
        matches!(self, Col::Todo | Col::InProgress)
    }
}

/// Which column a status is shown in.
pub fn column_of(s: Status) -> Col {
    match s {
        Status::Pending | Status::Blocked => Col::Todo,
        Status::InProgress | Status::NeedsFix => Col::InProgress,
        Status::Review => Col::Review,
        Status::Done => Col::Done,
        Status::Approved => Col::Approved,
    }
}

/// `#id` prefix match, or case-insensitive text in subject / description. Empty matches all.
pub fn matches_search(t: &Task, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    if let Some(id) = q.strip_prefix('#') {
        return t.id.to_lowercase().starts_with(id);
    }
    t.subject.to_lowercase().contains(&q) || t.description.to_lowercase().contains(&q)
}

/// Per-task facts derived once per frame.
struct Ctx<'a> {
    v: &'a View<'a>,
    comments: HashMap<&'a str, usize>,
    last_touch: HashMap<&'a str, u64>,
}

impl<'a> Ctx<'a> {
    fn new(v: &'a View<'a>) -> Self {
        let mut comments: HashMap<&str, usize> = HashMap::new();
        let mut last_touch: HashMap<&str, u64> = HashMap::new();
        for m in &v.state.messages {
            if let Scope::Task { task } = &m.scope {
                *comments.entry(task.as_str()).or_default() += 1;
                let e = last_touch.entry(task.as_str()).or_default();
                *e = (*e).max(m.seq);
            }
        }
        Ctx {
            v,
            comments,
            last_touch,
        }
    }

    fn touched(&self, t: &Task) -> u64 {
        self.last_touch
            .get(t.id.as_str())
            .copied()
            .unwrap_or(0)
            .max(t.created_seq)
    }
}

/// Visible tasks per column, filtered and sorted.
fn collect<'a>(ctx: &Ctx<'a>, f: &BoardFilter) -> [Vec<&'a Task>; 5] {
    let mut tasks: Vec<&Task> = ctx
        .v
        .state
        .tasks
        .iter()
        .filter(|t| !t.archived)
        .filter(|t| f.owner.as_ref().is_none_or(|o| t.owner.as_ref() == Some(o)))
        .filter(|t| matches_search(t, &f.search))
        .filter(|t| f.folder.as_ref().is_none_or(|ids| ids.contains(&t.id)))
        .collect();
    match f.sort {
        Sort::Updated => tasks.sort_by_key(|t| std::cmp::Reverse(ctx.touched(t))),
        Sort::Created => tasks.sort_by_key(|t| std::cmp::Reverse(t.created_seq)),
        Sort::Owner => tasks.sort_by(|a, b| match (&a.owner, &b.owner) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }),
        Sort::Manual => tasks.sort_by_key(|t| std::cmp::Reverse(ctx.touched(t))),
    }
    let mut out: [Vec<&Task>; 5] = Default::default();
    for t in tasks {
        out[col_index(column_of(t.status))].push(t);
    }
    if f.sort == Sort::Manual {
        if let Some(team) = ctx.v.team() {
            for c in Col::ALL {
                let i = col_index(c);
                if let Some(order) = team.column_order.get(c.order_key()) {
                    out[i] = apply_manual_order(std::mem::take(&mut out[i]), order);
                }
            }
        }
    }
    out
}

/// Tasks listed in `order` come first, in that order; the rest keep their relative order after.
pub fn apply_manual_order<'a>(tasks: Vec<&'a Task>, order: &[String]) -> Vec<&'a Task> {
    let (mut listed, rest): (Vec<&Task>, Vec<&Task>) =
        tasks.into_iter().partition(|t| order.contains(&t.id));
    listed.sort_by_key(|t| order.iter().position(|o| *o == t.id));
    listed.extend(rest);
    listed
}

/// `ids` with `moving` taken out and re-inserted next to `target` (before it, or after when
/// `after`). Unknown ids leave the list unchanged.
pub fn move_in_order(ids: &[String], moving: &str, target: &str, after: bool) -> Vec<String> {
    if moving == target || !ids.iter().any(|i| i == moving) || !ids.iter().any(|i| i == target) {
        return ids.to_vec();
    }
    let mut v: Vec<String> = ids.iter().filter(|i| *i != moving).cloned().collect();
    let at = v.iter().position(|i| i == target).unwrap_or(0) + usize::from(after);
    v.insert(at.min(v.len()), moving.to_string());
    v
}

/// What is being dragged: a card of column `col` (index like `col_index`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct DragItem {
    col: usize,
    task: String,
}

fn col_index(c: Col) -> usize {
    match c {
        Col::Todo => 0,
        Col::InProgress => 1,
        Col::Review => 2,
        Col::Done => 3,
        Col::Approved => 4,
    }
}

pub fn show(ui: &mut Ui, v: &View, filter: &mut BoardFilter, out: &mut Intents) {
    toolbar(ui, v, filter, out);
    ui.add_space(4.0);

    if filter.show_trash {
        trash_view(ui, v, out);
        return;
    }

    let ctx = Ctx::new(v);
    let cols = collect(&ctx, filter);
    let visible = visible_columns(filter);
    if visible.is_empty() {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(v.tr.t("rteams.board.allHidden")).color(theme::MUTED));
        });
        return;
    }

    // Rows of columns: grid keeps the 3 + 2 split, columns mode is a single row.
    let rows: Vec<Vec<Col>> = match filter.layout {
        Layout::Grid => [&Col::ROW1[..], &Col::ROW2[..]]
            .iter()
            .map(|r| r.iter().copied().filter(|c| visible.contains(c)).collect())
            .filter(|r: &Vec<Col>| !r.is_empty())
            .collect(),
        Layout::Columns => vec![visible.clone()],
    };

    let avail = ui.available_size();
    let aw = if avail.x.is_finite() { avail.x } else { 900.0 };
    let ah = if avail.y.is_finite() { avail.y } else { 600.0 };
    let max_per_row = rows.iter().map(Vec::len).max().unwrap_or(1) as f32;
    let w = aw.max(max_per_row * MIN_COL_W).max(1.0);
    let row_h = (ah / rows.len() as f32).floor().max(MIN_ROW_H);

    // Rows keep their height (>= MIN_ROW_H); when the region is shorter the page scrolls, so the
    // bottom row is never clipped.
    egui::ScrollArea::horizontal()
        .id_salt(("board-grid", filter.layout == Layout::Columns))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(w, row_h * rows.len() as f32), Sense::hover());
            for (ri, cols_in_row) in rows.iter().enumerate() {
                let dy = row_h * ri as f32;
                let n = cols_in_row.len() as f32;
                let cw = (w / n).floor().max(1.0);
                for (i, c) in cols_in_row.iter().enumerate() {
                    let min = rect.min + Vec2::new(cw * i as f32, dy);
                    let r = Rect::from_min_size(min, Vec2::new(cw, row_h));
                    column(
                        ui,
                        &ctx,
                        r,
                        *c,
                        &cols[col_index(*c)],
                        filter.sort == Sort::Manual,
                        out,
                    );
                }
            }
        });
}

fn trash_view(ui: &mut Ui, v: &View, out: &mut Intents) {
    let items = trash_list(&v.state.tasks);
    ui.label(
        RichText::new(v.tr.t("team.kanban.board.trash"))
            .size(14.0)
            .strong(),
    );
    ui.add_space(6.0);
    if items.is_empty() {
        ui.label(RichText::new(v.tr.t("rteams.board.trashEmpty")).color(theme::MUTED));
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("board-trash")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for t in items {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("#{}", short_id(&t.id)))
                            .size(11.0)
                            .color(theme::MUTED),
                    );
                    let mut job = LayoutJob::simple(
                        t.subject.clone(),
                        FontId::proportional(13.5),
                        theme::TEXT,
                        (ui.available_width() - 260.0).max(60.0),
                    );
                    job.wrap.max_rows = 1;
                    ui.add(egui::Label::new(job).selectable(false));
                    ui.with_layout(ELayout::right_to_left(Align::Center), |ui| {
                        if v.actor.is_some()
                            && ui.button(v.tr.t("team.kanban.trash.restore")).clicked()
                        {
                            out.push(Intent::Run(GuiCmd::Archive {
                                task: t.id.clone(),
                                archived: false,
                            }));
                        }
                        theme::pill(ui, format!("{:?}", t.status), theme::MUTED);
                        if let Some(o) = &t.owner {
                            ui.label(RichText::new(o).size(11.5).color(v.color(o)));
                        }
                    });
                });
                ui.add_space(2.0);
                theme::rule(ui);
            }
        });
}

fn toolbar(ui: &mut Ui, v: &View, f: &mut BoardFilter, out: &mut Intents) {
    let archived = v.state.tasks.iter().filter(|t| t.archived).count();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        // Search box: rounded, with a leading magnifier.
        egui::Frame::new()
            .fill(theme::CARD)
            .stroke(Stroke::new(1.0, theme::hairline(0.12)))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(Margin::symmetric(8, 0))
            .show(ui, |ui| {
                ui.set_height(30.0);
                ui.horizontal_centered(|ui| {
                    icons::icon(ui, Icon::Search, 14.0, theme::MUTED);
                    ui.add(
                        egui::TextEdit::singleline(&mut f.search)
                            .hint_text(v.tr.t("team.kanban.search.placeholder"))
                            .frame(egui::Frame::NONE)
                            .desired_width(220.0),
                    );
                });
            });
        let n = f.active_count();
        let label = if n > 0 {
            format!("{} ({n})", v.tr.t("rteams.board.filter"))
        } else {
            v.tr.t("rteams.board.filter")
        };
        let fr = theme::pill_icon_button(ui, Some(Icon::Filter), &label, n > 0);
        egui::Popup::menu(&fr)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_min_width(190.0);
                ui.label(RichText::new(v.tr.t("team.kanban.filter.title")).strong());
                ui.add_space(4.0);
                ui.label(
                    RichText::new(v.tr.t("team.kanban.filter.teammate"))
                        .size(11.0)
                        .color(theme::MUTED),
                );
                ui.selectable_value(&mut f.owner, None, v.tr.t("rteams.board.allTeammates"));
                for m in v.members() {
                    ui.selectable_value(&mut f.owner, Some(m.name.clone()), &m.name);
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(v.tr.t("team.kanban.filter.column"))
                        .size(11.0)
                        .color(theme::MUTED),
                );
                for c in Col::ALL {
                    let mut shown = !f.hidden[col_index(c)];
                    if ui.checkbox(&mut shown, v.tr.t(c.title_key())).changed() {
                        f.hidden[col_index(c)] = !shown;
                    }
                }
                ui.add_space(4.0);
                if ui.button(v.tr.t("team.kanban.filter.clearAll")).clicked() {
                    f.owner = None;
                    f.hidden = [false; 5];
                }
            });
        let sr = theme::pill_icon_button(ui, None, &v.tr.t(f.sort.label_key()), false);
        egui::Popup::menu(&sr).show(|ui| {
            ui.set_min_width(170.0);
            for s in Sort::ALL {
                if theme::menu_item(ui, None, &v.tr.t(s.label_key())).clicked() {
                    f.sort = s;
                    ui.close();
                }
            }
        });
        ui.with_layout(ELayout::right_to_left(Align::Center), |ui| {
            if theme::pill_icon_button(ui, Some(Icon::Plus), &v.tr.t("rteams.board.newTask"), false)
                .clicked()
            {
                out.push(Intent::NewTask { owner: None });
            }
            ui.add_space(4.0);
            egui::Frame::new()
                .stroke(Stroke::new(1.0, theme::hairline(0.12)))
                .corner_radius(egui::CornerRadius::same(8))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    // right-to-left: columns first, then grid.
                    if theme::icon_button_active(
                        ui,
                        Icon::Columns,
                        &v.tr.t("team.kanban.board.columnsView"),
                        f.layout == Layout::Columns,
                    )
                    .clicked()
                    {
                        f.layout = Layout::Columns;
                    }
                    if theme::icon_button_active(
                        ui,
                        Icon::LayoutDashboard,
                        &v.tr.t("team.kanban.board.gridView"),
                        f.layout == Layout::Grid,
                    )
                    .clicked()
                    {
                        f.layout = Layout::Grid;
                    }
                });
            if archived > 0 || f.show_trash {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    ui.label(
                        RichText::new(archived.to_string())
                            .size(12.0)
                            .color(theme::MUTED),
                    );
                    if theme::icon_button_active(
                        ui,
                        Icon::Trash,
                        &v.tr.t("team.kanban.board.trash"),
                        f.show_trash,
                    )
                    .clicked()
                    {
                        f.show_trash = !f.show_trash;
                    }
                });
            }
        });
    });
}

/// A rect from two corners that can never have negative size.
fn safe_rect(min: egui::Pos2, max: egui::Pos2) -> Rect {
    Rect::from_min_max(min, egui::pos2(max.x.max(min.x), max.y.max(min.y)))
}

fn column(
    ui: &mut Ui,
    ctx: &Ctx,
    rect: Rect,
    col: Col,
    tasks: &[&Task],
    manual: bool,
    out: &mut Intents,
) {
    let color = col.color();
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 0.0, theme::CARD);
    theme::glow(&painter, rect, color, 220.0);

    let inner = safe_rect(
        rect.min + Vec2::new(14.0, 12.0),
        rect.max - Vec2::new(14.0, 0.0),
    );
    let mut cui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(ELayout::top_down(Align::Min)),
    );
    cui.set_clip_rect(rect.intersect(ui.clip_rect()));
    cui.spacing_mut().item_spacing = Vec2::new(6.0, 0.0);

    // header
    cui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        icons::icon(ui, col.icon(), 14.0, color);
        ui.label(
            RichText::new(ctx.v.tr.t(col.title_key()).to_uppercase())
                .size(12.0)
                .strong()
                .extra_letter_spacing(0.8),
        );
        ui.with_layout(ELayout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(tasks.len().to_string())
                    .size(12.0)
                    .color(theme::MUTED),
            );
        });
    });
    cui.add_space(10.0);

    let body_h = cui.available_height().max(1.0);
    egui::ScrollArea::vertical()
        .id_salt(("board-col", col_index(col)))
        .max_height(body_h)
        .auto_shrink([false, false])
        .show(&mut cui, |ui| {
            let n = tasks.len();
            let mut dropped: Option<(String, String, bool)> = None;
            for (i, t) in tasks.iter().enumerate() {
                let rect = card(ui, ctx, col, t, manual, out);
                if manual {
                    if let Some(d) = drop_target(ui, col, rect) {
                        dropped = Some((d.0, t.id.clone(), d.1));
                    }
                }
                if i + 1 < n {
                    separator(ui);
                }
            }
            if let Some((moving, target, after)) = dropped {
                let ids: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();
                let order = move_in_order(&ids, &moving, &target, after);
                if order != ids {
                    out.push(Intent::Run(GuiCmd::SetColumnOrder {
                        column: col.order_key().to_string(),
                        order,
                    }));
                }
            }
            if col.has_add() {
                ui.add_space(12.0);
                if add_task_button(ui, &ctx.v.tr.t("rteams.board.addTask")) {
                    out.push(Intent::NewTask { owner: None });
                }
            }
            ui.add_space(16.0);
        });
}

/// While a card of `col` is dragged over `rect`: draw the insertion line and, when the pointer
/// is released there, return `(dragged task id, drop after this card)`.
fn drop_target(ui: &Ui, col: Col, rect: Rect) -> Option<(String, bool)> {
    let ctx = ui.ctx();
    let item = egui::DragAndDrop::payload::<DragItem>(ctx)?;
    if item.col != col_index(col) {
        return None;
    }
    let pos = ctx.pointer_interact_pos()?;
    if !rect.contains(pos) {
        return None;
    }
    let after = pos.y > rect.center().y;
    let y = if after { rect.bottom() } else { rect.top() };
    ui.painter()
        .hline(rect.x_range(), y, Stroke::new(2.0, col.color()));
    ctx.input(|i| i.pointer.any_released())
        .then(|| (item.task.clone(), after))
}

fn separator(ui: &mut Ui) {
    let w = ui.available_width().max(1.0);
    let (r, _) = ui.allocate_exact_size(Vec2::new(w, 9.0), Sense::hover());
    let half = w * 0.25;
    ui.painter().hline(
        (r.center().x - half)..=(r.center().x + half),
        r.center().y,
        Stroke::new(1.0, theme::hairline(0.10)),
    );
}

fn add_task_button(ui: &mut Ui, label: &str) -> bool {
    let w = ui.available_width().max(1.0);
    let (r, resp) = ui.allocate_exact_size(Vec2::new(w, 44.0), Sense::click());
    let a = if resp.hovered() { 0.28 } else { 0.14 };
    let pts = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    ui.painter().extend(Shape::dashed_line(
        &pts,
        Stroke::new(1.0, theme::hairline(a)),
        4.0,
        3.0,
    ));
    ui.painter().text(
        r.center(),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::proportional(13.0),
        theme::MUTED,
    );
    resp.clicked()
}

type QIcon = (Icon, Color32);
const Q_OK: QIcon = (Icon::Check, theme::SUCCESS);

#[allow(non_snake_case)]
fn ARCHIVE_ACTION(task: String) -> (QIcon, &'static str, Option<GuiCmd>) {
    (
        (Icon::Trash, theme::ERROR),
        "Archive",
        Some(GuiCmd::Archive {
            task,
            archived: true,
        }),
    )
}

/// Localized tooltip for a quick-action label (English names are the identifiers).
fn qa_tip(tr: &rteams_services::i18n::I18n, tip: &str) -> String {
    let key = match tip {
        "Start" => "team.kanban.taskCard.start",
        "Complete" => "team.kanban.taskCard.complete",
        "Request review" => "team.kanban.taskCard.requestReview",
        "Approve" => "team.kanban.taskCard.approve",
        "Request changes" => "team.kanban.taskCard.requestChanges",
        "Archive" => "common.taskContextMenu.archive",
        "Start review" => "rteams.board.startReview",
        _ => return tip.to_string(),
    };
    tr.t(key)
}

/// Quick actions legal for `s`: (icon, tooltip, command or None = open the task).
fn quick_actions(
    s: Status,
    id: &str,
    review_started: bool,
) -> Vec<(QIcon, &'static str, Option<GuiCmd>)> {
    let task = id.to_string();
    match s {
        Status::Pending | Status::NeedsFix => {
            vec![(
                (Icon::Play, theme::SUCCESS),
                "Start",
                Some(GuiCmd::Start { task }),
            )]
        }
        Status::InProgress => vec![
            (Q_OK, "Complete", Some(GuiCmd::Complete { task })),
            ((Icon::Eye, theme::COL_REVIEW), "Request review", None),
        ],
        Status::Approved => vec![
            (
                Q_OK,
                "Complete",
                Some(GuiCmd::Complete { task: task.clone() }),
            ),
            ARCHIVE_ACTION(task),
        ],
        Status::Review => {
            let mut v = Vec::new();
            if !review_started {
                v.push((
                    (Icon::Play, theme::COL_REVIEW),
                    "Start review",
                    Some(GuiCmd::StartReview { task: task.clone() }),
                ));
            }
            v.push((Q_OK, "Approve", Some(GuiCmd::Approve { task, note: None })));
            v.push(((Icon::Edit, theme::WARNING), "Request changes", None));
            v
        }
        Status::Done => vec![ARCHIVE_ACTION(task)],
        Status::Blocked => vec![],
    }
}

fn card(ui: &mut Ui, ctx: &Ctx, col: Col, t: &Task, manual: bool, out: &mut Intents) -> Rect {
    let v = ctx.v;
    let state = v.state;
    let blocked = t.status == Status::Blocked;
    let width = ui.available_width().max(1.0);
    let bg = ui.painter().add(Shape::Noop);

    let frame = egui::Frame::new()
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            let cw = (width - 20.0).max(1.0);
            ui.set_width(cw);
            ui.set_min_height(CARD_MIN_H);
            ui.add_space(2.0);
            ui.spacing_mut().item_spacing = Vec2::new(5.0, 4.0);

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("#{}", short_id(&t.id)))
                        .size(10.5)
                        .color(theme::MUTED),
                );
                let n = ctx.comments.get(t.id.as_str()).copied().unwrap_or(0);
                icons::icon(ui, Icon::MessageSquare, 12.0, theme::MUTED);
                if n > 0 {
                    theme::count_badge(ui, n, true);
                }
                ui.label(RichText::new(n.to_string()).size(10.5).color(theme::MUTED));
                if let Some(owner) = &t.owner {
                    ui.with_layout(ELayout::right_to_left(Align::Center), |ui| {
                        let c = v.color(owner);
                        ui.label(RichText::new(owner).size(12.0).color(c));
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                        ui.painter()
                            .circle_filled(r.center(), 7.0, theme::tint(c, 0.25));
                        ui.painter()
                            .circle_stroke(r.center(), 6.0, Stroke::new(1.5, c));
                    });
                }
            });

            let mut job = LayoutJob::simple(
                t.subject.clone(),
                FontId::proportional(14.5),
                theme::TEXT,
                cw,
            );
            job.wrap.max_rows = 2;
            ui.add(egui::Label::new(job).selectable(false));
            ui.add_space(4.0);

            let chips = ChipSet::of(t);
            if chips.any() || t.status == Status::NeedsFix {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(5.0, 3.0);
                    if t.status == Status::NeedsFix {
                        theme::pill(
                            ui,
                            v.tr.t("team.taskDetail.reviewStates.needsFix"),
                            theme::ERROR,
                        );
                    }
                    if chips.awaiting {
                        theme::pill(ui, v.tr.t("rteams.board.awaitingAnswer"), theme::WARNING);
                    }
                    if chips.reviewing {
                        theme::pill(ui, v.tr.t("rteams.board.reviewing"), theme::COL_REVIEW);
                    }
                    if chips.attachments > 0 {
                        ui.label(
                            RichText::new(format!("\u{1F4CE} {}", chips.attachments))
                                .size(11.0)
                                .color(theme::TEXT_2),
                        )
                        .on_hover_text(v.tr.t("team.taskDetail.sections.attachments"));
                    }
                    if chips.related > 0 {
                        ui.label(
                            RichText::new(format!("\u{1F517} {}", chips.related))
                                .size(11.0)
                                .color(theme::TEXT_2),
                        )
                        .on_hover_text(v.tr.t("team.taskDetail.related.title"));
                    }
                });
            }
            if blocked {
                let dep = t.blocked_by.first().and_then(|id| state.task(id));
                let text = match dep {
                    Some(d) => format!(
                        "\u{21A9} {} #{} {}",
                        v.tr.t("team.taskDetail.related.blockedBy"),
                        short_id(&d.id),
                        d.subject
                    ),
                    None => format!("\u{21A9} {}", v.tr.t("rteams.board.blocked")),
                };
                let mut j = LayoutJob::simple(text, FontId::proportional(11.0), theme::WARNING, cw);
                j.wrap.max_rows = 1;
                ui.add(egui::Label::new(j).selectable(false));
            } else {
                let unmet = state.unmet_dependencies(t);
                if !unmet.is_empty() {
                    let ids: Vec<String> =
                        unmet.iter().map(|i| format!("#{}", short_id(i))).collect();
                    let mut j = LayoutJob::simple(
                        v.tr.t_args("rteams.board.waitingOn", &[("ids", &ids.join(", "))]),
                        FontId::proportional(11.0),
                        theme::WARNING,
                        cw,
                    );
                    j.wrap.max_rows = 1;
                    ui.add(egui::Label::new(j).selectable(false));
                }
            }
            if col == Col::Review {
                if let Some(r) = &t.reviewer {
                    ui.add(
                        egui::Label::new(
                            RichText::new(v.tr.t_args("rteams.board.reviewer", &[("name", r)]))
                                .size(11.0)
                                .color(theme::TEXT_2),
                        )
                        .selectable(false),
                    );
                }
            }
        });

    let rect = frame.response.rect;
    let sense = if manual {
        Sense::click_and_drag()
    } else {
        Sense::click()
    };
    let resp = ui.interact(rect, ui.scope_id().with(("card", &t.id)), sense);
    let dragging_any = egui::DragAndDrop::has_any_payload(ui.ctx());
    if manual && resp.drag_started() {
        egui::DragAndDrop::set_payload(
            ui.ctx(),
            DragItem {
                col: col_index(col),
                task: t.id.clone(),
            },
        );
    }
    let being_dragged = manual
        && egui::DragAndDrop::payload::<DragItem>(ui.ctx())
            .is_some_and(|d| d.task == t.id && d.col == col_index(col));
    if being_dragged {
        paint_drag_ghost(ui, t, rect.width());
    }
    let hovered = ui.rect_contains_pointer(rect) && !dragging_any;

    // background (drawn behind the text through the reserved shape slot)
    let mut fill = Color32::TRANSPARENT;
    if blocked {
        fill = theme::tint(theme::WARNING, 0.07);
    }
    if hovered {
        fill = fill.blend(Color32::from_white_alpha(10));
    }
    if being_dragged {
        fill = fill.blend(Color32::from_white_alpha(18));
    }
    ui.painter().set(
        bg,
        Shape::rect_filled(rect, egui::CornerRadius::same(4), fill),
    );

    let mut acted = false;
    if hovered {
        if let Some(_actor) = v.actor {
            let acts = quick_actions(t.status, &t.id, t.review_started);
            let size = 24.0;
            let gap = 4.0;
            let total = acts.len() as f32 * size + acts.len().saturating_sub(1) as f32 * gap;
            let mut y = rect.center().y - total / 2.0;
            for ((ic, col_c), tip, cmd) in acts {
                let r = Rect::from_min_size(egui::pos2(rect.left() + 2.0, y), Vec2::splat(size));
                let id = ui.scope_id().with(("qa", &t.id, tip));
                let b = ui.interact(r, id, Sense::click());
                let fill = theme::tint(col_c, if b.hovered() { 0.28 } else { 0.16 });
                let p = ui.painter();
                p.circle_filled(r.center(), size / 2.0, theme::RAISED);
                p.circle_filled(r.center(), size / 2.0, fill);
                p.circle_stroke(
                    r.center(),
                    size / 2.0,
                    Stroke::new(1.0, theme::tint(col_c, 0.5)),
                );
                icons::draw(
                    p,
                    Rect::from_center_size(r.center(), Vec2::splat(12.0)),
                    ic,
                    col_c,
                );
                if b.on_hover_text(qa_tip(v.tr, tip)).clicked() {
                    acted = true;
                    match cmd {
                        Some(c) => out.push(Intent::Run(c)),
                        None => out.push(Intent::OpenTask(t.id.clone())),
                    }
                }
                y += size + gap;
            }
        }
    }
    if hovered
        && !acted
        && ui.ctx().memory(|m| m.focused().is_none())
        && ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Space))
    {
        out.push(Intent::OpenTask(t.id.clone()));
        acted = true;
    }
    if resp.clicked() && !acted {
        out.push(Intent::OpenTask(t.id.clone()));
    }
    if being_dragged {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if hovered {
        ui.ctx().set_cursor_icon(if manual {
            egui::CursorIcon::Grab
        } else {
            egui::CursorIcon::PointingHand
        });
    }
    rect
}

/// Small floating copy of the card's subject that follows the pointer while dragging.
fn paint_drag_ghost(ui: &Ui, t: &Task, width: f32) {
    let Some(pos) = ui.ctx().pointer_interact_pos() else {
        return;
    };
    let layer = egui::LayerId::new(egui::Order::Tooltip, ui.scope_id().with("board-drag-ghost"));
    let p = ui.ctx().layer_painter(layer);
    let w = width.clamp(80.0, 260.0);
    let galley = p.layout(
        t.subject.clone(),
        FontId::proportional(13.0),
        theme::TEXT,
        (w - 16.0).max(1.0),
    );
    let r = Rect::from_min_size(pos + Vec2::new(8.0, 8.0), galley.size() + Vec2::splat(12.0));
    p.rect_filled(r, 6.0, theme::tint(theme::RAISED, 0.92));
    p.rect_stroke(
        r,
        6.0,
        Stroke::new(1.0, theme::hairline(0.25)),
        egui::StrokeKind::Inside,
    );
    p.galley(r.min + Vec2::splat(6.0), galley, theme::TEXT);
}

/// Which chips a card shows (derived from the task, so it is testable without drawing).
#[derive(Debug, Default, PartialEq, Eq)]
struct ChipSet {
    awaiting: bool,
    attachments: usize,
    related: usize,
    reviewing: bool,
}

impl ChipSet {
    fn of(t: &Task) -> Self {
        ChipSet {
            awaiting: t.clarification.is_some(),
            attachments: t.attachments.len(),
            related: t.related.len(),
            reviewing: t.review_started && t.status == Status::Review,
        }
    }

    fn any(&self) -> bool {
        self.awaiting || self.attachments > 0 || self.related > 0 || self.reviewing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::{fixture, render};

    fn task(id: &str, subject: &str, desc: &str) -> Task {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "subject": subject,
            "description": desc,
            "owner": null,
            "status": "pending",
            "blocked_by": [],
            "reviewer": null,
            "blocked_from": null,
            "blocked_reason": null,
            "archived": false,
            "created_seq": 0,
        }))
        .unwrap()
    }

    #[test]
    fn every_status_maps_to_a_column() {
        assert_eq!(column_of(Status::Pending), Col::Todo);
        assert_eq!(column_of(Status::Blocked), Col::Todo);
        assert_eq!(column_of(Status::InProgress), Col::InProgress);
        assert_eq!(column_of(Status::NeedsFix), Col::InProgress);
        assert_eq!(column_of(Status::Review), Col::Review);
        assert_eq!(column_of(Status::Done), Col::Done);
        assert_eq!(column_of(Status::Approved), Col::Approved);
        for s in Status::ALL {
            let _ = column_of(s).title_key();
        }
    }

    #[test]
    fn search_matches_id_prefix_and_text() {
        let t = task("abc12345", "Fix Login", "handles OAuth");
        assert!(matches_search(&t, ""));
        assert!(matches_search(&t, "#abc"));
        assert!(!matches_search(&t, "#bc"));
        assert!(matches_search(&t, "login"));
        assert!(matches_search(&t, "OAUTH"));
        assert!(!matches_search(&t, "nothing"));
    }

    #[test]
    fn review_actions_include_approve_and_changes() {
        let a = quick_actions(Status::Review, "1", true);
        assert_eq!(a.len(), 2);
        assert!(a[1].2.is_none());
        let b = quick_actions(Status::Review, "1", false);
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].1, "Start review");
        assert_eq!(quick_actions(Status::Done, "1", false).len(), 1);
        assert!(quick_actions(Status::Blocked, "1", false).is_empty());
        assert!(quick_actions(Status::InProgress, "1", false)
            .iter()
            .any(|a| a.1 == "Request review"));
    }

    #[test]
    fn renders_without_panicking() {
        let (home, data) = fixture("board");
        let view = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let mut filter = BoardFilter::default();
        let mut intents = Intents::default();
        render(|ui| show(ui, &view, &mut filter, &mut intents));
        filter.search = "#1".into();
        filter.owner = Some("ada".into());
        filter.sort = Sort::Owner;
        render(|ui| show(ui, &view, &mut filter, &mut intents));
        filter.owner = None;
        filter.layout = Layout::Columns;
        render(|ui| show(ui, &view, &mut filter, &mut intents));
        filter.hidden = [true, false, true, false, true];
        render(|ui| show(ui, &view, &mut filter, &mut intents));
        filter.hidden = [true; 5];
        render(|ui| show(ui, &view, &mut filter, &mut intents));
        filter.hidden = [false; 5];
        filter.show_trash = true;
        render(|ui| show(ui, &view, &mut filter, &mut intents));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn column_visibility_filter() {
        let mut f = BoardFilter::default();
        assert_eq!(visible_columns(&f).len(), 5);
        assert_eq!(f.active_count(), 0);
        f.hidden[col_index(Col::Review)] = true;
        f.owner = Some("ada".into());
        let v = visible_columns(&f);
        assert_eq!(v.len(), 4);
        assert!(!v.contains(&Col::Review));
        assert_eq!(f.active_count(), 2);
    }

    #[test]
    fn trash_lists_archived_only() {
        let mut a = task("a", "A", "");
        a.archived = true;
        let b = task("b", "B", "");
        let tasks = vec![a, b];
        let t = trash_list(&tasks);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].id, "a");
        assert!(trash_list(&[]).is_empty());
    }

    fn ids(v: &[&Task]) -> Vec<String> {
        v.iter().map(|t| t.id.clone()).collect()
    }

    #[test]
    fn manual_order_lists_ordered_first_then_rest() {
        let (a, b, c, d) = (
            task("a", "A", ""),
            task("b", "B", ""),
            task("c", "C", ""),
            task("d", "D", ""),
        );
        let out = apply_manual_order(vec![&a, &b, &c, &d], &["c".into(), "a".into(), "zz".into()]);
        assert_eq!(ids(&out), ["c", "a", "b", "d"]);
        let out = apply_manual_order(vec![&a, &b], &[]);
        assert_eq!(ids(&out), ["a", "b"]);
    }

    #[test]
    fn move_in_order_inserts_before_or_after() {
        let v: Vec<String> = ["a", "b", "c", "d"].map(String::from).to_vec();
        assert_eq!(move_in_order(&v, "d", "b", false), ["a", "d", "b", "c"]);
        assert_eq!(move_in_order(&v, "a", "c", true), ["b", "c", "a", "d"]);
        assert_eq!(move_in_order(&v, "a", "b", false), v);
        assert_eq!(move_in_order(&v, "a", "a", true), v);
        assert_eq!(move_in_order(&v, "x", "a", true), v);
        assert_eq!(move_in_order(&v, "a", "x", true), v);
    }

    #[test]
    fn order_keys_are_status_snake_case() {
        for c in Col::ALL {
            let s = match c {
                Col::Todo => Status::Pending,
                Col::InProgress => Status::InProgress,
                Col::Review => Status::Review,
                Col::Done => Status::Done,
                Col::Approved => Status::Approved,
            };
            let json = serde_json::to_value(s).unwrap();
            assert_eq!(json.as_str(), Some(c.order_key()));
        }
    }

    #[test]
    fn chips_follow_task_fields() {
        let mut t = task("a", "A", "");
        assert!(!ChipSet::of(&t).any());
        t.clarification = Some("which db?".into());
        t.related = vec!["b".into(), "c".into()];
        t.attachments = vec![rteams_core::Attachment {
            name: "n".into(),
            path: "p".into(),
        }];
        t.review_started = true;
        let c = ChipSet::of(&t);
        assert!(c.awaiting && c.any());
        assert_eq!((c.attachments, c.related), (1, 2));
        assert!(!c.reviewing, "only while in review");
        t.status = Status::Review;
        assert!(ChipSet::of(&t).reviewing);
    }

    #[test]
    fn renders_manual_sort_chips_and_start_review() {
        use crate::actions::Commander;
        let (home, mut data) = fixture("board-manual");
        let mut c = Commander::new(home.clone(), "t".into());
        for cmd in [
            GuiCmd::SetColumnOrder {
                column: "pending".into(),
                order: vec!["4".into(), "1".into()],
            },
            GuiCmd::SetClarification {
                task: "1".into(),
                question: Some("which db?".into()),
            },
            GuiCmd::StartReview { task: "3".into() },
        ] {
            c.run("boss", &cmd)
                .unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
        }
        data.refresh().unwrap();
        let view = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let team = view.team().unwrap();
        assert_eq!(team.column_order["pending"], ["4", "1"]);
        let mut filter = BoardFilter {
            sort: Sort::Manual,
            layout: Layout::Columns,
            ..Default::default()
        };
        let ctx = Ctx::new(&view);
        let cols = collect(&ctx, &filter);
        let todo = ids(&cols[col_index(Col::Todo)]);
        let pos = |id: &str| todo.iter().position(|t| t == id);
        assert!(pos("4").is_some() && pos("4") < pos("1"), "{todo:?}");
        let mut intents = Intents::default();
        render(|ui| show(ui, &view, &mut filter, &mut intents));
        filter.layout = Layout::Grid;
        render(|ui| show(ui, &view, &mut filter, &mut intents));
        assert!(intents.0.is_empty());
        let _ = std::fs::remove_dir_all(home);
    }
}
