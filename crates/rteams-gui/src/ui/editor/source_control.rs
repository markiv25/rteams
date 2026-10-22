//! Source Control sidebar panel: changed files (staged / unstaged / untracked), stage, unstage,
//! discard (confirmed), commit, per-file diff and history, and repository initialisation.
//! Every git call runs on a worker thread; results come back over a channel.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui;
use rteams_services::editor::gitops::{self, Change, Changes, LogEntry};

use crate::theme;

enum Msg {
    Status(Changes),
    Op(Result<String, String>),
    Doc(ViewDoc),
    Log(String, Result<Vec<LogEntry>, String>),
}

/// Read-only text shown in a window (a diff or a file at a commit).
pub struct ViewDoc {
    pub title: String,
    pub text: Result<String, String>,
    pub is_diff: bool,
}

pub struct SourceControl {
    pub open: bool,
    changes: Changes,
    loaded: bool,
    message: String,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    busy: usize,
    result: Option<Result<String, String>>,
    view: Option<ViewDoc>,
    history: Option<(String, Result<Vec<LogEntry>, String>)>,
    confirm: Option<Change>,
}

impl Default for SourceControl {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        SourceControl {
            open: false,
            changes: Changes::default(),
            loaded: false,
            message: String::new(),
            tx,
            rx,
            busy: 0,
            result: None,
            view: None,
            history: None,
            confirm: None,
        }
    }
}

#[derive(Default)]
pub struct ScOut {
    /// Open this project-relative file in the editor.
    pub open_file: Option<String>,
}

impl SourceControl {
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    fn bg(&mut self, ctx: &egui::Context, f: impl FnOnce() -> Msg + Send + 'static) {
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.busy += 1;
        std::thread::spawn(move || {
            let _ = tx.send(f());
            ctx.request_repaint();
        });
    }

    pub fn refresh(&mut self, root: &Path, ctx: &egui::Context) {
        let root = root.to_path_buf();
        self.bg(ctx, move || Msg::Status(gitops::changes(&root)));
    }

    /// Run a mutating operation, then refresh.
    fn op(
        &mut self,
        ctx: &egui::Context,
        f: impl FnOnce() -> Result<String, String> + Send + 'static,
    ) {
        self.bg(ctx, move || Msg::Op(f()));
    }

    /// Collect finished work (call every frame). True when an operation completed.
    pub fn poll(&mut self, root: &Path, ctx: &egui::Context) -> bool {
        let mut changed = false;
        while let Ok(m) = self.rx.try_recv() {
            self.busy = self.busy.saturating_sub(1);
            match m {
                Msg::Status(c) => {
                    self.changes = c;
                    self.loaded = true;
                }
                Msg::Op(r) => {
                    if r.is_ok() {
                        self.message_clear_on_commit(&r);
                    }
                    self.result = Some(r);
                    changed = true;
                    self.refresh(root, ctx);
                }
                Msg::Doc(d) => self.view = Some(d),
                Msg::Log(f, r) => self.history = Some((f, r)),
            }
        }
        changed
    }

    fn message_clear_on_commit(&mut self, r: &Result<String, String>) {
        if let Ok(s) = r {
            if s.starts_with("Committed") {
                self.message.clear();
            }
        }
    }

    fn files_section(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        title: &str,
        items: &[Change],
        staged: bool,
        out: &mut ScOut,
    ) {
        if items.is_empty() {
            return;
        }
        let top = self.changes.top.clone();
        egui::CollapsingHeader::new(
            egui::RichText::new(format!("{title} ({})", items.len())).strong(),
        )
        .id_salt(("sc-sec", title))
        .default_open(true)
        .show(ui, |ui| {
            for c in items {
                ui.horizontal(|ui| {
                    let color = match c.letter(staged) {
                        'A' | 'U' => theme::SUCCESS,
                        'D' | '!' => theme::ERROR,
                        _ => theme::WARNING,
                    };
                    ui.label(
                        egui::RichText::new(c.letter(staged).to_string())
                            .monospace()
                            .color(color),
                    );
                    let name = match &c.orig_path {
                        Some(o) => format!("{o} -> {}", c.path),
                        None => c.path.clone(),
                    };
                    let r = ui
                        .add(
                            egui::Label::new(egui::RichText::new(name).small())
                                .truncate()
                                .sense(egui::Sense::click()),
                        )
                        .on_hover_text(&c.path);
                    if r.clicked() {
                        let (c2, t2) = (c.clone(), top.clone());
                        let title =
                            format!("{} ({})", c.path, if staged { "staged" } else { "diff" });
                        self.bg(ctx, move || {
                            Msg::Doc(ViewDoc {
                                title,
                                text: gitops::diff(&t2, &c2, staged),
                                is_diff: true,
                            })
                        });
                    }
                    if r.double_clicked() {
                        out.open_file = self.changes.project_rel(c);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if staged {
                            if ui.small_button("−").on_hover_text("Unstage").clicked() {
                                let (t, p) = (top.clone(), vec![c.path.clone()]);
                                self.op(ctx, move || {
                                    gitops::unstage(&t, &p).map(|_| "Unstaged".into())
                                });
                            }
                        } else {
                            if ui.small_button("+").on_hover_text("Stage").clicked() {
                                let (t, p) = (top.clone(), vec![c.path.clone()]);
                                self.op(ctx, move || {
                                    gitops::stage(&t, &p).map(|_| "Staged".into())
                                });
                            }
                            if ui
                                .small_button("Discard")
                                .on_hover_text(if c.untracked {
                                    "Delete untracked file"
                                } else {
                                    "Discard changes"
                                })
                                .clicked()
                            {
                                self.confirm = Some(c.clone());
                            }
                        }
                    });
                });
            }
        });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, root: &Path, current_rel: Option<&str>) -> ScOut {
        let ctx = ui.ctx().clone();
        let mut out = ScOut::default();
        if !self.loaded && self.busy == 0 {
            self.refresh(root, &ctx);
        }
        ui.horizontal(|ui| {
            ui.strong("Source Control");
            if self.busy > 0 {
                ui.spinner();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("🗙").on_hover_text("Close").clicked() {
                    self.open = false;
                }
                if ui.small_button("↺").on_hover_text("Refresh").clicked() {
                    self.refresh(root, &ctx);
                }
            });
        });
        ui.separator();
        if !self.loaded {
            return out;
        }
        if !self.changes.is_repo {
            ui.label(
                egui::RichText::new("This folder is not a git repository.").color(theme::MUTED),
            );
            if ui.button("Initialize repository").clicked() {
                let r = root.to_path_buf();
                self.op(&ctx, move || {
                    gitops::init_repo(&r).map(|s| format!("Initialized: {s}"))
                });
            }
            self.show_result(ui);
            return out;
        }
        let top = self.changes.top.clone();
        let staged: Vec<Change> = self.changes.staged().into_iter().cloned().collect();
        let unstaged: Vec<Change> = self.changes.unstaged().into_iter().cloned().collect();
        let untracked: Vec<Change> = self.changes.untracked().into_iter().cloned().collect();

        ui.add(
            egui::TextEdit::multiline(&mut self.message)
                .hint_text("Commit message")
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        ui.horizontal_wrapped(|ui| {
            let can = !staged.is_empty() && !self.message.trim().is_empty() && self.busy == 0;
            if ui.add_enabled(can, egui::Button::new("Commit")).clicked() {
                let (r, m) = (root.to_path_buf(), self.message.clone());
                self.op(&ctx, move || {
                    gitops::commit_staged(&r, &m).map(|s| format!("Committed {s}"))
                });
            }
            if ui
                .add_enabled(
                    !unstaged.is_empty() || !untracked.is_empty(),
                    egui::Button::new("Stage all"),
                )
                .clicked()
            {
                let t = top.clone();
                self.op(&ctx, move || {
                    gitops::stage_all(&t).map(|_| "Staged all".into())
                });
            }
            if ui
                .add_enabled(!staged.is_empty(), egui::Button::new("Unstage all"))
                .clicked()
            {
                let t = top.clone();
                self.op(&ctx, move || {
                    gitops::unstage_all(&t).map(|_| "Unstaged all".into())
                });
            }
        });
        if !self.changes.has_commits
            && self.busy == 0
            && ui
                .button("Create initial commit")
                .on_hover_text("git add -A, then commit")
                .clicked()
        {
            let r = root.to_path_buf();
            self.op(&ctx, move || {
                gitops::initial_commit(&r, "Initial commit").map(|s| format!("Committed {s}"))
            });
        }
        self.show_result(ui);
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("sc-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if staged.is_empty() && unstaged.is_empty() && untracked.is_empty() {
                    ui.label(egui::RichText::new("No changes.").color(theme::MUTED));
                }
                self.files_section(ui, &ctx, "Staged Changes", &staged, true, &mut out);
                self.files_section(ui, &ctx, "Changes", &unstaged, false, &mut out);
                self.files_section(ui, &ctx, "Untracked", &untracked, false, &mut out);
                ui.add_space(6.0);
                self.history_section(ui, &ctx, root, current_rel);
            });
        out
    }

    fn show_result(&self, ui: &mut egui::Ui) {
        match &self.result {
            Some(Ok(s)) => {
                ui.colored_label(theme::SUCCESS, s);
            }
            Some(Err(e)) => {
                ui.colored_label(theme::ERROR, e);
            }
            None => {}
        }
    }

    fn history_section(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        root: &Path,
        current_rel: Option<&str>,
    ) {
        ui.strong("File History");
        let Some(rel) = current_rel else {
            ui.label(egui::RichText::new("Open a file to see its history.").color(theme::MUTED));
            return;
        };
        if ui.button(format!("Show history of {rel}")).clicked() {
            let (r, f) = (root.to_path_buf(), rel.to_string());
            self.bg(ctx, move || {
                let l = gitops::file_log(&r, &f);
                Msg::Log(f, l)
            });
        }
        let mut pick: Option<(String, LogEntry)> = None;
        match &self.history {
            Some((f, Err(e))) => {
                ui.colored_label(theme::ERROR, format!("{f}: {e}"));
            }
            Some((f, Ok(l))) => {
                ui.label(egui::RichText::new(f).small().color(theme::TEXT_2));
                if l.is_empty() {
                    ui.label(
                        egui::RichText::new("No commits touch this file.").color(theme::MUTED),
                    );
                }
                for e in l {
                    let r = ui
                        .add(
                            egui::Label::new(
                                egui::RichText::new(format!(
                                    "{}  {}  {}  {}",
                                    e.short, e.date, e.author, e.subject
                                ))
                                .small()
                                .monospace(),
                            )
                            .truncate()
                            .sense(egui::Sense::click()),
                        )
                        .on_hover_text("View this file at this commit");
                    if r.clicked() {
                        pick = Some((f.clone(), e.clone()));
                    }
                }
            }
            None => {}
        }
        if let Some((f, e)) = pick {
            let r: PathBuf = root.to_path_buf();
            let title = format!("{f} @ {}", e.short);
            self.bg(ctx, move || {
                Msg::Doc(ViewDoc {
                    title,
                    text: gitops::file_at(&r, &e.hash, &f),
                    is_diff: false,
                })
            });
        }
    }

    /// Floating windows: the diff / file viewer and the discard confirmation.
    pub fn show_windows(&mut self, ctx: &egui::Context) {
        if let Some(d) = &self.view {
            let mut open = true;
            egui::Window::new(&d.title)
                .open(&mut open)
                .default_size([640.0, 420.0])
                .show(ctx, |ui| {
                    egui::ScrollArea::both().show(ui, |ui| match &d.text {
                        Err(e) => {
                            ui.colored_label(theme::ERROR, e);
                        }
                        Ok(t) if t.trim().is_empty() => {
                            ui.label(egui::RichText::new("(empty)").color(theme::MUTED));
                        }
                        Ok(t) => {
                            for l in t.lines() {
                                let color = if !d.is_diff {
                                    theme::TEXT
                                } else if l.starts_with("+++") || l.starts_with("---") {
                                    theme::TEXT_2
                                } else if l.starts_with('+') {
                                    theme::SUCCESS
                                } else if l.starts_with('-') {
                                    theme::ERROR
                                } else if l.starts_with("@@") {
                                    theme::INFO
                                } else {
                                    theme::TEXT_2
                                };
                                ui.label(egui::RichText::new(l).monospace().small().color(color));
                            }
                        }
                    });
                });
            if !open {
                self.view = None;
            }
        }
        if let Some(c) = self.confirm.clone() {
            let mut decision = None;
            egui::Window::new("Discard changes?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(if c.untracked {
                        format!("Delete untracked file {}? This cannot be undone.", c.path)
                    } else {
                        format!(
                            "Discard all unstaged changes to {}? This cannot be undone.",
                            c.path
                        )
                    });
                    ui.horizontal(|ui| {
                        if ui
                            .button(if c.untracked { "Delete" } else { "Discard" })
                            .clicked()
                        {
                            decision = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            decision = Some(false);
                        }
                    });
                });
            match decision {
                Some(true) => {
                    let t = self.changes.top.clone();
                    self.op(ctx, move || {
                        gitops::discard(&t, &c).map(|_| "Discarded".into())
                    });
                    self.confirm = None;
                }
                Some(false) => self.confirm = None,
                None => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::render;

    #[test]
    fn renders_headless_and_commits() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        let g = |a: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(p)
                .args(a)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        if !g(&["init", "-q"]) {
            return;
        }
        g(&["config", "user.name", "T"]);
        g(&["config", "user.email", "t@e.com"]);
        g(&["config", "commit.gpgsign", "false"]);
        std::fs::write(p.join("a.txt"), "hi\n").unwrap();
        let mut sc = SourceControl {
            open: true,
            ..Default::default()
        };
        let frame = |sc: &mut SourceControl| {
            render(|ui| {
                let ctx = ui.ctx().clone();
                sc.poll(p, &ctx);
                sc.show(ui, p, Some("a.txt"));
                sc.show_windows(&ctx);
            });
        };
        for _ in 0..60 {
            frame(&mut sc);
            if sc.loaded {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(sc.changes.is_repo);
        assert_eq!(sc.changes.untracked().len(), 1);
        let t = sc.changes.top.clone();
        gitops::stage_all(&t).unwrap();
        sc.refresh(p, &egui::Context::default());
        for _ in 0..60 {
            frame(&mut sc);
            if !sc.changes.staged().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(sc.changes.staged().len(), 1);
        sc.confirm = Some(sc.changes.staged()[0].clone());
        sc.view = Some(ViewDoc {
            title: "x".into(),
            text: Ok("+a\n-b\n@@ x".into()),
            is_diff: true,
        });
        frame(&mut sc);
    }
}
