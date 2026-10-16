//! Read cache with a generation fence, and read markers.
//!
//! A slow background read must not overwrite a fresher one: every request takes a generation
//! from `ReadCache::begin`, and `finish` drops results older than the newest request.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
struct Slot<T> {
    key: Option<(u64, u64)>,
    latest_gen: u64,
    data: Option<T>,
}

/// Cache keyed by path, valid while `(mtime, len)` is unchanged.
#[derive(Debug, Default)]
pub struct ReadCache<T> {
    slots: HashMap<PathBuf, Slot<T>>,
}

impl<T: Clone> ReadCache<T> {
    pub fn new() -> Self {
        ReadCache {
            slots: HashMap::new(),
        }
    }

    /// Cached data when the file is unchanged.
    pub fn get(&self, path: &Path, mtime: u64, len: u64) -> Option<T> {
        let s = self.slots.get(path)?;
        (s.key == Some((mtime, len)))
            .then(|| s.data.clone())
            .flatten()
    }

    /// Start a read: returns its generation.
    pub fn begin(&mut self, path: &Path) -> u64 {
        let s = self
            .slots
            .entry(path.to_path_buf())
            .or_insert_with(|| Slot {
                key: None,
                latest_gen: 0,
                data: None,
            });
        s.latest_gen += 1;
        s.latest_gen
    }

    /// Store a finished read; `false` (dropped) when a newer read was requested meanwhile.
    pub fn finish(&mut self, path: &Path, generation: u64, mtime: u64, len: u64, data: T) -> bool {
        let Some(s) = self.slots.get_mut(path) else {
            return false;
        };
        if generation < s.latest_gen {
            return false;
        }
        s.key = Some((mtime, len));
        s.data = Some(data);
        true
    }
}

/// How much of each member's log the user has seen: `<home>/<team>/log-read.json`.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReadMarks {
    #[serde(default)]
    pub schema_version: u32,
    /// Member -> `MemberTail::total` when last viewed.
    #[serde(default)]
    pub seen: HashMap<String, u64>,
}

impl ReadMarks {
    fn path(home: &Path, team: &str) -> PathBuf {
        home.join(team).join("log-read.json")
    }

    pub fn load(home: &Path, team: &str) -> Self {
        std::fs::read(Self::path(home, team))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, home: &Path, team: &str) -> std::io::Result<()> {
        let mut m = self.clone();
        m.schema_version = 1;
        let bytes = serde_json::to_vec_pretty(&m).map_err(std::io::Error::other)?;
        crate::atomic::write(&Self::path(home, team), &bytes)
    }

    pub fn mark(&mut self, member: &str, total: u64) {
        let e = self.seen.entry(member.to_string()).or_insert(0);
        *e = (*e).max(total);
    }

    /// Entries produced since the member was last marked.
    pub fn unread(&self, member: &str, total: u64) -> u64 {
        total.saturating_sub(self.seen.get(member).copied().unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_reads_are_dropped() {
        let mut c: ReadCache<String> = ReadCache::new();
        let p = Path::new("/x");
        let slow = c.begin(p);
        let fast = c.begin(p);
        assert!(c.finish(p, fast, 2, 20, "fresh".into()));
        assert!(!c.finish(p, slow, 1, 10, "stale".into()));
        assert_eq!(c.get(p, 2, 20).as_deref(), Some("fresh"));
        assert_eq!(c.get(p, 3, 20), None);
    }

    #[test]
    fn marks_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let mut m = ReadMarks::load(d.path(), "t");
        assert_eq!(m.unread("bob", 7), 7);
        m.mark("bob", 5);
        m.mark("bob", 3);
        m.save(d.path(), "t").unwrap();
        let m = ReadMarks::load(d.path(), "t");
        assert_eq!(m.unread("bob", 7), 2);
        assert_eq!(m.unread("bob", 4), 0);
    }
}
