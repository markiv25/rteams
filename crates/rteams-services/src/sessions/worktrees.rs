//! Worktree grouping: sessions of every git worktree of a repository belong under one repo,
//! found through the git common dir (like the original's `WorktreeGrouper`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::scanner::ProjectInfo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    /// Canonical main repository root (or the cwd itself outside git).
    pub root: String,
    pub label: String,
    pub is_worktree: bool,
}

fn label_of(p: &str) -> String {
    Path::new(p)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string())
}

fn git_lines(dir: &Path, args: &[&str]) -> Option<Vec<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect()
    })
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Path-pattern fallback for directories that no longer exist.
fn from_pattern(cwd: &str) -> Option<String> {
    for marker in ["/.claude/worktrees/", "/.worktrees/", "/.git/worktrees/"] {
        if let Some(i) = cwd.find(marker) {
            return Some(cwd[..i].to_string());
        }
    }
    None
}

/// Repo identity of a working directory.
pub fn repo_of(cwd: &str) -> RepoInfo {
    let dir = Path::new(cwd);
    if dir.is_dir() {
        if let Some(l) = git_lines(dir, &["rev-parse", "--git-common-dir", "--show-toplevel"]) {
            if l.len() >= 2 {
                let common = {
                    let c = PathBuf::from(&l[0]);
                    canon(&if c.is_absolute() { c } else { dir.join(c) })
                };
                let top = canon(Path::new(&l[1]));
                let root = if common.file_name().is_some_and(|n| n == ".git") {
                    common
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or(common.clone())
                } else {
                    common
                };
                let root_s = root.to_string_lossy().into_owned();
                return RepoInfo {
                    label: label_of(&root_s),
                    is_worktree: top != root,
                    root: root_s,
                };
            }
        }
    }
    if let Some(root) = from_pattern(cwd) {
        return RepoInfo {
            label: label_of(&root),
            is_worktree: true,
            root,
        };
    }
    RepoInfo {
        label: label_of(cwd),
        is_worktree: false,
        root: cwd.to_string(),
    }
}

/// Memoised `repo_of`.
#[derive(Default)]
pub struct RepoCache(HashMap<String, RepoInfo>);

impl RepoCache {
    pub fn get(&mut self, cwd: &str) -> RepoInfo {
        self.0
            .entry(cwd.to_string())
            .or_insert_with(|| repo_of(cwd))
            .clone()
    }
}

/// A repository with its projects (the main checkout and each worktree).
#[derive(Debug, Clone)]
pub struct RepoGroup {
    pub key: String,
    pub label: String,
    pub projects: Vec<ProjectInfo>,
    pub session_count: usize,
    pub last_ms: i64,
}

/// Group projects by repository, newest activity first.
pub fn group_by_repo(projects: &[ProjectInfo]) -> Vec<RepoGroup> {
    let mut m: HashMap<String, RepoGroup> = HashMap::new();
    for p in projects {
        let g = m.entry(p.repo_key.clone()).or_insert_with(|| RepoGroup {
            key: p.repo_key.clone(),
            label: p.repo_label.clone(),
            projects: Vec::new(),
            session_count: 0,
            last_ms: 0,
        });
        g.session_count += p.session_count;
        g.last_ms = g.last_ms.max(p.last_ms);
        g.projects.push(p.clone());
    }
    let mut v: Vec<RepoGroup> = m.into_values().collect();
    for g in &mut v {
        g.projects.sort_by_key(|p| std::cmp::Reverse(p.last_ms));
    }
    v.sort_by(|a, b| b.last_ms.cmp(&a.last_ms).then_with(|| a.key.cmp(&b.key)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.email=a@b.c", "-c", "user.name=t"])
            .args(args)
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[test]
    fn pattern_fallback_for_missing_dirs() {
        let r = repo_of("/nonexistent/proj/.claude/worktrees/feat-x");
        assert_eq!(r.root, "/nonexistent/proj");
        assert!(r.is_worktree);
        assert_eq!(r.label, "proj");
        let r = repo_of("/nonexistent/plain");
        assert!(!r.is_worktree);
    }

    #[test]
    fn real_worktrees_share_a_root() {
        let d = tempfile::tempdir().unwrap();
        let main = d.path().join("repo");
        std::fs::create_dir_all(&main).unwrap();
        if !git(&main, &["init", "-q"]) {
            return; // no git on this machine
        }
        std::fs::write(main.join("f"), "x").unwrap();
        assert!(git(&main, &["add", "f"]));
        assert!(git(&main, &["commit", "-qm", "i"]));
        let wt = d.path().join("wt");
        assert!(git(
            &main,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "w"]
        ));
        let a = repo_of(main.to_str().unwrap());
        let b = repo_of(wt.to_str().unwrap());
        assert_eq!(a.root, b.root);
        assert!(!a.is_worktree);
        assert!(b.is_worktree);
        assert_eq!(a.label, "repo");
    }
}
