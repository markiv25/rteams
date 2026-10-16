//! Editor tab sessions: open tabs, active tab, scroll positions and unsaved drafts persisted at
//! `<home>/editor/<sha256(root)[..16]>.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::files::sha256_hex;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TabState {
    /// Path relative to the project root, `/` separated.
    pub path: String,
    /// (line, col), 1-based.
    #[serde(default)]
    pub cursor: (usize, usize),
    #[serde(default)]
    pub scroll: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    pub text: String,
    /// Hash of the disk content the draft was based on.
    pub base_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorSession {
    pub schema_version: u32,
    #[serde(default)]
    pub open_tabs: Vec<TabState>,
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default = "yes")]
    pub sidebar_open: bool,
    #[serde(default)]
    pub unsaved_drafts: BTreeMap<String, Draft>,
}

fn yes() -> bool {
    true
}

impl Default for EditorSession {
    fn default() -> Self {
        EditorSession {
            schema_version: SCHEMA_VERSION,
            open_tabs: vec![],
            active: None,
            sidebar_open: true,
            unsaved_drafts: BTreeMap::new(),
        }
    }
}

pub fn session_path(home: &Path, root: &Path) -> PathBuf {
    let h = sha256_hex(root.to_string_lossy().as_bytes());
    home.join("editor").join(format!("{}.json", &h[..16]))
}

/// Load the session for `root`; a missing or corrupt file gives an empty session.
pub fn load(home: &Path, root: &Path) -> EditorSession {
    std::fs::read(session_path(home, root))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save(home: &Path, root: &Path, s: &EditorSession) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(s).map_err(std::io::Error::other)?;
    crate::atomic::write(&session_path(home, root), &bytes)
}

/// How a restored draft relates to the file on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftState {
    /// Disk still matches the draft's base: restore it ("Recovered unsaved changes ...").
    Restore,
    /// Disk changed since: ask the user (conflict dialog).
    Conflict,
}

pub fn classify_draft(d: &Draft, disk_hash: &str) -> DraftState {
    if d.base_hash == disk_hash {
        DraftState::Restore
    } else {
        DraftState::Conflict
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let home = tempfile::tempdir().unwrap();
        let root = Path::new("/some/project");
        assert_eq!(load(home.path(), root), EditorSession::default());
        let mut s = EditorSession::default();
        s.open_tabs.push(TabState {
            path: "src/a.rs".into(),
            cursor: (3, 4),
            scroll: 12.5,
        });
        s.active = Some("src/a.rs".into());
        s.sidebar_open = false;
        s.unsaved_drafts.insert(
            "src/a.rs".into(),
            Draft {
                text: "x".into(),
                base_hash: "h".into(),
            },
        );
        save(home.path(), root, &s).unwrap();
        assert_eq!(load(home.path(), root), s);
        assert_ne!(
            session_path(home.path(), root),
            session_path(home.path(), Path::new("/other"))
        );
        assert_eq!(
            classify_draft(&s.unsaved_drafts["src/a.rs"], "h"),
            DraftState::Restore
        );
        assert_eq!(
            classify_draft(&s.unsaved_drafts["src/a.rs"], "z"),
            DraftState::Conflict
        );
        std::fs::write(session_path(home.path(), root), "garbage").unwrap();
        assert_eq!(load(home.path(), root), EditorSession::default());
    }
}
