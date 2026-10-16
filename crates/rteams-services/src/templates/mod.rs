//! Team templates (original `team-templates`): eight starter teams embedded from `data.json`.

use serde::{Deserialize, Serialize};

use crate::team_import::{DraftMember, TeamDraft};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateMember {
    pub name: String,
    pub role: String,
    pub workflow: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamTemplate {
    #[serde(default)]
    pub schema_version: u32,
    pub id: String,
    pub version: u32,
    pub name: String,
    pub description: String,
    pub team_prompt: String,
    pub members: Vec<TemplateMember>,
}

const DATA: &str = include_str!("data.json");

/// All templates in the original's order.
pub fn all() -> Vec<TeamTemplate> {
    serde_json::from_str(DATA).expect("templates/data.json is valid")
}

pub fn get(id: &str) -> Option<TeamTemplate> {
    all().into_iter().find(|t| t.id == id)
}

impl TeamTemplate {
    /// i18n key of the template's display name.
    pub fn name_key(&self) -> String {
        format!("team.templates.names.{}", self.id)
    }

    pub fn description_key(&self) -> String {
        format!("team.templates.descriptions.{}", self.id)
    }

    /// An independent editable draft: the name stays user-chosen, `lead_prompt` is the team
    /// prompt, each member's standing prompt is its workflow.
    pub fn to_draft(
        &self,
        team_name: &str,
        project_path: Option<String>,
        runtime: &str,
    ) -> TeamDraft {
        TeamDraft {
            team_name: team_name.trim().to_string(),
            lead_prompt: self.team_prompt.clone(),
            project_path,
            members: self
                .members
                .iter()
                .map(|m| DraftMember {
                    name: m.name.clone(),
                    role: m.role.clone(),
                    runtime: runtime.to_string(),
                    prompt: m.workflow.clone(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_parses_with_eight_complete_templates() {
        let t = all();
        assert_eq!(t.len(), 8);
        assert_eq!(t[0].id, "software-product");
        assert_eq!(t[0].name, "Software Product Team");
        assert_eq!(t[0].members.len(), 3);
        for t in &t {
            assert!(
                !t.team_prompt.is_empty() && !t.members.is_empty(),
                "{}",
                t.id
            );
            assert!(t
                .members
                .iter()
                .all(|m| !m.name.is_empty() && !m.workflow.is_empty()));
        }
        assert!(get("learning").is_some() && get("nope").is_none());
    }

    #[test]
    fn draft_copies_roster() {
        let d = get("software-product")
            .unwrap()
            .to_draft(" my-team ", Some("/p".into()), "claude");
        assert_eq!(d.team_name, "my-team");
        assert_eq!(d.members[1].role, "Software Engineer");
        assert_eq!(d.members[1].runtime, "claude");
        assert!(d.lead_prompt.starts_with("Coordinate"));
    }
}
