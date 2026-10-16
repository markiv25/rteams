//! Project identity: canonical path (resolves `/private/var` vs `/var`, trailing slashes,
//! case on case-insensitive filesystems), git worktree -> main repo root, and a label taken from
//! the git remote name or the directory name.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// Merge key: the main repo root (or canonical path) as a string.
    pub key: String,
    /// Canonical path of the directory itself.
    pub path: String,
    pub label: String,
    pub exists: bool,
}

/// Canonical form of `path`; for a missing path, a lexically cleaned one (no trailing slash).
pub fn canonical(path: &str) -> String {
    let p = Path::new(path.trim());
    match std::fs::canonicalize(p) {
        Ok(c) => c.to_string_lossy().into_owned(),
        Err(_) => {
            let s = path.trim();
            if s.len() > 1 {
                s.trim_end_matches(['/', '\\']).to_string()
            } else {
                s.to_string()
            }
        }
    }
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// The main repository root for `dir`: the worktree's `--git-common-dir` parent, else the
/// top level. `None` when `dir` is not inside a git repository.
pub fn git_root(dir: &Path) -> Option<PathBuf> {
    if let Some(common) = git(dir, &["rev-parse", "--git-common-dir"]) {
        let c = PathBuf::from(&common);
        let c = if c.is_absolute() { c } else { dir.join(c) };
        if let Ok(c) = std::fs::canonicalize(c) {
            if c.file_name().is_some_and(|n| n == ".git") {
                return c.parent().map(Path::to_path_buf);
            }
        }
    }
    git(dir, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

/// Repo name from an `origin`-style remote URL (`git@h:o/name.git`, `https://h/o/name`).
pub fn remote_name(url: &str) -> Option<String> {
    let u = url.trim().trim_end_matches('/');
    let last = u.rsplit(['/', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    (!name.is_empty()).then(|| name.to_string())
}

fn dir_label(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// Resolve the identity of a project path.
pub fn resolve(path: &str) -> Identity {
    let canon = canonical(path);
    let p = PathBuf::from(&canon);
    let exists = p.is_dir();
    if !exists {
        return Identity {
            key: canon.clone(),
            label: dir_label(&p),
            path: canon,
            exists,
        };
    }
    let root = git_root(&p);
    let key = root
        .as_ref()
        .map_or_else(|| canon.clone(), |r| r.to_string_lossy().into_owned());
    let label = root
        .as_deref()
        .and_then(|r| git(r, &["remote", "get-url", "origin"]))
        .and_then(|u| remote_name(&u))
        .unwrap_or_else(|| dir_label(root.as_deref().unwrap_or(&p)));
    Identity {
        key,
        path: canon,
        label,
        exists,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_names() {
        assert_eq!(
            remote_name("git@github.com:o/repo.git").as_deref(),
            Some("repo")
        );
        assert_eq!(remote_name("https://h/o/repo/").as_deref(), Some("repo"));
        assert_eq!(remote_name(""), None);
    }

    #[test]
    fn canonical_trims_missing_and_resolves_existing() {
        assert_eq!(canonical("/no/such/dir/"), "/no/such/dir");
        let d = tempfile::tempdir().unwrap();
        let a = canonical(&d.path().to_string_lossy());
        let b = canonical(&format!("{}/", d.path().display()));
        assert_eq!(a, b);
    }

    #[test]
    fn missing_path_label_is_dir_name() {
        let i = resolve("/no/such/proj");
        assert!(!i.exists);
        assert_eq!(i.label, "proj");
    }

    #[test]
    fn plain_dir_identity_is_itself() {
        let d = tempfile::tempdir().unwrap();
        let i = resolve(&d.path().to_string_lossy());
        assert!(i.exists);
        assert!(!i.label.is_empty());
    }
}
