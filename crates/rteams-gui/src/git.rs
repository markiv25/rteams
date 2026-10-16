//! Thin wrappers over the `git` binary: per-member worktrees, init/commit, per-file diff and
//! revert. Everything shells out with `std::process::Command`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!("git {}: {}", args.join(" "), err.trim()))
    }
}

/// `<home>/<team>/wt/<member>` on branch `rteams/<member>`, created from `project`'s HEAD.
/// Reused when it already exists.
pub fn ensure_worktree(
    project: &Path,
    home: &Path,
    team: &str,
    member: &str,
) -> Result<PathBuf, String> {
    let wt = home.join(team).join("wt").join(member);
    if wt.join(".git").exists() {
        return Ok(wt);
    }
    if let Some(parent) = wt.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let branch = format!("rteams/{member}");
    let wt_s = wt.to_string_lossy().into_owned();
    git(project, &["worktree", "add", "-B", &branch, &wt_s])?;
    Ok(wt)
}

pub fn init_repo(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| format!("create {}: {e}", path.display()))?;
    git(path, &["init"]).map(|_| ())
}

pub fn initial_commit(path: &Path) -> Result<(), String> {
    git(path, &["add", "-A"])?;
    git(
        path,
        &[
            "-c",
            "user.name=rteams",
            "-c",
            "user.email=rteams@localhost",
            "commit",
            "-m",
            "Initial commit",
            "--allow-empty",
        ],
    )
    .map(|_| ())
}

/// Checked-out branch name; `None` outside a repo or on a detached HEAD.
pub fn current_branch(path: &Path) -> Option<String> {
    let s = git(path, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()?;
    let s = s.trim();
    (!s.is_empty() && s != "HEAD").then(|| s.to_string())
}

fn is_tracked(repo: &Path, path: &str) -> bool {
    git(repo, &["ls-files", "--error-unmatch", "--", path]).is_ok()
}

/// Unified diff of `path` (absolute or repo-relative): working tree vs HEAD; an untracked file
/// is shown as a full addition.
pub fn diff_file(repo: &Path, path: &str) -> Result<String, String> {
    let rel = relative(repo, path);
    if is_tracked(repo, &rel) {
        // A repo without commits has no HEAD; fall back to the index.
        return git(repo, &["diff", "HEAD", "--", &rel])
            .or_else(|_| git(repo, &["diff", "--", &rel]));
    }
    let full = repo.join(&rel);
    if !full.exists() {
        return Ok(String::new());
    }
    // `--no-index` exits 1 when files differ, which is the expected case.
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["diff", "--no-index", "--", "/dev/null", &rel])
        .output()
        .map_err(|e| format!("git: {e}"))?;
    match out.status.code() {
        Some(0) | Some(1) => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        _ => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
    }
}

/// Discard the change: restore a tracked file, delete an untracked one.
pub fn revert_file(repo: &Path, path: &str) -> Result<(), String> {
    let rel = relative(repo, path);
    if is_tracked(repo, &rel) {
        git(repo, &["checkout", "--", &rel]).map(|_| ())
    } else {
        let full = repo.join(&rel);
        if !full.starts_with(repo) || rel.split('/').any(|c| c == "..") {
            return Err(format!("{path} is outside the repository"));
        }
        std::fs::remove_file(&full).map_err(|e| format!("remove {}: {e}", full.display()))
    }
}

/// Repo-relative form of `path` (unchanged when already relative or outside the repo).
fn relative(repo: &Path, path: &str) -> String {
    let p = Path::new(path);
    if p.is_absolute() {
        let canon = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
        for base in [repo, canon.as_path()] {
            if let Ok(r) = p.strip_prefix(base) {
                return r.to_string_lossy().into_owned();
            }
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("rteams-git-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn repo(tag: &str) -> PathBuf {
        let p = temp(tag);
        init_repo(&p).unwrap();
        std::fs::write(p.join("a.txt"), "one\ntwo\n").unwrap();
        initial_commit(&p).unwrap();
        p
    }

    #[test]
    fn branch_diff_and_revert() {
        let r = repo("diff");
        assert!(current_branch(&r).is_some());
        assert_eq!(diff_file(&r, "a.txt").unwrap(), "");
        std::fs::write(r.join("a.txt"), "one\nthree\n").unwrap();
        let d = diff_file(&r, "a.txt").unwrap();
        assert!(d.contains("-two") && d.contains("+three"), "{d}");
        // Absolute paths work too.
        let abs = r.join("a.txt");
        assert!(diff_file(&r, abs.to_str().unwrap())
            .unwrap()
            .contains("+three"));
        revert_file(&r, "a.txt").unwrap();
        assert_eq!(
            std::fs::read_to_string(r.join("a.txt")).unwrap(),
            "one\ntwo\n"
        );

        std::fs::write(r.join("new.txt"), "hello\n").unwrap();
        let d = diff_file(&r, "new.txt").unwrap();
        assert!(d.contains("+hello"), "{d}");
        revert_file(&r, "new.txt").unwrap();
        assert!(!r.join("new.txt").exists());
        assert!(revert_file(&r, "../x").is_err());
        let _ = std::fs::remove_dir_all(r);
    }

    #[test]
    fn not_a_repo_has_no_branch() {
        let p = temp("none");
        std::fs::create_dir_all(&p).unwrap();
        assert_eq!(current_branch(&p), None);
        let _ = std::fs::remove_dir_all(p);
    }

    #[test]
    fn worktree_is_created_and_reused() {
        let r = repo("wt");
        let home = temp("wthome");
        let wt = ensure_worktree(&r, &home, "t", "ada").unwrap();
        assert!(wt.join("a.txt").exists());
        assert_eq!(current_branch(&wt).as_deref(), Some("rteams/ada"));
        assert_eq!(ensure_worktree(&r, &home, "t", "ada").unwrap(), wt);
        let _ = std::fs::remove_dir_all(r);
        let _ = std::fs::remove_dir_all(home);
    }
}
