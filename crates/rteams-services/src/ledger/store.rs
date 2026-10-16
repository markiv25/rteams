//! Append-only `<home>/<team>/ledger.jsonl`. The last entry per id wins; the file is compacted
//! to the newest `MAX_ENTRIES` ids on open.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::stable_json::hash_json;

pub const MAX_ENTRIES: usize = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryState {
    Started,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub hash: String,
    pub state: EntryState,
    /// Kept while `Started` so a crashed command can be replayed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LedgerError {
    #[error("command_id_reused: `{0}` was used with a different payload")]
    CommandIdReused(String),
    #[error("ledger io: {0}")]
    Io(String),
    #[error("unknown command `{0}`")]
    Unknown(String),
}

/// What `begin` decided.
#[derive(Debug, Clone, PartialEq)]
pub enum Begin {
    /// First time (or a retry after failure): run it, then `complete` / `fail`.
    Execute,
    /// Already done: return the stored result, run nothing.
    Replay(Value),
    /// Started earlier and never finished (crash or concurrent run).
    Unfinished,
}

#[derive(Debug)]
pub struct Ledger {
    path: PathBuf,
    entries: BTreeMap<String, Entry>,
    /// Insertion order of ids (oldest first) for compaction.
    order: Vec<String>,
}

pub fn ledger_path(home: &Path, team: &str) -> PathBuf {
    home.join(team).join("ledger.jsonl")
}

impl Ledger {
    pub fn open(home: &Path, team: &str) -> Ledger {
        Ledger::open_file(ledger_path(home, team))
    }

    pub fn open_file(path: PathBuf) -> Ledger {
        let mut l = Ledger {
            path,
            entries: BTreeMap::new(),
            order: vec![],
        };
        if let Ok(text) = std::fs::read_to_string(&l.path) {
            for line in text.lines() {
                // A torn last line (crash mid-append) is skipped.
                if let Ok(e) = serde_json::from_str::<Entry>(line) {
                    l.put(e);
                }
            }
        }
        if l.order.len() > MAX_ENTRIES {
            let _ = l.compact(MAX_ENTRIES);
        }
        l
    }

    fn put(&mut self, e: Entry) {
        if self.entries.insert(e.id.clone(), e.clone()).is_some() {
            self.order.retain(|i| *i != e.id);
        }
        self.order.push(e.id);
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.get(id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entries that were started but never completed or failed, oldest first.
    pub fn unacked(&self) -> Vec<Entry> {
        self.order
            .iter()
            .filter_map(|i| self.entries.get(i))
            .filter(|e| e.state == EntryState::Started)
            .cloned()
            .collect()
    }

    fn append(&mut self, e: Entry) -> Result<(), LedgerError> {
        let io = |e: std::io::Error| LedgerError::Io(e.to_string());
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(io)?;
        let mut line = serde_json::to_string(&e).map_err(|e| LedgerError::Io(e.to_string()))?;
        line.push('\n');
        f.write_all(line.as_bytes()).map_err(io)?;
        f.sync_data().map_err(io)?;
        self.put(e);
        Ok(())
    }

    /// Idempotency gate. Same id + different payload hash is an error.
    pub fn begin(&mut self, id: &str, payload: &Value, now: i64) -> Result<Begin, LedgerError> {
        let hash = hash_json(payload);
        match self.entries.get(id) {
            Some(e) if e.hash != hash => Err(LedgerError::CommandIdReused(id.to_string())),
            Some(e) if e.state == EntryState::Done => {
                Ok(Begin::Replay(e.result.clone().unwrap_or(Value::Null)))
            }
            Some(e) if e.state == EntryState::Started => Ok(Begin::Unfinished),
            _ => {
                self.append(Entry {
                    id: id.to_string(),
                    hash,
                    state: EntryState::Started,
                    payload: Some(payload.clone()),
                    result: None,
                    at: now,
                })?;
                Ok(Begin::Execute)
            }
        }
    }

    pub fn complete(&mut self, id: &str, result: Value, now: i64) -> Result<(), LedgerError> {
        self.finish(id, EntryState::Done, result, now)
    }

    pub fn fail(&mut self, id: &str, error: &str, now: i64) -> Result<(), LedgerError> {
        self.finish(id, EntryState::Failed, Value::String(error.into()), now)
    }

    fn finish(
        &mut self,
        id: &str,
        state: EntryState,
        result: Value,
        now: i64,
    ) -> Result<(), LedgerError> {
        let prev = self
            .entries
            .get(id)
            .ok_or_else(|| LedgerError::Unknown(id.to_string()))?;
        let e = Entry {
            id: id.to_string(),
            hash: prev.hash.clone(),
            state,
            payload: prev.payload.clone().filter(|_| state == EntryState::Failed),
            result: Some(result),
            at: now,
        };
        self.append(e)
    }

    /// Rewrite the file with only the newest `keep` ids (atomic).
    pub fn compact(&mut self, keep: usize) -> std::io::Result<()> {
        while self.order.len() > keep {
            let old = self.order.remove(0);
            self.entries.remove(&old);
        }
        let mut out = String::new();
        for id in &self.order {
            if let Some(e) = self.entries.get(id) {
                out.push_str(&serde_json::to_string(e).map_err(std::io::Error::other)?);
                out.push('\n');
            }
        }
        crate::atomic::write(&self.path, out.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn begin_complete_replay_and_reuse() {
        let d = tempfile::tempdir().unwrap();
        let mut l = Ledger::open(d.path(), "t");
        let p = json!({"a": 1, "b": 2});
        assert_eq!(l.begin("c1", &p, 1).unwrap(), Begin::Execute);
        assert_eq!(l.begin("c1", &p, 2).unwrap(), Begin::Unfinished);
        l.complete("c1", json!("ok"), 3).unwrap();
        assert_eq!(
            l.begin("c1", &json!({"b": 2, "a": 1}), 4).unwrap(),
            Begin::Replay(json!("ok"))
        );
        assert_eq!(
            l.begin("c1", &json!({"a": 9}), 5),
            Err(LedgerError::CommandIdReused("c1".into()))
        );
        let l2 = Ledger::open(d.path(), "t");
        assert_eq!(l2.get("c1").unwrap().state, EntryState::Done);
    }

    #[test]
    fn failed_can_retry_and_torn_line_is_skipped() {
        let d = tempfile::tempdir().unwrap();
        let mut l = Ledger::open(d.path(), "t");
        l.begin("x", &json!(1), 1).unwrap();
        l.fail("x", "boom", 2).unwrap();
        assert_eq!(l.begin("x", &json!(1), 3).unwrap(), Begin::Execute);
        let p = ledger_path(d.path(), "t");
        let mut f = OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"{\"id\":\"torn").unwrap();
        let l = Ledger::open(d.path(), "t");
        assert_eq!(l.unacked().len(), 1);
    }

    #[test]
    fn compaction_keeps_newest() {
        let d = tempfile::tempdir().unwrap();
        let mut l = Ledger::open(d.path(), "t");
        for i in 0..10 {
            l.begin(&format!("c{i}"), &json!(i), i).unwrap();
            l.complete(&format!("c{i}"), json!(i), i).unwrap();
        }
        l.compact(3).unwrap();
        let l = Ledger::open(d.path(), "t");
        assert_eq!(l.len(), 3);
        assert!(l.get("c9").is_some() && l.get("c6").is_none());
    }
}
