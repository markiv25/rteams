//! Persisted organization map: `<home>/organizations/map.json`. Unknown fields at every level
//! are kept on load and written back, so newer files survive an older build.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::atomic;

pub const SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_COLOR: &str = "#818cf8";

type Extra = BTreeMap<String, Value>;

fn schema() -> u32 {
    SCHEMA_VERSION
}
fn color() -> String {
    DEFAULT_COLOR.into()
}

/// A department / squad inside an organization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    #[serde(default = "color")]
    pub color: String,
    #[serde(default)]
    pub description: String,
    /// Team directory names.
    #[serde(default)]
    pub teams: Vec<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Org {
    pub id: String,
    pub name: String,
    #[serde(default = "color")]
    pub color: String,
    #[serde(default)]
    pub description: String,
    /// Parent organization id; `None` = top level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default)]
    pub groups: Vec<Group>,
    /// Teams placed directly in the organization (not in a group).
    #[serde(default)]
    pub teams: Vec<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    DependsOn,
    Delegates,
    Observes,
    /// Reporting line between team leads: `source` reports to `target`. Must stay acyclic.
    ReportsTo,
}

impl RelationKind {
    pub const ALL: [RelationKind; 4] = [
        RelationKind::DependsOn,
        RelationKind::Delegates,
        RelationKind::Observes,
        RelationKind::ReportsTo,
    ];
    pub fn label(self) -> &'static str {
        match self {
            RelationKind::DependsOn => "Depends on",
            RelationKind::Delegates => "Delegates",
            RelationKind::Observes => "Observes",
            RelationKind::ReportsTo => "Reports to",
        }
    }
}

/// A manual relation between two teams.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    pub id: String,
    pub source: String,
    pub target: String,
    pub kind: RelationKind,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrgMap {
    #[serde(default = "schema")]
    pub schema_version: u32,
    #[serde(default)]
    pub orgs: Vec<Org>,
    #[serde(default)]
    pub relations: Vec<Relation>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for OrgMap {
    fn default() -> Self {
        OrgMap {
            schema_version: SCHEMA_VERSION,
            orgs: Vec::new(),
            relations: Vec::new(),
            extra: Extra::new(),
        }
    }
}

pub fn map_path(home: &Path) -> PathBuf {
    home.join("organizations").join("map.json")
}

impl OrgMap {
    /// Missing or unreadable file = an empty map.
    pub fn load(home: &Path) -> OrgMap {
        std::fs::read(map_path(home))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, home: &Path) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        atomic::write(&map_path(home), &bytes)
    }

    pub fn org(&self, id: &str) -> Option<&Org> {
        self.orgs.iter().find(|o| o.id == id)
    }

    /// Where `team` sits: `(org id, group id)`.
    pub fn placement(&self, team: &str) -> Option<(&str, Option<&str>)> {
        for o in &self.orgs {
            if o.teams.iter().any(|t| t == team) {
                return Some((&o.id, None));
            }
            for g in &o.groups {
                if g.teams.iter().any(|t| t == team) {
                    return Some((&o.id, Some(&g.id)));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_unknown_fields() {
        let home = tempfile::tempdir().unwrap();
        let raw = serde_json::json!({
            "schema_version": 1, "future": {"a": 1},
            "orgs": [{"id": "o", "name": "O", "mystery": [1, 2],
                      "groups": [{"id": "g", "name": "G", "teams": ["t"], "x": true}]}],
            "relations": [{"id": "r", "source": "a", "target": "b", "kind": "reports_to", "w": 3}]
        });
        std::fs::create_dir_all(home.path().join("organizations")).unwrap();
        std::fs::write(map_path(home.path()), raw.to_string()).unwrap();
        let m = OrgMap::load(home.path());
        assert_eq!(m.orgs[0].color, DEFAULT_COLOR);
        assert_eq!(m.placement("t"), Some(("o", Some("g"))));
        m.save(home.path()).unwrap();
        let back: Value =
            serde_json::from_slice(&std::fs::read(map_path(home.path())).unwrap()).unwrap();
        assert_eq!(back["future"], raw["future"]);
        assert_eq!(back["orgs"][0]["mystery"], raw["orgs"][0]["mystery"]);
        assert_eq!(back["orgs"][0]["groups"][0]["x"], true);
        assert_eq!(back["relations"][0]["w"], 3);
        assert_eq!(back["schema_version"], 1);
        assert_eq!(OrgMap::load(home.path()), m);
    }

    #[test]
    fn missing_or_corrupt_is_empty() {
        let home = tempfile::tempdir().unwrap();
        assert!(OrgMap::load(home.path()).orgs.is_empty());
        std::fs::create_dir_all(home.path().join("organizations")).unwrap();
        std::fs::write(map_path(home.path()), "{not json").unwrap();
        assert!(OrgMap::load(home.path()).orgs.is_empty());
    }
}
