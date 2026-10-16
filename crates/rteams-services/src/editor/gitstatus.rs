//! Per-file git status via `git status --porcelain=v2 --branch -z`.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitMark {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileGit {
    pub mark: GitMark,
    /// Has a change in the index.
    pub staged: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitStatus {
    pub branch: Option<String>,
    /// Keyed by path relative to the project root, `/` separated.
    pub files: HashMap<String, FileGit>,
}

impl GitStatus {
    pub fn get(&self, rel: &str) -> Option<FileGit> {
        self.files.get(rel).copied()
    }

    /// A directory is decorated when any file below it changed.
    pub fn dir_dirty(&self, rel: &str) -> bool {
        let p = format!("{rel}/");
        self.files.keys().any(|k| k.starts_with(&p))
    }
}

fn mark_for(x: char, y: char) -> (GitMark, bool) {
    let staged = x != '.' && x != ' ';
    let m = if x == 'U' || y == 'U' || (x == 'A' && y == 'A') || (x == 'D' && y == 'D') {
        GitMark::Conflicted
    } else if x == 'R' || y == 'R' {
        GitMark::Renamed
    } else if x == 'D' || y == 'D' {
        GitMark::Deleted
    } else if x == 'A' {
        GitMark::Added
    } else {
        GitMark::Modified
    };
    (m, staged)
}

/// Parse `--porcelain=v2 --branch -z` output. `prefix` is `git rev-parse --show-prefix`
/// (project root relative to the repo root, with trailing `/`, or empty).
pub fn parse(out: &str, prefix: &str) -> GitStatus {
    let mut st = GitStatus::default();
    let mut it = out.split('\0');
    let strip = |p: &str| -> Option<String> {
        p.strip_prefix(prefix)
            .map(|s| s.trim_end_matches('/').to_string())
    };
    while let Some(rec) = it.next() {
        if let Some(h) = rec.strip_prefix("# branch.head ") {
            if h != "(detached)" {
                st.branch = Some(h.to_string());
            }
        } else if let Some(p) = rec.strip_prefix("? ") {
            if let Some(p) = strip(p) {
                st.files.insert(
                    p,
                    FileGit {
                        mark: GitMark::Untracked,
                        staged: false,
                    },
                );
            }
        } else if rec.starts_with("1 ") || rec.starts_with("u ") || rec.starts_with("2 ") {
            let n = match &rec[..1] {
                "1" => 9,
                "2" => 10,
                _ => 11,
            };
            let parts: Vec<&str> = rec.splitn(n, ' ').collect();
            if parts.len() == n {
                let mut xy = parts[1].chars();
                let (x, y) = (xy.next().unwrap_or('.'), xy.next().unwrap_or('.'));
                let (mark, staged) = if rec.starts_with("u ") {
                    (GitMark::Conflicted, false)
                } else {
                    mark_for(x, y)
                };
                if let Some(p) = strip(parts[n - 1]) {
                    st.files.insert(p, FileGit { mark, staged });
                }
            }
            if rec.starts_with("2 ") {
                it.next(); // original path of a rename
            }
        }
    }
    st
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let o = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Status of the project at `root`; empty when `root` is not inside a git work tree.
pub fn status(root: &Path) -> GitStatus {
    let Some(prefix) = git(root, &["rev-parse", "--show-prefix"]) else {
        return GitStatus::default();
    };
    let Some(out) = git(
        root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
    ) else {
        return GitStatus::default();
    };
    parse(&out, prefix.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v2() {
        let out = "# branch.oid abc\0# branch.head main\0\
1 .M N... 100644 100644 100644 a b src/a.rs\0\
1 A. N... 000000 100644 100644 a b new.rs\0\
2 R. N... 100644 100644 100644 a b R100 moved.rs\0old.rs\0\
u UU N... 1 2 3 4 a b c conflict.rs\0\
? untracked dir/x.txt\0";
        let s = parse(out, "");
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(
            s.get("src/a.rs"),
            Some(FileGit {
                mark: GitMark::Modified,
                staged: false
            })
        );
        assert_eq!(
            s.get("new.rs"),
            Some(FileGit {
                mark: GitMark::Added,
                staged: true
            })
        );
        assert_eq!(s.get("moved.rs").unwrap().mark, GitMark::Renamed);
        assert_eq!(s.get("conflict.rs").unwrap().mark, GitMark::Conflicted);
        assert_eq!(
            s.get("untracked dir/x.txt").unwrap().mark,
            GitMark::Untracked
        );
        assert!(s.dir_dirty("src"));
        assert!(!s.dir_dirty("lib"));
    }

    #[test]
    fn real_repo() {
        let d = tempfile::tempdir().unwrap();
        if git(d.path(), &["init", "-q"]).is_none() {
            return;
        }
        std::fs::write(d.path().join("a.txt"), "x").unwrap();
        let s = status(d.path());
        assert_eq!(s.get("a.txt").unwrap().mark, GitMark::Untracked);
        assert!(s.branch.is_some());
        assert!(status(Path::new("/nonexistent-rteams")).files.is_empty());
    }
}
