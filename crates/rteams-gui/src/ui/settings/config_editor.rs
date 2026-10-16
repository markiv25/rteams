//! Raw JSON view/editor for `config.json` with live validation. Valid edits are applied to the
//! in-memory config (the screen saves it with its debounce); invalid text is kept but not applied.

use eframe::egui::{self, RichText};
use rteams_services::config::AppConfig;
use rteams_services::i18n::I18n;

use crate::theme;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Idle,
    Saved,
    Invalid(String),
}

#[derive(Default)]
pub struct ConfigEditor {
    pub open: bool,
    pub text: String,
    pub status: Status,
}

/// Full pretty JSON of the config (the user's own file, token included).
pub fn to_text(cfg: &AppConfig) -> String {
    serde_json::to_string_pretty(cfg).unwrap_or_default()
}

/// Parse and check a raw config text.
pub fn validate(text: &str) -> Result<AppConfig, String> {
    AppConfig::import(text)
}

impl ConfigEditor {
    pub fn open(&mut self, cfg: &AppConfig) {
        self.text = to_text(cfg);
        self.status = Status::Idle;
        self.open = true;
    }

    /// Apply `text` if valid; returns true when `cfg` changed.
    pub fn apply(&mut self, cfg: &mut AppConfig) -> bool {
        match validate(&self.text) {
            Ok(new) => {
                self.status = Status::Saved;
                if *cfg != new {
                    *cfg = new;
                    return true;
                }
                false
            }
            Err(e) => {
                self.status = Status::Invalid(e);
                false
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, cfg: &mut AppConfig, tr: &I18n) -> bool {
        if !self.open {
            return false;
        }
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr.t("settings.configEditor.title")).strong());
            if ui
                .button(format!(
                    "{} {}",
                    tr.t("settings.configEditor.footer.escapeKey"),
                    tr.t("settings.configEditor.footer.toClose")
                ))
                .clicked()
            {
                self.open = false;
            }
        });
        let resp = egui::ScrollArea::vertical()
            .id_salt("config-editor-scroll")
            .max_height(320.0)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.text)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(14),
                )
            })
            .inner;
        if resp.changed() {
            changed = self.apply(cfg);
        }
        match &self.status {
            Status::Idle => {
                crate::ui::settings::hint(ui, &tr.t("settings.configEditor.footer.autoSave"))
            }
            Status::Saved => {
                ui.colored_label(theme::SUCCESS, tr.t("settings.configEditor.status.saved"));
            }
            Status::Invalid(e) => {
                ui.colored_label(
                    theme::ERROR,
                    format!("{}: {e}", tr.t("settings.configEditor.status.invalidJson")),
                );
            }
        }
        if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.open = false;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_cases() {
        assert!(validate("{nope").is_err());
        assert!(validate("[1]").is_err());
        assert!(validate(r#"{"schema_version":99}"#).is_err());
        assert!(validate(r#"{"general":{"theme":5}}"#).is_err());
        assert!(validate("{}").is_ok());
    }

    #[test]
    fn apply_only_valid_text() {
        let mut cfg = AppConfig::default();
        let mut ed = ConfigEditor::default();
        ed.open(&cfg);
        assert!(!ed.apply(&mut cfg));
        assert_eq!(ed.status, Status::Saved);
        ed.text = ed.text.replace("\"system\"", "\"dark\"");
        assert!(ed.apply(&mut cfg));
        assert_eq!(cfg.general.theme, "dark");
        ed.text.push('x');
        let before = cfg.clone();
        assert!(!ed.apply(&mut cfg));
        assert!(matches!(ed.status, Status::Invalid(_)));
        assert_eq!(cfg, before);
    }
}
