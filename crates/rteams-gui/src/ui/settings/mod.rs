//! Settings screen: General, Notifications, Advanced, CLI Runtime, Connection (SSH), Workspace
//! profiles. Every change is saved to `<home>/config.json` with a short debounce
//! (`AppConfig::save`); the shell polls `config_changed` / `take_theme_change` to react.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use rteams_services::config::AppConfig;
use rteams_services::i18n::I18n;
use serde_json::{Map, Value};

use super::dashboard::kit;
use super::{FeatureCtx, Screen, TabId};
use crate::icons::{self, Icon};
use crate::theme;

pub mod advanced;
pub mod cli_runtime;
pub mod config_editor;
pub mod connection;
pub mod general;
pub mod local_claude_root;
pub mod notifications;
pub mod permissions;
pub mod workspace_profiles;

const DEBOUNCE: Duration = Duration::from_millis(350);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    General,
    Notifications,
    Advanced,
    Permissions,
    CliRuntime,
    Providers,
    Connection,
    Workspace,
}

impl Section {
    const ALL: [Section; 8] = [
        Section::General,
        Section::Notifications,
        Section::Advanced,
        Section::Permissions,
        Section::CliRuntime,
        Section::Providers,
        Section::Connection,
        Section::Workspace,
    ];

    fn icon(self) -> Icon {
        match self {
            Section::General => Icon::Settings,
            Section::Notifications => Icon::Bell,
            Section::Advanced | Section::Permissions => Icon::Shield,
            Section::CliRuntime => Icon::Terminal,
            Section::Providers => Icon::Cpu,
            Section::Connection => Icon::Globe,
            Section::Workspace => Icon::Folder,
        }
    }

    fn label_key(self) -> &'static str {
        match self {
            Section::General => "settings.tabs.general.label",
            Section::Notifications => "settings.tabs.notifications.label",
            Section::Advanced => "settings.tabs.advanced.label",
            Section::Permissions => "rteams.settings.permissions.nav",
            Section::CliRuntime => "settings.cliRuntime.title",
            Section::Providers => "settings.runtimeProvider.tabs.providers",
            Section::Connection => "settings.connection.title",
            Section::Workspace => "settings.workspaceProfiles.title",
        }
    }
}

pub struct SettingsScreen {
    cfg: Option<AppConfig>,
    home: Option<PathBuf>,
    pub section: Section,
    dirty_at: Option<Instant>,
    theme_change: Option<String>,
    changed: bool,
    locale_tr: Option<I18n>,
    save_error: Option<String>,
    general: general::State,
    notifications: notifications::State,
    advanced: advanced::State,
    permissions: permissions::State,
    cli: cli_runtime::State,
    connection: connection::State,
    workspace: workspace_profiles::State,
    providers: crate::providers::Providers,
    /// Overrides `dirs::home_dir()` (tests).
    user_home: Option<PathBuf>,
}

impl Default for SettingsScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsScreen {
    pub fn new() -> Self {
        SettingsScreen {
            cfg: None,
            home: None,
            section: Section::General,
            dirty_at: None,
            theme_change: None,
            changed: false,
            locale_tr: None,
            save_error: None,
            general: Default::default(),
            notifications: Default::default(),
            advanced: Default::default(),
            permissions: Default::default(),
            cli: cli_runtime::State::new(),
            connection: Default::default(),
            workspace: Default::default(),
            providers: Default::default(),
            user_home: None,
        }
    }

    /// New theme (`dark` | `light` | `system`) if the user changed it since the last call.
    #[cfg_attr(not(test), allow(dead_code))] // called by the shell once a light theme exists
    pub fn take_theme_change(&mut self) -> Option<String> {
        self.theme_change.take()
    }

    /// True once after a change was saved, so the shell reloads `AppConfig`.
    pub fn config_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// Write any pending change now.
    pub fn flush(&mut self) {
        if let (Some(cfg), Some(home)) = (&mut self.cfg, &self.home) {
            if self.dirty_at.take().is_some() {
                // Other screens (Providers) write keys to `extra` meanwhile: keep the disk copy of
                // keys this cached config lacks, and of provider-owned keys, instead of clobbering.
                for (k, v) in AppConfig::load(home).extra {
                    if k.starts_with("providers.") || !cfg.extra.contains_key(&k) {
                        cfg.extra.insert(k, v);
                    }
                }
                match cfg.save(home) {
                    Ok(()) => {
                        self.save_error = None;
                        self.changed = true;
                    }
                    Err(e) => self.save_error = Some(e.to_string()),
                }
            }
        }
    }

    fn ensure_loaded(&mut self, home: &Path) {
        if self.home.as_deref() != Some(home) {
            self.flush();
            self.home = Some(home.to_path_buf());
            let cfg = AppConfig::load(home);
            self.locale_tr = Some(I18n::from_pref(&cfg.general.locale));
            self.cfg = Some(cfg);
            self.connection = Default::default();
            self.permissions = Default::default();
        }
    }

    fn user_home(&self) -> PathBuf {
        self.user_home
            .clone()
            .or_else(rteams_services::claude_root::user_home)
            .unwrap_or_default()
    }

    fn mark_dirty(&mut self) {
        if self.dirty_at.is_none() {
            self.dirty_at = Some(Instant::now());
        }
    }
}

impl Screen for SettingsScreen {
    fn id(&self) -> TabId {
        TabId::Settings
    }

    fn title_key(&self) -> &'static str {
        "settings.view.title"
    }

    fn tick(&mut self, ctx: &egui::Context, c: &mut FeatureCtx) {
        self.ensure_loaded(c.home);
        if let Some(t) = self.dirty_at {
            if t.elapsed() >= DEBOUNCE {
                self.flush();
            } else {
                ctx.request_repaint_after(DEBOUNCE);
            }
        }
    }

    fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        self.ensure_loaded(c.home);
        let tr = c.tr;
        let top = ui.available_height();
        ui.add_space(4.0);
        ui.label(
            RichText::new(tr.t("settings.view.title"))
                .size(22.0)
                .strong(),
        );
        hint(ui, &tr.t("settings.view.description"));
        ui.add_space(14.0);
        let user_home = self.user_home();
        let home = c.home.to_path_buf();
        let members: Vec<Option<String>> = c.view.map_or_else(Vec::new, |v| {
            v.agent_members()
                .iter()
                .map(|m| crate::providers::member_runtime(m).map(str::to_string))
                .collect()
        });
        let before_theme = self.cfg.as_ref().map(|c| c.general.theme.clone());
        let mut changed = false;
        let body_h = (ui.available_height() - 8.0).max(120.0);
        let _ = top;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 24.0;
            // Left nav list.
            ui.allocate_ui_with_layout(
                egui::vec2(210.0, body_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for s in Section::ALL {
                        if nav_item(
                            ui,
                            s.icon(),
                            &crate::ui::onboarding::tx(
                                tr,
                                s.label_key(),
                                if s == Section::Permissions {
                                    "Permissions"
                                } else {
                                    ""
                                },
                            ),
                            self.section == s,
                        )
                        .clicked()
                        {
                            self.section = s;
                        }
                    }
                },
            );
            let Some(cfg) = self.cfg.as_mut() else { return };
            let locale_tr = self.locale_tr.get_or_insert_with(I18n::english);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), body_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("settings_body")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_max_width(820.0);
                            kit::checkbox_visuals(ui);
                            match self.section {
                                Section::General => {
                                    changed |= general::show(
                                        ui,
                                        cfg,
                                        &mut self.general,
                                        tr,
                                        locale_tr,
                                        &user_home,
                                    );
                                }
                                Section::Notifications => {
                                    changed |=
                                        notifications::show(ui, cfg, &mut self.notifications, tr);
                                }
                                Section::Advanced => {
                                    changed |=
                                        advanced::show(ui, cfg, &mut self.advanced, tr, &home);
                                }
                                Section::Permissions => {
                                    changed |=
                                        permissions::show(ui, cfg, &mut self.permissions, tr);
                                }
                                Section::CliRuntime => {
                                    changed |= cli_runtime::show(ui, cfg, &mut self.cli, tr);
                                }
                                Section::Providers => {
                                    self.providers.show_section(ui, tr, &members);
                                }
                                Section::Connection => {
                                    changed |=
                                        connection::show(ui, cfg, &mut self.connection, tr, &home);
                                }
                                Section::Workspace => {
                                    changed |=
                                        workspace_profiles::show(ui, cfg, &mut self.workspace, tr);
                                }
                            }
                            ui.add_space(24.0);
                        });
                },
            );
        });
        if let (Some(b), Some(cfg)) = (before_theme, &self.cfg) {
            if b != cfg.general.theme {
                self.theme_change = Some(cfg.general.theme.clone());
            }
        }
        if changed {
            self.mark_dirty();
        }
        if let Some(e) = &self.save_error {
            ui.colored_label(theme::ERROR, e);
        }
        if self.dirty_at.is_some() {
            ui.ctx().request_repaint_after(DEBOUNCE);
        }
    }
}

/// Left-nav row: 36px, icon + label, accent-tinted when selected.
fn nav_item(ui: &mut egui::Ui, icon: Icon, label: &str, selected: bool) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 36.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let (fill, fg) = if selected {
            (theme::tint(theme::ACCENT, 0.14), theme::ACCENT)
        } else if resp.hovered() {
            (egui::Color32::from_rgb(0x20, 0x22, 0x2d), theme::TEXT)
        } else {
            (egui::Color32::TRANSPARENT, theme::TEXT_2)
        };
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(10), fill);
        icons::draw(
            ui.painter(),
            egui::Rect::from_center_size(
                egui::pos2(rect.left() + 22.0, rect.center().y),
                egui::vec2(16.0, 16.0),
            ),
            icon,
            fg,
        );
        ui.painter().text(
            egui::pos2(rect.left() + 40.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(14.0),
            fg,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ---- shared helpers for the section modules ------------------------------------------------

pub(crate) fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(13.0).color(theme::MUTED));
}

/// A titled soft card: rounded, 18px padding, hairline border, generous spacing.
pub(crate) fn card(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::RAISED)
        .stroke(egui::Stroke::new(1.0, theme::hairline(0.07)))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 10.0;
            ui.label(RichText::new(title).size(15.0).strong());
            body(ui);
        });
    ui.add_space(14.0);
}

/// Switch row: label + description on the left, the switch on the right; true when flipped.
pub(crate) fn toggle(ui: &mut egui::Ui, value: &mut bool, label: &str, desc: &str) -> bool {
    let full = ui.available_width();
    let mut flipped = false;
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2((full - 56.0).max(40.0), 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(RichText::new(label).size(14.0));
                if !desc.is_empty() {
                    hint(ui, desc);
                }
            },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            flipped = theme::switch(ui, value).changed();
        });
    });
    flipped
}

pub(crate) fn extra_bool(extra: &Map<String, Value>, key: &str, default: bool) -> bool {
    extra.get(key).and_then(Value::as_bool).unwrap_or(default)
}

pub(crate) fn extra_set(extra: &mut Map<String, Value>, key: &str, v: Value) {
    extra.insert(key.to_string(), v);
}

pub(crate) fn extra_str<'a>(extra: &'a Map<String, Value>, key: &str) -> &'a str {
    extra.get(key).and_then(Value::as_str).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::render;

    fn temp_home(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rteams-settings-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn draw(screen: &mut SettingsScreen, home: &Path) {
        let tr = I18n::english();
        let mut out = crate::view::Intents::default();
        let mut req = Vec::new();
        render(|ui| {
            let mut c = FeatureCtx {
                home,
                team: None,
                view: None,
                out: &mut out,
                tr: &tr,
                req: &mut req,
            };
            screen.show(ui, &mut c);
        });
    }

    #[test]
    fn every_section_renders_at_three_sizes() {
        let home = temp_home("render");
        let mut s = SettingsScreen::new();
        s.user_home = Some(home.join("uh"));
        s.cli = cli_runtime::State::with_rows(vec![]);
        s.connection
            .set_profiles(vec![rteams_services::ssh::SshProfile {
                id: "1".into(),
                name: "box".into(),
                host: "box".into(),
                ..Default::default()
            }]);
        s.ensure_loaded(&home);
        s.cfg
            .as_mut()
            .unwrap()
            .extra
            .insert(connection::ENABLED.into(), serde_json::json!(true));
        for sec in Section::ALL {
            s.section = sec;
            draw(&mut s, &home);
        }
        s.advanced.editor.open(&AppConfig::default());
        s.section = Section::Advanced;
        draw(&mut s, &home);
        assert!(
            !home.join("config.json").exists(),
            "rendering alone must not write"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn change_is_debounced_then_saved_and_reported() {
        let home = temp_home("save");
        let mut s = SettingsScreen::new();
        s.ensure_loaded(&home);
        s.cfg.as_mut().unwrap().general.theme = "dark".into();
        s.mark_dirty();
        assert!(!s.config_changed());
        s.flush();
        assert!(s.config_changed());
        assert!(!s.config_changed());
        assert_eq!(AppConfig::load(&home).general.theme, "dark");
        // A key another screen wrote meanwhile survives the next Settings save.
        let mut other = AppConfig::load(&home);
        other.extra.insert(
            "providers.model_limits.x.y".into(),
            serde_json::json!({ "context": 5 }),
        );
        other.save(&home).unwrap();
        if let Some(c) = s.cfg.as_mut() {
            c.general.theme = "light".into();
        }
        s.mark_dirty();
        s.flush();
        let disk = AppConfig::load(&home);
        assert_eq!(disk.general.theme, "light");
        assert!(disk.extra.contains_key("providers.model_limits.x.y"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn theme_change_flag_is_taken_once() {
        let mut s = SettingsScreen::new();
        s.theme_change = Some("light".into());
        assert_eq!(s.take_theme_change().as_deref(), Some("light"));
        assert_eq!(s.take_theme_change(), None);
    }
}
