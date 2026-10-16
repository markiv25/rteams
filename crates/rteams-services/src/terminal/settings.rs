//! Terminal preferences and the per-team tab list (`<home>/<team>/terminal.json`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{atomic, paths::AppPaths};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TabSpec {
    pub id: String,
    pub title: String,
    /// One of the original palette names: slate, sky, blue, cyan, teal, emerald, lime, amber,
    /// orange, rose, violet.
    pub color: String,
    pub cwd: Option<PathBuf>,
    /// `team` runtime or `local` shell.
    pub kind: String,
}

impl Default for TabSpec {
    fn default() -> Self {
        TabSpec {
            id: uuid::Uuid::new_v4().simple().to_string()[..8].to_string(),
            title: String::new(),
            color: "slate".into(),
            cwd: None,
            kind: "local".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalSettings {
    pub schema_version: u32,
    /// `dark` | `light`.
    pub theme: String,
    /// `compact` | `default` | `large`.
    pub font_preset: String,
    pub font_size: f32,
    pub scrollback: usize,
    /// Shell program; `None` = `$SHELL`.
    pub shell: Option<String>,
    /// `transparent` | `solid` | `image`.
    pub bg_mode: String,
    pub opacity: f32,
    pub image_path: String,
    pub image_fit: String,
    pub wrap: bool,
    pub drawer_height: f32,
    pub tabs: Vec<TabSpec>,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        TerminalSettings {
            schema_version: SCHEMA_VERSION,
            theme: "dark".into(),
            font_preset: "default".into(),
            font_size: 13.0,
            scrollback: crate::terminal::session::SCROLLBACK_LINES,
            shell: None,
            bg_mode: "solid".into(),
            opacity: 1.0,
            image_path: String::new(),
            image_fit: "cover".into(),
            wrap: true,
            drawer_height: 320.0,
            tabs: Vec::new(),
        }
    }
}

impl TerminalSettings {
    /// `<home>/<team>/terminal.json`, or `<home>/terminal.json` without a team.
    pub fn path(home: &Path, team: Option<&str>) -> PathBuf {
        let p = AppPaths::new(home);
        match team {
            Some(t) => p.team_file(t, "terminal.json"),
            None => p.app("terminal.json"),
        }
    }

    /// Missing or corrupt files yield defaults.
    pub fn load(home: &Path, team: Option<&str>) -> Self {
        let mut s: TerminalSettings = std::fs::read(Self::path(home, team))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        s.clamp();
        s
    }

    pub fn save(&self, home: &Path, team: Option<&str>) -> std::io::Result<()> {
        let mut s = self.clone();
        s.schema_version = SCHEMA_VERSION;
        let bytes = serde_json::to_vec_pretty(&s).map_err(std::io::Error::other)?;
        atomic::write(&Self::path(home, team), &bytes)
    }

    pub fn clamp(&mut self) {
        self.font_size = self.font_size.clamp(8.0, 32.0);
        self.opacity = self.opacity.clamp(0.1, 1.0);
        self.scrollback = self.scrollback.clamp(100, 100_000);
        self.drawer_height = self.drawer_height.clamp(120.0, 4000.0);
    }

    /// Font size implied by a preset name.
    pub fn preset_size(preset: &str) -> f32 {
        match preset {
            "compact" => 11.0,
            "large" => 16.0,
            _ => 13.0,
        }
    }

    /// Reset everything except the tab list.
    pub fn reset_appearance(&mut self) {
        let tabs = std::mem::take(&mut self.tabs);
        let shell = self.shell.take();
        *self = TerminalSettings {
            tabs,
            shell,
            ..Default::default()
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            TerminalSettings::load(dir.path(), Some("t")),
            TerminalSettings::default()
        );
        let mut s = TerminalSettings {
            font_size: 15.0,
            ..Default::default()
        };
        s.tabs.push(TabSpec {
            title: "build".into(),
            cwd: Some("/tmp".into()),
            ..Default::default()
        });
        s.save(dir.path(), Some("t")).unwrap();
        assert!(dir.path().join("t/terminal.json").exists());
        assert_eq!(TerminalSettings::load(dir.path(), Some("t")), s);
        s.save(dir.path(), None).unwrap();
        assert!(dir.path().join("terminal.json").exists());
    }

    #[test]
    fn corrupt_and_out_of_range() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("terminal.json"),
            r#"{"font_size": 500, "opacity": 0}"#,
        )
        .unwrap();
        let s = TerminalSettings::load(dir.path(), None);
        assert_eq!(s.font_size, 32.0);
        assert_eq!(s.opacity, 0.1);
        std::fs::write(dir.path().join("terminal.json"), "garbage").unwrap();
        assert_eq!(
            TerminalSettings::load(dir.path(), None),
            TerminalSettings::default()
        );
    }
}
