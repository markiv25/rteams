//! Plugin detail dialog: metadata, capabilities, README, scope picker and install / uninstall /
//! enable / disable.

use eframe::egui::{self, RichText};
use rteams_services::ext::plugin_catalog::{self, PluginCatalogItem};
use rteams_services::ext::plugins::InstalledPlugin;
use rteams_services::i18n::I18n;

use super::store_tabs::{big_count, tx, Task};
use crate::theme;
use crate::ui::editor::md_preview;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginAction {
    None,
    Close,
    Install {
        id: String,
        scope: String,
    },
    Uninstall {
        id: String,
        scope: String,
    },
    Toggle {
        id: String,
        scope: String,
        enable: bool,
    },
}

pub struct PluginDetail {
    pub item: PluginCatalogItem,
    pub scope: String,
    readme: Option<Result<String, String>>,
    readme_task: Task<Result<String, String>>,
    readme_started: bool,
}

impl PluginDetail {
    pub fn new(item: PluginCatalogItem) -> Self {
        PluginDetail {
            item,
            scope: "user".into(),
            readme: None,
            readme_task: Task::default(),
            readme_started: false,
        }
    }

    /// `can_run`: the runtime is ready to install / uninstall. `installed`: this plugin's
    /// installed instances (one per scope). `busy`: an operation is running.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        tr: &I18n,
        offline: bool,
        can_run: bool,
        installed: &[InstalledPlugin],
        busy: bool,
        has_project: bool,
    ) -> PluginAction {
        if let Some(r) = self.readme_task.take() {
            self.readme = Some(r);
        }
        if !self.readme_started && !offline {
            self.readme_started = true;
            let item = self.item.clone();
            self.readme_task
                .start(ctx, move || plugin_catalog::fetch_readme(&item));
        }
        let mut action = PluginAction::None;
        let it = self.item.clone();
        let id = it.plugin_id.clone();
        egui::Window::new(it.display_name.clone())
            .collapsible(false)
            .default_width(680.0)
            .default_height(580.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height((ui.available_height() - 90.0).max(100.0))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.label(RichText::new(&it.description).color(theme::TEXT_2));
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            theme::pill(ui, it.category.clone(), theme::INFO);
                            for c in it.capabilities() {
                                theme::pill(ui, c, theme::ACCENT);
                            }
                            for i in installed {
                                theme::pill(
                                    ui,
                                    format!("installed ({}){}", i.scope, if i.enabled { "" } else { ", disabled" }),
                                    if i.enabled { theme::SUCCESS } else { theme::WARNING },
                                );
                            }
                        });
                        ui.add_space(6.0);
                        let unknown = tx(tr, "extensions.pluginDetail.unknown", "Unknown");
                        egui::Grid::new("plugin_meta").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                            let mut row = |k: String, v: String| {
                                ui.label(RichText::new(k).color(theme::MUTED));
                                ui.label(v);
                                ui.end_row();
                            };
                            row(
                                tx(tr, "extensions.pluginDetail.metadata.author", "Author"),
                                it.author.clone().unwrap_or_else(|| unknown.clone()),
                            );
                            row(tx(tr, "extensions.pluginDetail.metadata.category", "Category"), it.category.clone());
                            row(
                                tx(tr, "extensions.pluginDetail.metadata.source", "Source"),
                                if it.is_external { "external".into() } else { it.marketplace.clone() },
                            );
                            row(
                                tx(tr, "extensions.pluginDetail.metadata.version", "Version"),
                                it.version.clone().unwrap_or_else(|| unknown.clone()),
                            );
                            let caps = it.capabilities();
                            row(
                                tx(tr, "extensions.pluginDetail.metadata.capabilities", "Capabilities"),
                                if caps.is_empty() { "-".into() } else { caps.join(", ") },
                            );
                            if let Some(n) = it.installs {
                                row(tx(tr, "extensions.pluginDetail.metadata.installs", "Installs"), big_count(n));
                            }
                        });
                        if let Some(h) = &it.homepage {
                            ui.hyperlink_to(tx(tr, "extensions.pluginDetail.links.homepage", "Homepage"), h);
                        }
                        theme::rule(ui);
                        match (&self.readme, offline) {
                            (Some(Ok(text)), _) => md_preview::show(ui, text),
                            (Some(Err(_)), _) | (None, true) => {
                                ui.label(
                                    RichText::new(tx(tr, "extensions.pluginDetail.readme.empty", "No README available."))
                                        .color(theme::MUTED),
                                );
                            }
                            (None, false) => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(tx(tr, "extensions.pluginDetail.readme.loading", "Loading README..."));
                                });
                            }
                        }
                    });
                ui.add_space(6.0);
                if !can_run {
                    ui.label(
                        RichText::new(tx(
                            tr,
                            "extensions.store.runtime.requiredForMutations",
                            "The configured runtime is required to install or uninstall extensions. Install or repair it from the Dashboard.",
                        ))
                        .small()
                        .color(theme::WARNING),
                    );
                }
                ui.horizontal(|ui| {
                    if theme::ghost(ui, "Close").clicked() {
                        action = PluginAction::Close;
                    }
                    ui.label(tx(tr, "extensions.pluginDetail.scope.label", "Scope:"));
                    egui::ComboBox::from_id_salt("plugin_scope")
                        .selected_text(scope_text(tr, &self.scope))
                        .show_ui(ui, |ui| {
                            for s in ["user", "project", "local"] {
                                if s != "user" && !has_project {
                                    continue;
                                }
                                ui.selectable_value(&mut self.scope, s.to_string(), scope_text(tr, s));
                            }
                        });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let here = installed.iter().find(|i| i.scope == self.scope);
                        let go = can_run && !busy;
                        if busy {
                            ui.spinner();
                        }
                        match here {
                            Some(i) => {
                                if ui.add_enabled(go, egui::Button::new(RichText::new("Uninstall").strong()).fill(theme::ERROR)).clicked() {
                                    action = PluginAction::Uninstall { id: id.clone(), scope: self.scope.clone() };
                                }
                                let label = if i.enabled { "Disable" } else { "Enable" };
                                if ui.add_enabled(go, egui::Button::new(label)).clicked() {
                                    action = PluginAction::Toggle { id: id.clone(), scope: self.scope.clone(), enable: !i.enabled };
                                }
                            }
                            None => {
                                if ui.add_enabled(go, egui::Button::new(RichText::new("Install").strong()).fill(theme::ACCENT)).clicked() {
                                    action = PluginAction::Install { id: id.clone(), scope: self.scope.clone() };
                                }
                            }
                        }
                    });
                });
            });
        action
    }
}

fn scope_text(tr: &I18n, s: &str) -> String {
    match s {
        "project" => tx(
            tr,
            "extensions.pluginDetail.scope.options.project",
            "Project (shared)",
        ),
        "local" => tx(
            tr,
            "extensions.pluginDetail.scope.options.local",
            "Local (gitignored)",
        ),
        _ => tx(
            tr,
            "extensions.pluginDetail.scope.options.user",
            "User (global)",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::render;

    fn item() -> PluginCatalogItem {
        PluginCatalogItem {
            plugin_id: "demo@market".into(),
            name: "demo".into(),
            display_name: "Demo".into(),
            marketplace: "market".into(),
            description: "A demo plugin".into(),
            category: "development".into(),
            has_mcp: true,
            installs: Some(1234),
            ..Default::default()
        }
    }

    #[test]
    fn renders_in_each_state() {
        let tr = I18n::default();
        let inst = InstalledPlugin {
            id: "demo@market".into(),
            name: "demo".into(),
            marketplace: "market".into(),
            version: Some("1".into()),
            scope: "user".into(),
            enabled: false,
            install_path: None,
            project_path: None,
        };
        let mut d = PluginDetail::new(item());
        for (can_run, busy, installed) in [
            (true, false, vec![]),
            (false, false, vec![]),
            (true, true, vec![inst]),
        ] {
            render(|ui| {
                let a = d.show(ui.ctx(), &tr, true, can_run, &installed, busy, true);
                assert_eq!(a, PluginAction::None);
            });
        }
    }
}
