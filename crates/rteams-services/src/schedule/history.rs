//! Run history of schedule fires: `<home>/<team>/schedule-runs/<schedule_id>.jsonl`. Each line
//! is a full snapshot of one run; the newest line per `run_id` wins, so a run is updated by
//! appending its new state.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    WarmingUp,
    Warm,
    Running,
    Completed,
    Failed,
    Interrupted,
    Cancelled,
}

impl RunStatus {
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Pending | Self::WarmingUp | Self::Warm | Self::Running
        )
    }

    /// i18n key of the status label.
    pub fn key(self) -> &'static str {
        match self {
            Self::Pending => "team.schedule.runStatus.pending",
            Self::WarmingUp => "team.schedule.runStatus.warmingUp",
            Self::Warm => "team.schedule.runStatus.warm",
            Self::Running => "team.schedule.runStatus.running",
            Self::Completed => "team.schedule.runStatus.completed",
            Self::Failed => "team.schedule.runStatus.failed",
            Self::Interrupted => "team.schedule.runStatus.interrupted",
            Self::Cancelled => "team.schedule.runStatus.cancelled",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::WarmingUp => "Warming up",
            Self::Warm => "Warm",
            Self::Running => "Running",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Interrupted => "Interrupted",
            Self::Cancelled => "Cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: String,
    pub schedule_id: String,
    #[serde(default)]
    pub member: String,
    pub scheduled_for: i64,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub ended_at: Option<i64>,
    pub status: RunStatus,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub retry: u32,
    /// File name under `<home>/<team>/runs/<member>/` holding the agent output.
    #[serde(default)]
    pub run_file: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

impl RunRecord {
    pub fn new(schedule_id: &str, member: &str, scheduled_for: i64) -> Self {
        RunRecord {
            run_id: format!("{schedule_id}-{scheduled_for}"),
            schedule_id: schedule_id.to_string(),
            member: member.to_string(),
            scheduled_for,
            started_at: None,
            ended_at: None,
            status: RunStatus::Pending,
            exit_code: None,
            retry: 0,
            run_file: None,
            error: None,
        }
    }
}

pub fn dir(home: &Path, team: &str) -> PathBuf {
    home.join(team).join("schedule-runs")
}

/// Schedule ids become file names; keep them path-safe.
fn file_for(home: &Path, team: &str, schedule_id: &str) -> PathBuf {
    let safe: String = schedule_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir(home, team).join(format!("{safe}.jsonl"))
}

/// Append a snapshot of `rec`.
pub fn append(home: &Path, team: &str, rec: &RunRecord) -> std::io::Result<()> {
    let path = file_for(home, team, &rec.schedule_id);
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    let mut line = serde_json::to_string(rec).map_err(std::io::Error::other)?;
    line.push('\n');
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())
}

/// Runs of one schedule, oldest first (torn or foreign lines skipped).
pub fn read(home: &Path, team: &str, schedule_id: &str) -> Vec<RunRecord> {
    let raw = std::fs::read_to_string(file_for(home, team, schedule_id)).unwrap_or_default();
    let mut by_id: HashMap<String, RunRecord> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for rec in raw
        .lines()
        .filter_map(|l| serde_json::from_str::<RunRecord>(l).ok())
    {
        if !by_id.contains_key(&rec.run_id) {
            order.push(rec.run_id.clone());
        }
        by_id.insert(rec.run_id.clone(), rec);
    }
    let mut v: Vec<RunRecord> = order
        .into_iter()
        .filter_map(|id| by_id.remove(&id))
        .collect();
    v.sort_by_key(|r| r.scheduled_for);
    v
}

/// Every run of every schedule of the team, newest first.
pub fn read_all(home: &Path, team: &str) -> Vec<RunRecord> {
    let mut all = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir(home, team)) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "jsonl") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    all.extend(read(home, team, stem));
                }
            }
        }
    }
    all.sort_by_key(|r| std::cmp::Reverse(r.scheduled_for));
    all
}

/// Latest `scheduled_for` per schedule id: the input `runner::due` needs.
pub fn last_scheduled(home: &Path, team: &str) -> HashMap<String, i64> {
    let mut m: HashMap<String, i64> = HashMap::new();
    for r in read_all(home, team) {
        let e = m.entry(r.schedule_id).or_insert(i64::MIN);
        *e = (*e).max(r.scheduled_for);
    }
    m
}

/// Mark runs still marked active (the app died mid-run) as interrupted. Returns how many.
pub fn interrupt_stale(home: &Path, team: &str, now_ms: i64) -> usize {
    let mut n = 0;
    for mut r in read_all(home, team)
        .into_iter()
        .filter(|r| r.status.is_active())
    {
        r.status = RunStatus::Interrupted;
        r.ended_at = Some(now_ms);
        if append(home, team, &r).is_ok() {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_fold_by_run_id() {
        let d = tempfile::tempdir().unwrap();
        let mut r = RunRecord::new("s1", "bob", 1000);
        append(d.path(), "t", &r).unwrap();
        r.status = RunStatus::Completed;
        r.exit_code = Some(0);
        append(d.path(), "t", &r).unwrap();
        append(d.path(), "t", &RunRecord::new("s1", "bob", 2000)).unwrap();
        let v = read(d.path(), "t", "s1");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].status, RunStatus::Completed);
        assert_eq!(last_scheduled(d.path(), "t")["s1"], 2000);
    }

    #[test]
    fn stale_runs_become_interrupted() {
        let d = tempfile::tempdir().unwrap();
        append(d.path(), "t", &RunRecord::new("s", "bob", 5)).unwrap();
        assert_eq!(interrupt_stale(d.path(), "t", 99), 1);
        let v = read(d.path(), "t", "s");
        assert_eq!(v[0].status, RunStatus::Interrupted);
        assert_eq!(interrupt_stale(d.path(), "t", 100), 0);
    }

    #[test]
    fn torn_line_is_skipped() {
        let d = tempfile::tempdir().unwrap();
        append(d.path(), "t", &RunRecord::new("s", "bob", 5)).unwrap();
        let p = file_for(d.path(), "t", "s");
        let mut f = OpenOptions::new().append(true).open(p).unwrap();
        f.write_all(b"{\"run_id\":\"x").unwrap();
        assert_eq!(read(d.path(), "t", "s").len(), 1);
    }
}
