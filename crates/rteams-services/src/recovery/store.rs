//! Recovery jobs on disk: `<home>/<team>/recovery.json`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Waiting for `next_at`.
    Pending,
    /// The wake order was handed out; waits for the outcome.
    Dispatched,
    /// Gave up; the user has to act.
    Manual,
    /// The member succeeded afterwards.
    Recovered,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub member: String,
    /// `retry_transient` | `retry_at_reset` | a manual-reason text.
    pub reason: String,
    /// Retries already dispatched.
    pub attempt: u32,
    pub next_at: i64,
    pub expires_at: i64,
    pub state: JobState,
    #[serde(default)]
    pub class: Option<super::classify::FailureClass>,
    #[serde(default)]
    pub detail: String,
}

impl Job {
    pub fn is_open(&self) -> bool {
        matches!(self.state, JobState::Pending | JobState::Dispatched)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Jobs {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub jobs: Vec<Job>,
}

fn path(home: &Path, team: &str) -> PathBuf {
    home.join(team).join("recovery.json")
}

pub fn load(home: &Path, team: &str) -> Jobs {
    std::fs::read(path(home, team))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save(home: &Path, team: &str, jobs: &Jobs) -> std::io::Result<()> {
    let mut j = jobs.clone();
    j.schema_version = 1;
    let bytes = serde_json::to_vec_pretty(&j).map_err(std::io::Error::other)?;
    crate::atomic::write(&path(home, team), &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_missing() {
        let d = tempfile::tempdir().unwrap();
        assert!(load(d.path(), "t").jobs.is_empty());
        let j = Jobs {
            schema_version: 0,
            jobs: vec![Job {
                id: "a".into(),
                member: "bob".into(),
                reason: "retry_transient".into(),
                attempt: 1,
                next_at: 5,
                expires_at: 9,
                state: JobState::Pending,
                class: None,
                detail: String::new(),
            }],
        };
        save(d.path(), "t", &j).unwrap();
        let l = load(d.path(), "t");
        assert_eq!(l.jobs, j.jobs);
        assert_eq!(l.schema_version, 1);
    }
}
