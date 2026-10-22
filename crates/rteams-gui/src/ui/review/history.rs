//! Draft history: every applied review mutation (reject hunk/file, edited file save) is
//! recorded under `<home>/<team>/review-history/<task>/` with the file's content from before,
//! so it can be previewed and restored.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::git;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    RejectHunk,
    RejectFile,
    Edit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    /// Unix seconds.
    pub ts: u64,
    pub task: String,
    pub file: String,
    pub kind: Kind,
    /// Whether the file existed before the mutation.
    pub existed_before: bool,
    /// File content before the mutation (empty when it did not exist).
    pub before: String,
    /// Hash of the file right after the mutation (`None` when it was removed).
    pub after_hash: Option<u64>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RestoreError {
    /// The file changed since the recorded mutation; restoring needs `force`.
    Changed,
    Other(String),
}

fn safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn dir(home: &Path, team: &str, task: &str) -> PathBuf {
    home.join(team).join("review-history").join(safe(task))
}

fn next_id() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{nanos:020}-{}", N.fetch_add(1, Ordering::Relaxed))
}

/// Snapshot taken just before a mutation.
pub struct Before {
    pub existed: bool,
    pub text: String,
}

impl Before {
    /// `None` when the file exists but is not UTF-8 text (not recordable).
    pub fn take(repo: &Path, file: &str) -> Option<Before> {
        match git::read_text(repo, file) {
            Some(text) => Some(Before {
                existed: true,
                text,
            }),
            None if git::content_hash(repo, file).is_none() => Some(Before {
                existed: false,
                text: String::new(),
            }),
            None => None,
        }
    }
}

/// Record a finished mutation of `file`.
pub fn record(
    home: &Path,
    team: &str,
    task: &str,
    repo: &Path,
    file: &str,
    kind: Kind,
    before: Before,
) -> Result<Entry, String> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let e = Entry {
        id: next_id(),
        ts,
        task: task.to_string(),
        file: file.to_string(),
        kind,
        existed_before: before.existed,
        before: before.text,
        after_hash: git::content_hash(repo, file),
    };
    let d = dir(home, team, task);
    std::fs::create_dir_all(&d).map_err(|x| format!("create {}: {x}", d.display()))?;
    let json = serde_json::to_string_pretty(&e).map_err(|x| x.to_string())?;
    std::fs::write(d.join(format!("{}.json", e.id)), json).map_err(|x| x.to_string())?;
    Ok(e)
}

/// Entries for a task, newest first.
pub fn list(home: &Path, team: &str, task: &str) -> Vec<Entry> {
    let mut v: Vec<Entry> = std::fs::read_dir(dir(home, team, task))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|s| serde_json::from_str(&s).ok())
        .collect();
    v.sort_by(|a, b| b.id.cmp(&a.id));
    v
}

/// Put the file back as it was before the recorded mutation. Without `force`, refuses when
/// the file no longer matches what the mutation produced.
pub fn restore(repo: &Path, e: &Entry, force: bool) -> Result<(), RestoreError> {
    if !force && git::content_hash(repo, &e.file) != e.after_hash {
        return Err(RestoreError::Changed);
    }
    let r = if e.existed_before {
        git::write_text(repo, &e.file, &e.before)
    } else {
        git::remove_text(repo, &e.file)
    };
    r.map_err(RestoreError::Other)
}

/// `YYYY-MM-DD HH:MM:SS` (UTC) for a unix timestamp.
pub fn fmt_ts(ts: u64) -> String {
    let days = (ts / 86400) as i64;
    let rem = ts % 86400;
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}
