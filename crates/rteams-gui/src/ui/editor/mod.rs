//! The Project Editor screen: file tree | tabs + editor, quick open, search in files, status bar.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use eframe::egui;
use rteams_services::editor::conflict::{self, BaseStamp, DiskState};
use rteams_services::editor::files::{self, WriteOutcome};
use rteams_services::editor::gitstatus::{self, GitStatus};
use rteams_services::editor::search;
use rteams_services::editor::sessions::{self, Draft, DraftState, EditorSession, TabState};
use rteams_services::editor::watch::{FsEvent, FsKind, FsWatcher};

use super::{CloseBlocker, CloseParticipant, FeatureCtx, Screen, ShellRequest, TabId};
use crate::theme;

pub mod code_view;
pub mod dialogs;
pub mod find_panel;
pub mod goto_line;
pub mod highlight;
pub mod image_preview;
pub mod md_preview;
pub mod quick_open;
pub mod search_panel;
pub mod shortcuts;
pub mod status_bar;
pub mod tabs;
pub mod tree;

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Self-cleaning temp directory (the gui crate has no `tempfile` dev-dependency).
    pub struct TmpDir(PathBuf);

    impl TmpDir {
        pub fn new() -> TmpDir {
            static N: AtomicUsize = AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "rteams-editor-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TmpDir(p)
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

use dialogs::{Dialog, DialogResult};
use shortcuts::Cmd;
use tabs::{Banner, BannerAction, Tab, TabAction, TabKind};
use tree::{TreeAction, TreeState};

const SESSION_FLUSH: Duration = Duration::from_secs(2);
const GIT_DEBOUNCE: Duration = Duration::from_millis(800);

pub struct EditorScreen {
    home: PathBuf,
    root: Option<PathBuf>,
    /// Canonical root, to map watcher paths back to relative paths.
    croot: Option<PathBuf>,
    pending_open: Option<(PathBuf, Option<PathBuf>)>,
    tree: TreeState,
    tabs: Vec<Tab>,
    active: usize,
    git: GitStatus,
    git_rx: Option<Receiver<GitStatus>>,
    git_stale_at: Option<Instant>,
    files_cache: Option<Vec<String>>,
    files_rx: Option<Receiver<Vec<String>>>,
    quick: quick_open::QuickOpen,
    search: search_panel::SearchPanel,
    dialog: Dialog,
    show_shortcuts: bool,
    sidebar_open: bool,
    wrap: bool,
    watching: bool,
    watcher: Option<FsWatcher>,
    watched: HashSet<PathBuf>,
    watch_dirty: bool,
    session_dirty: bool,
    last_flush: Instant,
}

impl Default for EditorScreen {
    fn default() -> Self {
        Self::new(PathBuf::new())
    }
}

impl EditorScreen {
    pub fn new(home: PathBuf) -> Self {
        EditorScreen {
            home,
            root: None,
            croot: None,
            pending_open: None,
            tree: TreeState::default(),
            tabs: vec![],
            active: 0,
            git: GitStatus::default(),
            git_rx: None,
            git_stale_at: None,
            files_cache: None,
            files_rx: None,
            quick: Default::default(),
            search: Default::default(),
            dialog: Dialog::None,
            show_shortcuts: false,
            sidebar_open: true,
            wrap: false,
            watching: true,
            watcher: None,
            watched: HashSet::new(),
            watch_dirty: true,
            session_dirty: false,
            last_flush: Instant::now(),
        }
    }

    #[cfg(test)]
    pub fn open_tab_paths(&self) -> Vec<&str> {
        self.tabs.iter().map(|t| t.rel.as_str()).collect()
    }

    /// Open `root` as the project (restoring its saved tabs); optionally open `file` in it.
    pub fn open_root(&mut self, root: PathBuf, file: Option<PathBuf>) {
        if self.home.as_os_str().is_empty() {
            self.pending_open = Some((root, file));
            return;
        }
        if self.root.as_deref() == Some(root.as_path()) {
            if let Some(f) = file {
                self.open_abs(&f);
            }
            return;
        }
        self.flush_session();
        self.tabs.clear();
        self.active = 0;
        self.tree = TreeState::default();
        self.git = GitStatus::default();
        self.files_cache = None;
        self.dialog = Dialog::None;
        self.watched.clear();
        self.watch_dirty = true;
        self.croot = root.canonicalize().ok();
        self.root = Some(root.clone());
        self.tree.load(&root, "");
        self.git_stale_at = Some(Instant::now() - GIT_DEBOUNCE);

        let s = sessions::load(&self.home, &root);
        self.sidebar_open = s.sidebar_open;
        for ts in &s.open_tabs {
            let Ok(c) = files::read_text(&root, Path::new(&ts.path)) else {
                continue;
            };
            let disk_hash = c.hash.clone();
            let mut t = Tab::from_content(&ts.path, c);
            t.cursor = if ts.cursor.0 == 0 { (1, 1) } else { ts.cursor };
            t.scroll = ts.scroll;
            if let Some(d) = s.unsaved_drafts.get(&ts.path) {
                if t.kind == TabKind::Text && !t.read_only() {
                    t.text = d.text.clone();
                    t.dirty = true;
                    t.banner = Some(Banner::DraftRecovered);
                    if sessions::classify_draft(d, &disk_hash) == DraftState::Conflict {
                        t.base = BaseStamp {
                            mtime_ms: 0,
                            hash: d.base_hash.clone(),
                        };
                        t.banner = Some(Banner::ChangedOnDisk);
                    }
                }
            }
            self.tabs.push(t);
        }
        self.active = s
            .active
            .and_then(|a| self.tabs.iter().position(|t| t.rel == a))
            .unwrap_or(0);
        if let Some(a) = self.tabs.get(self.active) {
            let rel = a.rel.clone();
            self.tree.reveal(&root, &rel);
        }
        if let Some(f) = file {
            self.open_abs(&f);
        }
    }

    fn open_abs(&mut self, f: &Path) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let rel = if f.is_absolute() {
            f.strip_prefix(&root)
                .ok()
                .map(Path::to_path_buf)
                .or_else(|| {
                    self.croot
                        .as_ref()
                        .and_then(|c| f.strip_prefix(c).ok().map(Path::to_path_buf))
                })
        } else {
            Some(f.to_path_buf())
        };
        if let Some(rel) = rel {
            self.open_file(&files::rel_string(&rel), None);
        }
    }

    /// Open (or activate) `rel`; `jump` places the caret at (line, col).
    pub fn open_file(&mut self, rel: &str, jump: Option<(usize, usize)>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        if let Some(i) = self.tabs.iter().position(|t| t.rel == rel) {
            self.active = i;
        } else {
            match files::read_text(&root, Path::new(rel)) {
                Ok(c) => {
                    self.tabs.push(Tab::from_content(rel, c));
                    self.active = self.tabs.len() - 1;
                    self.watch_dirty = true;
                }
                Err(e) => {
                    if let Some(t) = self.tabs.get_mut(self.active) {
                        t.banner = Some(Banner::SaveFailed(e.to_string()));
                    }
                    return;
                }
            }
        }
        if let Some(t) = self.tabs.get_mut(self.active) {
            t.goto = jump;
        }
        self.tree.reveal(&root, rel);
        self.session_dirty = true;
    }

    // ---- saving, closing ----

    /// Save tab `i`. `Err` carries a message; on a conflict the conflict dialog is opened.
    fn save_tab(&mut self, i: usize, overwrite: bool) -> Result<(), String> {
        let Some(root) = self.root.clone() else {
            return Ok(());
        };
        let Some(t) = self.tabs.get_mut(i) else {
            return Ok(());
        };
        if !t.dirty || t.read_only() {
            return Ok(());
        }
        let expected = if overwrite || t.base.hash.is_empty() {
            None
        } else {
            Some(t.base.hash.clone())
        };
        match files::write_text(
            &root,
            Path::new(&t.rel),
            &t.text,
            t.encoding,
            expected.as_deref(),
        ) {
            Ok(WriteOutcome::Written { mtime_ms, hash }) => {
                t.base = BaseStamp { mtime_ms, hash };
                t.dirty = false;
                t.banner = None;
                self.git_stale_at = Some(Instant::now() - GIT_DEBOUNCE);
                self.session_dirty = true;
                Ok(())
            }
            Ok(WriteOutcome::Conflict { .. }) => {
                let disk = files::read_text(&root, Path::new(&t.rel))
                    .map(|c| c.text)
                    .unwrap_or_default();
                self.dialog = Dialog::conflict(&t.rel, &t.text, &disk);
                self.active = i;
                Err("conflict".into())
            }
            Err(e) => {
                t.banner = Some(Banner::SaveFailed(e.to_string()));
                Err(e.to_string())
            }
        }
    }

    fn save_all(&mut self) -> Result<(), String> {
        for i in 0..self.tabs.len() {
            self.save_tab(i, false)?;
        }
        Ok(())
    }

    fn close_tab(&mut self, i: usize, force: bool) {
        let Some(t) = self.tabs.get(i) else { return };
        if t.dirty && !force {
            self.dialog = Dialog::Unsaved {
                tab: Some(i),
                names: vec![t.rel.clone()],
            };
            return;
        }
        self.tabs.remove(i);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        } else if i < self.active {
            self.active -= 1;
        }
        self.watch_dirty = true;
        self.session_dirty = true;
    }

    fn dirty_names(&self) -> Vec<String> {
        self.tabs
            .iter()
            .filter(|t| t.dirty)
            .map(|t| t.rel.clone())
            .collect()
    }

    fn reload_tab(&mut self, rel: &str) {
        let Some(root) = self.root.clone() else {
            return;
        };
        if let (Some(t), Ok(c)) = (
            self.tabs.iter_mut().find(|t| t.rel == rel),
            files::read_text(&root, Path::new(rel)),
        ) {
            let (cursor, scroll) = (t.cursor, t.scroll);
            *t = Tab::from_content(rel, c);
            t.cursor = cursor;
            t.scroll = scroll;
            self.session_dirty = true;
        }
    }

    /// Keep the buffer, adopt the current disk state as its base so the next save overwrites.
    fn keep_mine(&mut self, rel: &str) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let base = files::read_text(&root, Path::new(rel))
            .map(|c| BaseStamp::from(&c))
            .unwrap_or_default();
        if let Some(t) = self.tabs.iter_mut().find(|t| t.rel == rel) {
            t.base = base;
            t.dirty = true;
            t.banner = None;
        }
    }

    // ---- file operations ----

    fn remap_tabs(&mut self, from: &str, to: &str) {
        for t in &mut self.tabs {
            if t.rel == from {
                t.rel = to.to_string();
            } else if let Some(rest) = t.rel.strip_prefix(&format!("{from}/")) {
                t.rel = format!("{to}/{rest}");
            }
        }
        self.session_dirty = true;
        self.watch_dirty = true;
    }

    fn refresh_tree(&mut self) {
        if let Some(root) = self.root.clone() {
            self.tree.reload_all(&root);
        }
        self.files_cache = None;
        self.git_stale_at = Some(Instant::now() - GIT_DEBOUNCE);
    }

    fn handle_tree(&mut self, acts: Vec<TreeAction>, c: &mut FeatureCtx) {
        let Some(root) = self.root.clone() else {
            return;
        };
        for a in acts {
            match a {
                TreeAction::Open(rel) => self.open_file(&rel, None),
                TreeAction::NewFile(p) => self.dialog = Dialog::new_entry(&p, false),
                TreeAction::NewFolder(p) => self.dialog = Dialog::new_entry(&p, true),
                TreeAction::Rename(r) => self.dialog = Dialog::rename(&r),
                TreeAction::Delete { rel, .. } => self.dialog = Dialog::Delete { rel },
                TreeAction::Reveal(rel) => reveal(&root.join(rel)),
                TreeAction::Expanded(_) => self.watch_dirty = true,
                TreeAction::Move { from, to_dir } => {
                    match files::move_to(&root, Path::new(&from), Path::new(&to_dir)) {
                        Ok(new) => {
                            self.remap_tabs(&from, &files::rel_string(&new));
                            self.tree.expand(&root, &to_dir);
                            self.refresh_tree();
                        }
                        Err(e) => c.req.push(ShellRequest::Toast(e.to_string())),
                    }
                }
            }
        }
    }

    fn handle_dialog(&mut self, r: DialogResult, c: &mut FeatureCtx) {
        let Some(root) = self.root.clone() else {
            return;
        };
        if r == DialogResult::Pending {
            return;
        }
        self.dialog = Dialog::None;
        match r {
            DialogResult::Pending | DialogResult::Cancel => {}
            DialogResult::Create {
                parent,
                is_dir,
                name,
            } => {
                let res = if is_dir {
                    files::create_dir(&root, Path::new(&parent), &name)
                } else {
                    files::create_file(&root, Path::new(&parent), &name)
                };
                match res {
                    Ok(rel) => {
                        self.tree.expand(&root, &parent);
                        self.refresh_tree();
                        if !is_dir {
                            self.open_file(&files::rel_string(&rel), None);
                        }
                    }
                    Err(e) => c.req.push(ShellRequest::Toast(e.to_string())),
                }
            }
            DialogResult::Rename { rel, name } => {
                match files::rename(&root, Path::new(&rel), &name) {
                    Ok(new) => {
                        self.remap_tabs(&rel, &files::rel_string(&new));
                        self.refresh_tree();
                    }
                    Err(e) => c.req.push(ShellRequest::Toast(e.to_string())),
                }
            }
            DialogResult::Trash { rel } => match files::trash_path(&root, Path::new(&rel)) {
                Ok(()) => {
                    let prefix = format!("{rel}/");
                    while let Some(i) = self
                        .tabs
                        .iter()
                        .position(|t| t.rel == rel || t.rel.starts_with(&prefix))
                    {
                        self.close_tab(i, true);
                    }
                    self.refresh_tree();
                }
                Err(e) => c.req.push(ShellRequest::Toast(e.to_string())),
            },
            DialogResult::SaveAllAndClose { tab } => {
                let ok = match tab {
                    Some(i) => self.save_tab(i, false),
                    None => self.save_all(),
                };
                if ok.is_ok() {
                    match tab {
                        Some(i) => self.close_tab(i, true),
                        None => c.req.push(ShellRequest::OpenTab(TabId::Dashboard)),
                    }
                }
            }
            DialogResult::DiscardAndClose { tab } => match tab {
                Some(i) => self.close_tab(i, true),
                None => {
                    for t in &mut self.tabs {
                        t.dirty = false;
                    }
                    self.session_dirty = true;
                    c.req.push(ShellRequest::OpenTab(TabId::Dashboard));
                }
            },
            DialogResult::Overwrite { rel } => {
                if let Some(i) = self.tabs.iter().position(|t| t.rel == rel) {
                    let _ = self.save_tab(i, true);
                }
            }
            DialogResult::Reload { rel } => self.reload_tab(&rel),
            DialogResult::KeepMine { rel } => self.keep_mine(&rel),
        }
    }

    fn request_close_editor(&mut self, c: &mut FeatureCtx) {
        let names = self.dirty_names();
        if names.is_empty() {
            c.req.push(ShellRequest::OpenTab(TabId::Dashboard));
        } else {
            self.dialog = Dialog::Unsaved { tab: None, names };
        }
    }

    // ---- background work ----

    fn spawn_git(&mut self, ctx: &egui::Context) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(gitstatus::status(&root));
            ctx.request_repaint();
        });
        self.git_rx = Some(rx);
    }

    fn ensure_files_cache(&mut self, ctx: &egui::Context) {
        if self.files_cache.is_some() || self.files_rx.is_some() {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(search::list_files(&root));
            ctx.request_repaint();
        });
        self.files_rx = Some(rx);
    }

    fn on_fs_event(&mut self, ev: &FsEvent) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let rel = self
            .croot
            .as_ref()
            .and_then(|c| ev.path.strip_prefix(c).ok())
            .or_else(|| ev.path.strip_prefix(&root).ok())
            .map(files::rel_string);
        let Some(rel) = rel else { return };
        if rel == ".git" || rel.starts_with(".git/") {
            return;
        }
        let abs = root.join(&rel);
        if let Some(i) = self.tabs.iter().position(|t| t.rel == rel) {
            let state = conflict::check(&abs, &self.tabs[i].base);
            match state {
                DiskState::Unchanged => {}
                DiskState::Changed { .. } => {
                    if self.tabs[i].dirty {
                        self.tabs[i].banner = Some(Banner::ChangedOnDisk);
                    } else {
                        self.reload_tab(&rel);
                    }
                }
                DiskState::Deleted => self.tabs[i].banner = Some(Banner::Deleted),
            }
        }
        if matches!(ev.kind, FsKind::Created | FsKind::Deleted) {
            self.files_cache = None;
        }
        self.git_stale_at = Some(Instant::now());
    }

    /// Everything that must run each frame, visible or not.
    fn pump(&mut self, ctx: &egui::Context, home: &Path) {
        if self.home.as_os_str().is_empty() {
            self.home = home.to_path_buf();
        }
        if let Some((r, f)) = self.pending_open.take() {
            self.open_root(r, f);
        }
        if self.root.is_none() {
            return;
        }
        if self.watcher.is_none() && self.watching {
            let c2 = ctx.clone();
            self.watcher = FsWatcher::new(move || c2.request_repaint()).ok();
            self.watch_dirty = true;
        }
        if self.watch_dirty {
            self.sync_watches();
        }
        let mut evs = Vec::new();
        if let Some(w) = &self.watcher {
            while let Ok(e) = w.events.try_recv() {
                evs.push(e);
            }
        }
        if !evs.is_empty() {
            let mut seen = HashSet::new();
            for e in evs {
                if seen.insert((e.path.clone(), e.kind as u8)) {
                    self.on_fs_event(&e);
                }
            }
            self.refresh_tree();
        }
        if let Some(rx) = &self.git_rx {
            if let Ok(g) = rx.try_recv() {
                self.git = g;
                self.git_rx = None;
            }
        }
        if self.git_rx.is_none() {
            if let Some(at) = self.git_stale_at {
                if at.elapsed() >= GIT_DEBOUNCE {
                    self.git_stale_at = None;
                    self.spawn_git(ctx);
                } else {
                    ctx.request_repaint_after(GIT_DEBOUNCE);
                }
            }
        }
        if let Some(rx) = &self.files_rx {
            if let Ok(f) = rx.try_recv() {
                self.files_cache = Some(f);
                self.files_rx = None;
            }
        }
        self.search.poll();
        if self.session_dirty || self.tabs.iter().any(|t| t.dirty) {
            if self.last_flush.elapsed() >= SESSION_FLUSH {
                self.flush_session();
            } else {
                ctx.request_repaint_after(SESSION_FLUSH);
            }
        }
    }

    fn sync_watches(&mut self) {
        self.watch_dirty = false;
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(w) = &mut self.watcher else { return };
        let mut want: HashSet<PathBuf> = HashSet::new();
        want.insert(root.clone());
        for d in &self.tree.expanded {
            want.insert(root.join(d));
        }
        for t in &self.tabs {
            if let Some(p) = root.join(&t.rel).parent() {
                want.insert(p.to_path_buf());
            }
        }
        for d in self.watched.difference(&want) {
            w.unwatch_dir(d);
        }
        for d in want.difference(&self.watched) {
            w.watch_dir(d);
        }
        self.watched = want;
    }

    fn session(&self) -> EditorSession {
        let mut s = EditorSession {
            sidebar_open: self.sidebar_open,
            active: self.tabs.get(self.active).map(|t| t.rel.clone()),
            ..Default::default()
        };
        for t in &self.tabs {
            s.open_tabs.push(TabState {
                path: t.rel.clone(),
                cursor: t.cursor,
                scroll: t.scroll,
            });
            if t.dirty && t.kind == TabKind::Text && !t.large && !t.read_only() {
                s.unsaved_drafts.insert(
                    t.rel.clone(),
                    Draft {
                        text: t.text.clone(),
                        base_hash: t.base.hash.clone(),
                    },
                );
            }
        }
        s
    }

    /// Persist tabs and drafts now.
    pub fn flush_session(&mut self) {
        self.last_flush = Instant::now();
        self.session_dirty = false;
        if let (Some(root), false) = (&self.root, self.home.as_os_str().is_empty()) {
            let _ = sessions::save(&self.home, root, &self.session());
        }
    }

    // ---- drawing ----

    fn handle_cmds(&mut self, ctx: &egui::Context, c: &mut FeatureCtx) {
        for cmd in shortcuts::poll(ctx) {
            match cmd {
                Cmd::QuickOpen => {
                    self.quick.toggle();
                    self.ensure_files_cache(ctx);
                }
                Cmd::Save => {
                    let a = self.active;
                    let _ = self.save_tab(a, false);
                }
                Cmd::SaveAll => {
                    let _ = self.save_all();
                }
                Cmd::CloseTab => {
                    let a = self.active;
                    self.close_tab(a, false);
                }
                Cmd::SearchInFiles => {
                    self.sidebar_open = true;
                    self.search.toggle();
                }
                Cmd::NextTab if !self.tabs.is_empty() => {
                    self.active = (self.active + 1) % self.tabs.len()
                }
                Cmd::PrevTab if !self.tabs.is_empty() => {
                    self.active = (self.active + self.tabs.len() - 1) % self.tabs.len()
                }
                Cmd::SplitPreview | Cmd::FullPreview => {
                    if let Some(t) = self.tabs.get_mut(self.active) {
                        let target = if cmd == Cmd::SplitPreview {
                            tabs::MdMode::Split
                        } else {
                            tabs::MdMode::Full
                        };
                        t.md = if t.md == target {
                            tabs::MdMode::Off
                        } else {
                            target
                        };
                    }
                }
                Cmd::ToggleSidebar => self.sidebar_open = !self.sidebar_open,
                Cmd::Refresh => {
                    self.refresh_tree();
                }
                _ => {}
            }
        }
        let _ = c;
    }

    fn show_body(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        let Some(root) = self.root.clone() else {
            return;
        };
        if self.tabs.is_empty() {
            ui.add_space(40.0);
            crate::ui::dashboard::kit::empty_state(
                ui,
                crate::icons::Icon::File,
                &c.tr.t("team.editor.empty.selectFile"),
                "",
            );
            return;
        }
        let acts = tabs::show_bar(ui, &self.tabs, self.active, c.tr);
        for a in acts {
            match a {
                TabAction::Activate(i) => {
                    self.active = i;
                    self.session_dirty = true;
                }
                TabAction::Close(i) => self.close_tab(i, false),
            }
        }
        ui.separator();
        let Some(t) = self.tabs.get_mut(self.active) else {
            return;
        };
        let mut banner_act = None;
        if let Some(b) = &t.banner {
            banner_act = tabs::show_banner(ui, b, c.tr);
        }
        if t.kind != TabKind::Text {
            tabs::show_non_text(ui, &root, t, c.tr);
        } else {
            if t.truncated {
                ui.label(
                    egui::RichText::new(c.tr.t("team.editor.largeFilePreview"))
                        .color(theme::WARNING)
                        .small(),
                );
            } else if t.large {
                ui.label(
                    egui::RichText::new(c.tr.t("team.editor.largeFileMode"))
                        .color(theme::WARNING)
                        .small(),
                );
            }
            let wrap = self.wrap;
            let is_md = t.language() == "Markdown" && !t.large;
            let r = if is_md {
                tabs::edit_markdown(ui, t, wrap, c.tr)
            } else {
                tabs::edit_text(ui, t, wrap)
            };
            if r.changed {
                t.dirty = true;
                self.session_dirty = true;
            }
            if let Some(cur) = r.cursor.filter(|c| c.0 > 0) {
                t.cursor = cur;
            }
        }
        let rel = t.rel.clone();
        let idx = self.active;
        match banner_act {
            Some(BannerAction::Reload) => self.reload_tab(&rel),
            Some(BannerAction::Keep) => {
                if self.tabs[idx].banner == Some(Banner::Deleted) {
                    self.tabs[idx].base = BaseStamp::default();
                    self.tabs[idx].dirty = true;
                    self.tabs[idx].banner = None;
                } else {
                    self.keep_mine(&rel);
                }
            }
            Some(BannerAction::Retry) => {
                let _ = self.save_tab(idx, false);
            }
            Some(BannerAction::Dismiss) => self.tabs[idx].banner = None,
            None => {}
        }
    }
}

fn reveal(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = open::that(path.parent().unwrap_or(path));
    }
}

impl Screen for EditorScreen {
    fn id(&self) -> TabId {
        TabId::Editor
    }

    fn title_key(&self) -> &'static str {
        "team.editor.ariaLabel"
    }

    fn tick(&mut self, ctx: &egui::Context, c: &mut FeatureCtx) {
        self.pump(ctx, c.home);
    }

    fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        let ctx = ui.ctx().clone();
        self.pump(&ctx, c.home);
        let Some(root) = self.root.clone() else {
            ui.add_space(60.0);
            crate::ui::dashboard::kit::empty_state(
                ui,
                crate::icons::Icon::Folder,
                "No project folder open",
                "Open a folder to browse and edit its files.",
            );
            ui.vertical_centered(|ui| {
                if theme::pill_icon_button(
                    ui,
                    Some(crate::icons::Icon::Folder),
                    &c.tr.t("dashboard.recentProjects.selectFolder"),
                    true,
                )
                .clicked()
                {
                    if let Some(p) = crate::ui::dashboard::project_picker::pick_folder(c.tr) {
                        self.open_root(p.into(), None);
                    }
                }
            });
            return;
        };
        if !self.dialog.is_open() && !self.quick.open {
            self.handle_cmds(&ctx, c);
        }

        // Status bar.
        let tab = self.tabs.get(self.active);
        let info = status_bar::StatusInfo {
            cursor: tab.filter(|t| t.kind == TabKind::Text).map(|t| t.cursor),
            language: tab
                .filter(|t| t.kind == TabKind::Text)
                .map(|t| t.language()),
            branch: self.git.branch.as_deref(),
            encoding: tab
                .filter(|t| t.kind == TabKind::Text)
                .map(|t| t.encoding.label()),
            large: tab.is_some_and(|t| t.large),
        };
        let mut sr = status_bar::StatusResponse::default();
        egui::Panel::bottom("editor-status")
            .exact_size(24.0)
            .frame(
                egui::Frame::new()
                    .fill(theme::SIDEBAR)
                    .inner_margin(egui::Margin::symmetric(10, 3)),
            )
            .show(ui, |ui| {
                sr = status_bar::show(ui, &info, self.watcher.is_some(), self.wrap, c.tr)
            });
        if sr.toggle_wrap {
            self.wrap = !self.wrap;
        }
        if sr.shortcuts {
            self.show_shortcuts = true;
        }
        if sr.toggle_watch {
            if self.watcher.take().is_some() {
                self.watched.clear();
                self.watching = false;
            } else {
                self.watching = true;
            }
        }

        // Sidebar.
        let mut tree_acts = Vec::new();
        let mut jump = None;
        let mut close_editor = false;
        if self.sidebar_open {
            egui::Panel::left("editor-sidebar")
                .default_size(250.0)
                .size_range(160.0..=480.0)
                .frame(
                    egui::Frame::new()
                        .fill(theme::SIDEBAR)
                        .inner_margin(egui::Margin::symmetric(8, 8)),
                )
                .show(ui, |ui| {
                    if self.search.open {
                        jump = self.search.show(ui, &root, c.tr);
                        return;
                    }
                    ui.horizontal(|ui| {
                        ui.strong(c.tr.t("team.editor.sidebar.explorer"));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button("🗙")
                                .on_hover_text(c.tr.t("team.editor.actions.closeTooltip"))
                                .clicked()
                            {
                                close_editor = true;
                            }
                            if ui
                                .small_button("↺")
                                .on_hover_text(c.tr.t("team.editor.actions.refreshTooltip"))
                                .clicked()
                            {
                                self.refresh_tree();
                            }
                        });
                    });
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt("editor-tree")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            tree_acts = self.tree.show(ui, &root, &self.git, c.tr);
                        });
                });
        }
        self.handle_tree(tree_acts, c);
        if let Some(j) = jump {
            self.open_file(&j.rel, Some((j.line, j.col)));
        }
        if close_editor {
            self.request_close_editor(c);
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::symmetric(6, 4)),
            )
            .show(ui, |ui| self.show_body(ui, c));

        // Overlays.
        if self.quick.open {
            self.ensure_files_cache(&ctx);
            if let Some(rel) = self.quick.show(&ctx, self.files_cache.as_deref(), c.tr) {
                self.open_file(&rel, None);
            }
        }
        if self.show_shortcuts {
            shortcuts::show(&ctx, &mut self.show_shortcuts, c.tr);
        }
        if self.dialog.is_open() {
            let r = dialogs::show(&ctx, &mut self.dialog, c.tr);
            self.handle_dialog(r, c);
        }
    }
}

impl CloseParticipant for EditorScreen {
    fn blockers(&self) -> Vec<CloseBlocker> {
        self.tabs
            .iter()
            .filter(|t| t.dirty)
            .map(|t| CloseBlocker {
                owner: "editor".into(),
                detail: t.rel.clone(),
            })
            .collect()
    }

    fn save_all(&mut self) -> Result<(), String> {
        EditorScreen::save_all(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{testing::render, Intents};
    use rteams_services::i18n::I18n;

    fn project() -> (testutil::TmpDir, testutil::TmpDir) {
        let home = testutil::TmpDir::new();
        let proj = testutil::TmpDir::new();
        std::fs::create_dir_all(proj.path().join("src")).unwrap();
        std::fs::write(proj.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(proj.path().join("README.md"), "# hi\n").unwrap();
        (home, proj)
    }

    #[test]
    fn renders_and_edits_headless() {
        let (home, proj) = project();
        let tr = I18n::default();
        let mut ed = EditorScreen::new(home.path().to_path_buf());
        ed.open_root(
            proj.path().to_path_buf(),
            Some(PathBuf::from("src/main.rs")),
        );
        assert_eq!(ed.open_tab_paths(), vec!["src/main.rs"]);
        let mut out = Intents::default();
        let mut req = vec![];
        render(|ui| {
            let mut c = FeatureCtx {
                home: home.path(),
                team: None,
                view: None,
                out: &mut out,
                tr: &tr,
                req: &mut req,
            };
            ed.show(ui, &mut c);
        });
        ed.tabs[0].text.push_str("// x\n");
        ed.tabs[0].dirty = true;
        assert_eq!(ed.blockers().len(), 1);
        ed.dialog = Dialog::Unsaved {
            tab: None,
            names: vec!["src/main.rs".into()],
        };
        ed.quick.toggle();
        ed.show_shortcuts = true;
        render(|ui| {
            let mut c = FeatureCtx {
                home: home.path(),
                team: None,
                view: None,
                out: &mut out,
                tr: &tr,
                req: &mut req,
            };
            ed.show(ui, &mut c);
        });
        ed.dialog = Dialog::None;
        ed.quick.open = false;
        CloseParticipant::save_all(&mut ed).unwrap();
        assert!(ed.blockers().is_empty());
        assert_eq!(
            std::fs::read_to_string(proj.path().join("src/main.rs")).unwrap(),
            "fn main() {}\n// x\n"
        );
    }

    #[test]
    fn session_restores_tabs_and_drafts() {
        let (home, proj) = project();
        let mut ed = EditorScreen::new(home.path().to_path_buf());
        ed.open_root(proj.path().to_path_buf(), Some("README.md".into()));
        ed.open_file("src/main.rs", None);
        ed.tabs[1].text = "draft\n".into();
        ed.tabs[1].dirty = true;
        ed.flush_session();
        let mut ed2 = EditorScreen::new(home.path().to_path_buf());
        ed2.open_root(proj.path().to_path_buf(), None);
        assert_eq!(ed2.open_tab_paths(), vec!["README.md", "src/main.rs"]);
        assert_eq!(ed2.active, 1);
        assert!(ed2.tabs[1].dirty && ed2.tabs[1].text == "draft\n");
        assert_eq!(ed2.tabs[1].banner, Some(Banner::DraftRecovered));
    }

    #[test]
    fn save_conflict_and_rename_remap() {
        let (home, proj) = project();
        let mut ed = EditorScreen::new(home.path().to_path_buf());
        ed.open_root(proj.path().to_path_buf(), Some("README.md".into()));
        ed.tabs[0].text = "mine\n".into();
        ed.tabs[0].dirty = true;
        std::fs::write(proj.path().join("README.md"), "theirs\n").unwrap();
        assert!(ed.save_tab(0, false).is_err());
        assert!(matches!(ed.dialog, Dialog::Conflict { .. }));
        assert!(ed.save_tab(0, true).is_ok());
        assert_eq!(
            std::fs::read_to_string(proj.path().join("README.md")).unwrap(),
            "mine\n"
        );
        ed.remap_tabs("README.md", "DOC.md");
        assert_eq!(ed.tabs[0].rel, "DOC.md");
    }
}
