//! `Create team in <org>`: makes a new team on disk, records its placement, then asks the
//! shell to open it.

use std::path::Path;

use eframe::egui::{self, RichText};
use rteams_services::i18n::I18n;

use crate::actions::{Commander, GuiCmd};
use crate::forms::parse_members;
use crate::theme;

pub struct CreateTeamDialog {
    pub org: String,
    pub group: Option<String>,
    /// Shown in the title.
    pub label: String,
    name: String,
    lead: String,
    members: String,
    error: Option<String>,
}

pub enum DialogResult {
    Open,
    Cancelled,
    /// The team was created; the caller records `(org, group)` placement for it.
    Created(String),
}

/// Team directory names are limited to what the store accepts.
pub fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && !s.starts_with('.')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

impl CreateTeamDialog {
    pub fn new(org: String, group: Option<String>, label: String) -> Self {
        CreateTeamDialog {
            org,
            group,
            label,
            name: String::new(),
            lead: "lead".into(),
            members: String::new(),
            error: None,
        }
    }

    fn create(&mut self, home: &Path) -> Result<String, String> {
        let name = self.name.trim().to_string();
        if !valid_name(&name) {
            return Err("Team names may use letters, digits, '.', '_' and '-'.".into());
        }
        if home.join(&name).join("events.jsonl").exists() {
            return Err(format!("Team {name} already exists."));
        }
        let lead = self.lead.trim();
        if lead.is_empty() {
            return Err("A lead name is required.".into());
        }
        Commander::new(home.to_path_buf(), name.clone())
            .run(
                lead,
                &GuiCmd::CreateTeam {
                    name: name.clone(),
                    members: parse_members(&self.members),
                },
            )
            .map(|_| name)
    }

    pub fn show(&mut self, ctx: &egui::Context, home: &Path, tr: &I18n) -> DialogResult {
        let mut result = DialogResult::Open;
        let mut open = true;
        let title = tr.t_args(
            "team.organizations.graph.actions.createTeamIn",
            &[("label", &self.label)],
        );
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(tr.t("team.organizations.editor.teamPlaceholder"));
                ui.text_edit_singleline(&mut self.name);
                ui.label("Lead");
                ui.text_edit_singleline(&mut self.lead);
                ui.label("Members (name [role], one per line)");
                ui.add(egui::TextEdit::multiline(&mut self.members).desired_rows(3));
                if let Some(e) = &self.error {
                    ui.label(RichText::new(e).color(theme::ERROR));
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !self.name.trim().is_empty(),
                            egui::Button::new(tr.t("team.organizations.editor.actions.create")),
                        )
                        .clicked()
                    {
                        match self.create(home) {
                            Ok(n) => result = DialogResult::Created(n),
                            Err(e) => self.error = Some(e),
                        }
                    }
                    if ui.button(tr.t("common.actions.cancel")).clicked() {
                        result = DialogResult::Cancelled;
                    }
                });
            });
        if !open && matches!(result, DialogResult::Open) {
            result = DialogResult::Cancelled;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_creation() {
        assert!(valid_name("a-b_1.x"));
        assert!(!valid_name(".hidden") && !valid_name("a b") && !valid_name(""));
        let home = std::env::temp_dir().join(format!("rteams-gui-orgdlg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let mut d = CreateTeamDialog::new("o".into(), None, "O".into());
        d.name = "bad name".into();
        assert!(d.create(&home).is_err());
        d.name = "newteam".into();
        d.members = "ada dev".into();
        assert_eq!(d.create(&home).unwrap(), "newteam");
        assert!(d.create(&home).unwrap_err().contains("exists"));
        assert!(rteams_services::orgs::projection::team_summary(
            &home,
            "newteam",
            std::time::SystemTime::now()
        )
        .is_some());
        let _ = std::fs::remove_dir_all(&home);
    }
}
