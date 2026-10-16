//! Plugins tab: search, "Browse by fit" filters (categories, capabilities, installed only), sort,
//! the catalog as cards and the detail dialog. Installed state and every change go through the
//! `claude plugin` CLI on worker threads.

use eframe::egui::{self, RichText};
use rteams_services::ext::cli::RuntimeState;
use rteams_services::ext::plugin_catalog::{
    self, PluginCatalog, PluginCatalogItem, PluginFilter, PluginSort, CAPABILITIES,
};
use rteams_services::ext::plugins::{InstalledPlugin, PluginCli};

use super::plugin_detail::{PluginAction, PluginDetail};
use super::store_tabs::{big_count, card, notice_line, tx, txa, ExtCtx, Task};
use crate::theme;

const PAGE: usize = 30;

type OpResult = (String, Result<String, String>);

pub struct PluginsPanel {
    catalog: Option<PluginCatalog>,
    catalog_task: Task<PluginCatalog>,
    force: bool,
    pub installed: Vec<InstalledPlugin>,
    installed_task: Task<Result<Vec<InstalledPlugin>, String>>,
    installed_loaded: bool,
    installed_error: Option<String>,
    pub filter: PluginFilter,
    pub sort: PluginSort,
    pub shown: usize,
    filters_open: bool,
    detail: Option<PluginDetail>,
    op_task: Task<OpResult>,
    notice: Option<(bool, String)>,
}

impl Default for PluginsPanel {
    fn default() -> Self {
        PluginsPanel {
            catalog: None,
            catalog_task: Task::default(),
            force: false,
            installed: Vec::new(),
            installed_task: Task::default(),
            installed_loaded: false,
            installed_error: None,
            filter: PluginFilter::default(),
            sort: PluginSort::Popular,
            shown: PAGE,
            filters_open: true,
            detail: None,
            op_task: Task::default(),
            notice: None,
        }
    }
}

fn sort_label(tr: &rteams_services::i18n::I18n, s: PluginSort) -> String {
    match s {
        PluginSort::Popular => tx(tr, "extensions.pluginsPanel.sort.popular", "Popular"),
        PluginSort::NameAsc => tx(tr, "extensions.pluginsPanel.sort.nameAsc", "Name A-Z"),
        PluginSort::NameDesc => tx(tr, "extensions.pluginsPanel.sort.nameDesc", "Name Z-A"),
        PluginSort::Category => tx(tr, "extensions.pluginsPanel.sort.category", "Category"),
    }
}

impl PluginsPanel {
    pub fn invalidate_catalog(&mut self) {
        self.catalog = None;
        self.force = true;
        self.installed_loaded = false;
    }

    fn is_installed(&self, id: &str) -> bool {
        self.installed.iter().any(|i| i.id == id)
    }

    fn matching(&self) -> Vec<&PluginCatalogItem> {
        let Some(c) = &self.catalog else {
            return Vec::new();
        };
        plugin_catalog::apply(&c.items, &self.filter, self.sort, &|id| {
            self.is_installed(id)
        })
    }

    fn run_op(
        &mut self,
        ctx: &egui::Context,
        x: &ExtCtx,
        label: String,
        f: impl FnOnce(&PluginCli) -> Result<String, String> + Send + 'static,
    ) {
        let cwd = x.paths.project.clone();
        self.op_task.start(ctx, move || {
            let r = match PluginCli::detect(cwd.as_deref()) {
                Some(cli) => f(&cli),
                None => Err("claude CLI not found".into()),
            };
            (label, r)
        });
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        x: &mut ExtCtx,
        ready: bool,
        runtime: &Option<RuntimeState>,
    ) {
        let tr = x.tr;
        if let Some(c) = self.catalog_task.take() {
            self.catalog = Some(c);
        }
        if let Some(r) = self.installed_task.take() {
            match r {
                Ok(l) => {
                    self.installed = l;
                    self.installed_error = None;
                }
                Err(e) => self.installed_error = Some(e),
            }
        }
        if let Some((label, r)) = self.op_task.take() {
            match r {
                Ok(_) => {
                    x.toast(label.clone());
                    self.notice = Some((true, label));
                }
                Err(e) => self.notice = Some((false, format!("{label} failed: {e}"))),
            }
            self.installed_loaded = false;
        }
        if self.catalog.is_none() && !self.catalog_task.busy() {
            if x.offline {
                self.catalog = Some(PluginCatalog::default());
            } else {
                let (home, force) = (x.home.to_path_buf(), self.force);
                self.force = false;
                self.catalog_task
                    .start(ui.ctx(), move || plugin_catalog::load(&home, force));
            }
        }
        let can_run = ready && !x.offline && matches!(runtime, Some(RuntimeState::Ready { .. }));
        if !self.installed_loaded && !self.installed_task.busy() {
            if can_run {
                let cwd = x.paths.project.clone();
                self.installed_task.start(ui.ctx(), move || {
                    PluginCli::detect(cwd.as_deref())
                        .ok_or_else(|| "claude CLI not found".to_string())?
                        .list_installed()
                });
            }
            self.installed_loaded = true;
        }

        // -- toolbar
        ui.horizontal_wrapped(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.filter.query)
                    .hint_text(tx(
                        tr,
                        "extensions.pluginsPanel.searchPlaceholder",
                        "Search plugins...",
                    ))
                    .desired_width(300.0),
            );
            if r.changed() {
                self.shown = PAGE;
            }
            egui::ComboBox::from_id_salt("plugins_sort")
                .selected_text(sort_label(tr, self.sort))
                .show_ui(ui, |ui| {
                    for s in [
                        PluginSort::Popular,
                        PluginSort::NameAsc,
                        PluginSort::NameDesc,
                        PluginSort::Category,
                    ] {
                        ui.selectable_value(&mut self.sort, s, sort_label(tr, s));
                    }
                });
            let n = self.filter.active_count();
            let label = if n > 0 {
                format!(
                    "{} ({})",
                    tx(tr, "extensions.pluginsPanel.browseByFit", "Browse by fit"),
                    txa(
                        tr,
                        "extensions.pluginsPanel.activeFilters",
                        "{{count}} active",
                        &[("count", &n.to_string())]
                    )
                )
            } else {
                tx(tr, "extensions.pluginsPanel.browseByFit", "Browse by fit")
            };
            ui.toggle_value(&mut self.filters_open, label);
            if n > 0
                && theme::ghost(
                    ui,
                    &tx(
                        tr,
                        "extensions.pluginsPanel.clearAllFilters",
                        "Clear all filters",
                    ),
                )
                .clicked()
            {
                self.filter = PluginFilter {
                    query: self.filter.query.clone(),
                    ..Default::default()
                };
            }
        });
        if !can_run && !x.offline {
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
        ui.label(
            RichText::new(tx(
                tr,
                "extensions.pluginsPanel.providerSupportNotice",
                "Plugin support is currently guaranteed for Anthropic (Claude) sessions only.",
            ))
            .small()
            .color(theme::MUTED),
        );
        notice_line(ui, &self.notice);
        if let Some(e) = &self.installed_error {
            ui.colored_label(
                theme::WARNING,
                format!("Could not read installed plugins: {e}"),
            );
        }
        if let Some(e) = self.catalog.as_ref().and_then(|c| c.error.as_ref()) {
            ui.colored_label(
                theme::WARNING,
                format!("Catalog unavailable, showing cached data: {e}"),
            );
        }
        if self.filters_open {
            self.filter_panel(ui, x);
        }

        // -- results
        let items: Vec<PluginCatalogItem> = self.matching().into_iter().cloned().collect();
        let total_all = self.catalog.as_ref().map_or(0, |c| c.items.len());
        if self.catalog_task.busy() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading catalog...").color(theme::MUTED));
            });
        }
        let shown = items.len().min(self.shown);
        ui.label(
            RichText::new(txa(
                tr,
                "extensions.pluginsPanel.showing",
                "Showing {{shown}} of {{total}} plugins",
                &[
                    ("shown", &shown.to_string()),
                    ("total", &total_all.to_string()),
                ],
            ))
            .small()
            .color(theme::MUTED),
        );
        let mut open: Option<PluginCatalogItem> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if items.is_empty() && !self.catalog_task.busy() {
                    let filtered =
                        self.filter.active_count() > 0 || !self.filter.query.trim().is_empty();
                    ui.vertical_centered(|ui| {
                        ui.add_space(20.0);
                        ui.label(
                            RichText::new(if filtered {
                                tx(
                                    tr,
                                    "extensions.pluginsPanel.empty.filteredTitle",
                                    "No plugins match your filters",
                                )
                            } else {
                                tx(
                                    tr,
                                    "extensions.pluginsPanel.empty.title",
                                    "No plugins available",
                                )
                            })
                            .size(16.0)
                            .strong(),
                        );
                        ui.label(
                            RichText::new(if filtered {
                                tx(
                                    tr,
                                    "extensions.pluginsPanel.empty.filteredDescription",
                                    "Try adjusting your search or filter criteria",
                                )
                            } else {
                                tx(
                                    tr,
                                    "extensions.pluginsPanel.empty.description",
                                    "Check back later for new plugins",
                                )
                            })
                            .color(theme::MUTED),
                        );
                    });
                    return;
                }
                let spacing = 8.0;
                let avail = ui.available_width();
                let cols = ((avail + spacing) / (300.0 + spacing)).floor().max(1.0) as usize;
                let w = ((avail - spacing * (cols as f32 - 1.0)) / cols as f32).max(120.0);
                egui::Grid::new("plugins_grid")
                    .num_columns(cols)
                    .spacing([spacing, spacing])
                    .show(ui, |ui| {
                        for (i, it) in items.iter().take(self.shown).enumerate() {
                            ui.vertical(|ui| {
                                ui.set_width(w);
                                card(ui, |ui| {
                                    ui.set_width(w - 26.0);
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(
                                            RichText::new(&it.display_name).strong().size(14.5),
                                        );
                                        if self.is_installed(&it.plugin_id) {
                                            theme::pill(ui, "Installed", theme::SUCCESS);
                                        }
                                    });
                                    ui.horizontal_wrapped(|ui| {
                                        theme::pill(ui, it.category.clone(), theme::INFO);
                                        for c in it.capabilities() {
                                            theme::pill(ui, c, theme::MUTED);
                                        }
                                        if let Some(n) = it.installs {
                                            ui.label(
                                                RichText::new(format!("{} installs", big_count(n)))
                                                    .small()
                                                    .color(theme::MUTED),
                                            );
                                        }
                                    });
                                    ui.label(
                                        RichText::new(crate::runtimes::clip(&it.description, 140))
                                            .small()
                                            .color(theme::TEXT_2),
                                    );
                                    if ui.small_button("Details").clicked() {
                                        open = Some(it.clone());
                                    }
                                });
                            });
                            if (i + 1) % cols == 0 {
                                ui.end_row();
                            }
                        }
                    });
                if items.len() > self.shown {
                    ui.add_space(6.0);
                    if ui
                        .button(tx(tr, "extensions.mcpPanel.loadMore", "Load more"))
                        .clicked()
                    {
                        self.shown += PAGE;
                    }
                }
            });
        if let Some(it) = open {
            self.detail = Some(PluginDetail::new(it));
        }
        self.dialog(ui.ctx(), x, can_run);
    }

    fn filter_panel(&mut self, ui: &mut egui::Ui, x: &ExtCtx) {
        let tr = x.tr;
        let cats = self
            .catalog
            .as_ref()
            .map(|c| plugin_catalog::category_counts(&c.items))
            .unwrap_or_default();
        egui::Frame::new()
            .fill(theme::CARD)
            .stroke(egui::Stroke::new(1.0, theme::hairline(0.07)))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(tx(
                        tr,
                        "extensions.pluginsPanel.filterDescription",
                        "Narrow the catalog by category, capability, or installed state.",
                    ))
                    .small()
                    .color(theme::MUTED),
                );
                ui.label(
                    RichText::new(tx(tr, "extensions.pluginsPanel.categories", "Categories"))
                        .strong(),
                );
                ui.horizontal_wrapped(|ui| {
                    for (c, n) in &cats {
                        let mut on = self.filter.categories.contains(c);
                        if ui.toggle_value(&mut on, format!("{c} ({n})")).changed() {
                            toggle(&mut self.filter.categories, c, on);
                            self.shown = PAGE;
                        }
                    }
                });
                ui.label(
                    RichText::new(tx(
                        tr,
                        "extensions.pluginsPanel.capabilities",
                        "Capabilities",
                    ))
                    .strong(),
                );
                ui.horizontal_wrapped(|ui| {
                    for c in CAPABILITIES {
                        let mut on = self.filter.capabilities.iter().any(|x| x == c);
                        if ui.toggle_value(&mut on, c).changed() {
                            toggle(&mut self.filter.capabilities, c, on);
                            self.shown = PAGE;
                        }
                    }
                    if ui
                        .toggle_value(
                            &mut self.filter.installed_only,
                            tx(
                                tr,
                                "extensions.pluginsPanel.installedOnly",
                                "Installed only",
                            ),
                        )
                        .changed()
                    {
                        self.shown = PAGE;
                    }
                });
            });
        ui.add_space(4.0);
    }

    fn dialog(&mut self, ctx: &egui::Context, x: &mut ExtCtx, can_run: bool) {
        let Some(mut d) = self.detail.take() else {
            return;
        };
        let installed: Vec<InstalledPlugin> = self
            .installed
            .iter()
            .filter(|i| i.id == d.item.plugin_id)
            .cloned()
            .collect();
        let act = d.show(
            ctx,
            x.tr,
            x.offline,
            can_run,
            &installed,
            self.op_task.busy(),
            x.paths.project.is_some(),
        );
        match act {
            PluginAction::None => self.detail = Some(d),
            PluginAction::Close => {}
            PluginAction::Install { id, scope } => {
                self.run_op(ctx, x, format!("Installed {id}"), move |c| {
                    c.install(&id, &scope)
                });
                self.detail = Some(d);
            }
            PluginAction::Uninstall { id, scope } => {
                self.run_op(ctx, x, format!("Uninstalled {id}"), move |c| {
                    c.uninstall(&id, &scope)
                });
                self.detail = Some(d);
            }
            PluginAction::Toggle { id, scope, enable } => {
                let label = format!("{} {id}", if enable { "Enabled" } else { "Disabled" });
                self.run_op(ctx, x, label, move |c| c.set_enabled(&id, &scope, enable));
                self.detail = Some(d);
            }
        }
    }
}

fn toggle(v: &mut Vec<String>, item: &str, on: bool) {
    v.retain(|x| x != item);
    if on {
        v.push(item.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::ShellRequest;
    use crate::view::testing::render;
    use rteams_services::ext::mcp_config::ClientPaths;
    use rteams_services::i18n::I18n;

    fn items() -> Vec<PluginCatalogItem> {
        let m = serde_json::json!({"name":"m","plugins":[
            {"name":"alpha","description":"first","category":"development","source":"./a","lspServers":{"x":{}}},
            {"name":"beta","description":"second","category":"database","source":"./b"}]});
        plugin_catalog::parse_marketplace(&m)
    }

    fn frames(p: &mut PluginsPanel, ready: bool) {
        let tr = I18n::default();
        let paths = ClientPaths::new("/nonexistent-home", None);
        let mut req: Vec<ShellRequest> = Vec::new();
        let rt = if ready {
            None
        } else {
            Some(RuntimeState::Missing)
        };
        render(|ui| {
            let mut x = ExtCtx {
                home: std::path::Path::new("/tmp"),
                paths: &paths,
                offline: true,
                tr: &tr,
                req: &mut req,
            };
            p.show(ui, &mut x, ready, &rt);
        });
    }

    #[test]
    fn filters_and_renders() {
        let mut p = PluginsPanel::default();
        frames(&mut p, true); // offline: empty catalog
        assert!(p.matching().is_empty());
        p.catalog = Some(PluginCatalog {
            items: items(),
            ..Default::default()
        });
        assert_eq!(p.matching().len(), 2);
        frames(&mut p, false);
        p.filter.capabilities = vec!["lsp".into()];
        assert_eq!(p.matching()[0].name, "alpha");
        frames(&mut p, true);
        p.filter.capabilities.clear();
        p.filter.installed_only = true;
        assert!(p.matching().is_empty());
        frames(&mut p, true);
        p.installed.push(InstalledPlugin {
            id: "beta@m".into(),
            name: "beta".into(),
            marketplace: "m".into(),
            version: None,
            scope: "user".into(),
            enabled: true,
            install_path: None,
            project_path: None,
        });
        assert_eq!(p.matching()[0].name, "beta");
        p.detail = Some(PluginDetail::new(p.matching()[0].clone()));
        frames(&mut p, true);
        assert!(p.detail.is_some());
    }

    #[test]
    fn toggle_helper() {
        let mut v = vec!["a".to_string()];
        toggle(&mut v, "b", true);
        toggle(&mut v, "a", false);
        assert_eq!(v, ["b"]);
    }
}
