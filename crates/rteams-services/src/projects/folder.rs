//! Project folder checks for the create-team / import / picker forms. The native folder dialog
//! (`rfd`) lives in the GUI; this validates and creates the path.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FolderState {
    pub exists: bool,
    pub is_dir: bool,
    pub writable: bool,
    pub is_git_root: bool,
    pub git_branch: Option<String>,
    /// The directory is missing and `ensure` would create it.
    pub will_create: bool,
}

/// Expand a leading `~`.
pub fn expand(path: &str) -> PathBuf {
    let p = path.trim();
    if p == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(p));
    }
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(h) = dirs::home_dir() {
            return h.join(rest);
        }
    }
    PathBuf::from(p)
}

/// Branch from `.git/HEAD` (`None` for detached HEAD or no repo). Follows a `.git` file
/// (worktrees) to its gitdir.
pub fn git_branch(dir: &Path) -> Option<String> {
    let dot = dir.join(".git");
    let gitdir = if dot.is_file() {
        let s = std::fs::read_to_string(&dot).ok()?;
        let rel = s.trim().strip_prefix("gitdir:")?.trim().to_string();
        let p = PathBuf::from(rel);
        if p.is_absolute() {
            p
        } else {
            dir.join(p)
        }
    } else {
        dot
    };
    let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
    head.trim()
        .strip_prefix("ref: refs/heads/")
        .map(str::to_string)
}

pub fn state(path: &str) -> FolderState {
    let p = expand(path);
    if path.trim().is_empty() {
        return FolderState::default();
    }
    match std::fs::metadata(&p) {
        Ok(m) => FolderState {
            exists: true,
            is_dir: m.is_dir(),
            writable: !m.permissions().readonly(),
            is_git_root: p.join(".git").exists(),
            git_branch: git_branch(&p),
            will_create: false,
        },
        Err(_) => FolderState {
            will_create: p.is_absolute(),
            ..Default::default()
        },
    }
}

/// Validate a typed path: non-empty, absolute after `~` expansion, not an existing file.
pub fn validate(path: &str) -> Result<PathBuf, String> {
    if path.trim().is_empty() {
        return Err("Select a project...".into());
    }
    let p = expand(path);
    if !p.is_absolute() {
        return Err("Project path must be absolute".into());
    }
    if p.exists() && !p.is_dir() {
        return Err("Project path is not a directory".into());
    }
    Ok(p)
}

/// Validate and create the directory if missing ("If the directory does not exist, it will be
/// created automatically."). Returns the path.
pub fn ensure(path: &str) -> Result<PathBuf, String> {
    let p = validate(path)?;
    std::fs::create_dir_all(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_and_ensure() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("new/proj");
        let s = target.to_string_lossy().into_owned();
        let st = state(&s);
        assert!(!st.exists && st.will_create);
        ensure(&s).unwrap();
        let st = state(&s);
        assert!(st.exists && st.is_dir && st.writable && !st.will_create && !st.is_git_root);
        std::fs::create_dir_all(target.join(".git")).unwrap();
        std::fs::write(target.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let st = state(&s);
        assert!(st.is_git_root);
        assert_eq!(st.git_branch.as_deref(), Some("main"));
    }

    #[test]
    fn validate_rejects() {
        assert!(validate("").is_err());
        assert!(validate("relative/x").is_err());
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("f");
        std::fs::write(&f, "x").unwrap();
        assert!(validate(&f.to_string_lossy()).is_err());
        assert!(!state("").exists);
    }
}
