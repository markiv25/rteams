//! Bottom terminal drawer: header, tab strip, emulator, command bar, footer.
//!
//! The shell owns one `TerminalDrawer`, calls `open(cwd)` on `ShellRequest::OpenTerminal`,
//! `toggle()` from a shortcut/button, and `show(ui, c)` every frame **before** the central
//! content (it adds an `egui::Panel::bottom`). It does nothing while closed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use rteams_services::terminal::{TabSpec, TerminalSettings};

use super::settings_panel::{self, RuntimeAction};
use super::tabs::{TabAction, Tabs};
use super::widget;
use crate::theme;
use crate::ui::FeatureCtx;

struct Workspace {
    settings: TerminalSettings,
    tabs: Tabs,
    settings_open: bool,
}

impl Workspace {
    fn load(home: &Path, team: Option<&str>) -> Self {
        let settings = TerminalSettings::load(home, team);
        let mut tabs = Tabs::from_specs(&settings.tabs);
        if tabs.tabs.is_empty() {
            tabs.add(TabSpec::default());
        }
        Workspace {
            settings,
            tabs,
            settings_open: false,
        }
    }

    fn save(&mut self, home: &Path, team: Option<&str>) {
        self.settings.tabs = self.tabs.specs();
        let _ = self.settings.save(home, team);
    }
}

#[derive(Default)]
pub struct TerminalDrawer {
    open: bool,
    expanded: bool,
    ws: HashMap<Option<String>, Workspace>,
    pending: Vec<PathBuf>,
    confirm_close: Option<usize>,
    command: String,
    branch: Option<(PathBuf, Instant, Option<String>)>,
}

/// Current git branch (or short hash) for `cwd`, reading `.git/HEAD` upwards.
pub fn git_branch(cwd: &Path) -> Option<String> {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        let git = d.join(".git");
        let head = if git.is_file() {
            // worktree: `gitdir: <path>`
            let gd = std::fs::read_to_string(&git).ok()?;
            let gd = gd.trim().strip_prefix("gitdir:")?.trim().to_string();
            let p = PathBuf::from(gd);
            let p = if p.is_absolute() { p } else { d.join(p) };
            std::fs::read_to_string(p.join("HEAD")).ok()
        } else if git.is_dir() {
            std::fs::read_to_string(git.join("HEAD")).ok()
        } else {
            None
        };
        if let Some(h) = head {
            let h = h.trim();
            return Some(match h.strip_prefix("ref: refs/heads/") {
                Some(b) => b.to_string(),
                None => h.chars().take(8).collect(),
            });
        }
        dir = d.parent();
    }
    None
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

impl TerminalDrawer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open the drawer with a shell in `cwd` (reuses the first untouched tab, else adds one).
    pub fn open(&mut self, cwd: PathBuf) {
        self.open = true;
        self.pending.push(cwd);
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Tabs whose shell is alive (for a close-confirmation participant).
    pub fn running_tabs(&mut self) -> usize {
        let mut n = 0;
        for t in self.ws.values_mut().flat_map(|w| w.tabs.tabs.iter_mut()) {
            if t.session.as_mut().is_some_and(|s| s.is_alive()) {
                n += 1;
            }
        }
        n
    }

    fn branch_for(&mut self, cwd: &Path) -> Option<String> {
        let fresh = self
            .branch
            .as_ref()
            .is_some_and(|(p, at, _)| p == cwd && at.elapsed() < Duration::from_secs(3));
        if !fresh {
            self.branch = Some((cwd.to_path_buf(), Instant::now(), git_branch(cwd)));
        }
        self.branch.as_ref().and_then(|b| b.2.clone())
    }

    pub fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        if !self.open {
            return;
        }
        let key = c.team.map(str::to_string);
        let mut ws = self
            .ws
            .remove(&key)
            .unwrap_or_else(|| Workspace::load(c.home, c.team));
        let mut dirty = false;

        for cwd in std::mem::take(&mut self.pending) {
            let reuse = ws.tabs.tabs.len() == 1
                && ws.tabs.tabs[0].session.is_none()
                && ws.tabs.tabs[0].spec.cwd.is_none();
            if reuse {
                ws.tabs.tabs[0].spec.cwd = Some(cwd);
                ws.tabs.tabs[0].spec.kind = if c.team.is_some() { "team" } else { "local" }.into();
            } else if !ws
                .tabs
                .tabs
                .iter()
                .any(|t| t.spec.cwd.as_deref() == Some(cwd.as_path()))
            {
                ws.tabs.add(TabSpec {
                    cwd: Some(cwd),
                    kind: if c.team.is_some() { "team" } else { "local" }.into(),
                    ..Default::default()
                });
            } else if let Some(i) = ws
                .tabs
                .tabs
                .iter()
                .position(|t| t.spec.cwd.as_deref() == Some(cwd.as_path()))
            {
                ws.tabs.active = i;
            }
            ws.settings_open = false;
            dirty = true;
        }

        let tr = c.tr;
        let k = |s: &str| tr.t(&format!("team.terminalWorkspace.{s}"));
        let screen_h = ui.ctx().content_rect().height();
        let max_h = (screen_h - 120.0).max(160.0);
        let panel_id = if self.expanded {
            "terminal_drawer_expanded"
        } else {
            "terminal_drawer"
        };
        let default_h = if self.expanded {
            max_h
        } else {
            ws.settings.drawer_height.min(max_h)
        };
        let pal = widget::palette(&ws.settings.theme);
        let mut want_close = false;
        let mut runtime = RuntimeAction::None;
        let mut close_request: Option<usize> = None;

        let panel = egui::Panel::bottom(panel_id)
            .resizable(true)
            .default_size(default_h)
            .size_range(140.0..=max_h)
            .frame(
                egui::Frame::new()
                    .fill(theme::SIDEBAR)
                    .inner_margin(egui::Margin::symmetric(10, 6)),
            )
            .show(ui, |ui| {
                // Header.
                let active_kind = ws
                    .tabs
                    .tabs
                    .get(ws.tabs.active)
                    .map(|t| t.spec.kind.clone())
                    .unwrap_or_default();
                ui.horizontal(|ui| {
                    let title = match c.team {
                        Some(t) => {
                            tr.t_args("team.terminalWorkspace.teamTerminalTitle", &[("team", t)])
                        }
                        None => tr.t("common.terminal.title"),
                    };
                    ui.label(RichText::new(title).strong());
                    if active_kind == "team" {
                        theme::pill(ui, k("teamRuntimeBadge"), theme::ACCENT);
                    } else {
                        theme::pill(ui, k("localShellBadge"), theme::MUTED);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("🗙")
                            .on_hover_text(k("closeTerminalSheet"))
                            .clicked()
                        {
                            want_close = true;
                        }
                        let (glyph, tip) = if self.expanded {
                            ("⏷", k("restoreHalfHeightSheet"))
                        } else {
                            ("⏶", k("expandTerminalSheet"))
                        };
                        if ui.button(glyph).on_hover_text(tip).clicked() {
                            self.expanded = !self.expanded;
                        }
                        let tip = if ws.settings_open {
                            k("closeTerminalSettings")
                        } else {
                            k("openTerminalSettings")
                        };
                        if ui
                            .selectable_label(ws.settings_open, "⚙")
                            .on_hover_text(tip)
                            .clicked()
                        {
                            ws.settings_open = !ws.settings_open;
                        }
                    });
                });

                // Tab strip.
                let settings_open = ws.settings_open;
                for a in ws.tabs.bar(ui, tr, settings_open) {
                    match a {
                        TabAction::Select(i) => {
                            ws.tabs.active = i;
                            ws.settings_open = false;
                        }
                        TabAction::New => {
                            let cwd = ws
                                .tabs
                                .tabs
                                .get(ws.tabs.active)
                                .and_then(|t| t.spec.cwd.clone());
                            let kind = ws
                                .tabs
                                .tabs
                                .get(ws.tabs.active)
                                .map(|t| t.spec.kind.clone())
                                .unwrap_or_default();
                            ws.tabs.add(TabSpec {
                                cwd,
                                kind,
                                ..Default::default()
                            });
                            ws.settings_open = false;
                            dirty = true;
                        }
                        TabAction::Close(i) => close_request = Some(i),
                        TabAction::ToggleSettings => ws.settings_open = !ws.settings_open,
                        TabAction::Changed => dirty = true,
                    }
                }
                ui.add_space(2.0);

                if ws.settings_open {
                    let (changed, action) = settings_panel::show(ui, tr, &mut ws.settings);
                    dirty |= changed;
                    runtime = action;
                    return;
                }

                // Footer: command bar + cwd/branch.
                let default_cwd = home_dir();
                let active = ws.tabs.active;
                let cwd = ws
                    .tabs
                    .tabs
                    .get(active)
                    .and_then(|t| t.spec.cwd.clone())
                    .unwrap_or_else(|| default_cwd.clone());
                let branch = self.branch_for(&cwd);
                egui::Panel::bottom(ui.make_persistent_id("terminal_footer"))
                    .frame(egui::Frame::new().inner_margin(egui::Margin {
                        top: 4,
                        ..Default::default()
                    }))
                    .show_separator_line(false)
                    .show(ui, |ui| {
                        let tab = &mut ws.tabs.tabs[active];
                        ui.horizontal(|ui| {
                            let edit = ui.add(
                                egui::TextEdit::singleline(&mut self.command)
                                    .hint_text(k("commandPlaceholder"))
                                    .desired_width((ui.available_width() - 330.0).max(80.0)),
                            );
                            let enter =
                                edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            let run = ui
                                .button(k("commandRun"))
                                .on_hover_text(k("commandRunTitle"))
                                .clicked();
                            if (enter || run) && !self.command.is_empty() {
                                if let Some(s) = tab.session.as_mut() {
                                    s.write(format!("{}\r", self.command).as_bytes());
                                }
                                self.command.clear();
                                if enter {
                                    edit.request_focus();
                                }
                            }
                            if ui
                                .button(k("commandInterrupt"))
                                .on_hover_text(k("commandInterruptTitle"))
                                .clicked()
                            {
                                if let Some(s) = tab.session.as_mut() {
                                    s.write(&[3]);
                                }
                            }
                            if ui.button(k("copy")).clicked() {
                                if let Some(t) = tab.widget.selected_text() {
                                    ui.ctx().copy_text(t);
                                }
                            }
                            if ui.button(k("copyOutput")).clicked() {
                                if let Some(s) = tab.session.as_ref() {
                                    ui.ctx().copy_text(s.screen().text());
                                }
                            }
                            if ui.button(k("copyCommand")).clicked() && !self.command.is_empty() {
                                ui.ctx().copy_text(self.command.clone());
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(cwd.display().to_string())
                                    .small()
                                    .monospace()
                                    .color(theme::MUTED),
                            )
                            .on_hover_text(k("currentWorkingDirectory"));
                            if let Some(b) = &branch {
                                ui.label(
                                    RichText::new(tr.t_args(
                                        "team.terminalWorkspace.gitBranchTitle",
                                        &[("branch", b)],
                                    ))
                                    .small()
                                    .color(theme::MUTED),
                                );
                            }
                        });
                    });

                // Emulator.
                let shell = ws.settings.shell.clone();
                let bg = match ws.settings.bg_mode.as_str() {
                    "transparent" => None,
                    _ => Some(pal.bg.gamma_multiply(ws.settings.opacity)),
                };
                let tab = &mut ws.tabs.tabs[active];
                tab.ensure_session(ui.ctx(), shell.as_deref(), &default_cwd);
                if let Some(err) = tab.error.clone() {
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(k("runtimeUnavailableTitle"))
                            .strong()
                            .color(theme::ERROR),
                    );
                    ui.label(RichText::new(err).small().color(theme::MUTED));
                    if ui.button(k("settingsReconnect")).clicked() {
                        runtime = RuntimeAction::Reconnect;
                    }
                    return;
                }
                let dead = tab.session.as_mut().is_some_and(|s| !s.is_alive());
                if dead {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(k("runtimeDisconnectedTitle")).color(theme::WARNING),
                        );
                        ui.label(
                            RichText::new(k("runtimeDisconnectedDetail"))
                                .small()
                                .color(theme::MUTED),
                        );
                        if ui.button(k("settingsReconnect")).clicked() {
                            runtime = RuntimeAction::Reconnect;
                        }
                    });
                }
                if let Some(s) = tab.session.as_mut() {
                    let frame = egui::Frame::new().fill(bg.unwrap_or(Color32::TRANSPARENT));
                    frame.show(ui, |ui| {
                        widget::show(ui, active, s, &mut tab.widget, &ws.settings, bg);
                    });
                }
            });

        // Remember the user-dragged height (not while expanded).
        if !self.expanded && !ws.settings_open {
            let h = panel.response.rect.height();
            if (h - ws.settings.drawer_height).abs() > 2.0 && h > 100.0 {
                ws.settings.drawer_height = h;
                ws.settings.clamp();
            }
        }

        match runtime {
            RuntimeAction::None => {}
            RuntimeAction::Reconnect => {
                if let Some(t) = ws.tabs.active_tab() {
                    t.restart();
                }
            }
            RuntimeAction::Reload => ws.tabs.tabs.iter_mut().for_each(|t| t.restart()),
            RuntimeAction::Stop => {
                if let Some(s) = ws.tabs.active_tab().and_then(|t| t.session.as_mut()) {
                    s.kill();
                }
            }
        }

        if let Some(i) = close_request {
            if ws.tabs.tabs.get(i).is_some_and(|t| t.has_history()) {
                self.confirm_close = Some(i);
            } else {
                ws.tabs.remove(i);
                dirty = true;
            }
        }
        if let Some(i) = self.confirm_close {
            let mut done = false;
            egui::Window::new(k("closeTerminalTabDialogTitle"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label(k("closeTerminalTabDialogDescription"));
                    ui.horizontal(|ui| {
                        if ui.button(k("cancel")).clicked() {
                            done = true;
                        }
                        if ui
                            .button(RichText::new(k("closeTab")).color(theme::ERROR))
                            .clicked()
                        {
                            ws.tabs.remove(i);
                            dirty = true;
                            done = true;
                        }
                    });
                });
            if done {
                self.confirm_close = None;
            }
        }

        if want_close {
            self.open = false;
        }
        if dirty || ws.settings.tabs.len() != ws.tabs.tabs.len() {
            ws.save(c.home, c.team);
        }
        self.ws.insert(key, ws);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::Intents;

    fn with_ctx(
        home: &Path,
        team: Option<&str>,
        mut f: impl FnMut(&mut egui::Ui, &mut FeatureCtx),
    ) {
        let tr = rteams_services::i18n::I18n::default();
        let mut out = Intents(vec![]);
        let mut req = vec![];
        crate::view::testing::render(|ui| {
            let mut c = FeatureCtx {
                home,
                team,
                view: None,
                out: &mut out,
                tr: &tr,
                req: &mut req,
            };
            f(ui, &mut c);
        });
    }

    #[test]
    fn closed_drawer_draws_nothing_open_one_spawns_shell() {
        let home = std::env::temp_dir().join(format!("rteams-term-drawer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let mut d = TerminalDrawer::new();
        with_ctx(&home, Some("t"), |ui, c| d.show(ui, c));
        assert!(!d.is_open());
        d.open(std::env::temp_dir());
        with_ctx(&home, Some("t"), |ui, c| d.show(ui, c));
        assert!(d.is_open());
        assert_eq!(d.ws.get(&Some("t".to_string())).unwrap().tabs.tabs.len(), 1);
        assert!(&home.join("t/terminal.json").exists());
        d.toggle();
        assert!(!d.is_open());
    }

    #[test]
    fn git_branch_reads_head() {
        let d = std::env::temp_dir().join(format!("rteams-term-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".git")).unwrap();
        std::fs::write(d.join(".git/HEAD"), "ref: refs/heads/feat/x\n").unwrap();
        std::fs::create_dir_all(d.join("sub")).unwrap();
        assert_eq!(git_branch(&d.join("sub")).as_deref(), Some("feat/x"));
    }

    #[test]
    fn glyphs_exist() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();
        let missing: String = ctx.fonts_mut(|f| {
            "⏶⏷⚙🗙⏺■"
                .chars()
                .filter(|&c| !f.has_glyph(&egui::FontId::proportional(14.0), c))
                .collect()
        });
        assert!(missing.is_empty(), "missing glyphs: {missing}");
    }
}
