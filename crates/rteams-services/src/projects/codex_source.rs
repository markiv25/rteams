//! Codex source: `~/.codex/sessions/**/rollout-*.jsonl`; the first record is `session_meta`
//! with the session `cwd`.

use std::path::{Path, PathBuf};

use super::{mtime_ms, read_head, Candidate};

/// `~/.codex`.
pub fn default_home() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".codex"))
}

const MAX_FILES: usize = 400;

fn walk(dir: &Path, depth: usize, out: &mut Vec<(i64, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            if depth < 5 {
                walk(&p, depth + 1, out);
            }
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            out.push((e.metadata().map(|m| mtime_ms(&m)).unwrap_or(0), p));
        }
    }
}

fn cwd_of(path: &Path) -> Option<String> {
    let head = read_head(path, 64 * 1024)?;
    let line = head.lines().next()?;
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    v.pointer("/payload/cwd")
        .or_else(|| v.get("cwd"))?
        .as_str()
        .map(str::to_string)
}

/// Candidates from the newest rollout files under `<codex_home>/sessions`.
pub fn scan(codex_home: &Path) -> Vec<Candidate> {
    let mut files = Vec::new();
    walk(&codex_home.join("sessions"), 0, &mut files);
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files.truncate(MAX_FILES);
    files
        .into_iter()
        .filter_map(|(ms, p)| {
            Some(Candidate {
                path: cwd_of(&p)?,
                last_ms: ms,
                provider: "codex".into(),
                team: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_session_meta_cwd() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sessions/2026/01/02");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(
            p.join("rollout-1.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/w/x\"}}\n{}\n",
        )
        .unwrap();
        std::fs::write(p.join("rollout-2.jsonl"), "garbage\n").unwrap();
        let c = scan(d.path());
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].path, "/w/x");
        assert_eq!(c[0].provider, "codex");
    }
}
