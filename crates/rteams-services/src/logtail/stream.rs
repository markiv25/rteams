//! Offset-based tailing of a member's newest run file.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::{parse_line, LogEntry};

/// Entries kept per member.
pub const RING: usize = 400;
/// How much of a file's end is read on the first poll.
pub const INITIAL_BYTES: u64 = 256 * 1024;
/// Largest chunk read per poll.
const MAX_CHUNK: u64 = 4 * 1024 * 1024;

#[derive(Debug, Default)]
struct Cursor {
    path: PathBuf,
    offset: u64,
    /// Bytes of an unterminated last line.
    partial: Vec<u8>,
}

/// Tails one member.
#[derive(Debug, Default)]
pub struct MemberTail {
    member: String,
    cursor: Option<Cursor>,
    pub entries: VecDeque<LogEntry>,
    /// Entries ever produced (for read markers; never decreases).
    pub total: u64,
}

fn file_start_ms(p: &Path) -> i64 {
    p.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// The newest `*.jsonl` file of a run directory.
pub fn newest_run(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .max_by_key(|p| (file_start_ms(p), p.clone()))
}

impl MemberTail {
    pub fn new(member: &str) -> Self {
        MemberTail {
            member: member.to_string(),
            ..Default::default()
        }
    }

    /// Read what was appended since the last poll. Returns the new entries. Handles a new
    /// newest file (starts it from the top) and truncation (restarts from the top).
    pub fn poll(&mut self, runs_dir: &Path, now_ms: i64) -> Vec<LogEntry> {
        let Some(path) = newest_run(runs_dir) else {
            return vec![];
        };
        let Ok(mut f) = File::open(&path) else {
            return vec![];
        };
        let Ok(len) = f.metadata().map(|m| m.len()) else {
            return vec![];
        };
        let first = self.cursor.is_none();
        let same = self.cursor.as_ref().is_some_and(|c| c.path == path);
        let mut skip_first_line = false;
        let cur = self.cursor.get_or_insert_with(Cursor::default);
        if !same || len < cur.offset {
            *cur = Cursor {
                path: path.clone(),
                offset: 0,
                partial: vec![],
            };
            if first && len > INITIAL_BYTES {
                cur.offset = len - INITIAL_BYTES;
                skip_first_line = true;
            }
        }
        if len <= cur.offset {
            return vec![];
        }
        let want = (len - cur.offset).min(MAX_CHUNK);
        let mut buf = vec![0u8; want as usize];
        if f.seek(SeekFrom::Start(cur.offset)).is_err() || f.read_exact(&mut buf).is_err() {
            return vec![];
        }
        cur.offset += want;
        let mut data = std::mem::take(&mut cur.partial);
        data.extend_from_slice(&buf);
        // Keep an unterminated tail for the next poll.
        let end = data.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        cur.partial = data.split_off(end);
        let text = String::from_utf8_lossy(&data);
        let base = if first { file_start_ms(&path) } else { now_ms };
        let mut fresh = Vec::new();
        for (i, line) in text.lines().enumerate() {
            if skip_first_line && i == 0 {
                continue;
            }
            // History is ordered by line within the run; live lines carry the poll time.
            for mut e in parse_line(line, &self.member, base) {
                if first {
                    e.ts_ms = base + i as i64;
                }
                fresh.push(e);
            }
        }
        for e in &fresh {
            self.entries.push_back(e.clone());
            if self.entries.len() > RING {
                self.entries.pop_front();
            }
        }
        self.total += fresh.len() as u64;
        fresh
    }
}

/// Tails every member of a team.
#[derive(Debug, Default)]
pub struct TeamTail {
    pub members: HashMap<String, MemberTail>,
}

impl TeamTail {
    /// Poll `members` (tracking toggle: pass only the visible ones). Returns new entries.
    pub fn poll(
        &mut self,
        home: &Path,
        team: &str,
        members: &[String],
        now_ms: i64,
    ) -> Vec<LogEntry> {
        let mut fresh = Vec::new();
        for m in members {
            let dir = home.join(team).join("runs").join(m);
            fresh.extend(
                self.members
                    .entry(m.clone())
                    .or_insert_with(|| MemberTail::new(m))
                    .poll(&dir, now_ms),
            );
        }
        fresh
    }

    pub fn entries(&self, member: &str) -> Vec<LogEntry> {
        self.members
            .get(member)
            .map(|m| m.entries.iter().cloned().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn line(text: &str) -> String {
        format!(
            "{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}}}\n"
        )
    }

    fn append(p: &Path, s: &str) {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .unwrap()
            .write_all(s.as_bytes())
            .unwrap();
    }

    #[test]
    fn reads_incrementally_and_keeps_partial_lines() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("1000.jsonl");
        append(&f, &line("one"));
        let mut t = MemberTail::new("bob");
        assert_eq!(t.poll(d.path(), 50).len(), 1);
        assert!(t.poll(d.path(), 60).is_empty());
        let l2 = line("two");
        append(&f, &l2[..20]);
        assert!(t.poll(d.path(), 70).is_empty());
        append(&f, &l2[20..]);
        let n = t.poll(d.path(), 80);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].text, "two");
        assert_eq!(n[0].ts_ms, 80);
        assert_eq!(t.total, 2);
    }

    #[test]
    fn follows_new_file_and_truncation() {
        let d = tempfile::tempdir().unwrap();
        append(&d.path().join("1000.jsonl"), &line("a"));
        let mut t = MemberTail::new("bob");
        t.poll(d.path(), 1);
        append(&d.path().join("2000.jsonl"), &line("b"));
        let n = t.poll(d.path(), 2);
        assert_eq!(n[0].text, "b");
        append(
            &d.path().join("2000.jsonl"),
            &line("a much longer second line"),
        );
        assert_eq!(t.poll(d.path(), 2).len(), 1);
        std::fs::write(d.path().join("2000.jsonl"), line("c")).unwrap();
        let n = t.poll(d.path(), 3);
        assert_eq!(n[0].text, "c");
    }

    #[test]
    fn large_history_starts_near_the_end_and_ring_is_bounded() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("9.jsonl");
        let mut big = String::new();
        for i in 0..5000 {
            big.push_str(&line(&format!("line number {i} padding padding padding")));
        }
        append(&f, &big);
        let mut t = MemberTail::new("bob");
        let n = t.poll(d.path(), 1);
        assert!(n.len() < 5000);
        assert!(n.last().unwrap().text.contains("4999"));
        assert!(t.entries.len() <= RING);
    }

    #[test]
    fn team_tail_polls_listed_members_only() {
        let d = tempfile::tempdir().unwrap();
        for m in ["a", "b"] {
            let dir = d.path().join("t/runs").join(m);
            std::fs::create_dir_all(&dir).unwrap();
            append(&dir.join("5.jsonl"), &line(m));
        }
        let mut tt = TeamTail::default();
        let n = tt.poll(d.path(), "t", &["a".to_string()], 1);
        assert_eq!(n.len(), 1);
        assert!(tt.entries("b").is_empty());
    }
}
