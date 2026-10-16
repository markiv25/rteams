//! MCP Servers tab: search / runtime / sort toolbar, "MCP Health Status" section, the installed
//! servers of the chosen runtime (enable / disable / check / remove) and the catalog cards
//! (bundled popular servers plus the official registry) that open the install dialog.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use eframe::egui::{self, RichText};
use rteams_services::ext::mcp_config::{self, ClientPaths, InstalledServer, Runtime};
use rteams_services::ext::mcp_health::{self, Health, HealthStatus};
use rteams_services::ext::mcp_install;
use rteams_services::ext::mcp_registry::{self, Catalog, InstallSpec, McpCatalogItem};
use rteams_services::usage::ledger::now_ms;

use super::custom_mcp_dialog::{CustomMcpDialog, CustomResult};
use super::mcp_detail::{DetailAction, McpDetail};
use super::store_tabs::{card, dot, notice_line, tx, txa, ExtCtx, Task};
use crate::theme;

const PAGE: usize = 24;
const HEALTH_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum McpSort {
    #[default]
    Default,
    NameAsc,
    NameDesc,
    ToolsDesc,
}

#[derive(Default)]
struct HealthSlot {
    task: Task<Health>,
    result: Option<Health>,
}

enum Act {
    Check(InstalledServer),
    CheckAll,
    Toggle(InstalledServer, bool),
    Remove(InstalledServer),
    Open(Box<McpCatalogItem>),
}

pub struct McpPanel {
    catalog: Option<Catalog>,
    catalog_task: Task<Catalog>,
    force_catalog: bool,
    pub query: String,
    pub shown: usize,
    pub sort: McpSort,
    pub runtime: Runtime,
    pub installed: Vec<InstalledServer>,
    errors: Vec<String>,
    installed_key: Option<(Runtime, ClientPaths)>,
    dirty: bool,
    health: HashMap<String, HealthSlot>,
    checked_ms: Option<u64>,
    detail: Option<McpDetail>,
    pub custom: CustomMcpDialog,
    notice: Option<(bool, String)>,
    confirm_remove: Option<String>,
}

impl Default for McpPanel {
    fn default() -> Self {
        McpPanel {
            catalog: None,
            catalog_task: Task::default(),
            force_catalog: false,
            query: String::new(),
            shown: PAGE,
            sort: McpSort::Default,
            runtime: Runtime::Claude,
            installed: Vec::new(),
            errors: Vec::new(),
            installed_key: None,
            dirty: true,
            health: HashMap::new(),
            checked_ms: None,
            detail: None,
            custom: CustomMcpDialog::default(),
            notice: None,
            confirm_remove: None,
        }
    }
}

fn hkey(s: &InstalledServer) -> String {
    format!("{}:{}", s.scope.id(), s.name)
}

impl McpPanel {
    pub fn invalidate_catalog(&mut self) {
        self.catalog = None;
        self.force_catalog = true;
    }

    #[cfg(test)]
    pub fn invalidate_installed(&mut self) {
        self.dirty = true;
    }

    pub fn open_custom(&mut self) {
        self.custom.open_fresh();
    }

    fn reload_installed(&mut self, paths: &ClientPaths) {
        let (list, errors) = mcp_config::list(paths, self.runtime);
        self.installed = list;
        self.errors = errors;
        self.installed_key = Some((self.runtime, paths.clone()));
        self.dirty = false;
    }

    fn is_installed(&self, item: &McpCatalogItem) -> bool {
        let name = mcp_install::default_server_name(item);
        self.installed.iter().any(|s| s.name == name)
    }

    fn start_check(&mut self, ctx: &egui::Context, s: &InstalledServer, cwd: Option<PathBuf>) {
        let slot = self.health.entry(hkey(s)).or_default();
        let spec = s.spec.clone();
        slot.task.start(ctx, move || {
            mcp_health::check(&spec, cwd.as_deref(), HEALTH_TIMEOUT)
        });
        self.checked_ms = Some(now_ms());
    }

    fn visible_catalog(&self) -> Vec<&McpCatalogItem> {
        let Some(c) = &self.catalog else {
            return Vec::new();
        };
        let mut v = mcp_registry::filter(&c.items, &self.query);
        match self.sort {
            McpSort::NameAsc => v.sort_by_key(|i| i.name.to_lowercase()),
            McpSort::NameDesc => v.sort_by_key(|i| std::cmp::Reverse(i.name.to_lowercase())),
            _ => {}
        }
        v
    }

    fn visible_installed(&self) -> Vec<&InstalledServer> {
        let q = self.query.trim().to_lowercase();
        let mut v: Vec<&InstalledServer> = self
            .installed
            .iter()
            .filter(|s| q.is_empty() || s.name.to_lowercase().contains(&q))
            .collect();
        match self.sort {
            McpSort::NameAsc => v.sort_by_key(|s| s.name.to_lowercase()),
            McpSort::NameDesc => v.sort_by_key(|s| std::cmp::Reverse(s.name.to_lowercase())),
            McpSort::ToolsDesc => v.sort_by_key(|s| {
                std::cmp::Reverse(
                    self.health
                        .get(&hkey(s))
                        .and_then(|h| h.result.as_ref())
                        .and_then(|r| r.tool_count)
                        .unwrap_or(0),
                )
            }),
            McpSort::Default => {}
        }
        v
    }

    fn report(&mut self, x: &mut ExtCtx, ok: bool, msg: String) {
        if ok {
            x.toast(msg.clone());
        }
        self.notice = Some((ok, msg));
    }

    pub fn show(&mut self, ui: &mut egui::Ui, x: &mut ExtCtx) {
        let tr = x.tr;
        // -- background results
        if let Some(c) = self.catalog_task.take() {
            self.catalog = Some(c);
        }
        for slot in self.health.values_mut() {
            if let Some(h) = slot.task.take() {
                slot.result = Some(h);
            }
        }
        if self.catalog.is_none() && !self.catalog_task.busy() {
            if x.offline {
                self.catalog = Some(Catalog {
                    items: mcp_registry::builtin(),
                    ..Default::default()
                });
            } else {
                let (home, force) = (x.home.to_path_buf(), self.force_catalog);
                self.force_catalog = false;
                self.catalog_task
                    .start(ui.ctx(), move || mcp_registry::load(&home, force));
            }
        }
        if self.dirty || self.installed_key.as_ref() != Some(&(self.runtime, x.paths.clone())) {
            self.reload_installed(x.paths);
        }
        if self.health.values().any(|h| h.task.busy()) {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }

        // -- toolbar
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text(tx(
                        tr,
                        "extensions.mcpPanel.searchPlaceholder",
                        "Search MCP servers...",
                    ))
                    .desired_width(300.0),
            );
            egui::ComboBox::from_id_salt("mcp_runtime")
                .selected_text(self.runtime.label())
                .show_ui(ui, |ui| {
                    for r in Runtime::ALL {
                        ui.selectable_value(&mut self.runtime, r, r.label());
                    }
                });
            egui::ComboBox::from_id_salt("mcp_sort")
                .selected_text(match self.sort {
                    McpSort::Default => "Default".to_string(),
                    McpSort::NameAsc => tx(tr, "extensions.mcpPanel.sort.nameAsc", "Name A-Z"),
                    McpSort::NameDesc => tx(tr, "extensions.mcpPanel.sort.nameDesc", "Name Z-A"),
                    McpSort::ToolsDesc => {
                        tx(tr, "extensions.mcpPanel.sort.toolsDesc", "Most tools")
                    }
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.sort, McpSort::Default, "Default");
                    ui.selectable_value(
                        &mut self.sort,
                        McpSort::NameAsc,
                        tx(tr, "extensions.mcpPanel.sort.nameAsc", "Name A-Z"),
                    );
                    ui.selectable_value(
                        &mut self.sort,
                        McpSort::NameDesc,
                        tx(tr, "extensions.mcpPanel.sort.nameDesc", "Name Z-A"),
                    );
                    ui.selectable_value(
                        &mut self.sort,
                        McpSort::ToolsDesc,
                        tx(tr, "extensions.mcpPanel.sort.toolsDesc", "Most tools"),
                    );
                });
        });
        notice_line(ui, &self.notice);

        let mut acts: Vec<Act> = Vec::new();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.health_section(ui, x, &mut acts);
                self.installed_section(ui, x, &mut acts);
                self.catalog_section(ui, x, &mut acts);
            });

        // -- actions
        let cwd = x.paths.project.clone();
        for a in acts {
            match a {
                Act::Check(s) => self.start_check(ui.ctx(), &s, cwd.clone()),
                Act::CheckAll => {
                    for s in self.installed.clone().iter().filter(|s| s.enabled) {
                        self.start_check(ui.ctx(), s, cwd.clone());
                    }
                }
                Act::Toggle(s, on) => {
                    match mcp_config::set_enabled(x.paths, s.runtime, s.scope, &s.name, on) {
                        Ok(_) => {
                            self.dirty = true;
                            self.report(
                                x,
                                true,
                                format!("{} {}", s.name, if on { "enabled" } else { "disabled" }),
                            );
                        }
                        Err(e) => self.report(x, false, e.0),
                    }
                }
                Act::Remove(s) => {
                    match mcp_install::uninstall(x.paths, s.runtime, s.scope, &s.name) {
                        Ok(_) => {
                            self.dirty = true;
                            self.health.remove(&hkey(&s));
                            self.report(x, true, format!("Removed {}", s.name));
                        }
                        Err(e) => self.report(x, false, e),
                    }
                }
                Act::Open(item) => {
                    self.detail = Some(McpDetail::new(*item, x, self.runtime));
                }
            }
        }
        self.dialogs(ui.ctx(), x);
    }

    fn health_section(&mut self, ui: &mut egui::Ui, x: &mut ExtCtx, acts: &mut Vec<Act>) {
        let tr = x.tr;
        let checking = self.health.values().any(|h| h.task.busy());
        egui::Frame::new()
            .fill(theme::CARD)
            .stroke(egui::Stroke::new(1.0, theme::hairline(0.07)))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(tx(
                            tr,
                            "extensions.mcpPanel.health.title",
                            "MCP Health Status",
                        ))
                        .strong(),
                    );
                    if let Some(ms) = self.checked_ms {
                        ui.label(
                            RichText::new(txa(
                                tr,
                                "extensions.mcpPanel.health.lastChecked",
                                "Last checked {{time}}",
                                &[("time", &theme::hhmm(ms as i64))],
                            ))
                            .small()
                            .color(theme::MUTED),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if checking {
                            tx(tr, "extensions.mcpPanel.health.checking", "Checking...")
                        } else {
                            tx(tr, "extensions.mcpPanel.health.checkStatus", "Check Status")
                        };
                        if ui
                            .add_enabled(
                                !checking && !self.installed.is_empty(),
                                egui::Button::new(label),
                            )
                            .clicked()
                        {
                            acts.push(Act::CheckAll);
                        }
                    });
                });
                ui.label(
                    RichText::new(tx(
                        tr,
                        "extensions.mcpPanel.health.description",
                        "Run diagnostics from this page to verify installed MCP connectivity.",
                    ))
                    .small()
                    .color(theme::MUTED),
                );
            });
        ui.add_space(6.0);
    }

    fn installed_section(&mut self, ui: &mut egui::Ui, x: &mut ExtCtx, acts: &mut Vec<Act>) {
        let tr = x.tr;
        for e in &self.errors {
            ui.colored_label(theme::WARNING, e);
        }
        let list: Vec<InstalledServer> = self.visible_installed().into_iter().cloned().collect();
        let title = format!(
            "{} ({})",
            tx(tr, "extensions.mcpPanel.diagnostics.title", "Installed"),
            self.runtime.label()
        );
        theme::section(
            ui,
            "mcp_installed",
            "🔧",
            &title,
            Some(list.len()),
            true,
            |_| {},
            |ui| {
                if list.is_empty() {
                    ui.label(
                        RichText::new("No MCP servers configured for this runtime.")
                            .color(theme::MUTED),
                    );
                }
                for s in &list {
                    let key = hkey(s);
                    card(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            let hs = self.health.get(&key);
                            let (color, running) = match hs {
                                Some(h) if h.task.busy() => (theme::INFO, true),
                                Some(HealthSlot {
                                    result: Some(r), ..
                                }) => (
                                    match r.status {
                                        HealthStatus::Ok => theme::SUCCESS,
                                        HealthStatus::NeedsAuth => theme::WARNING,
                                        _ => theme::ERROR,
                                    },
                                    false,
                                ),
                                _ => (theme::MUTED, false),
                            };
                            dot(ui, if s.enabled { color } else { theme::MUTED });
                            ui.label(RichText::new(&s.name).strong());
                            theme::pill(ui, s.scope.id(), theme::ACCENT);
                            theme::pill(ui, s.spec.transport.id(), theme::INFO);
                            if !s.enabled {
                                theme::pill(ui, "disabled", theme::WARNING);
                            }
                            if running {
                                ui.spinner();
                            }
                            if let Some(r) = hs.and_then(|h| h.result.as_ref()) {
                                match (r.status, r.tool_count) {
                                    (HealthStatus::Ok, Some(n)) => {
                                        ui.label(
                                            RichText::new(format!("{n} tools"))
                                                .small()
                                                .color(theme::TEXT_2),
                                        );
                                    }
                                    (HealthStatus::Ok, None) => {}
                                    _ => {
                                        if let Some(e) = &r.error {
                                            ui.label(
                                                RichText::new(crate::runtimes::clip(e, 90))
                                                    .small()
                                                    .color(theme::ERROR),
                                            );
                                        }
                                    }
                                }
                            }
                        });
                        ui.label(
                            RichText::new(crate::runtimes::clip(&s.spec.launch_target(), 120))
                                .monospace()
                                .small()
                                .color(theme::MUTED),
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .small_button(tx(
                                    tr,
                                    "extensions.mcpPanel.health.checkStatus",
                                    "Check Status",
                                ))
                                .clicked()
                            {
                                acts.push(Act::Check(s.clone()));
                            }
                            let mut on = s.enabled;
                            if ui.checkbox(&mut on, "Enabled").changed() {
                                acts.push(Act::Toggle(s.clone(), on));
                            }
                            if self.confirm_remove.as_deref() == Some(key.as_str()) {
                                if theme::solid(ui, "Confirm remove", theme::ERROR).clicked() {
                                    self.confirm_remove = None;
                                    acts.push(Act::Remove(s.clone()));
                                }
                                if theme::ghost(ui, "Cancel").clicked() {
                                    self.confirm_remove = None;
                                }
                            } else if theme::ghost(ui, "Remove").clicked() {
                                self.confirm_remove = Some(key.clone());
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
            },
        );
    }

    fn catalog_section(&mut self, ui: &mut egui::Ui, x: &mut ExtCtx, acts: &mut Vec<Act>) {
        let tr = x.tr;
        if self.catalog_task.busy() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading catalog...").color(theme::MUTED));
            });
        }
        if let Some(e) = self.catalog.as_ref().and_then(|c| c.error.as_ref()) {
            ui.colored_label(
                theme::WARNING,
                format!("Registry unavailable, showing cached and bundled servers: {e}"),
            );
        }
        let items: Vec<McpCatalogItem> = self.visible_catalog().into_iter().cloned().collect();
        let total = items.len();
        if total == 0 && !self.catalog_task.busy() {
            let searching = !self.query.trim().is_empty();
            ui.vertical_centered(|ui| {
                ui.add_space(20.0);
                ui.label(
                    RichText::new(if searching {
                        tx(
                            tr,
                            "extensions.mcpPanel.empty.searchTitle",
                            "No servers found",
                        )
                    } else {
                        tx(
                            tr,
                            "extensions.mcpPanel.empty.title",
                            "No MCP servers available",
                        )
                    })
                    .size(16.0)
                    .strong(),
                );
                ui.label(
                    RichText::new(if searching {
                        tx(
                            tr,
                            "extensions.mcpPanel.empty.searchDescription",
                            "Try a different search term",
                        )
                    } else {
                        tx(
                            tr,
                            "extensions.mcpPanel.empty.description",
                            "Check back later for new servers",
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
        egui::Grid::new("mcp_catalog_grid")
            .num_columns(cols)
            .spacing([spacing, spacing])
            .show(ui, |ui| {
                for (i, it) in items.iter().take(self.shown).enumerate() {
                    let installed = self.is_installed(it);
                    ui.vertical(|ui| {
                        ui.set_width(w);
                        card(ui, |ui| {
                            ui.set_width(w - 26.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(&it.name).strong().size(14.5));
                                if installed {
                                    theme::pill(ui, "Installed", theme::SUCCESS);
                                }
                            });
                            ui.horizontal_wrapped(|ui| {
                                theme::pill(ui, it.source.clone(), theme::INFO);
                                match &it.install {
                                    Some(InstallSpec::Package { .. }) => {
                                        theme::pill(ui, "stdio", theme::MUTED);
                                    }
                                    Some(InstallSpec::Remote { .. }) => {
                                        theme::pill(ui, "remote", theme::MUTED);
                                    }
                                    None => {
                                        theme::pill(ui, "manual", theme::WARNING);
                                    }
                                }
                                if it.requires_auth {
                                    theme::pill(ui, "auth", theme::WARNING);
                                }
                            });
                            ui.label(
                                RichText::new(crate::runtimes::clip(&it.description, 140))
                                    .small()
                                    .color(theme::TEXT_2),
                            );
                            let label = if installed {
                                tx(
                                    tr,
                                    "extensions.mcpDetail.install.manage",
                                    "Manage Installation",
                                )
                            } else {
                                tx(tr, "extensions.mcpDetail.install.install", "Install Server")
                            };
                            if ui.small_button(label).clicked() {
                                acts.push(Act::Open(Box::new(it.clone())));
                            }
                        });
                    });
                    if (i + 1) % cols == 0 {
                        ui.end_row();
                    }
                }
            });
        if total > self.shown {
            ui.add_space(6.0);
            if ui
                .button(tx(tr, "extensions.mcpPanel.loadMore", "Load more"))
                .clicked()
            {
                self.shown += PAGE;
            }
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context, x: &mut ExtCtx) {
        // Detail dialog.
        if let Some(mut d) = self.detail.take() {
            let installed = {
                let name = mcp_install::default_server_name(&d.item);
                self.installed.iter().find(|s| s.name == name).cloned()
            };
            let health = installed
                .as_ref()
                .and_then(|s| self.health.get(&hkey(s)))
                .and_then(|h| h.result.clone());
            let act = d.show(ctx, x, self.runtime, installed.as_ref(), health.as_ref());
            match act {
                DetailAction::None => self.detail = Some(d),
                DetailAction::Close => {}
                DetailAction::Install { name, scope, spec } => {
                    match mcp_install::install(x.paths, self.runtime, scope, &name, &spec) {
                        Ok(()) => {
                            self.dirty = true;
                            self.report(x, true, format!("Installed {name}"));
                        }
                        Err(e) => {
                            d.error = Some(e);
                            self.detail = Some(d);
                        }
                    }
                }
                DetailAction::Remove { name, scope } => {
                    match mcp_install::uninstall(x.paths, self.runtime, scope, &name) {
                        Ok(_) => {
                            self.dirty = true;
                            self.report(x, true, format!("Removed {name}"));
                        }
                        Err(e) => {
                            d.error = Some(e);
                            self.detail = Some(d);
                        }
                    }
                }
            }
        }
        // Custom dialog.
        match self.custom.show(ctx, x, self.runtime) {
            CustomResult::None => {}
            CustomResult::Close => self.custom.open = false,
            CustomResult::Add { name, scope, spec } => {
                match mcp_install::install(x.paths, self.runtime, scope, &name, &spec) {
                    Ok(()) => {
                        self.custom.open = false;
                        self.dirty = true;
                        self.report(x, true, format!("Installed {name}"));
                    }
                    Err(e) => self.custom.error = Some(e),
                }
            }
        }
    }

    /// Scope of a server `name` in the current list (test helper).
    #[cfg(test)]
    fn scope_of(&self, name: &str) -> Option<mcp_config::Scope> {
        self.installed
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.scope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::ShellRequest;
    use crate::view::testing::render;
    use rteams_services::ext::mcp_config::{Scope, ServerSpec};
    use rteams_services::i18n::I18n;

    fn run_frames(
        panel: &mut McpPanel,
        home: &std::path::Path,
        paths: &ClientPaths,
        req: &mut Vec<ShellRequest>,
    ) {
        let tr = I18n::default();
        render(|ui| {
            let mut x = ExtCtx {
                home,
                paths,
                offline: true,
                tr: &tr,
                req: &mut *req,
            };
            panel.show(ui, &mut x);
        });
    }

    #[test]
    fn lists_installs_toggles_and_removes_through_config_files() {
        let d = tempfile::tempdir().unwrap();
        let paths = ClientPaths::new(d.path().join("u"), Some(d.path().join("p")));
        let mut panel = McpPanel::default();
        let mut req = Vec::new();
        run_frames(&mut panel, d.path(), &paths, &mut req);
        assert!(panel.installed.is_empty());
        assert!(panel.catalog.as_ref().unwrap().items.len() >= 20);

        // Install through the same call the dialog makes.
        let item = mcp_registry::find(&mcp_registry::builtin(), "io.modelcontextprotocol/memory")
            .unwrap()
            .clone();
        let spec =
            mcp_install::spec_for(&item, &Default::default(), &Default::default(), &[]).unwrap();
        mcp_install::install(&paths, Runtime::Claude, Scope::Project, "memory", &spec).unwrap();
        panel.invalidate_installed();
        run_frames(&mut panel, d.path(), &paths, &mut req);
        assert_eq!(panel.scope_of("memory"), Some(Scope::Project));
        assert!(panel.is_installed(&item));

        mcp_config::set_enabled(&paths, Runtime::Claude, Scope::Project, "memory", false).unwrap();
        panel.invalidate_installed();
        run_frames(&mut panel, d.path(), &paths, &mut req);
        assert!(!panel.installed[0].enabled);

        // Switching runtime re-reads that runtime's own file.
        mcp_config::upsert(
            &paths,
            Runtime::Codex,
            Scope::User,
            "cx",
            &ServerSpec::stdio("x", &[]),
        )
        .unwrap();
        panel.runtime = Runtime::Codex;
        run_frames(&mut panel, d.path(), &paths, &mut req);
        assert_eq!(panel.installed.len(), 1);
        assert_eq!(panel.installed[0].name, "cx");
    }

    #[test]
    fn search_sort_and_paging() {
        let d = tempfile::tempdir().unwrap();
        let paths = ClientPaths::new(d.path().join("u"), None);
        let mut panel = McpPanel::default();
        let mut req = Vec::new();
        run_frames(&mut panel, d.path(), &paths, &mut req);
        let all = panel.visible_catalog().len();
        panel.query = "github".into();
        assert!(panel.visible_catalog().len() < all);
        panel.query.clear();
        panel.sort = McpSort::NameDesc;
        let v = panel.visible_catalog();
        assert!(v[0].name.to_lowercase() >= v[1].name.to_lowercase());
        panel.shown = 3;
        run_frames(&mut panel, d.path(), &paths, &mut req);
        panel.query = "zzzz-no-match".into();
        run_frames(&mut panel, d.path(), &paths, &mut req);
    }

    #[test]
    fn custom_dialog_installs_into_chosen_scope() {
        let d = tempfile::tempdir().unwrap();
        let paths = ClientPaths::new(d.path().join("u"), Some(d.path().join("p")));
        let mut panel = McpPanel::default();
        let mut req = Vec::new();
        panel.open_custom();
        panel.custom.name = "cust".into();
        panel.custom.package = "some-pkg".into();
        panel.custom.scope = Some(Scope::Project);
        run_frames(&mut panel, d.path(), &paths, &mut req);
        // Same path the Add button takes.
        let (name, spec) = panel.custom.build().unwrap();
        mcp_install::install(&paths, Runtime::Claude, Scope::Project, &name, &spec).unwrap();
        panel.invalidate_installed();
        run_frames(&mut panel, d.path(), &paths, &mut req);
        assert_eq!(panel.scope_of("cust"), Some(Scope::Project));
    }
}
