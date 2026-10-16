//! "Add Custom MCP Server" dialog: server name, transport (npm stdio package, any stdio command,
//! remote HTTP or legacy SSE), env vars / headers and the scope to install into.

use eframe::egui::{self, RichText};
use rteams_services::ext::mcp_config::{Runtime, Scope, ServerSpec};
use rteams_services::ext::mcp_install;

use super::mcp_detail::{scope_choices, scope_label};
use super::store_tabs::{kv_editor, rows_to_map, tx, ExtCtx};
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CustomTransport {
    #[default]
    Npm,
    Command,
    Http,
    Sse,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CustomResult {
    None,
    Close,
    Add {
        name: String,
        scope: Scope,
        spec: ServerSpec,
    },
}

#[derive(Debug, Default)]
pub struct CustomMcpDialog {
    pub open: bool,
    pub name: String,
    pub transport: CustomTransport,
    pub package: String,
    pub version: String,
    pub command: String,
    pub args: String,
    pub url: String,
    pub env: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub scope: Option<Scope>,
    pub error: Option<String>,
    pub busy: bool,
}

impl CustomMcpDialog {
    pub fn open_fresh(&mut self) {
        *self = CustomMcpDialog {
            open: true,
            ..Default::default()
        };
    }

    /// Validate the form into `(name, spec)`.
    pub fn build(&self) -> Result<(String, ServerSpec), String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("Server name is required".into());
        }
        mcp_install::validate_name(name)?;
        let env = rows_to_map(&self.env);
        let headers = rows_to_map(&self.headers);
        let spec = match self.transport {
            CustomTransport::Npm => mcp_install::custom_npm(&self.package, &self.version, &env)?,
            CustomTransport::Command => {
                let args: Vec<String> = self.args.split_whitespace().map(str::to_string).collect();
                mcp_install::custom_command(&self.command, &args, &env)?
            }
            CustomTransport::Http => mcp_install::custom_http(&self.url, false, &headers)?,
            CustomTransport::Sse => mcp_install::custom_http(&self.url, true, &headers)?,
        };
        Ok((name.to_string(), spec))
    }

    pub fn show(&mut self, ctx: &egui::Context, x: &ExtCtx, runtime: Runtime) -> CustomResult {
        let mut result = CustomResult::None;
        if !self.open {
            return result;
        }
        let tr = x.tr;
        let scopes = scope_choices(runtime, x.paths);
        let scope = *self
            .scope
            .get_or_insert_with(|| scopes.first().copied().unwrap_or(Scope::User));
        let mut scope = if scopes.contains(&scope) {
            scope
        } else {
            scopes.first().copied().unwrap_or(Scope::User)
        };
        egui::Window::new(tx(
            tr,
            "extensions.customMcp.title",
            "Add Custom MCP Server",
        ))
        .collapsible(false)
        .default_width(520.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(
                RichText::new(tx(
                    tr,
                    "extensions.customMcp.description",
                    "Add a server manually without the catalog.",
                ))
                .color(theme::TEXT_2),
            );
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .max_height((ui.available_height() - 60.0).max(120.0))
                .show(ui, |ui| {
                    ui.label(tx(
                        tr,
                        "extensions.customMcp.fields.serverName",
                        "Server Name",
                    ));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.name)
                            .hint_text(tx(
                                tr,
                                "extensions.customMcp.placeholders.serverName",
                                "my-server",
                            ))
                            .desired_width(320.0),
                    );
                    ui.add_space(4.0);
                    ui.label(tx(
                        tr,
                        "extensions.customMcp.fields.transportType",
                        "Transport Type",
                    ));
                    ui.horizontal_wrapped(|ui| {
                        for (t, label) in [
                            (
                                CustomTransport::Npm,
                                tx(tr, "extensions.customMcp.transport.stdio", "Stdio (npm)"),
                            ),
                            (CustomTransport::Command, "Stdio (command)".to_string()),
                            (CustomTransport::Http, "HTTP".to_string()),
                            (CustomTransport::Sse, "SSE".to_string()),
                        ] {
                            ui.selectable_value(&mut self.transport, t, label);
                        }
                    });
                    ui.add_space(4.0);
                    match self.transport {
                        CustomTransport::Npm => {
                            ui.label(tx(
                                tr,
                                "extensions.customMcp.fields.npmPackage",
                                "npm Package",
                            ));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.package)
                                    .hint_text("@scope/server-name")
                                    .desired_width(320.0),
                            );
                            ui.label(tx(
                                tr,
                                "extensions.customMcp.fields.versionOptional",
                                "Version (optional)",
                            ));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.version)
                                    .hint_text(tx(
                                        tr,
                                        "extensions.customMcp.placeholders.latest",
                                        "latest",
                                    ))
                                    .desired_width(160.0),
                            );
                        }
                        CustomTransport::Command => {
                            ui.label("Command");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.command)
                                    .hint_text("npx, uvx, node, ./server")
                                    .desired_width(320.0),
                            );
                            ui.label("Arguments");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.args)
                                    .hint_text("-y my-package --flag")
                                    .desired_width(420.0),
                            );
                        }
                        CustomTransport::Http | CustomTransport::Sse => {
                            ui.label(tx(
                                tr,
                                "extensions.customMcp.fields.serverUrl",
                                "Server URL",
                            ));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.url)
                                    .hint_text(tx(
                                        tr,
                                        "extensions.customMcp.placeholders.serverUrl",
                                        "https://api.example.com/mcp",
                                    ))
                                    .desired_width(420.0),
                            );
                        }
                    }
                    ui.add_space(4.0);
                    if matches!(self.transport, CustomTransport::Http | CustomTransport::Sse) {
                        ui.label(tx(tr, "extensions.customMcp.fields.headers", "Headers"));
                        kv_editor(
                            ui,
                            "custom_headers",
                            &mut self.headers,
                            &tx(
                                tr,
                                "extensions.customMcp.placeholders.headerName",
                                "Header-Name",
                            ),
                            &tx(tr, "extensions.customMcp.placeholders.value", "value"),
                            true,
                        );
                    } else {
                        ui.label(tx(
                            tr,
                            "extensions.customMcp.fields.environmentVariables",
                            "Environment Variables",
                        ));
                        kv_editor(
                            ui,
                            "custom_env",
                            &mut self.env,
                            &tx(
                                tr,
                                "extensions.customMcp.placeholders.envVarName",
                                "ENV_VAR_NAME",
                            ),
                            &tx(tr, "extensions.customMcp.placeholders.value", "value"),
                            true,
                        );
                    }
                    ui.add_space(4.0);
                    ui.label(tx(tr, "extensions.customMcp.fields.scope", "Scope"));
                    ui.horizontal(|ui| {
                        for s in &scopes {
                            ui.selectable_value(&mut scope, *s, scope_label(tr, *s));
                        }
                    });
                });
            if let Some(e) = &self.error {
                ui.colored_label(theme::ERROR, e);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if theme::ghost(ui, &tx(tr, "extensions.customMcp.actions.cancel", "Cancel"))
                    .clicked()
                {
                    result = CustomResult::Close;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if self.busy {
                        tx(
                            tr,
                            "extensions.customMcp.actions.installing",
                            "Installing...",
                        )
                    } else {
                        tx(tr, "extensions.customMcp.actions.add", "Add")
                    };
                    if ui
                        .add_enabled(
                            !self.busy,
                            egui::Button::new(RichText::new(label).strong()).fill(theme::ACCENT),
                        )
                        .clicked()
                    {
                        match self.build() {
                            Ok((name, spec)) => {
                                self.error = None;
                                result = CustomResult::Add { name, scope, spec };
                            }
                            Err(e) => self.error = Some(e),
                        }
                    }
                });
            });
        });
        self.scope = Some(scope);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_each_transport() {
        let mut d = CustomMcpDialog::default();
        assert!(d.build().is_err());
        d.name = "my-server".into();
        assert!(d.build().is_err(), "npm package required");
        d.package = "@x/y".into();
        d.version = "1.2.3".into();
        d.env = vec![("API_KEY".into(), "v".into())];
        let (n, s) = d.build().unwrap();
        assert_eq!(n, "my-server");
        assert_eq!(s.args, ["-y", "@x/y@1.2.3"]);
        assert_eq!(s.env["API_KEY"], "v");
        d.env = vec![("bad key".into(), "v".into())];
        assert!(d.build().is_err());
        d.transport = CustomTransport::Command;
        d.env.clear();
        d.command = "node".into();
        d.args = "server.js --port 1".into();
        assert_eq!(d.build().unwrap().1.args, ["server.js", "--port", "1"]);
        d.transport = CustomTransport::Http;
        assert!(d.build().is_err());
        d.url = "https://a.test/mcp".into();
        d.headers = vec![("Authorization".into(), "Bearer t".into())];
        let s = d.build().unwrap().1;
        assert_eq!(s.headers["Authorization"], "Bearer t");
        d.transport = CustomTransport::Sse;
        assert_eq!(d.build().unwrap().1.transport.id(), "sse");
        d.name = "bad name".into();
        assert!(d.build().is_err());
    }
}
