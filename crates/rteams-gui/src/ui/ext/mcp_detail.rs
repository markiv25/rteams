//! MCP server detail dialog: metadata, links, tools (after a health check) and the install form
//! (server name, scope, env vars / headers auto-filled from saved API keys, extra arguments).
//! Installed servers get "Manage Installation" instead.

use eframe::egui::{self, RichText};
use rteams_services::ext::apikeys;
use rteams_services::ext::mcp_config::{ClientPaths, InstalledServer, Runtime, Scope, ServerSpec};
use rteams_services::ext::mcp_health::{Health, HealthStatus};
use rteams_services::ext::mcp_install;
use rteams_services::ext::mcp_registry::{InstallSpec, McpCatalogItem, PackageKind};
use rteams_services::i18n::I18n;

use super::store_tabs::{big_count, kv_editor, rows_to_map, tx, txa, ExtCtx};
use crate::theme;

/// Scopes selectable for `rt`: user first; project / local only with an open project.
pub fn scope_choices(rt: Runtime, paths: &ClientPaths) -> Vec<Scope> {
    [Scope::User, Scope::Project, Scope::Local]
        .into_iter()
        .filter(|s| rt.scopes().contains(s))
        .filter(|s| *s == Scope::User || paths.project.is_some())
        .collect()
}

pub fn scope_label(tr: &I18n, s: Scope) -> String {
    match s {
        Scope::Local => tx(tr, "extensions.mcpDetail.scope.local", "Local"),
        Scope::Project => tx(tr, "extensions.mcpDetail.scope.project", "Project"),
        Scope::User => "User (global)".into(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DetailAction {
    None,
    Close,
    Install {
        name: String,
        scope: Scope,
        spec: ServerSpec,
    },
    Remove {
        name: String,
        scope: Scope,
    },
}

pub struct McpDetail {
    pub item: McpCatalogItem,
    pub name: String,
    pub scope: Scope,
    pub env: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub extra_args: String,
    pub error: Option<String>,
    pub busy: bool,
}

impl McpDetail {
    pub fn new(item: McpCatalogItem, x: &ExtCtx, runtime: Runtime) -> Self {
        let saved = apikeys::Store::new(x.home).autofill_pairs(x.project());
        let filled = mcp_install::autofill_env(&item, &saved);
        let env = item
            .env_vars
            .iter()
            .map(|e| {
                (
                    e.name.clone(),
                    filled.get(&e.name).cloned().unwrap_or_default(),
                )
            })
            .collect();
        let headers = item
            .auth_headers
            .iter()
            .map(|h| (h.key.clone(), h.value_template.clone().unwrap_or_default()))
            .collect();
        let scope = scope_choices(runtime, x.paths)
            .first()
            .copied()
            .unwrap_or(Scope::User);
        McpDetail {
            name: mcp_install::default_server_name(&item),
            item,
            scope,
            env,
            headers,
            extra_args: String::new(),
            error: None,
            busy: false,
        }
    }

    /// Validate the form into an install request.
    pub fn build(&self) -> Result<(String, ServerSpec), String> {
        let name = self.name.trim().to_string();
        mcp_install::validate_name(&name)?;
        let env = rows_to_map(&self.env);
        let headers = rows_to_map(&self.headers);
        let missing = mcp_install::missing_required(&self.item, &env, &headers);
        if !missing.is_empty() {
            return Err(format!("Missing required values: {}", missing.join(", ")));
        }
        // Drop empty optional values so they are not written as empty strings.
        let env = env.into_iter().filter(|(_, v)| !v.is_empty()).collect();
        let headers = headers.into_iter().filter(|(_, v)| !v.is_empty()).collect();
        let args: Vec<String> = self
            .extra_args
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let spec = mcp_install::spec_for(&self.item, &env, &headers, &args)?;
        Ok((name, spec))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        x: &ExtCtx,
        runtime: Runtime,
        installed: Option<&InstalledServer>,
        health: Option<&Health>,
    ) -> DetailAction {
        let tr = x.tr;
        let mut action = DetailAction::None;
        let scopes = scope_choices(runtime, x.paths);
        let item = self.item.clone();
        egui::Window::new(item.name.clone())
            .collapsible(false)
            .default_width(620.0)
            .default_height(560.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height((ui.available_height() - 56.0).max(100.0))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.label(RichText::new(&item.description).color(theme::TEXT_2));
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            theme::pill(ui, item.source.clone(), theme::INFO);
                            if installed.is_some() {
                                theme::pill(ui, "Installed", theme::SUCCESS);
                            }
                            if item.requires_auth {
                                theme::pill(
                                    ui,
                                    tx(
                                        tr,
                                        "extensions.mcpDetail.auth.required",
                                        "This server requires authentication",
                                    ),
                                    theme::WARNING,
                                );
                            }
                        });
                        ui.add_space(6.0);
                        self.metadata(ui, tr);
                        ui.horizontal_wrapped(|ui| {
                            if let Some(u) = &item.repository_url {
                                ui.hyperlink_to(
                                    tx(tr, "extensions.mcpDetail.links.repository", "Repository"),
                                    u,
                                );
                            }
                            if let Some(u) = &item.website_url {
                                ui.hyperlink_to(
                                    tx(tr, "extensions.mcpDetail.links.website", "Website"),
                                    u,
                                );
                            }
                        });
                        if let Some(h) = health {
                            theme::rule(ui);
                            health_summary(ui, h, tr);
                        }
                        theme::rule(ui);
                        match installed {
                            Some(s) => self.manage(ui, tr, s, &mut action),
                            None => self.install_form(ui, x, &scopes),
                        }
                    });
                if let Some(e) = &self.error {
                    ui.colored_label(theme::ERROR, e);
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if theme::ghost(ui, "Close").clicked() {
                        action = DetailAction::Close;
                    }
                    if installed.is_none() && item.install.is_some() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let label = if self.busy {
                                tx(
                                    tr,
                                    "extensions.customMcp.actions.installing",
                                    "Installing...",
                                )
                            } else {
                                tx(tr, "extensions.mcpDetail.install.install", "Install Server")
                            };
                            if ui
                                .add_enabled(
                                    !self.busy,
                                    egui::Button::new(RichText::new(label).strong())
                                        .fill(theme::ACCENT),
                                )
                                .clicked()
                            {
                                match self.build() {
                                    Ok((name, spec)) => {
                                        self.error = None;
                                        action = DetailAction::Install {
                                            name,
                                            scope: self.scope,
                                            spec,
                                        };
                                    }
                                    Err(e) => self.error = Some(e),
                                }
                            }
                        });
                    }
                });
            });
        action
    }

    fn metadata(&self, ui: &mut egui::Ui, tr: &I18n) {
        let it = &self.item;
        egui::Grid::new("mcp_detail_meta")
            .num_columns(2)
            .spacing([16.0, 4.0])
            .show(ui, |ui| {
                let mut row = |label: String, value: String| {
                    ui.label(RichText::new(label).color(theme::MUTED));
                    ui.label(value);
                    ui.end_row();
                };
                row(
                    tx(tr, "extensions.mcpDetail.metadata.source", "Source"),
                    it.source.clone(),
                );
                if let Some(v) = &it.version {
                    row(
                        tx(tr, "extensions.mcpDetail.metadata.version", "Version"),
                        v.clone(),
                    );
                }
                row(
                    tx(
                        tr,
                        "extensions.mcpDetail.metadata.installType",
                        "Install Type",
                    ),
                    match &it.install {
                        Some(InstallSpec::Package {
                            kind, identifier, ..
                        }) => {
                            let k = match kind {
                                PackageKind::Npm => "npm",
                                PackageKind::Pypi => "pypi",
                                PackageKind::Oci => "docker",
                            };
                            format!("{k}: {identifier}")
                        }
                        Some(InstallSpec::Remote { url, sse }) => {
                            format!("HTTP: {}{}", if *sse { "sse " } else { "" }, url)
                        }
                        None => tx(
                            tr,
                            "extensions.mcpDetail.install.manualSetupRequired",
                            "Manual setup required",
                        ),
                    },
                );
                if let Some(s) = it.stars {
                    row(
                        tx(
                            tr,
                            "extensions.mcpDetail.metadata.githubStars",
                            "GitHub Stars",
                        ),
                        big_count(s),
                    );
                }
                if let Some(p) = &it.published_at {
                    row(
                        tx(tr, "extensions.mcpDetail.metadata.published", "Published"),
                        p.chars().take(10).collect(),
                    );
                }
                if let Some(p) = &it.updated_at {
                    row(
                        tx(tr, "extensions.mcpDetail.metadata.updated", "Updated"),
                        p.chars().take(10).collect(),
                    );
                }
            });
    }

    fn manage(
        &mut self,
        ui: &mut egui::Ui,
        tr: &I18n,
        s: &InstalledServer,
        action: &mut DetailAction,
    ) {
        ui.label(
            RichText::new(tx(
                tr,
                "extensions.mcpDetail.install.manage",
                "Manage Installation",
            ))
            .strong(),
        );
        ui.label(
            RichText::new(format!("{} ({})", s.name, s.scope.id()))
                .small()
                .color(theme::TEXT_2),
        );
        ui.label(
            RichText::new(format!(
                "{}: {}",
                tx(
                    tr,
                    "extensions.mcpDetail.diagnostics.launchTarget",
                    "Launch Target"
                ),
                s.spec.launch_target()
            ))
            .monospace()
            .small(),
        );
        if theme::solid(ui, "Remove", theme::ERROR).clicked() {
            *action = DetailAction::Remove {
                name: s.name.clone(),
                scope: s.scope,
            };
        }
    }

    fn install_form(&mut self, ui: &mut egui::Ui, x: &ExtCtx, scopes: &[Scope]) {
        let tr = x.tr;
        if self.item.install.is_none() {
            ui.label(
                RichText::new(tx(
                    tr,
                    "extensions.mcpDetail.install.manualSetupRequired",
                    "Manual setup required",
                ))
                .strong(),
            );
            ui.label(
                RichText::new(tx(
                    tr,
                    "extensions.mcpDetail.install.manualSetupDescription",
                    "This server requires manual setup. Check the repository for installation instructions.",
                ))
                .color(theme::MUTED),
            );
            return;
        }
        ui.label(tx(
            tr,
            "extensions.mcpDetail.form.serverName",
            "Server Name",
        ));
        ui.add(
            egui::TextEdit::singleline(&mut self.name)
                .hint_text(tx(
                    tr,
                    "extensions.mcpDetail.placeholders.serverName",
                    "my-server",
                ))
                .desired_width(320.0),
        );
        ui.label(tx(tr, "extensions.mcpDetail.form.scope", "Scope"));
        ui.horizontal(|ui| {
            for s in scopes {
                ui.selectable_value(&mut self.scope, *s, scope_label(tr, *s));
            }
        });
        let is_remote = matches!(self.item.install, Some(InstallSpec::Remote { .. }));
        if is_remote {
            ui.add_space(4.0);
            ui.label(tx(tr, "extensions.mcpDetail.form.headers", "Headers"));
            kv_editor(
                ui,
                "mcp_detail_headers",
                &mut self.headers,
                "Header-Name",
                "value",
                true,
            );
            if self.item.auth_headers.is_empty() {
                ui.label(
                    RichText::new(tx(
                        tr,
                        "extensions.mcpDetail.auth.remoteMayNeedHeaders",
                        "Remote MCP servers may still require custom headers or API keys even when the registry does not describe them.",
                    ))
                    .small()
                    .color(theme::MUTED),
                );
            }
        } else {
            ui.add_space(4.0);
            ui.label(tx(
                tr,
                "extensions.mcpDetail.form.environmentVariables",
                "Environment Variables",
            ));
            let saved = apikeys::Store::new(x.home).autofill_pairs(x.project());
            if self
                .env
                .iter()
                .any(|(k, v)| !v.is_empty() && saved.iter().any(|(sk, sv)| sk == k && sv == v))
            {
                ui.label(
                    RichText::new(tx(
                        tr,
                        "extensions.mcpDetail.form.autoFilled",
                        "Auto-filled",
                    ))
                    .small()
                    .color(theme::SUCCESS),
                );
            }
            kv_editor(
                ui,
                "mcp_detail_env",
                &mut self.env,
                "ENV_VAR_NAME",
                "value",
                true,
            );
            ui.label("Extra arguments");
            ui.add(
                egui::TextEdit::singleline(&mut self.extra_args)
                    .hint_text("/path/one /path/two")
                    .desired_width(420.0),
            );
        }
        let required: Vec<String> = self
            .item
            .env_vars
            .iter()
            .filter(|e| e.is_required)
            .map(|e| e.name.clone())
            .chain(
                self.item
                    .auth_headers
                    .iter()
                    .filter(|h| h.is_required)
                    .map(|h| h.key.clone()),
            )
            .collect();
        if !required.is_empty() {
            ui.label(
                RichText::new(txa(
                    tr,
                    "extensions.mcpDetail.required",
                    "Required: {{names}}",
                    &[("names", &required.join(", "))],
                ))
                .small()
                .color(theme::WARNING),
            );
        }
    }
}

/// Status line + tool list from a health check.
pub fn health_summary(ui: &mut egui::Ui, h: &Health, tr: &I18n) {
    let (color, text) = match h.status {
        HealthStatus::Ok => (theme::SUCCESS, "Connected"),
        HealthStatus::NeedsAuth => (theme::WARNING, "Needs authentication"),
        HealthStatus::Timeout => (theme::ERROR, "Timed out"),
        HealthStatus::Error => (theme::ERROR, "Failed to connect"),
    };
    ui.horizontal(|ui| {
        super::store_tabs::dot(ui, color);
        ui.label(RichText::new(text).strong());
        if let Some(n) = &h.server_name {
            ui.label(
                RichText::new(format!(
                    "{n} {}",
                    h.server_version.clone().unwrap_or_default()
                ))
                .color(theme::TEXT_2),
            );
        }
        ui.label(
            RichText::new(format!("{} ms", h.elapsed_ms))
                .small()
                .color(theme::MUTED),
        );
    });
    if let Some(e) = &h.error {
        ui.colored_label(theme::ERROR, e);
    }
    if let Some(n) = h.tool_count {
        ui.label(
            RichText::new(txa(
                tr,
                "extensions.mcpDetail.tools.title",
                "Tools ({{count}})",
                &[("count", &n.to_string())],
            ))
            .strong(),
        );
        ui.horizontal_wrapped(|ui| {
            for t in &h.tool_names {
                theme::pill(ui, t.clone(), theme::ACCENT);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rteams_services::ext::mcp_registry::{builtin, find};

    fn xctx<'a>(
        home: &'a std::path::Path,
        paths: &'a ClientPaths,
        tr: &'a I18n,
        req: &'a mut Vec<crate::ui::ShellRequest>,
    ) -> ExtCtx<'a> {
        ExtCtx {
            home,
            paths,
            offline: true,
            tr,
            req,
        }
    }

    #[test]
    fn scope_choices_depend_on_project_and_runtime() {
        let with = ClientPaths::new("/h", Some("/p".into()));
        let without = ClientPaths::new("/h", None);
        assert_eq!(
            scope_choices(Runtime::Claude, &with),
            [Scope::User, Scope::Project, Scope::Local]
        );
        assert_eq!(
            scope_choices(Runtime::Codex, &with),
            [Scope::User, Scope::Project]
        );
        assert_eq!(scope_choices(Runtime::Claude, &without), [Scope::User]);
    }

    #[test]
    fn form_validates_required_and_autofills_keys() {
        let d = tempfile::tempdir().unwrap();
        rteams_services::ext::apikeys::Store::new(d.path())
            .add("brave", "BRAVE_API_KEY", "key-abc", None)
            .unwrap();
        let paths = ClientPaths::new(d.path().join("u"), None);
        let tr = I18n::default();
        let mut req = Vec::new();
        let x = xctx(d.path(), &paths, &tr, &mut req);
        let item = find(&builtin(), "com.brave/brave-search").unwrap().clone();
        let mut det = McpDetail::new(item.clone(), &x, Runtime::Claude);
        assert_eq!(det.name, "brave-search");
        assert_eq!(
            det.env,
            [("BRAVE_API_KEY".to_string(), "key-abc".to_string())]
        );
        let (name, spec) = det.build().unwrap();
        assert_eq!(name, "brave-search");
        assert_eq!(spec.env["BRAVE_API_KEY"], "key-abc");
        det.env[0].1.clear();
        assert!(det.build().unwrap_err().contains("BRAVE_API_KEY"));
        det.env[0].1 = "k".into();
        det.name = "bad name".into();
        assert!(det.build().is_err());
    }

    #[test]
    fn dialog_renders_install_and_manage_modes() {
        let d = tempfile::tempdir().unwrap();
        let paths = ClientPaths::new(d.path().join("u"), Some(d.path().join("p")));
        let tr = I18n::default();
        let items = builtin();
        for id in [
            "io.modelcontextprotocol/filesystem",
            "com.github/github",
            "com.brave/brave-search",
        ] {
            let mut req = Vec::new();
            let item = find(&items, id).unwrap().clone();
            let mut det = McpDetail::new(
                item,
                &xctx(d.path(), &paths, &tr, &mut req),
                Runtime::Claude,
            );
            let installed = InstalledServer {
                name: "x".into(),
                runtime: Runtime::Claude,
                scope: Scope::User,
                spec: ServerSpec::stdio("npx", &[]),
                enabled: true,
                file: "/f".into(),
            };
            for inst in [None, Some(&installed)] {
                crate::view::testing::render(|ui| {
                    let mut r = Vec::new();
                    let x = xctx(d.path(), &paths, &tr, &mut r);
                    det.show(ui.ctx(), &x, Runtime::Claude, inst, None);
                });
            }
        }
    }
}
