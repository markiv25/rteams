//! CLI Runtime: one row per agent runtime with detection status, a binary path override and a
//! default model. Overrides live in `AppConfig.extra["runtimes"][<id>] = {bin, model}`.

use std::sync::mpsc::{self, Receiver};

use eframe::egui::{self, RichText};
use rteams_services::config::AppConfig;
use rteams_services::i18n::I18n;
use serde_json::{json, Value};

use super::{card, hint};
use crate::runtimes::{self, AuthStatus, Detection, ModelInfo};
use crate::theme;

#[derive(Debug, Clone)]
pub struct Row {
    pub id: String,
    pub label: String,
    pub detection: Detection,
    pub auth: AuthStatus,
    pub models: Vec<ModelInfo>,
    pub install_hint: String,
}

pub struct State {
    rows: Option<Vec<Row>>,
    rx: Option<Receiver<Vec<Row>>>,
    auto: bool,
}

impl State {
    pub fn new() -> Self {
        State {
            rows: None,
            rx: None,
            auto: true,
        }
    }

    /// Pre-filled and never probing real CLIs (tests).
    #[cfg(test)]
    pub fn with_rows(rows: Vec<Row>) -> Self {
        State {
            rows: Some(rows),
            rx: None,
            auto: false,
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let rows = runtimes::all()
                .iter()
                .map(|r| Row {
                    id: r.id().into(),
                    label: r.label().into(),
                    detection: r.detect(),
                    auth: r.auth_status(),
                    models: r.models(),
                    install_hint: r.install_hint().into(),
                })
                .collect();
            let _ = tx.send(rows);
            ctx.request_repaint();
        });
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

pub fn get(cfg: &AppConfig, id: &str, field: &str) -> String {
    cfg.extra
        .get("runtimes")
        .and_then(|r| r.get(id))
        .and_then(|r| r.get(field))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// Set (or clear when empty) `runtimes.<id>.<field>`.
pub fn set(cfg: &mut AppConfig, id: &str, field: &str, value: &str) {
    let runtimes = cfg.extra.entry("runtimes").or_insert_with(|| json!({}));
    if !runtimes.is_object() {
        *runtimes = json!({});
    }
    let entry = runtimes
        .as_object_mut()
        .unwrap()
        .entry(id)
        .or_insert_with(|| json!({}));
    if let Some(o) = entry.as_object_mut() {
        if value.is_empty() {
            o.remove(field);
        } else {
            o.insert(field.into(), json!(value));
        }
    }
}

pub fn show(ui: &mut egui::Ui, cfg: &mut AppConfig, st: &mut State, tr: &I18n) -> bool {
    let mut changed = false;
    if let Some(rx) = &st.rx {
        if let Ok(rows) = rx.try_recv() {
            st.rows = Some(rows);
            st.rx = None;
        }
    }
    if st.rows.is_none() && st.rx.is_none() && st.auto {
        st.start(ui.ctx());
    }
    ui.horizontal(|ui| {
        ui.heading(tr.t("settings.cliRuntime.title"));
        let busy = st.rx.is_some();
        if ui
            .add_enabled(
                !busy,
                egui::Button::new(tr.t("settings.cliRuntime.actions.recheck")),
            )
            .clicked()
        {
            st.rows = None;
            st.start(ui.ctx());
        }
    });
    let Some(rows) = st.rows.clone() else {
        ui.label(tr.t("settings.cliRuntime.loading.aiProviders"));
        return false;
    };
    for r in rows {
        card(ui, &r.label, |ui| {
            match (&r.detection.path, &r.detection.version) {
                (Some(p), v) => {
                    let ver = v.clone().unwrap_or_default();
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            theme::SUCCESS,
                            tr.t_args(
                                "settings.cliRuntime.installer.installed",
                                &[("version", &ver)],
                            ),
                        );
                        ui.monospace(p.display().to_string());
                    });
                }
                (None, _) => {
                    ui.colored_label(
                        theme::WARNING,
                        tr.t_args(
                            "settings.cliRuntime.status.notInstalled",
                            &[("runtime", &r.label)],
                        ),
                    );
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&r.install_hint).monospace());
                        if ui.small_button("Copy").clicked() {
                            ui.ctx().copy_text(r.install_hint.clone());
                        }
                    });
                }
            }
            let auth = match &r.auth {
                AuthStatus::SignedIn(Some(who)) => format!(
                    "{} ({who})",
                    tr.t("settings.cliRuntime.providerTerminal.login")
                ),
                AuthStatus::SignedIn(None) => tr.t("settings.cliRuntime.providerTerminal.login"),
                AuthStatus::SignedOut => tr.t("settings.cliRuntime.providerTerminal.logout"),
                AuthStatus::Unknown => "?".into(),
            };
            hint(ui, &auth);

            let mut bin = get(cfg, &r.id, "bin");
            ui.horizontal(|ui| {
                ui.label("Binary");
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut bin)
                            .desired_width(280.0)
                            .hint_text("auto"),
                    )
                    .changed()
                {
                    set(cfg, &r.id, "bin", bin.trim());
                    changed = true;
                }
                if ui.small_button("...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_file() {
                        set(cfg, &r.id, "bin", &p.to_string_lossy());
                        changed = true;
                    }
                }
            });
            if !bin.trim().is_empty() && !std::path::Path::new(bin.trim()).is_file() {
                ui.colored_label(
                    theme::WARNING,
                    tr.t_args(
                        "settings.cliRuntime.status.configuredNotFound",
                        &[("runtime", &r.label)],
                    ),
                );
            }

            let cur = get(cfg, &r.id, "model");
            ui.horizontal(|ui| {
                ui.label("Model");
                let shown = if cur.is_empty() {
                    "default".to_string()
                } else {
                    cur.clone()
                };
                egui::ComboBox::from_id_salt(("cli-model", &r.id))
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(cur.is_empty(), "default").clicked() {
                            set(cfg, &r.id, "model", "");
                            changed = true;
                        }
                        for m in &r.models {
                            if ui.selectable_label(cur == m.id, &m.label).clicked() {
                                set(cfg, &r.id, "model", &m.id);
                                changed = true;
                            }
                        }
                    });
            });
        });
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_set_get_clear() {
        let mut c = AppConfig::default();
        assert_eq!(get(&c, "claude", "bin"), "");
        set(&mut c, "claude", "bin", "/x/claude");
        set(&mut c, "claude", "model", "opus");
        assert_eq!(get(&c, "claude", "bin"), "/x/claude");
        set(&mut c, "claude", "bin", "");
        assert_eq!(get(&c, "claude", "bin"), "");
        assert_eq!(get(&c, "claude", "model"), "opus");
    }

    #[test]
    fn renders_prefilled_rows() {
        use crate::view::testing::render;
        let rows = vec![
            Row {
                id: "claude".into(),
                label: "Claude Code".into(),
                detection: Detection {
                    path: Some("/b/claude".into()),
                    version: Some("1.0".into()),
                },
                auth: AuthStatus::SignedIn(None),
                models: vec![ModelInfo {
                    id: "opus".into(),
                    label: "Opus".into(),
                }],
                install_hint: "npm i -g x".into(),
            },
            Row {
                id: "codex".into(),
                label: "Codex".into(),
                detection: Detection::default(),
                auth: AuthStatus::Unknown,
                models: vec![],
                install_hint: "npm i -g y".into(),
            },
        ];
        let mut st = State::with_rows(rows);
        let mut cfg = AppConfig::default();
        let tr = I18n::english();
        render(|ui| {
            assert!(!show(ui, &mut cfg, &mut st, &tr));
        });
    }
}
