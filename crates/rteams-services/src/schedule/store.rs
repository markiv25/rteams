//! Small per-team scheduler state: which warm-ups already happened, so a 15 s tick does not
//! probe the runtime twice for the same fire. `<home>/<team>/schedule-runs/state.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchedulerState {
    #[serde(default)]
    pub schema_version: u32,
    /// Schedule id -> `scheduled_for` of the last fire that was warmed up.
    #[serde(default)]
    pub warmed: BTreeMap<String, i64>,
}

fn path(home: &Path, team: &str) -> PathBuf {
    super::history::dir(home, team).join("state.json")
}

pub fn load(home: &Path, team: &str) -> SchedulerState {
    std::fs::read(path(home, team))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save(home: &Path, team: &str, st: &SchedulerState) -> std::io::Result<()> {
    let mut st = st.clone();
    st.schema_version = 1;
    let bytes = serde_json::to_vec_pretty(&st).map_err(std::io::Error::other)?;
    crate::atomic::write(&path(home, team), &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_missing() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(load(d.path(), "t"), SchedulerState::default());
        let mut s = SchedulerState::default();
        s.warmed.insert("a".into(), 5);
        save(d.path(), "t", &s).unwrap();
        assert_eq!(load(d.path(), "t").warmed["a"], 5);
    }
}
