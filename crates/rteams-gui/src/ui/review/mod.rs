//! Per-task change review at hunk level: a file tree on the left, the selected file's hunks on
//! the right (unified or side by side), accept/reject per hunk or file, line comments, and
//! j/k/a/r keys. Rejecting reverse-applies the hunk to the working tree with `git apply -R`.

pub mod conflict;
pub mod history;
pub mod store;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;

use eframe::egui::{self, Color32, Key, Modifiers, RichText};

use crate::git::{self, FileDiff, LineKind};
use crate::theme;
use crate::view::{Intent, Intents, View};
use conflict::{Candidate, Pending, Reason};
use history::{Before, Entry, Kind, RestoreError};
use store::{hunk_id, Decision, LineComment, ReviewState};

const DIFF_ADD: Color32 = Color32::from_rgb(0x4a, 0xde, 0x80);
const DIFF_DEL: Color32 = Color32::from_rgb(0xf8, 0x71, 0x71);
const MAX_LINES_PER_HUNK: usize = 600;

/// Where the repository and the review file live.
pub struct ReviewCtx<'a> {
    pub home: &'a Path,
    pub team: &'a str,
    pub repo: &'a Path,
    pub task: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    Accept(usize, usize),
    Reject(usize, usize),
    AcceptFile(usize),
    RejectFile(usize),
    AcceptAll,
    RejectAll,
    Select(usize, usize),
    AddComment,
    Post,
    Refresh,
    Preview(usize, usize),
    ConfirmPreview,
    ClosePreview,
    EditStart(usize),
    EditSave,
    EditCancel,
    ToggleHistory,
    HistPreview(String),
    Restore(String),
    ConflictMine,
    ConflictTheirs,
    ConflictEditor,
    ConflictCancel,
    RejectRename(String),
    ReapplyRename(String),
}

/// Reject-preview state: the file text a hunk's rejection would leave.
#[derive(Debug)]
struct Preview {
    file: String,
    key: String,
    text: String,
}

/// An in-progress edit of a file's final content.
#[derive(Debug)]
struct Edit {
    file: String,
    text: String,
    base_hash: Option<u64>,
}

#[derive(Debug, Default)]
pub struct ReviewDialog {
    open: bool,
    paths: Vec<String>,
    files: Vec<FileDiff>,
    errors: Vec<String>,
    state: ReviewState,
    needs_load: bool,
    sel_file: usize,
    sel_hunk: usize,
    split: bool,
    comment_at: Option<(String, u32)>,
    comment_text: String,
    scroll_to_sel: bool,
    status: Option<String>,
    /// Content hash of each file when the diff was loaded.
    loaded: BTreeMap<String, Option<u64>>,
    conflict: Option<Vec<Candidate>>,
    stale_dismissed: bool,
    history: Vec<Entry>,
    show_history: bool,
    hist_preview: Option<String>,
    preview: Option<Preview>,
    edit: Option<Edit>,
    renames: Vec<FileDiff>,
}

impl ReviewDialog {
    pub fn open(&mut self, paths: Vec<String>) {
        self.paths = paths;
        self.open = true;
        self.needs_load = true;
        self.sel_file = 0;
        self.sel_hunk = 0;
        self.status = None;
        self.stale_dismissed = false;
        self.conflict = None;
        self.preview = None;
        self.edit = None;
        self.show_history = false;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    #[cfg(test)]
    pub fn files(&self) -> &[FileDiff] {
        &self.files
    }

    pub fn load(&mut self, rc: &ReviewCtx) {
        self.state = ReviewState::load(rc.home, rc.team, rc.task);
        self.files.clear();
        self.errors.clear();
        for p in &self.paths {
            match git::file_hunks(rc.repo, p) {
                Ok(Some(f)) => self.files.push(f),
                Ok(None) => {}
                Err(e) => self.errors.push(format!("{p}: {e}")),
            }
        }
        self.files.sort_by(|a, b| a.path.cmp(&b.path));
        self.loaded.clear();
        for p in &self.paths {
            let rel = git::relative_path(rc.repo, p);
            let h = git::content_hash(rc.repo, &rel);
            self.loaded.insert(rel, h);
        }
        self.history = history::list(rc.home, rc.team, rc.task);
        self.renames = git::renames(rc.repo)
            .into_iter()
            .filter(|r| {
                self.paths.iter().any(|p| {
                    let rel = git::relative_path(rc.repo, p);
                    rel == r.path || Some(&rel) == r.renamed_from.as_ref()
                })
            })
            .collect();
        if self.conflict.is_none() && !self.stale_dismissed {
            let c = conflict::stale_decisions(&self.state, &self.files);
            if !c.is_empty() {
                self.conflict = Some(c);
            }
        }
        self.needs_load = false;
        self.clamp();
    }

    fn clamp(&mut self) {
        if self.sel_file >= self.files.len() {
            self.sel_file = self.files.len().saturating_sub(1);
        }
        let n = self.files.get(self.sel_file).map_or(0, |f| f.hunks.len());
        if self.sel_hunk >= n {
            self.sel_hunk = n.saturating_sub(1);
        }
    }

    fn decision(&self, f: usize, h: usize) -> Option<Decision> {
        let file = self.files.get(f)?;
        let hunk = file.hunks.get(h)?;
        self.state.hunk(&file.path, &hunk.key())
    }

    /// (accepted, pending) over the hunks still in the diff; rejected hunks are gone from it.
    pub fn counts(&self) -> (usize, usize, usize) {
        let (mut acc, mut pend) = (0, 0);
        for (fi, f) in self.files.iter().enumerate() {
            for hi in 0..f.hunks.len() {
                match self.decision(fi, hi) {
                    Some(Decision::Accepted) => acc += 1,
                    _ => pend += 1,
                }
            }
        }
        (acc, self.state.count(Decision::Rejected), pend)
    }

    /// Change stats over every file in the review: `(files, added, removed)` lines.
    pub fn totals(&self) -> (usize, usize, usize) {
        let added = self.files.iter().map(FileDiff::added).sum();
        let removed = self.files.iter().map(FileDiff::removed).sum();
        (self.files.len(), added, removed)
    }

    fn mark(&mut self, f: usize, h: usize, d: Decision) {
        if let Some((file, hunk)) = self
            .files
            .get(f)
            .and_then(|x| x.hunks.get(h).map(|y| (&x.path, y)))
        {
            self.state.hunks.insert(hunk_id(file, &hunk.key()), d);
        }
    }

    fn save(&mut self, rc: &ReviewCtx) {
        if let Err(e) = self.state.save(rc.home, rc.team, rc.task) {
            self.status = Some(e);
        }
    }

    /// Apply one action. Reject writes to the working tree.
    pub fn apply(&mut self, rc: &ReviewCtx, act: Act, out: &mut Intents) {
        match act {
            Act::Select(f, h) => {
                self.sel_file = f;
                self.sel_hunk = h;
                self.clamp();
            }
            Act::Accept(f, h) => {
                self.mark(f, h, Decision::Accepted);
                self.save(rc);
            }
            Act::Reject(f, h) => {
                let mut cs = Vec::new();
                self.reject_one(rc, f, h, &mut cs);
                self.finish_mutation(rc, cs);
            }
            Act::AcceptFile(f) => {
                for h in 0..self.files.get(f).map_or(0, |x| x.hunks.len()) {
                    self.mark(f, h, Decision::Accepted);
                }
                if let Some(file) = self.files.get(f) {
                    self.state
                        .files
                        .insert(file.path.clone(), Decision::Accepted);
                }
                self.save(rc);
            }
            Act::RejectFile(f) => {
                let mut cs = Vec::new();
                self.reject_file(rc, f, &mut cs);
                self.finish_mutation(rc, cs);
            }
            Act::AcceptAll => {
                for f in 0..self.files.len() {
                    self.apply(rc, Act::AcceptFile(f), out);
                }
            }
            Act::RejectAll => {
                // Highest hunk first; git matches by context so earlier hunks stay applicable.
                let mut cs = Vec::new();
                for f in 0..self.files.len() {
                    for h in (0..self.files[f].hunks.len()).rev() {
                        if self.decision(f, h) != Some(Decision::Accepted) {
                            self.reject_one(rc, f, h, &mut cs);
                        }
                    }
                }
                self.finish_mutation(rc, cs);
            }
            Act::AddComment => {
                if let Some((file, line)) = self.comment_at.take() {
                    let text = self.comment_text.trim().to_string();
                    if !text.is_empty() {
                        self.state.comments.push(LineComment { file, line, text });
                        self.save(rc);
                    }
                }
                self.comment_text.clear();
            }
            Act::Post => {
                out.push(Intent::Run(crate::actions::GuiCmd::Comment {
                    task: rc.task.to_string(),
                    text: self.state.summary(),
                }));
                self.status = Some("posted".into());
            }
            Act::Refresh => {
                self.stale_dismissed = false;
                self.load(rc)
            }
            Act::Preview(f, h) => {
                if let Some(file) = self.files.get(f) {
                    match git::preview_reject(rc.repo, file, h) {
                        Ok(text) => {
                            self.preview = Some(Preview {
                                file: file.path.clone(),
                                key: file.hunks[h].key(),
                                text,
                            })
                        }
                        Err(e) => self.status = Some(e),
                    }
                }
            }
            Act::ConfirmPreview => {
                if let Some(p) = self.preview.take() {
                    let idx = self
                        .files
                        .iter()
                        .position(|x| x.path == p.file)
                        .and_then(|fi| {
                            let hi = self.files[fi].hunks.iter().position(|h| h.key() == p.key)?;
                            Some((fi, hi))
                        });
                    match idx {
                        Some((fi, hi)) => self.apply(rc, Act::Reject(fi, hi), out),
                        None => self.status = Some("the hunk no longer applies".into()),
                    }
                }
            }
            Act::ClosePreview => self.preview = None,
            Act::EditStart(f) => {
                if let Some(file) = self.files.get(f) {
                    match git::read_text(rc.repo, &file.path) {
                        Some(text) => {
                            self.edit = Some(Edit {
                                file: file.path.clone(),
                                text,
                                base_hash: git::content_hash(rc.repo, &file.path),
                            })
                        }
                        None => self.status = Some(format!("{} is not editable text", file.path)),
                    }
                }
            }
            Act::EditCancel => self.edit = None,
            Act::EditSave => {
                if let Some(e) = self.edit.take() {
                    if conflict::changed_since(e.base_hash, git::content_hash(rc.repo, &e.file)) {
                        self.conflict = Some(vec![Candidate {
                            file: e.file,
                            reason: Reason::FileChanged,
                            pending: Pending::SaveEdit { text: e.text },
                        }]);
                    } else {
                        self.save_edit(rc, &e.file, &e.text);
                        self.save(rc);
                        self.load(rc);
                    }
                }
            }
            Act::ToggleHistory => {
                self.show_history = !self.show_history;
                self.history = history::list(rc.home, rc.team, rc.task);
            }
            Act::HistPreview(id) => {
                self.hist_preview = (self.hist_preview.as_ref() != Some(&id)).then_some(id);
            }
            Act::Restore(id) => {
                if let Some(e) = self.history.iter().find(|e| e.id == id).cloned() {
                    match history::restore(rc.repo, &e, false) {
                        Ok(()) => self.after_restore(rc, &e),
                        Err(RestoreError::Changed) => {
                            self.conflict = Some(vec![Candidate {
                                file: e.file,
                                reason: Reason::RestoreChanged,
                                pending: Pending::Restore { id },
                            }])
                        }
                        Err(RestoreError::Other(m)) => self.status = Some(m),
                    }
                }
            }
            Act::ConflictCancel => {
                self.stale_dismissed = true;
                self.conflict = None;
            }
            Act::ConflictEditor => {
                for c in self.conflict.iter().flatten() {
                    crate::task_dialog::open_external(&rc.repo.join(&c.file).to_string_lossy());
                }
            }
            Act::ConflictMine => self.resolve_mine(rc),
            Act::ConflictTheirs => self.resolve_theirs(rc),
            Act::RejectRename(to) => {
                let from = self
                    .renames
                    .iter()
                    .find(|r| r.path == to)
                    .and_then(|r| r.renamed_from.clone());
                match from {
                    Some(from) => match git::reject_rename(rc.repo, &from, &to) {
                        Ok(()) => {
                            self.state.renames.insert(to, from);
                            self.save(rc);
                            self.load(rc);
                        }
                        Err(e) => self.status = Some(e),
                    },
                    None => self.status = Some("not a rename".into()),
                }
            }
            Act::ReapplyRename(to) => {
                if let Some(from) = self.state.renames.get(&to).cloned() {
                    match git::reapply_rename(rc.repo, &from, &to) {
                        Ok(()) => {
                            self.state.renames.remove(&to);
                            self.save(rc);
                            self.load(rc);
                        }
                        Err(e) => self.status = Some(e),
                    }
                }
            }
        }
    }

    /// Reject one hunk, unless the file changed on disk or the hunk no longer applies; then
    /// it becomes a conflict candidate.
    fn reject_one(&mut self, rc: &ReviewCtx, f: usize, h: usize, cs: &mut Vec<Candidate>) {
        let Some(file) = self.files.get(f).cloned() else {
            return;
        };
        let Some(hunk) = file.hunks.get(h) else {
            return;
        };
        let pending = Pending::RejectHunk { key: hunk.key() };
        if self.file_changed(rc, &file.path) {
            cs.push(Candidate {
                file: file.path,
                reason: Reason::FileChanged,
                pending,
            });
        } else if !git::reject_applies(rc.repo, &file, h) {
            cs.push(Candidate {
                file: file.path,
                reason: Reason::NoLongerApplies,
                pending,
            });
        } else {
            self.do_reject(rc, &file, f, h);
        }
    }

    fn file_changed(&self, rc: &ReviewCtx, path: &str) -> bool {
        self.loaded
            .get(path)
            .is_some_and(|l| conflict::changed_since(*l, git::content_hash(rc.repo, path)))
    }

    fn rehash(&mut self, rc: &ReviewCtx, path: &str) {
        self.loaded
            .insert(path.to_string(), git::content_hash(rc.repo, path));
    }

    fn record(&mut self, rc: &ReviewCtx, file: &str, kind: Kind, before: Option<Before>) {
        if let Some(b) = before {
            if let Err(e) = history::record(rc.home, rc.team, rc.task, rc.repo, file, kind, b) {
                self.status = Some(e);
            }
        }
    }

    fn do_reject(&mut self, rc: &ReviewCtx, file: &FileDiff, f: usize, h: usize) {
        let before = Before::take(rc.repo, &file.path);
        match git::reject_hunk(rc.repo, file, h) {
            Ok(()) => {
                self.mark(f, h, Decision::Rejected);
                self.record(rc, &file.path, Kind::RejectHunk, before);
                self.rehash(rc, &file.path);
            }
            Err(e) => self.status = Some(e),
        }
    }

    fn reject_file(&mut self, rc: &ReviewCtx, f: usize, cs: &mut Vec<Candidate>) {
        let Some(file) = self.files.get(f).cloned() else {
            return;
        };
        if self.file_changed(rc, &file.path) {
            cs.push(Candidate {
                file: file.path,
                reason: Reason::FileChanged,
                pending: Pending::RejectFile,
            });
            return;
        }
        self.do_reject_file(rc, &file, f);
    }

    fn do_reject_file(&mut self, rc: &ReviewCtx, file: &FileDiff, f: usize) {
        let before = Before::take(rc.repo, &file.path);
        match git::revert_file(rc.repo, &file.path) {
            Ok(()) => {
                for h in 0..file.hunks.len() {
                    self.mark(f, h, Decision::Rejected);
                }
                self.state
                    .files
                    .insert(file.path.clone(), Decision::Rejected);
                self.record(rc, &file.path, Kind::RejectFile, before);
                self.rehash(rc, &file.path);
            }
            Err(e) => self.status = Some(e),
        }
    }

    fn save_edit(&mut self, rc: &ReviewCtx, file: &str, text: &str) {
        let before = Before::take(rc.repo, file);
        match git::write_text(rc.repo, file, text) {
            Ok(()) => {
                self.record(rc, file, Kind::Edit, before);
                self.rehash(rc, file);
            }
            Err(e) => self.status = Some(e),
        }
    }

    fn after_restore(&mut self, rc: &ReviewCtx, e: &Entry) {
        if e.kind != Kind::Edit {
            let prefix = format!("{}#", e.file);
            self.state
                .hunks
                .retain(|k, d| !(k.starts_with(&prefix) && *d == Decision::Rejected));
            if self.state.files.get(&e.file) == Some(&Decision::Rejected) {
                self.state.files.remove(&e.file);
            }
        }
        self.save(rc);
        self.load(rc);
        self.status = None;
    }

    fn finish_mutation(&mut self, rc: &ReviewCtx, cs: Vec<Candidate>) {
        self.save(rc);
        if cs.is_empty() {
            self.load(rc);
        } else {
            self.conflict = Some(cs);
        }
    }

    /// "Keep mine": re-apply each pending action against the file as it is now.
    fn resolve_mine(&mut self, rc: &ReviewCtx) {
        let Some(cs) = self.conflict.take() else {
            return;
        };
        self.stale_dismissed = true;
        self.load(rc);
        for c in cs {
            let fi = self.files.iter().position(|x| x.path == c.file);
            match c.pending {
                Pending::RejectHunk { key } => {
                    let hit = fi.and_then(|fi| {
                        let hi = self.files[fi].hunks.iter().position(|h| h.key() == key)?;
                        Some((fi, hi))
                    });
                    match hit {
                        Some((fi, hi)) if git::reject_applies(rc.repo, &self.files[fi], hi) => {
                            let file = self.files[fi].clone();
                            self.do_reject(rc, &file, fi, hi);
                        }
                        _ => self.status = Some(format!("{}: the hunk no longer applies", c.file)),
                    }
                }
                Pending::RejectFile => {
                    if let Some(fi) = fi {
                        let file = self.files[fi].clone();
                        self.do_reject_file(rc, &file, fi);
                    }
                }
                Pending::SaveEdit { text } => self.save_edit(rc, &c.file, &text),
                Pending::Restore { id } => {
                    if let Some(e) = self.history.iter().find(|e| e.id == id).cloned() {
                        match history::restore(rc.repo, &e, true) {
                            Ok(()) => self.after_restore(rc, &e),
                            Err(RestoreError::Other(m)) => self.status = Some(m),
                            Err(RestoreError::Changed) => {}
                        }
                    }
                }
                Pending::Nothing => {}
            }
        }
        self.save(rc);
        self.load(rc);
    }

    /// "Take theirs": drop the pending actions and stale decisions; the file stays as on disk.
    fn resolve_theirs(&mut self, rc: &ReviewCtx) {
        let Some(cs) = self.conflict.take() else {
            return;
        };
        if cs.iter().any(|c| c.reason == Reason::SavedStale) {
            for id in conflict::stale_ids(&self.state, &self.files) {
                self.state.hunks.remove(&id);
            }
        }
        self.stale_dismissed = true;
        self.save(rc);
        self.load(rc);
    }

    /// Move the hunk selection by `delta`, rolling over file boundaries.
    pub fn step(&mut self, delta: i32) {
        let flat: Vec<(usize, usize)> = self
            .files
            .iter()
            .enumerate()
            .flat_map(|(fi, f)| (0..f.hunks.len()).map(move |hi| (fi, hi)))
            .collect();
        if flat.is_empty() {
            return;
        }
        let cur = flat
            .iter()
            .position(|p| *p == (self.sel_file, self.sel_hunk))
            .unwrap_or(0) as i32;
        let next = (cur + delta).clamp(0, flat.len() as i32 - 1) as usize;
        (self.sel_file, self.sel_hunk) = flat[next];
        self.scroll_to_sel = true;
    }

    pub fn show(&mut self, ctx: &egui::Context, v: &View, rc: &ReviewCtx, out: &mut Intents) {
        if !self.open {
            return;
        }
        if self.needs_load {
            self.load(rc);
        }
        let mut acts: Vec<Act> = Vec::new();
        if !ctx.egui_wants_keyboard_input() {
            ctx.input_mut(|i| {
                if i.consume_key(Modifiers::NONE, Key::J) {
                    self.step(1);
                }
                if i.consume_key(Modifiers::NONE, Key::K) {
                    self.step(-1);
                }
                if i.consume_key(Modifiers::NONE, Key::A) {
                    acts.push(Act::Accept(self.sel_file, self.sel_hunk));
                }
                if i.consume_key(Modifiers::NONE, Key::R) {
                    acts.push(Act::Reject(self.sel_file, self.sel_hunk));
                }
            });
        }
        let tr = v.tr;
        let (acc, rej, pend) = self.counts();
        let (n_files, n_add, n_del) = self.totals();
        let width = (ctx.content_rect().width() * 0.94).min(1240.0);
        let cell = RefCell::new(acts);
        let mut open = true;
        let status = self.status.clone();
        let show_history = self.show_history;
        theme::dialog_with_footer(
            ctx,
            ("review_dialog", rc.task),
            &tr.t("team.taskDetail.changes.reviewTaskDiff"),
            width,
            &mut open,
            |ui| self.body(ui, v, &cell),
            |ui| {
                if theme::pill_button(ui, &tr.t("rteams.review.postSummary"), true).clicked() {
                    cell.borrow_mut().push(Act::Post);
                }
                if theme::pill_button(ui, &tr.t("team.review.toolbar.actions.rejectAll"), false)
                    .clicked()
                {
                    cell.borrow_mut().push(Act::RejectAll);
                }
                if theme::pill_button(ui, &tr.t("team.review.toolbar.actions.acceptAll"), false)
                    .clicked()
                {
                    cell.borrow_mut().push(Act::AcceptAll);
                }
                if theme::pill_button(ui, &tr.t("rteams.review.history"), show_history).clicked() {
                    cell.borrow_mut().push(Act::ToggleHistory);
                }
                if ui
                    .button(tr.t("team.taskDetail.changes.refreshShort"))
                    .clicked()
                {
                    cell.borrow_mut().push(Act::Refresh);
                }
                ui.label(
                    RichText::new(format!(
                        "+{n_add} -{n_del} {}",
                        tr.t_count("team.review.toolbar.stats.acrossFiles", n_files as u64)
                    ))
                    .small()
                    .color(theme::MUTED),
                );
                ui.label(
                    RichText::new(format!(
                        "{}  \u{b7}  {}  \u{b7}  {}",
                        tr.t_count("team.review.toolbar.stats.accepted", acc as u64),
                        tr.t_count("team.review.toolbar.stats.rejected", rej as u64),
                        tr.t_count("team.review.toolbar.stats.pending", pend as u64),
                    ))
                    .small()
                    .color(theme::MUTED),
                );
                if let Some(s) = &status {
                    let (t, c) = if s == "posted" {
                        (tr.t("rteams.review.posted"), theme::SUCCESS)
                    } else {
                        (s.clone(), theme::ERROR_FG)
                    };
                    ui.label(RichText::new(t).small().color(c));
                }
            },
        );
        if !open {
            self.open = false;
        }
        let mut acts = cell.into_inner();
        self.conflict_dialog(ctx, v, rc, &mut acts);
        for a in acts {
            if a == Act::Post {
                self.status = None;
            }
            self.apply(rc, a, out);
        }
    }

    fn body(&mut self, ui: &mut egui::Ui, v: &View, acts: &RefCell<Vec<Act>>) {
        let tr = v.tr;
        let h = (ui.ctx().content_rect().height() * 0.9 - 190.0).max(140.0);
        if self.show_history {
            self.history_ui(ui, v, h, acts);
            return;
        }
        self.renames_ui(ui, tr, acts);
        if self.files.is_empty() {
            ui.label(
                RichText::new(tr.t("team.review.fileTree.empty.noChangedFiles"))
                    .color(theme::MUTED),
            );
            for e in &self.errors {
                ui.label(RichText::new(e).small().color(theme::ERROR_FG));
            }
            return;
        }
        let left_w = (ui.available_width() * 0.28).clamp(110.0, 260.0);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(left_w, h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("review_tree")
                        .max_height(h)
                        .show(ui, |ui| self.tree(ui, tr, acts));
                },
            );
            ui.separator();
            let w = ui.available_width().max(60.0);
            ui.allocate_ui_with_layout(
                egui::vec2(w, h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.right(ui, v, h, acts),
            );
        });
        ui.label(
            RichText::new(tr.t("rteams.review.keys"))
                .small()
                .color(theme::MUTED),
        );
    }

    fn tree(&self, ui: &mut egui::Ui, tr: &rteams_services::i18n::I18n, acts: &RefCell<Vec<Act>>) {
        let mut last_dir = String::new();
        for (fi, f) in self.files.iter().enumerate() {
            let (dir, name) = match f.path.rsplit_once('/') {
                Some((d, n)) => (d, n),
                None => ("", f.path.as_str()),
            };
            if dir != last_dir && !dir.is_empty() {
                ui.label(RichText::new(format!("{dir}/")).small().color(theme::MUTED));
            }
            last_dir = dir.to_string();
            let all_acc = !f.hunks.is_empty()
                && (0..f.hunks.len()).all(|h| self.decision(fi, h) == Some(Decision::Accepted));
            let mark = if all_acc { "[ok] " } else { "" };
            let sel = fi == self.sel_file;
            ui.horizontal(|ui| {
                ui.add_space(if dir.is_empty() { 0.0 } else { 10.0 });
                let r = ui.selectable_label(
                    sel,
                    RichText::new(format!("{mark}{name}"))
                        .monospace()
                        .size(12.0),
                );
                if r.clicked() {
                    acts.borrow_mut().push(Act::Select(fi, 0));
                }
                if f.is_new {
                    theme::pill(ui, tr.t("team.review.fileTree.badges.new"), theme::INFO);
                }
                ui.label(
                    RichText::new(format!("+{}", f.added()))
                        .small()
                        .color(DIFF_ADD),
                );
                ui.label(
                    RichText::new(format!("-{}", f.removed()))
                        .small()
                        .color(DIFF_DEL),
                );
            });
        }
    }

    fn right(&mut self, ui: &mut egui::Ui, v: &View, h: f32, acts: &RefCell<Vec<Act>>) {
        let tr = v.tr;
        let fi = self.sel_file;
        let Some(file) = self.files.get(fi).cloned() else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&file.path).monospace().strong());
            if theme::pill_button(ui, &tr.t("rteams.review.unified"), !self.split).clicked() {
                self.split = false;
            }
            if theme::pill_button(ui, &tr.t("rteams.review.split"), self.split).clicked() {
                self.split = true;
            }
            if theme::pill_button(ui, &tr.t("rteams.review.edit"), self.edit.is_some()).clicked() {
                acts.borrow_mut().push(Act::EditStart(fi));
            }
            if theme::pill_button(ui, &tr.t("team.review.fileHeader.actions.accept"), false)
                .clicked()
            {
                acts.borrow_mut().push(Act::AcceptFile(fi));
            }
            if theme::pill_button(ui, &tr.t("team.review.fileHeader.actions.reject"), false)
                .clicked()
            {
                acts.borrow_mut().push(Act::RejectFile(fi));
            }
        });
        if file.binary {
            ui.label(
                RichText::new(tr.t("team.review.fileHeader.contentUnavailable.title"))
                    .color(theme::MUTED),
            );
            return;
        }
        if self.preview.as_ref().is_some_and(|p| p.file == file.path) {
            self.preview_ui(ui, tr, h, acts);
            return;
        }
        if self.edit.as_ref().is_some_and(|e| e.file == file.path) {
            self.edit_ui(ui, tr, h, acts);
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt(("review_hunks", &file.path))
            .max_height(h - 30.0)
            .show(ui, |ui| {
                for (hi, hunk) in file.hunks.iter().enumerate() {
                    let selected = hi == self.sel_hunk;
                    let decision = self.state.hunk(&file.path, &hunk.key());
                    let stroke = if selected {
                        theme::ACCENT
                    } else {
                        theme::hairline(0.07)
                    };
                    let resp = egui::Frame::new()
                        .stroke(egui::Stroke::new(if selected { 1.5 } else { 1.0 }, stroke))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::same(6))
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    RichText::new(&hunk.header)
                                        .monospace()
                                        .small()
                                        .color(theme::INFO),
                                );
                                ui.label(
                                    RichText::new(format!("+{}", hunk.added()))
                                        .small()
                                        .color(DIFF_ADD),
                                );
                                ui.label(
                                    RichText::new(format!("-{}", hunk.removed()))
                                        .small()
                                        .color(DIFF_DEL),
                                );
                                if decision == Some(Decision::Accepted) {
                                    theme::pill(ui, "ok", theme::SUCCESS);
                                }
                                if theme::pill_button(ui, &tr.t("rteams.review.preview"), false)
                                    .clicked()
                                {
                                    acts.borrow_mut().push(Act::Preview(fi, hi));
                                }
                                if theme::pill_button(
                                    ui,
                                    &tr.t("team.review.fileHeader.actions.reject"),
                                    false,
                                )
                                .clicked()
                                {
                                    acts.borrow_mut().push(Act::Reject(fi, hi));
                                }
                                if theme::pill_button(
                                    ui,
                                    &tr.t("team.review.fileHeader.actions.accept"),
                                    false,
                                )
                                .clicked()
                                {
                                    acts.borrow_mut().push(Act::Accept(fi, hi));
                                }
                            });
                            self.hunk_lines(ui, &file, hi, acts);
                        });
                    if resp.response.interact(egui::Sense::click()).clicked() {
                        acts.borrow_mut().push(Act::Select(fi, hi));
                    }
                    if selected && self.scroll_to_sel {
                        resp.response.scroll_to_me(Some(egui::Align::Center));
                        self.scroll_to_sel = false;
                    }
                    ui.add_space(4.0);
                }
            });
    }

    fn preview_ui(
        &self,
        ui: &mut egui::Ui,
        tr: &rteams_services::i18n::I18n,
        h: f32,
        acts: &RefCell<Vec<Act>>,
    ) {
        let Some(p) = &self.preview else { return };
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr.t("rteams.review.previewTitle")).strong());
            if theme::pill_button(ui, &tr.t("team.review.fileHeader.actions.reject"), true)
                .clicked()
            {
                acts.borrow_mut().push(Act::ConfirmPreview);
            }
            if theme::pill_button(ui, &tr.t("rteams.review.close"), false).clicked() {
                acts.borrow_mut().push(Act::ClosePreview);
            }
        });
        egui::ScrollArea::both()
            .id_salt("review_preview")
            .max_height(h - 60.0)
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(RichText::new(&p.text).monospace().size(12.0))
                        .wrap_mode(egui::TextWrapMode::Extend),
                );
            });
    }

    fn edit_ui(
        &mut self,
        ui: &mut egui::Ui,
        tr: &rteams_services::i18n::I18n,
        h: f32,
        acts: &RefCell<Vec<Act>>,
    ) {
        ui.horizontal(|ui| {
            if theme::pill_button(ui, &tr.t("rteams.review.save"), true).clicked() {
                acts.borrow_mut().push(Act::EditSave);
            }
            if theme::pill_button(ui, &tr.t("rteams.review.discard"), false).clicked() {
                acts.borrow_mut().push(Act::EditCancel);
            }
        });
        let Some(e) = self.edit.as_mut() else { return };
        egui::ScrollArea::vertical()
            .id_salt("review_edit")
            .max_height(h - 60.0)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut e.text)
                        .code_editor()
                        .desired_width(f32::INFINITY),
                );
            });
    }

    fn history_ui(&self, ui: &mut egui::Ui, v: &View, h: f32, acts: &RefCell<Vec<Act>>) {
        let tr = v.tr;
        ui.label(RichText::new(tr.t("rteams.review.history")).strong());
        if self.history.is_empty() {
            ui.label(RichText::new(tr.t("rteams.review.historyEmpty")).color(theme::MUTED));
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("review_history")
            .max_height(h)
            .show(ui, |ui| {
                for e in &self.history {
                    let kind = match e.kind {
                        Kind::RejectHunk => "rteams.review.kindRejectHunk",
                        Kind::RejectFile => "rteams.review.kindRejectFile",
                        Kind::Edit => "rteams.review.kindEdit",
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(history::fmt_ts(e.ts))
                                .small()
                                .color(theme::MUTED),
                        );
                        ui.label(RichText::new(tr.t(kind)).small());
                        ui.label(RichText::new(&e.file).monospace().small());
                        if theme::pill_button(ui, &tr.t("rteams.review.preview"), false).clicked() {
                            acts.borrow_mut().push(Act::HistPreview(e.id.clone()));
                        }
                        if theme::pill_button(ui, &tr.t("team.review.restore"), false).clicked() {
                            acts.borrow_mut().push(Act::Restore(e.id.clone()));
                        }
                    });
                    if self.hist_preview.as_ref() == Some(&e.id) {
                        let t: String = e.before.lines().take(200).collect::<Vec<_>>().join("\n");
                        ui.add(
                            egui::Label::new(RichText::new(t).monospace().size(11.0))
                                .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    }
                }
            });
    }

    fn renames_ui(
        &self,
        ui: &mut egui::Ui,
        tr: &rteams_services::i18n::I18n,
        acts: &RefCell<Vec<Act>>,
    ) {
        for r in &self.renames {
            ui.horizontal_wrapped(|ui| {
                theme::pill(ui, tr.t("rteams.review.renamed"), theme::INFO);
                ui.label(
                    RichText::new(format!(
                        "{} -> {}",
                        r.renamed_from.as_deref().unwrap_or("?"),
                        r.path
                    ))
                    .monospace()
                    .small(),
                );
                if theme::pill_button(ui, &tr.t("team.review.fileHeader.actions.reject"), false)
                    .clicked()
                {
                    acts.borrow_mut().push(Act::RejectRename(r.path.clone()));
                }
            });
        }
        for (to, from) in &self.state.renames {
            ui.horizontal_wrapped(|ui| {
                theme::pill(
                    ui,
                    tr.t("team.review.fileTree.filters.rejected"),
                    theme::ERROR_FG,
                );
                ui.label(RichText::new(format!("{from} -> {to}")).monospace().small());
                if theme::pill_button(ui, &tr.t("rteams.review.reapplyRename"), false).clicked() {
                    acts.borrow_mut().push(Act::ReapplyRename(to.clone()));
                }
            });
        }
    }

    fn conflict_dialog(
        &mut self,
        ctx: &egui::Context,
        v: &View,
        rc: &ReviewCtx,
        acts: &mut Vec<Act>,
    ) {
        let Some(cs) = &self.conflict else { return };
        let tr = v.tr;
        let cell = RefCell::new(Vec::new());
        let mut open = true;
        theme::dialog_with_footer(
            ctx,
            ("review_conflict", rc.task),
            &tr.t("team.review.conflict.title"),
            520.0,
            &mut open,
            |ui| {
                ui.label(
                    RichText::new(tr.t("team.review.conflict.description")).color(theme::MUTED),
                );
                for c in cs {
                    let why = match c.reason {
                        Reason::FileChanged => "rteams.review.reasonChanged",
                        Reason::NoLongerApplies => "rteams.review.reasonNoApply",
                        Reason::SavedStale => "rteams.review.reasonStale",
                        Reason::RestoreChanged => "rteams.review.reasonRestore",
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&c.file).monospace().strong());
                        ui.label(RichText::new(tr.t(why)).small().color(theme::ERROR_FG));
                    });
                }
            },
            |ui| {
                if theme::pill_button(ui, &tr.t("rteams.review.keepMine"), true).clicked() {
                    cell.borrow_mut().push(Act::ConflictMine);
                }
                if theme::pill_button(ui, &tr.t("rteams.review.takeTheirs"), false).clicked() {
                    cell.borrow_mut().push(Act::ConflictTheirs);
                }
                if theme::pill_button(ui, &tr.t("team.review.conflict.editManually"), false)
                    .clicked()
                {
                    cell.borrow_mut().push(Act::ConflictEditor);
                }
                if ui.button(tr.t("team.review.conflict.cancel")).clicked() {
                    cell.borrow_mut().push(Act::ConflictCancel);
                }
            },
        );
        acts.extend(cell.into_inner());
        if !open && !acts.contains(&Act::ConflictMine) && !acts.contains(&Act::ConflictTheirs) {
            acts.push(Act::ConflictCancel);
        }
    }

    fn hunk_lines(
        &mut self,
        ui: &mut egui::Ui,
        file: &FileDiff,
        hi: usize,
        acts: &RefCell<Vec<Act>>,
    ) {
        let hunk = &file.hunks[hi];
        let add_bg = theme::tint(Color32::from_rgb(0x22, 0xc5, 0x5e), 0.16);
        let del_bg = theme::tint(Color32::from_rgb(0xef, 0x44, 0x44), 0.16);
        let mut clicked: Option<u32> = None;
        let num =
            |ui: &mut egui::Ui, n: Option<u32>, commented: bool, clicked: &mut Option<u32>| {
                let t = n.map_or(String::new(), |n| n.to_string());
                let color = if commented {
                    theme::ACCENT
                } else {
                    theme::MUTED
                };
                let r = ui.add(
                    egui::Label::new(RichText::new(t).monospace().size(11.0).color(color))
                        .sense(egui::Sense::click()),
                );
                if r.clicked() {
                    *clicked = n;
                }
            };
        let commented = |s: &ReviewState, n: Option<u32>| {
            n.is_some_and(|n| s.comments_at(&file.path, n).next().is_some())
        };
        let text = |t: &str, bg: Option<Color32>| {
            let mut r = RichText::new(if t.is_empty() { " " } else { t })
                .monospace()
                .size(12.0);
            if let Some(bg) = bg {
                r = r.background_color(bg);
            }
            egui::Label::new(r).wrap_mode(egui::TextWrapMode::Extend)
        };
        let bg_of = |k: LineKind| match k {
            LineKind::Add => Some(add_bg),
            LineKind::Del => Some(del_bg),
            LineKind::Context => None,
        };
        if self.split {
            egui::Grid::new(("rv_split", &file.path, hi))
                .num_columns(4)
                .spacing([6.0, 1.0])
                .show(ui, |ui| {
                    for (l, r) in git::side_by_side(hunk).into_iter().take(MAX_LINES_PER_HUNK) {
                        num(
                            ui,
                            l.and_then(|x| x.old_no),
                            commented(&self.state, l.and_then(|x| x.old_no)),
                            &mut clicked,
                        );
                        match l {
                            Some(l) => ui.add(text(&l.text, bg_of(l.kind))),
                            None => ui.label(""),
                        };
                        num(
                            ui,
                            r.and_then(|x| x.new_no),
                            commented(&self.state, r.and_then(|x| x.new_no)),
                            &mut clicked,
                        );
                        match r {
                            Some(r) => ui.add(text(&r.text, bg_of(r.kind))),
                            None => ui.label(""),
                        };
                        ui.end_row();
                    }
                });
        } else {
            egui::Grid::new(("rv_uni", &file.path, hi))
                .num_columns(3)
                .spacing([6.0, 1.0])
                .show(ui, |ui| {
                    for l in hunk.lines.iter().take(MAX_LINES_PER_HUNK) {
                        num(ui, l.old_no, commented(&self.state, l.old_no), &mut clicked);
                        num(ui, l.new_no, commented(&self.state, l.new_no), &mut clicked);
                        let sign = match l.kind {
                            LineKind::Add => '+',
                            LineKind::Del => '-',
                            LineKind::Context => ' ',
                        };
                        ui.add(text(&format!("{sign} {}", l.text), bg_of(l.kind)));
                        ui.end_row();
                    }
                });
        }
        if let Some(n) = clicked {
            self.comment_at = Some((file.path.clone(), n));
            self.comment_text.clear();
        }
        // Comments on lines of this hunk, then the composer when its line is in this hunk.
        let in_hunk = |n: u32| {
            hunk.lines
                .iter()
                .any(|l| l.new_no == Some(n) || l.old_no == Some(n))
        };
        for c in self
            .state
            .comments
            .iter()
            .filter(|c| c.file == file.path && in_hunk(c.line))
        {
            ui.label(
                RichText::new(format!("L{}: {}", c.line, c.text))
                    .small()
                    .color(theme::TEXT_2),
            );
        }
        let at_here = self
            .comment_at
            .as_ref()
            .is_some_and(|(f, n)| *f == file.path && in_hunk(*n));
        if at_here {
            let line = self.comment_at.as_ref().map_or(0, |c| c.1);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.comment_text)
                        .hint_text(format!("L{line}"))
                        .desired_width((ui.available_width() - 90.0).max(60.0)),
                );
                if ui.button("OK").clicked() {
                    acts.borrow_mut().push(Act::AddComment);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests;
