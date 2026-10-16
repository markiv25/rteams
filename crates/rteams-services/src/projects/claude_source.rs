//! Claude source: `~/.claude/projects/<encoded-path>/*.jsonl`. The directory name encodes the
//! path with `/` and `.` mapped to `-`, which is lossy, so the real cwd is read from the first
//! `cwd` field of the newest session file; decoding is only a fallback for paths that exist.

use std::path::{Path, PathBuf};

use super::{mtime_ms, read_head, Candidate};

/// `~/.claude`.
pub fn default_home() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude"))
}

/// Best-effort decode of an encoded project dir name; only returns a path that exists.
pub fn decode_dir_name(name: &str) -> Option<String> {
    if !name.starts_with('-') {
        return None;
    }
    let p = name.replace('-', "/");
    Path::new(&p).is_dir().then_some(p)
}

fn cwd_from_session(path: &Path) -> Option<String> {
    let head = read_head(path, 256 * 1024)?;
    head.lines().take(60).find_map(|l| {
        let v: serde_json::Value = serde_json::from_str(l).ok()?;
        v.get("cwd")?.as_str().map(str::to_string)
    })
}

/// One candidate per project directory under `<claude_home>/projects`.
pub fn scan(claude_home: &Path) -> Vec<Candidate> {
    let Ok(rd) = std::fs::read_dir(claude_home.join("projects")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        let mut newest: Option<(i64, PathBuf)> = None;
        if let Ok(files) = std::fs::read_dir(&dir) {
            for f in files.flatten() {
                let p = f.path();
                if p.extension().is_some_and(|x| x == "jsonl") {
                    let m = f.metadata().map(|m| mtime_ms(&m)).unwrap_or(0);
                    if newest.as_ref().is_none_or(|(n, _)| m > *n) {
                        newest = Some((m, p));
                    }
                }
            }
        }
        let Some((last_ms, file)) = newest else {
            continue;
        };
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(path) = cwd_from_session(&file).or_else(|| decode_dir_name(&name)) else {
            continue;
        };
        out.push(Candidate {
            path,
            last_ms,
            provider: "claude".into(),
            team: None,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cwd_from_newest_session_not_from_the_name() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("projects/-weird-my-app");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(
            p.join("a.jsonl"),
            "{\"type\":\"summary\"}\n{\"cwd\":\"/work/my.app\",\"x\":1}\n",
        )
        .unwrap();
        let c = scan(d.path());
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].path, "/work/my.app");
        assert!(c[0].last_ms > 0);
    }

    #[test]
    fn empty_or_missing_is_fine() {
        let d = tempfile::tempdir().unwrap();
        assert!(scan(d.path()).is_empty());
        assert_eq!(decode_dir_name("not-encoded"), None);
    }
}
