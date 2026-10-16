//! Full-text search across sessions: case-insensitive substring over user prompts and assistant
//! text (tool output and thinking are not searched), bounded in results and memory. Run
//! `spawn` to search on a worker thread.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

use super::chunks::{parse_file, Block, Role};
use super::metadata_index::SessionMeta;
use super::paths::Source;

pub const MAX_RESULTS: usize = 200;
const MAX_PER_SESSION: usize = 3;
const MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;
const MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;
const CONTEXT: usize = 40;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub session_id: String,
    pub path: String,
    pub cwd: String,
    pub source: Source,
    /// `user` or `assistant`.
    pub role: &'static str,
    pub snippet: String,
    /// Char offset / length of the match inside `snippet`.
    pub match_start: usize,
    pub match_len: usize,
    pub ts_ms: Option<i64>,
}

#[derive(Debug, Clone)]
struct Entry {
    role: &'static str,
    text: String,
    ts_ms: Option<i64>,
}

/// Extracted searchable text per file, keyed by path and invalidated by mtime / size.
#[derive(Default)]
pub struct SearchCache {
    map: HashMap<String, (i64, u64, Arc<Vec<Entry>>)>,
    bytes: usize,
}

impl SearchCache {
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    fn entries(&mut self, s: &SessionMeta) -> Option<Arc<Vec<Entry>>> {
        if let Some((m, l, e)) = self.map.get(&s.path) {
            if *m == s.mtime_ms && *l == s.len {
                return Some(e.clone());
            }
        }
        if s.len > MAX_FILE_BYTES {
            return None;
        }
        let mut v = Vec::new();
        parse_file(std::path::Path::new(&s.path), s.source, |m| {
            if m.sidechain {
                return;
            }
            if m.is_prompt() {
                v.push(Entry {
                    role: "user",
                    text: m.text(),
                    ts_ms: m.ts_ms,
                });
            } else if m.role == Role::Assistant {
                for b in &m.blocks {
                    if let Block::Text(t) = b {
                        if !t.trim().is_empty() {
                            v.push(Entry {
                                role: "assistant",
                                text: t.clone(),
                                ts_ms: m.ts_ms,
                            });
                        }
                    }
                }
            }
        })
        .ok()?;
        let size: usize = v.iter().map(|e| e.text.len()).sum();
        if self.bytes + size > MAX_CACHE_BYTES {
            self.map.clear();
            self.bytes = 0;
        }
        self.bytes += size;
        let arc = Arc::new(v);
        self.map
            .insert(s.path.clone(), (s.mtime_ms, s.len, arc.clone()));
        Some(arc)
    }
}

fn snippet(text: &str, at: usize, len: usize) -> (String, usize) {
    // `at` / `len` are byte offsets into `text` on char boundaries.
    let before: Vec<char> = text[..at].chars().rev().take(CONTEXT).collect();
    let start_chars = before.len();
    let mut out: String = String::new();
    let truncated_front = text[..at].chars().count() > start_chars;
    if truncated_front {
        out.push('\u{2026}');
    }
    out.extend(before.iter().rev());
    let lead = out.chars().count();
    out.push_str(&text[at..at + len]);
    let after: String = text[at + len..].chars().take(CONTEXT * 2).collect();
    out.push_str(&after);
    if text[at + len..].chars().count() > CONTEXT * 2 {
        out.push('\u{2026}');
    }
    let out: String = out
        .chars()
        .map(|c| {
            if c == '\n' || c == '\r' || c == '\t' {
                ' '
            } else {
                c
            }
        })
        .collect();
    (out, lead)
}

fn find_ci(text: &str, lower_q: &str) -> Option<(usize, usize)> {
    let lower = text.to_lowercase();
    if lower.len() == text.len() {
        let i = lower.find(lower_q)?;
        // Offsets from the lowercased copy are valid here because lengths are equal.
        let end = i + lower_q.len();
        if text.is_char_boundary(i) && text.is_char_boundary(end) {
            return Some((i, lower_q.len()));
        }
    }
    // Fallback: scan char starts and compare lowercase prefixes.
    for (i, _) in text.char_indices() {
        let mut taken = 0usize;
        let mut acc = String::new();
        for c in text[i..].chars() {
            acc.extend(c.to_lowercase());
            taken += c.len_utf8();
            if acc.len() >= lower_q.len() {
                break;
            }
        }
        if acc == lower_q {
            return Some((i, taken));
        }
    }
    None
}

/// Search `sessions` (in the given order) for `query`. Stops at `max` hits or when `cancel`
/// is set.
pub fn search(
    sessions: &[SessionMeta],
    query: &str,
    max: usize,
    cancel: &AtomicBool,
    cache: &mut SearchCache,
) -> Vec<SearchHit> {
    let q = query.trim().to_lowercase();
    let mut hits = Vec::new();
    if q.chars().count() < 2 {
        return hits;
    }
    let max = max.min(MAX_RESULTS);
    for s in sessions {
        if cancel.load(Ordering::Relaxed) || hits.len() >= max {
            break;
        }
        let Some(entries) = cache.entries(s) else {
            continue;
        };
        let mut per = 0;
        for e in entries.iter() {
            let Some((at, len)) = find_ci(&e.text, &q) else {
                continue;
            };
            let (sn, lead) = snippet(&e.text, at, len);
            hits.push(SearchHit {
                session_id: s.id.clone(),
                path: s.path.clone(),
                cwd: s.cwd.clone(),
                source: s.source,
                role: e.role,
                snippet: sn,
                match_start: lead,
                match_len: e.text[at..at + len].chars().count(),
                ts_ms: e.ts_ms,
            });
            per += 1;
            if per >= MAX_PER_SESSION || hits.len() >= max {
                break;
            }
        }
    }
    hits
}

/// A running search.
pub struct SearchJob {
    pub rx: Receiver<Vec<SearchHit>>,
    cancel: Arc<AtomicBool>,
}

impl SearchJob {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Hits if the worker has finished.
    pub fn try_take(&self) -> Option<Vec<SearchHit>> {
        self.rx.try_recv().ok()
    }
}

impl Drop for SearchJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Search on a worker thread; `cache` is shared so repeated searches are fast.
pub fn spawn(
    sessions: Vec<SessionMeta>,
    query: String,
    max: usize,
    cache: Arc<Mutex<SearchCache>>,
    on_done: impl FnOnce() + Send + 'static,
) -> SearchJob {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        let hits = match cache.lock() {
            Ok(mut g) => search(&sessions, &query, max, &c2, &mut g),
            Err(_) => Vec::new(),
        };
        let _ = tx.send(hits);
        on_done();
    });
    SearchJob { rx, cancel }
}

#[cfg(test)]
mod tests {
    use super::super::chunks::fixtures;
    use super::super::scanner::scan;
    use super::*;

    fn metas() -> (tempfile::TempDir, Vec<SessionMeta>) {
        let d = tempfile::tempdir().unwrap();
        let roots = super::super::scanner::tests::make_roots(d.path());
        let r = scan(&roots);
        (d, r.sessions)
    }

    #[test]
    fn finds_user_and_assistant_text_case_insensitively() {
        let (_d, s) = metas();
        let mut cache = SearchCache::default();
        let no = AtomicBool::new(false);
        let h = search(&s, "LOGIN BUG", 50, &no, &mut cache);
        assert!(h.iter().any(|x| x.role == "user" && x.session_id == "s1"));
        let h = search(&s, "read", 50, &no, &mut cache);
        let a = h.iter().find(|x| x.role == "assistant").unwrap();
        let sn: Vec<char> = a.snippet.chars().collect();
        let m: String = sn[a.match_start..a.match_start + a.match_len]
            .iter()
            .collect();
        assert_eq!(m.to_lowercase(), "read");
        // tool results and thinking are not searched
        assert!(search(&s, "fn login()", 50, &no, &mut cache).is_empty());
        assert!(search(&s, "look at auth", 50, &no, &mut cache).is_empty());
        assert!(!cache.is_empty());
    }

    #[test]
    fn results_are_capped_and_cancellable() {
        let (_d, s) = metas();
        let mut cache = SearchCache::default();
        let no = AtomicBool::new(false);
        assert_eq!(search(&s, "the", 2, &no, &mut cache).len(), 2);
        assert!(search(&s, "x", 10, &no, &mut cache).is_empty(), "too short");
        let yes = AtomicBool::new(true);
        assert!(search(&s, "login", 10, &yes, &mut cache).is_empty());
        let _ = fixtures::claude();
    }

    #[test]
    fn worker_thread_delivers() {
        let (_d, s) = metas();
        let job = spawn(s, "login".into(), 50, Arc::default(), || {});
        let mut got = None;
        for _ in 0..200 {
            if let Some(h) = job.try_take() {
                got = Some(h);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!got.expect("search finished").is_empty());
    }

    #[test]
    fn snippet_handles_unicode() {
        let t = "héllo wörld Ünïcode text";
        let (at, len) = find_ci(t, "ünïcode").unwrap();
        let (sn, lead) = snippet(t, at, len);
        assert_eq!(sn.chars().skip(lead).take(7).collect::<String>(), "Ünïcode");
    }
}
