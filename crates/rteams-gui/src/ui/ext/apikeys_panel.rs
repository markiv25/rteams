//! API Keys tab: saved keys (name, environment variable, scope, masked value), add / edit dialog
//! with presets (Anthropic, OpenAI, Gemini, OpenRouter, ...), copy env var name, delete with a
//! second click to confirm. Values live in `<home>/secrets.json` (see `ext::apikeys`).

use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use rteams_services::ext::apikeys::{ApiKeyMeta, Store, PRESETS};

use super::store_tabs::{card, notice_line, tx, txa, ExtCtx};
use crate::theme;

#[derive(Debug, Default, Clone)]
pub struct KeyForm {
    /// `Some` = editing that key (the value must be re-entered).
    pub id: Option<String>,
    pub name: String,
    pub env_key: String,
    pub value: String,
    pub project_bound: bool,
    pub error: Option<String>,
}

pub struct ApiKeysPanel {
    keys: Vec<ApiKeyMeta>,
    dirty: bool,
    pub form: Option<KeyForm>,
    confirm_delete: Option<String>,
    copied: Option<(String, Instant)>,
    notice: Option<(bool, String)>,
}

impl Default for ApiKeysPanel {
    fn default() -> Self {
        ApiKeysPanel {
            keys: Vec::new(),
            dirty: true,
            form: None,
            confirm_delete: None,
            copied: None,
            notice: None,
        }
    }
}

enum Act {
    Edit(ApiKeyMeta),
    Delete(String),
    CopyName(String),
}

impl ApiKeysPanel {
    #[cfg(test)]
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    pub fn show(&mut self, ui: &mut egui::Ui, x: &mut ExtCtx) {
        let tr = x.tr;
        let store = Store::new(x.home);
        if self.dirty {
            self.keys = store.list();
            self.dirty = false;
        }
        ui.label(
            RichText::new(tx(
                tr,
                "extensions.apiKeys.description",
                "Securely store API keys for auto-filling when installing MCP servers.",
            ))
            .color(theme::TEXT_2),
        );
        ui.label(
            RichText::new(txa(
                tr,
                "extensions.apiKeys.storage.osKeychain",
                "Keys are stored in {{backend}} with restricted file permissions (owner-only).",
                &[("backend", &store.backend_label())],
            ))
            .small()
            .color(theme::MUTED),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if theme::solid(
                ui,
                &tx(tr, "extensions.apiKeys.actions.add", "Add API Key"),
                theme::ACCENT,
            )
            .clicked()
            {
                self.form = Some(KeyForm::default());
            }
        });
        notice_line(ui, &self.notice);
        ui.add_space(4.0);

        let mut acts = Vec::new();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if self.keys.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(24.0);
                    ui.label(RichText::new(tx(tr, "extensions.apiKeys.empty.title", "No API keys saved")).size(16.0).strong());
                    ui.label(
                        RichText::new(tx(
                            tr,
                            "extensions.apiKeys.empty.description",
                            "Add keys to auto-fill environment variables when installing MCP servers.",
                        ))
                        .color(theme::MUTED),
                    );
                    if ui.button(tx(tr, "extensions.apiKeys.actions.addFirst", "Add your first key")).clicked() {
                        self.form = Some(KeyForm::default());
                    }
                });
            }
            for k in &self.keys {
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(&k.name).strong());
                                match &k.project {
                                    None => theme::pill(ui, "User", theme::ACCENT),
                                    Some(_) => theme::pill(ui, "Project", theme::INFO),
                                };
                            });
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(&k.env_key).monospace().color(theme::TEXT_2));
                                ui.label(
                                    RichText::new(format!("••••{}", k.last4))
                                        .monospace()
                                        .color(theme::MUTED),
                                );
                            });
                            if let Some(p) = &k.project {
                                ui.label(
                                    RichText::new(txa(tr, "extensions.apiKeys.form.boundTo", "Bound to {{path}}", &[("path", p)]))
                                        .small()
                                        .color(theme::MUTED),
                                );
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if self.confirm_delete.as_deref() == Some(k.id.as_str()) {
                                if theme::solid(ui, &tx(tr, "extensions.apiKeys.actions.confirmDelete", "Click again to confirm"), theme::ERROR).clicked() {
                                    acts.push(Act::Delete(k.id.clone()));
                                }
                            } else if theme::ghost(ui, &tx(tr, "extensions.apiKeys.actions.delete", "Delete")).clicked() {
                                self.confirm_delete = Some(k.id.clone());
                            }
                            if theme::ghost(ui, &tx(tr, "extensions.apiKeys.actions.edit", "Edit")).clicked() {
                                acts.push(Act::Edit(k.clone()));
                            }
                            let just_copied = self
                                .copied
                                .as_ref()
                                .is_some_and(|(id, at)| id == &k.id && at.elapsed() < Duration::from_secs(2));
                            let label = if just_copied {
                                tx(tr, "extensions.apiKeys.actions.copied", "Copied!")
                            } else {
                                tx(tr, "extensions.apiKeys.actions.copyEnvVarName", "Copy env var name")
                            };
                            if theme::ghost(ui, &label).clicked() {
                                acts.push(Act::CopyName(k.id.clone()));
                            }
                        });
                    });
                });
                ui.add_space(4.0);
            }
        });

        for a in acts {
            match a {
                Act::Edit(k) => {
                    self.form = Some(KeyForm {
                        id: Some(k.id),
                        name: k.name,
                        env_key: k.env_key,
                        value: String::new(),
                        project_bound: k.project.is_some(),
                        error: None,
                    })
                }
                Act::Delete(id) => {
                    self.confirm_delete = None;
                    match store.delete(&id) {
                        Ok(_) => {
                            self.dirty = true;
                            self.notice = Some((true, "API key deleted".into()));
                        }
                        Err(e) => self.notice = Some((false, e)),
                    }
                }
                Act::CopyName(id) => {
                    if let Some(k) = self.keys.iter().find(|k| k.id == id) {
                        ui.ctx().copy_text(k.env_key.clone());
                        self.copied = Some((id, Instant::now()));
                        ui.ctx().request_repaint_after(Duration::from_millis(2100));
                    }
                }
            }
        }
        self.dialog(ui.ctx(), x, &store);
    }

    /// Save the open form; returns the error text on failure.
    pub fn save_form(
        &mut self,
        store: &Store,
        project: Option<&std::path::Path>,
    ) -> Result<(), String> {
        let Some(f) = self.form.clone() else {
            return Ok(());
        };
        let proj = if f.project_bound {
            Some(
                project
                    .ok_or_else(|| "Project-scoped API keys require an active project".to_string())?
                    .to_string_lossy()
                    .into_owned(),
            )
        } else {
            None
        };
        match &f.id {
            Some(id) => store.update(id, &f.name, &f.env_key, &f.value, proj.as_deref())?,
            None => store.add(&f.name, &f.env_key, &f.value, proj.as_deref())?,
        };
        self.dirty = true;
        self.form = None;
        Ok(())
    }

    fn dialog(&mut self, ctx: &egui::Context, x: &mut ExtCtx, store: &Store) {
        let tr = x.tr;
        let Some(mut f) = self.form.take() else {
            return;
        };
        let editing = f.id.is_some();
        let project = x.project().map(|p| p.to_path_buf());
        let (mut cancel, mut save) = (false, false);
        egui::Window::new(if editing {
            tx(tr, "extensions.apiKeys.form.editTitle", "Edit API Key")
        } else {
            tx(tr, "extensions.apiKeys.form.addTitle", "Add API Key")
        })
        .collapsible(false)
        .default_width(460.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(
                RichText::new(if editing {
                    tx(
                        tr,
                        "extensions.apiKeys.form.editDescription",
                        "Update the key details. You must re-enter the value.",
                    )
                } else {
                    tx(
                        tr,
                        "extensions.apiKeys.form.addDescription",
                        "Store an API key for auto-filling in MCP server installations.",
                    )
                })
                .color(theme::TEXT_2),
            );
            ui.add_space(6.0);
            if !editing {
                ui.horizontal_wrapped(|ui| {
                    for (label, env) in PRESETS {
                        if ui.small_button(label).clicked() {
                            f.env_key = env.to_string();
                            if f.name.trim().is_empty() {
                                f.name = label.to_string();
                            }
                        }
                    }
                });
            }
            ui.label(tx(tr, "extensions.apiKeys.form.name", "Name"));
            ui.add(
                egui::TextEdit::singleline(&mut f.name)
                    .hint_text(tx(
                        tr,
                        "extensions.apiKeys.form.namePlaceholder",
                        "e.g. OpenAI Production",
                    ))
                    .desired_width(380.0),
            );
            ui.label(tx(
                tr,
                "extensions.apiKeys.form.environmentVariableName",
                "Environment Variable Name",
            ));
            ui.add(
                egui::TextEdit::singleline(&mut f.env_key)
                    .hint_text(tx(
                        tr,
                        "extensions.apiKeys.form.envVarPlaceholder",
                        "e.g. OPENAI_API_KEY",
                    ))
                    .desired_width(380.0),
            );
            ui.label(tx(tr, "extensions.apiKeys.form.value", "Value"));
            ui.add(
                egui::TextEdit::singleline(&mut f.value)
                    .password(true)
                    .hint_text(if editing {
                        tx(
                            tr,
                            "extensions.apiKeys.form.reenterValue",
                            "Re-enter key value",
                        )
                    } else {
                        tx(tr, "extensions.apiKeys.form.valuePlaceholder", "sk-...")
                    })
                    .desired_width(380.0),
            );
            ui.label(tx(tr, "extensions.apiKeys.form.scope", "Scope"));
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut f.project_bound,
                    false,
                    tx(
                        tr,
                        "extensions.apiKeys.form.userScopeLabel",
                        "User (global)",
                    ),
                );
                let label = match &project {
                    Some(p) => txa(
                        tr,
                        "extensions.apiKeys.form.projectScopeLabel",
                        "Project: {{project}}",
                        &[(
                            "project",
                            &p.file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                        )],
                    ),
                    None => tx(
                        tr,
                        "extensions.apiKeys.form.projectUnavailable",
                        "Project unavailable",
                    ),
                };
                ui.add_enabled_ui(project.is_some(), |ui| {
                    ui.selectable_value(&mut f.project_bound, true, label);
                });
            });
            if let Some(e) = &f.error {
                ui.colored_label(theme::ERROR, e);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if theme::ghost(ui, &tx(tr, "extensions.apiKeys.form.cancel", "Cancel")).clicked() {
                    cancel = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if editing {
                        tx(tr, "extensions.apiKeys.form.update", "Update")
                    } else {
                        tx(tr, "extensions.apiKeys.form.save", "Save")
                    };
                    if ui
                        .add(egui::Button::new(RichText::new(label).strong()).fill(theme::ACCENT))
                        .clicked()
                    {
                        save = true;
                    }
                });
            });
        });
        if cancel {
            return;
        }
        self.form = Some(f);
        if save {
            if let Err(e) = self.save_form(store, project.as_deref()) {
                if let Some(f) = &mut self.form {
                    f.error = Some(e);
                }
            } else {
                self.notice = Some((true, "API key saved".into()));
            }
        }
    }

    /// Number of saved keys (test helper).
    #[cfg(test)]
    fn count(&self) -> usize {
        self.keys.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::ShellRequest;
    use crate::view::testing::render;
    use rteams_services::ext::mcp_config::ClientPaths;
    use rteams_services::i18n::I18n;

    fn frames(p: &mut ApiKeysPanel, home: &std::path::Path) {
        let tr = I18n::default();
        let paths = ClientPaths::new(home.join("u"), None);
        let mut req: Vec<ShellRequest> = Vec::new();
        render(|ui| {
            let mut x = ExtCtx {
                home,
                paths: &paths,
                offline: true,
                tr: &tr,
                req: &mut req,
            };
            p.show(ui, &mut x);
        });
    }

    #[test]
    fn add_edit_delete_flow() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        let mut p = ApiKeysPanel::default();
        frames(&mut p, d.path());
        assert_eq!(p.count(), 0);

        p.form = Some(KeyForm {
            name: "OpenAI".into(),
            env_key: "OPENAI_API_KEY".into(),
            value: "sk-abcd1234".into(),
            ..Default::default()
        });
        frames(&mut p, d.path()); // dialog renders
        p.save_form(&store, None).unwrap();
        frames(&mut p, d.path());
        assert_eq!(p.count(), 1);
        assert_eq!(p.keys[0].last4, "1234");
        assert_eq!(
            store.env_for("codex", None),
            [("OPENAI_API_KEY".to_string(), "sk-abcd1234".to_string())]
        );

        // Edit requires a new value.
        let k = p.keys[0].clone();
        p.form = Some(KeyForm {
            id: Some(k.id.clone()),
            name: "OpenAI".into(),
            env_key: "OPENAI_API_KEY".into(),
            ..Default::default()
        });
        assert!(p.save_form(&store, None).is_err());
        p.form.as_mut().unwrap().value = "sk-new-9999".into();
        p.save_form(&store, None).unwrap();
        frames(&mut p, d.path());
        assert_eq!(p.keys[0].last4, "9999");

        // Project scope needs a project.
        p.form = Some(KeyForm {
            name: "p".into(),
            env_key: "P_KEY".into(),
            value: "v".into(),
            project_bound: true,
            ..Default::default()
        });
        assert!(p
            .save_form(&store, None)
            .unwrap_err()
            .contains("active project"));
        p.save_form(&store, Some(std::path::Path::new("/work/proj")))
            .unwrap();
        frames(&mut p, d.path());
        assert_eq!(p.count(), 2);

        store.delete(&k.id).unwrap();
        p.invalidate();
        frames(&mut p, d.path());
        assert_eq!(p.count(), 1);
    }
}
