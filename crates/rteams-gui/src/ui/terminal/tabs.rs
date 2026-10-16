//! Terminal tabs: several sessions, `+`, inline rename, color, close.

use std::path::Path;

use eframe::egui::{self, Color32, RichText};
use rteams_services::i18n::I18n;
use rteams_services::terminal::{TabSpec, TermSize, TerminalSession};

use super::widget::WidgetState;
use crate::theme;

/// (id, i18n key suffix, color) in the original order.
pub const TAB_COLORS: [(&str, &str, Color32); 11] = [
    (
        "slate",
        "tabColorSlate",
        Color32::from_rgb(0x64, 0x74, 0x8b),
    ),
    ("sky", "tabColorSky", Color32::from_rgb(0x0e, 0xa5, 0xe9)),
    ("blue", "tabColorBlue", Color32::from_rgb(0x3b, 0x82, 0xf6)),
    ("cyan", "tabColorCyan", Color32::from_rgb(0x06, 0xb6, 0xd4)),
    ("teal", "tabColorTeal", Color32::from_rgb(0x14, 0xb8, 0xa6)),
    (
        "emerald",
        "tabColorEmerald",
        Color32::from_rgb(0x10, 0xb9, 0x81),
    ),
    ("lime", "tabColorLime", Color32::from_rgb(0x84, 0xcc, 0x16)),
    (
        "amber",
        "tabColorAmber",
        Color32::from_rgb(0xf5, 0x9e, 0x0b),
    ),
    (
        "orange",
        "tabColorOrange",
        Color32::from_rgb(0xf9, 0x73, 0x16),
    ),
    ("rose", "tabColorRose", Color32::from_rgb(0xf4, 0x3f, 0x5e)),
    (
        "violet",
        "tabColorViolet",
        Color32::from_rgb(0x8b, 0x5c, 0xf6),
    ),
];

pub fn color_of(name: &str) -> Color32 {
    TAB_COLORS
        .iter()
        .find(|c| c.0 == name)
        .map(|c| c.2)
        .unwrap_or(TAB_COLORS[0].2)
}

pub struct Tab {
    pub spec: TabSpec,
    pub session: Option<TerminalSession>,
    pub error: Option<String>,
    pub widget: WidgetState,
    rename: Option<String>,
}

impl Tab {
    fn new(spec: TabSpec) -> Self {
        Tab {
            spec,
            session: None,
            error: None,
            widget: WidgetState::default(),
            rename: None,
        }
    }

    /// Spawn the shell if it is not running yet.
    pub fn ensure_session(&mut self, ctx: &egui::Context, shell: Option<&str>, default_cwd: &Path) {
        if self.session.is_some() || self.error.is_some() {
            return;
        }
        let cwd = self
            .spec
            .cwd
            .clone()
            .unwrap_or_else(|| default_cwd.to_path_buf());
        match TerminalSession::spawn(&cwd, shell.map(str::to_string), TermSize::default()) {
            Ok(s) => {
                let ctx = ctx.clone();
                s.set_repaint(move || ctx.request_repaint());
                self.session = Some(s);
            }
            Err(e) => self.error = Some(e),
        }
    }

    pub fn restart(&mut self) {
        self.session = None;
        self.error = None;
        self.widget = WidgetState::default();
    }

    pub fn display_title(&self, n: usize) -> String {
        if !self.spec.title.is_empty() {
            return self.spec.title.clone();
        }
        if let Some(s) = &self.session {
            let t = s.title();
            if !t.trim().is_empty() {
                return t;
            }
        }
        format!("Terminal {}", n + 1)
    }

    /// Output worth confirming before it is thrown away.
    pub fn has_history(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| s.history_len() > 0 || s.screen().has_content())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabAction {
    Select(usize),
    New,
    Close(usize),
    ToggleSettings,
    /// Title or color changed (persist).
    Changed,
}

#[derive(Default)]
pub struct Tabs {
    pub tabs: Vec<Tab>,
    pub active: usize,
}

impl Tabs {
    pub fn from_specs(specs: &[TabSpec]) -> Self {
        Tabs {
            tabs: specs.iter().cloned().map(Tab::new).collect(),
            active: 0,
        }
    }

    pub fn specs(&self) -> Vec<TabSpec> {
        self.tabs.iter().map(|t| t.spec.clone()).collect()
    }

    pub fn add(&mut self, spec: TabSpec) -> usize {
        self.tabs.push(Tab::new(spec));
        self.active = self.tabs.len() - 1;
        self.active
    }

    pub fn remove(&mut self, i: usize) {
        if i < self.tabs.len() && self.tabs.len() > 1 {
            self.tabs.remove(i);
            if self.active >= self.tabs.len() || self.active > i {
                self.active = self.active.saturating_sub(1).min(self.tabs.len() - 1);
            }
        }
    }

    pub fn active_tab(&mut self) -> Option<&mut Tab> {
        let i = self.active;
        self.tabs.get_mut(i)
    }

    /// The tab strip. Returns what the user asked for; renames/colors are applied in place.
    pub fn bar(&mut self, ui: &mut egui::Ui, tr: &I18n, settings_active: bool) -> Vec<TabAction> {
        let k = |s: &str| tr.t(&format!("team.terminalWorkspace.{s}"));
        let mut actions = Vec::new();
        let n_tabs = self.tabs.len();
        let active = self.active;
        egui::ScrollArea::horizontal()
            .id_salt("terminal_tabs")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for i in 0..n_tabs {
                        let title = self.tabs[i].display_title(i);
                        let color = color_of(&self.tabs[i].spec.color);
                        let selected = !settings_active && i == active;
                        if let Some(buf) = self.tabs[i].rename.as_mut() {
                            let r = ui.add(egui::TextEdit::singleline(buf).desired_width(110.0));
                            r.request_focus();
                            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                            let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                            if enter || r.lost_focus() && !esc {
                                let v = buf.trim().to_string();
                                self.tabs[i].spec.title = v;
                                self.tabs[i].rename = None;
                                actions.push(TabAction::Changed);
                            } else if esc {
                                self.tabs[i].rename = None;
                            }
                            continue;
                        }
                        ui.label(RichText::new("⏺").color(color).small());
                        let resp = ui.selectable_label(selected, &title);
                        if resp.clicked() {
                            actions.push(TabAction::Select(i));
                        }
                        if resp.double_clicked() {
                            self.tabs[i].rename = Some(self.tabs[i].spec.title.clone());
                        }
                        resp.context_menu(|ui| {
                            if ui.button(k("renameTab")).clicked() {
                                self.tabs[i].rename = Some(title.clone());
                                ui.close();
                            }
                            ui.label(RichText::new(k("tabColor")).small().color(theme::MUTED));
                            for (id, label, c) in TAB_COLORS {
                                let text = RichText::new(format!("⏺ {}", k(label))).color(c);
                                if ui.button(text).clicked() {
                                    self.tabs[i].spec.color = id.to_string();
                                    actions.push(TabAction::Changed);
                                    ui.close();
                                }
                            }
                        });
                        let close = ui.add_enabled(
                            n_tabs > 1,
                            egui::Button::new(RichText::new("🗙").small().color(theme::MUTED))
                                .frame(false),
                        );
                        let close = if n_tabs > 1 {
                            close.on_hover_text(tr.t_args(
                                "team.terminalWorkspace.closeTerminalTab",
                                &[("tab", &title)],
                            ))
                        } else {
                            close.on_disabled_hover_text(k("createAnotherTabBeforeClosing"))
                        };
                        if close.clicked() {
                            actions.push(TabAction::Close(i));
                        }
                        ui.add_space(6.0);
                    }
                    if ui
                        .button("+")
                        .on_hover_text(k("createTerminalTab"))
                        .clicked()
                    {
                        actions.push(TabAction::New);
                    }
                    ui.separator();
                    if ui
                        .selectable_label(settings_active, k("settingsTab"))
                        .clicked()
                    {
                        actions.push(TabAction::ToggleSettings);
                    }
                });
            });
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_remove_active() {
        let mut t = Tabs::default();
        t.add(TabSpec::default());
        t.add(TabSpec::default());
        t.add(TabSpec::default());
        assert_eq!(t.active, 2);
        t.remove(2);
        assert_eq!(t.active, 1);
        t.remove(0);
        assert_eq!(t.tabs.len(), 1);
        t.remove(0);
        assert_eq!(t.tabs.len(), 1, "last tab cannot be closed");
        assert_eq!(t.specs().len(), 1);
    }

    #[test]
    fn bar_renders_headless() {
        let tr = I18n::default();
        let mut t = Tabs::default();
        t.add(TabSpec::default());
        t.add(TabSpec {
            title: "x".into(),
            ..Default::default()
        });
        crate::view::testing::render(|ui| {
            t.bar(ui, &tr, false);
        });
        assert_eq!(color_of("nope"), TAB_COLORS[0].2);
    }
}
