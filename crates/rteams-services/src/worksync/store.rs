//! Per-member sync record at `<home>/<team>/work-sync/<member>.json`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberSync {
    #[serde(default)]
    pub schema_version: u32,
    /// Fingerprint last observed (empty = no agenda).
    #[serde(default)]
    pub fingerprint: String,
    /// When `fingerprint` was first observed.
    #[serde(default)]
    pub first_seen_at: i64,
    /// Fingerprint of the last nudge (for the panel; dedup uses `attempts`).
    #[serde(default)]
    pub last_nudged_fingerprint: String,
    #[serde(default)]
    pub last_nudge_at: Option<i64>,
    /// Nudges sent for the current fingerprint.
    #[serde(default)]
    pub attempts: u32,
    /// Last tick at which the member was seen running (approximate end of its last run).
    #[serde(default)]
    pub last_running_at: Option<i64>,
    #[serde(default)]
    pub auto_resume_stopped: bool,
}

pub fn dir(home: &Path, team: &str) -> PathBuf {
    home.join(team).join("work-sync")
}

fn file(home: &Path, team: &str, member: &str) -> PathBuf {
    let safe: String = member
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir(home, team).join(format!("{safe}.json"))
}

pub fn load(home: &Path, team: &str, member: &str) -> MemberSync {
    std::fs::read(file(home, team, member))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save(home: &Path, team: &str, member: &str, s: &MemberSync) -> std::io::Result<()> {
    let mut s = s.clone();
    s.schema_version = 1;
    let bytes = serde_json::to_vec_pretty(&s).map_err(std::io::Error::other)?;
    crate::atomic::write(&file(home, team, member), &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_missing() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(load(d.path(), "t", "ada"), MemberSync::default());
        let s = MemberSync {
            fingerprint: "ab".into(),
            attempts: 2,
            ..Default::default()
        };
        save(d.path(), "t", "ada", &s).unwrap();
        let back = load(d.path(), "t", "ada");
        assert_eq!(back.attempts, 2);
        assert_eq!(back.schema_version, 1);
    }
}
