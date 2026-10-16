//! Per-file facts for the session list, cached at `<home>/sessions-index.json` and refreshed
//! incrementally: a file is re-read only when its mtime or size changed.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::chunks::{parse_file, Aggregate};
use super::paths::{session_id_of, SessionFile, Source};

const SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMeta {
    /// File stem; unique per file and what pin / hide lists store.
    pub id: String,
    pub path: String,
    pub source: Source,
    pub group: String,
    pub cwd: String,
    pub first_prompt: String,
    pub model: String,
    pub git_branch: String,
    /// Unix ms; 0 when the file carries no clock.
    pub start_ms: i64,
    pub end_ms: i64,
    pub message_count: u64,
    pub tool_count: u64,
    pub error_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
    pub mtime_ms: i64,
    pub len: u64,
}

impl SessionMeta {
    pub fn duration_ms(&self) -> i64 {
        (self.end_ms - self.start_ms).max(0)
    }

    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens + self.cache_write_tokens
    }
}

fn mtime_len(path: &Path) -> Option<(i64, u64)> {
    let m = std::fs::metadata(path).ok()?;
    let ms = m
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    Some((ms, m.len()))
}

/// Unix ms encoded in an rteams run file name (`<unix_ms>[-suffix].jsonl`).
fn run_start_ms(path: &Path) -> Option<i64> {
    path.file_stem()?.to_str()?.split('-').next()?.parse().ok()
}

/// Read a whole file once and summarise it. `None` for files with no conversation records.
pub fn extract(f: &SessionFile, mtime_ms: i64, len: u64) -> Option<SessionMeta> {
    let mut agg = Aggregate::default();
    parse_file(&f.path, f.source, |m| agg.push(&m)).ok()?;
    if agg.message_count == 0 {
        return None;
    }
    let t = agg.totals();
    let start = agg
        .start_ms
        .or_else(|| {
            (f.source == Source::Rteams)
                .then(|| run_start_ms(&f.path))
                .flatten()
        })
        .unwrap_or(0);
    let end = agg
        .end_ms
        .or_else(|| (start > 0).then_some(mtime_ms.max(start)))
        .unwrap_or(0);
    Some(SessionMeta {
        id: session_id_of(&f.path),
        path: f.path.to_string_lossy().into_owned(),
        source: f.source,
        group: f.group.clone(),
        cwd: agg.cwd.unwrap_or_default(),
        first_prompt: agg.first_prompt,
        model: agg.model,
        git_branch: agg.branch.unwrap_or_default(),
        start_ms: start,
        end_ms: end,
        message_count: agg.message_count,
        tool_count: agg.tool_count,
        error_count: agg.error_count,
        input_tokens: t.usage.input,
        output_tokens: t.usage.output,
        cache_read_tokens: t.usage.cache_read,
        cache_write_tokens: t.usage.cache_write,
        cost_usd: t.cost_usd,
        mtime_ms,
        len,
    })
}

#[derive(Debug, Default)]
pub struct MetadataIndex {
    entries: HashMap<String, SessionMeta>,
    /// Files re-read by the last `refresh_all` (the rest came from the cache).
    pub computed: usize,
    dirty: bool,
}

impl MetadataIndex {
    pub fn path(home: &Path) -> PathBuf {
        home.join("sessions-index.json")
    }

    pub fn load(home: &Path) -> MetadataIndex {
        let mut idx = MetadataIndex::default();
        let Some(v) = crate::atomic::read_json(&Self::path(home)) else {
            return idx;
        };
        if v.get("schema_version").and_then(|x| x.as_u64()) != Some(SCHEMA as u64) {
            return idx;
        }
        if let Some(list) = v.get("sessions") {
            if let Ok(list) = serde_json::from_value::<Vec<SessionMeta>>(list.clone()) {
                idx.entries = list.into_iter().map(|m| (m.path.clone(), m)).collect();
            }
        }
        idx
    }

    pub fn save(&mut self, home: &Path) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let mut list: Vec<&SessionMeta> = self.entries.values().collect();
        list.sort_by(|a, b| a.path.cmp(&b.path));
        let v = serde_json::json!({ "schema_version": SCHEMA, "sessions": list });
        crate::atomic::write_json(&Self::path(home), &v)?;
        self.dirty = false;
        Ok(())
    }

    pub fn get(&self, path: &str) -> Option<&SessionMeta> {
        self.entries.get(path)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Bring the index up to date with `files`; returns the metadata of every listed file that
    /// has conversation content. Entries of files that vanished are dropped.
    pub fn refresh_all(&mut self, files: &[SessionFile]) -> Vec<SessionMeta> {
        self.computed = 0;
        let mut out = Vec::with_capacity(files.len());
        let mut keep = HashSet::new();
        for f in files {
            let key = f.path.to_string_lossy().into_owned();
            let Some((mtime, len)) = mtime_len(&f.path) else {
                continue;
            };
            keep.insert(key.clone());
            if let Some(e) = self.entries.get(&key) {
                if e.mtime_ms == mtime && e.len == len && e.group == f.group {
                    out.push(e.clone());
                    continue;
                }
            }
            self.computed += 1;
            match extract(f, mtime, len) {
                Some(m) => {
                    self.entries.insert(key, m.clone());
                    self.dirty = true;
                    out.push(m);
                }
                None => {
                    if self.entries.remove(&key).is_some() {
                        self.dirty = true;
                    }
                }
            }
        }
        let before = self.entries.len();
        self.entries.retain(|k, _| keep.contains(k));
        if self.entries.len() != before {
            self.dirty = true;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::chunks::fixtures;
    use super::*;

    fn file(p: &Path, src: Source) -> SessionFile {
        SessionFile {
            path: p.to_path_buf(),
            source: src,
            group: "g".into(),
        }
    }

    #[test]
    fn extracts_claude_facts() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s1.jsonl");
        std::fs::write(&p, fixtures::claude()).unwrap();
        let m = extract(&file(&p, Source::Claude), 5, 6).unwrap();
        assert_eq!(m.id, "s1");
        assert_eq!(m.cwd, "/work/proj");
        assert_eq!(m.git_branch, "main");
        assert_eq!(m.model, "claude-sonnet-4-5");
        assert_eq!(m.first_prompt, "Fix the login bug please");
        assert_eq!(m.tool_count, 4);
        assert_eq!(m.duration_ms(), 66_000);
        assert_eq!(m.input_tokens, 310);
        assert!(m.cost_usd > 0.0);
    }

    #[test]
    fn incremental_refresh_and_persistence() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("home");
        let a = d.path().join("a.jsonl");
        let b = d.path().join("b.jsonl");
        std::fs::write(&a, fixtures::claude()).unwrap();
        std::fs::write(&b, fixtures::codex()).unwrap();
        let files = vec![file(&a, Source::Claude), file(&b, Source::Codex)];
        let mut idx = MetadataIndex::load(&home);
        assert_eq!(idx.refresh_all(&files).len(), 2);
        assert_eq!(idx.computed, 2);
        idx.save(&home).unwrap();
        assert!(MetadataIndex::path(&home).is_file());

        let mut idx = MetadataIndex::load(&home);
        assert_eq!(idx.len(), 2);
        assert_eq!(idx.refresh_all(&files).len(), 2);
        assert_eq!(idx.computed, 0, "unchanged files come from the cache");

        // Appending changes the size: only that file is re-read.
        let mut raw = fixtures::claude();
        raw.push_str("\n{\"type\":\"user\",\"timestamp\":\"2025-03-01T10:05:00Z\",\"message\":{\"role\":\"user\",\"content\":\"more\"}}\n");
        std::fs::write(&a, raw).unwrap();
        let metas = idx.refresh_all(&files);
        assert_eq!(idx.computed, 1);
        assert_eq!(
            metas[0].message_count,
            idx.get(&metas[0].path).unwrap().message_count
        );

        // A deleted file drops out.
        std::fs::remove_file(&b).unwrap();
        assert_eq!(idx.refresh_all(&files).len(), 1);
        assert_eq!(idx.len(), 1);
    }

    #[test]
    fn empty_files_are_skipped() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("e.jsonl");
        std::fs::write(&p, "{\"type\":\"summary\",\"summary\":\"x\"}\n").unwrap();
        assert!(extract(&file(&p, Source::Claude), 1, 1).is_none());
    }
}
