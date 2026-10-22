//! Source-control operations for the project editor: porcelain v2 change list, stage / unstage /
//! discard, commit, per-file diff and history. All git runs through `git -C <dir>` with
//! `GIT_TERMINAL_PROMPT=0` and a timeout; stderr is surfaced on failure. Never pushes.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(30);

/// One changed path. `x` / `y` are the porcelain index / worktree status letters (`.` = none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Relative to the repository root.
    pub path: String,
    /// Original path of a rename / copy.
    pub orig_path: Option<String>,
    pub x: char,
    pub y: char,
    pub untracked: bool,
    pub conflicted: bool,
}

impl Change {
    pub fn is_staged(&self) -> bool {
        !self.untracked && !self.conflicted && self.x != '.' && self.x != ' '
    }
    pub fn is_unstaged(&self) -> bool {
        !self.untracked && !self.conflicted && self.y != '.' && self.y != ' '
    }
    /// Status letter shown next to the file for a section.
    pub fn letter(&self, staged: bool) -> char {
        if self.untracked {
            'U'
        } else if self.conflicted {
            '!'
        } else if staged {
            self.x
        } else {
            self.y
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub is_repo: bool,
    pub branch: Option<String>,
    /// Whether HEAD points at a commit (false in a fresh repository).
    pub has_commits: bool,
    pub top: PathBuf,
    /// Project root relative to the repo root, with trailing `/` (or empty).
    pub prefix: String,
    pub items: Vec<Change>,
}

impl Changes {
    pub fn staged(&self) -> Vec<&Change> {
        self.items.iter().filter(|c| c.is_staged()).collect()
    }
    pub fn unstaged(&self) -> Vec<&Change> {
        self.items
            .iter()
            .filter(|c| c.is_unstaged() || c.conflicted)
            .collect()
    }
    pub fn untracked(&self) -> Vec<&Change> {
        self.items.iter().filter(|c| c.untracked).collect()
    }
    /// Path relative to the project root, when the change lies inside it.
    pub fn project_rel(&self, c: &Change) -> Option<String> {
        c.path
            .strip_prefix(self.prefix.as_str())
            .map(str::to_string)
    }
}

/// Parse `git status --porcelain=v2 --branch -z --untracked-files=all` bytes. Paths may contain
/// spaces (they are the last field of a record); renames carry the original path as the next
/// NUL-separated record.
pub fn parse_changes(out: &[u8]) -> Changes {
    let mut res = Changes {
        is_repo: true,
        has_commits: true,
        ..Default::default()
    };
    let text = String::from_utf8_lossy(out);
    let mut it = text.split('\0');
    while let Some(rec) = it.next() {
        if let Some(h) = rec.strip_prefix("# branch.head ") {
            if h != "(detached)" {
                res.branch = Some(h.to_string());
            }
        } else if let Some(o) = rec.strip_prefix("# branch.oid ") {
            res.has_commits = o != "(initial)";
        } else if let Some(p) = rec.strip_prefix("? ") {
            res.items.push(Change {
                path: p.to_string(),
                orig_path: None,
                x: '?',
                y: '?',
                untracked: true,
                conflicted: false,
            });
        } else if rec.starts_with("1 ") || rec.starts_with("2 ") || rec.starts_with("u ") {
            let kind = rec.as_bytes()[0];
            let n = match kind {
                b'1' => 9,
                b'2' => 10,
                _ => 11,
            };
            let parts: Vec<&str> = rec.splitn(n, ' ').collect();
            let orig = if kind == b'2' {
                it.next().map(str::to_string)
            } else {
                None
            };
            if parts.len() != n {
                continue;
            }
            let mut xy = parts[1].chars();
            let (x, y) = (xy.next().unwrap_or('.'), xy.next().unwrap_or('.'));
            res.items.push(Change {
                path: parts[n - 1].to_string(),
                orig_path: orig,
                x,
                y,
                untracked: false,
                conflicted: kind == b'u',
            });
        }
    }
    res
}

/// Run git in `dir`; stdout bytes on exit status 0 (or one of `ok_codes`), else stderr as `Err`.
fn run_codes(dir: &Path, args: &[&str], ok_codes: &[i32]) -> Result<Vec<u8>, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run git: {e}"))?;
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let t_out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("git {} timed out", args.first().unwrap_or(&"")));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let out = t_out.join().unwrap_or_default();
    let err = t_err.join().unwrap_or_default();
    let code = status.code().unwrap_or(-1);
    if code == 0 || ok_codes.contains(&code) {
        Ok(out)
    } else {
        let e = String::from_utf8_lossy(&err).trim().to_string();
        Err(if e.is_empty() {
            format!("git {} failed ({code})", args.first().unwrap_or(&""))
        } else {
            e
        })
    }
}

fn run(dir: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    run_codes(dir, args, &[])
}

fn run_str(dir: &Path, args: &[&str]) -> Result<String, String> {
    run(dir, args).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Changed files of the repository containing `root`. `is_repo == false` when there is none.
pub fn changes(root: &Path) -> Changes {
    let Ok(info) = run_str(root, &["rev-parse", "--show-toplevel", "--show-prefix"]) else {
        return Changes::default();
    };
    let mut lines = info.lines();
    let top = PathBuf::from(lines.next().unwrap_or(""));
    let prefix = lines.next().unwrap_or("").to_string();
    let Ok(out) = run(
        root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
    ) else {
        return Changes::default();
    };
    let mut c = parse_changes(&out);
    c.top = top;
    c.prefix = prefix;
    c
}

pub fn init_repo(root: &Path) -> Result<String, String> {
    run_str(root, &["init"]).map(|s| s.trim().to_string())
}

/// `git add -A` then a commit with `message`.
pub fn initial_commit(root: &Path, message: &str) -> Result<String, String> {
    run(root, &["add", "-A"])?;
    commit_staged(root, message)
}

fn with_paths<'a>(head: &[&'a str], paths: &'a [String]) -> Vec<&'a str> {
    let mut a: Vec<&str> = head.to_vec();
    a.push("--");
    a.extend(paths.iter().map(String::as_str));
    a
}

/// `top` is the repository root (`Changes::top`); `paths` are repo-relative.
pub fn stage(top: &Path, paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    run(top, &with_paths(&["add", "-A"], paths)).map(|_| ())
}

pub fn stage_all(top: &Path) -> Result<(), String> {
    run(top, &["add", "-A"]).map(|_| ())
}

pub fn unstage(top: &Path, paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let has_head = run(top, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok();
    if has_head {
        run(top, &with_paths(&["reset", "-q"], paths)).map(|_| ())
    } else {
        run(top, &with_paths(&["rm", "--cached", "-r", "-q"], paths)).map(|_| ())
    }
}

pub fn unstage_all(top: &Path) -> Result<(), String> {
    let has_head = run(top, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok();
    if has_head {
        run(top, &["reset", "-q"]).map(|_| ())
    } else {
        run(top, &["rm", "--cached", "-r", "-q", "--", "."]).map(|_| ())
    }
}

/// Throw away the worktree changes of one file; an untracked file is deleted.
pub fn discard(top: &Path, change: &Change) -> Result<(), String> {
    let p = [change.path.clone()];
    if change.untracked {
        run(top, &with_paths(&["clean", "-f", "-q"], &p)).map(|_| ())
    } else {
        run(top, &with_paths(&["checkout"], &p)).map(|_| ())
    }
}

/// Commit what is staged. Returns git's summary line.
pub fn commit_staged(dir: &Path, message: &str) -> Result<String, String> {
    if message.trim().is_empty() {
        return Err("empty commit message".into());
    }
    let out = run_str(dir, &["commit", "-q", "-m", message])?;
    let head = run_str(dir, &["log", "-1", "--format=%h %s"])?;
    let _ = out;
    Ok(head.trim().to_string())
}

/// Unified diff of one file. `path` is repo-relative; `top` the repo root.
pub fn diff(top: &Path, c: &Change, staged: bool) -> Result<String, String> {
    if c.untracked {
        let b = std::fs::read(top.join(&c.path)).map_err(|e| e.to_string())?;
        let t = String::from_utf8_lossy(&b);
        let mut s = format!("--- /dev/null\n+++ b/{}\n", c.path);
        for l in t.lines() {
            s.push('+');
            s.push_str(l);
            s.push('\n');
        }
        return Ok(s);
    }
    let p = [c.path.clone()];
    let head: &[&str] = if staged {
        &["diff", "--no-color", "--cached"]
    } else {
        &["diff", "--no-color"]
    };
    run_str(top, &with_paths(head, &p))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub date: String,
    pub subject: String,
}

pub fn parse_log(out: &str) -> Vec<LogEntry> {
    out.split('\x1e')
        .filter_map(|r| {
            let f: Vec<&str> = r.trim_matches(['\n', '\r']).split('\x1f').collect();
            (f.len() >= 5).then(|| LogEntry {
                hash: f[0].into(),
                short: f[1].into(),
                author: f[2].into(),
                date: f[3].into(),
                subject: f[4].into(),
            })
        })
        .collect()
}

/// `git log --follow` of a file. `rel` is relative to `root` (the project folder).
pub fn file_log(root: &Path, rel: &str) -> Result<Vec<LogEntry>, String> {
    let fmt = "--format=%H%x1f%h%x1f%an%x1f%ad%x1f%s%x1e";
    let s = run_str(
        root,
        &[
            "log",
            "--follow",
            "--date=short",
            "-n",
            "200",
            fmt,
            "--",
            rel,
        ],
    )?;
    Ok(parse_log(&s))
}

/// The file's content at `commit` (read-only view). `rel` is relative to `root`.
pub fn file_at(root: &Path, commit: &str, rel: &str) -> Result<String, String> {
    let spec = format!("{commit}:./{rel}");
    run_str(root, &["show", &spec])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_v2() {
        let out = b"# branch.oid abc123\0# branch.head main\0\
1 .M N... 100644 100644 100644 a b src/my file.rs\0\
1 A. N... 000000 100644 100644 a b new.rs\0\
1 MM N... 100644 100644 100644 a b both.rs\0\
2 R. N... 100644 100644 100644 a b R100 new name.rs\0old name.rs\0\
u UU N... 1 2 3 4 a b c conflict.rs\0\
? untracked dir/x y.txt\0";
        let c = parse_changes(out);
        assert!(c.is_repo && c.has_commits);
        assert_eq!(c.branch.as_deref(), Some("main"));
        assert_eq!(c.items.len(), 6);
        assert_eq!(c.items[0].path, "src/my file.rs");
        assert!(c.items[0].is_unstaged() && !c.items[0].is_staged());
        assert!(c.items[1].is_staged());
        assert!(c.items[2].is_staged() && c.items[2].is_unstaged());
        assert_eq!(c.items[3].path, "new name.rs");
        assert_eq!(c.items[3].orig_path.as_deref(), Some("old name.rs"));
        assert_eq!(c.items[3].letter(true), 'R');
        assert!(c.items[4].conflicted);
        assert_eq!(c.items[5].path, "untracked dir/x y.txt");
        assert!(c.items[5].untracked);
        assert_eq!(c.staged().len(), 4 - 1);
        assert_eq!(c.untracked().len(), 1);
        assert_eq!(c.unstaged().len(), 3);
    }

    #[test]
    fn initial_branch_and_empty() {
        let c = parse_changes(b"# branch.oid (initial)\0# branch.head main\0");
        assert!(!c.has_commits && c.items.is_empty());
        assert!(parse_changes(b"").items.is_empty());
    }

    #[test]
    fn parses_log() {
        let s = "aaa\x1fa\x1fAnn\x1f2024-01-02\x1fone | two\x1e\nbbb\x1fb\x1fBob\x1f2024-01-01\x1ffirst\x1e";
        let l = parse_log(s);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].subject, "one | two");
        assert_eq!(l[1].author, "Bob");
    }

    fn git_ok(d: &Path) -> bool {
        run(d, &["--version"]).is_ok()
    }

    fn repo() -> Option<tempfile::TempDir> {
        let d = tempfile::tempdir().unwrap();
        if !git_ok(d.path()) {
            return None;
        }
        init_repo(d.path()).unwrap();
        run(d.path(), &["config", "user.name", "T"]).unwrap();
        run(d.path(), &["config", "user.email", "t@example.com"]).unwrap();
        run(d.path(), &["config", "commit.gpgsign", "false"]).unwrap();
        Some(d)
    }

    #[test]
    fn not_a_repo() {
        let d = tempfile::tempdir().unwrap();
        if git_ok(d.path()) {
            // Only meaningful when the temp dir is outside any repository.
            let c = changes(d.path());
            if !c.is_repo {
                assert!(c.items.is_empty());
            }
        }
    }

    #[test]
    fn stage_unstage_commit_discard_history() {
        let Some(d) = repo() else { return };
        let p = d.path();
        assert!(changes(p).is_repo);
        std::fs::write(p.join("a b.txt"), "one\n").unwrap();
        let c = changes(p);
        assert!(!c.has_commits);
        assert_eq!(c.untracked().len(), 1);
        let top = c.top.clone();

        // Stage (fresh repo, no HEAD), unstage, restage.
        stage(&top, &["a b.txt".into()]).unwrap();
        assert_eq!(changes(p).staged().len(), 1);
        unstage(&top, &["a b.txt".into()]).unwrap();
        assert_eq!(changes(p).untracked().len(), 1);
        stage_all(&top).unwrap();
        assert!(commit_staged(p, "  ").is_err());
        let r = commit_staged(p, "first").unwrap();
        assert!(r.contains("first"));
        let c = changes(p);
        assert!(c.has_commits && c.items.is_empty());

        // Modify -> diff, stage, unstage, discard.
        std::fs::write(p.join("a b.txt"), "one\ntwo\n").unwrap();
        let c = changes(p);
        let ch = c.unstaged()[0].clone();
        assert!(diff(&top, &ch, false).unwrap().contains("+two"));
        stage(&top, std::slice::from_ref(&ch.path)).unwrap();
        let c = changes(p);
        assert_eq!(c.staged().len(), 1);
        assert!(diff(&top, c.staged()[0], true).unwrap().contains("+two"));
        unstage_all(&top).unwrap();
        assert_eq!(changes(p).staged().len(), 0);
        discard(&top, &changes(p).unstaged()[0].clone()).unwrap();
        assert_eq!(std::fs::read_to_string(p.join("a b.txt")).unwrap(), "one\n");

        // Untracked discard deletes.
        std::fs::write(p.join("junk.txt"), "x").unwrap();
        let c = changes(p);
        assert_eq!(c.untracked().len(), 1);
        assert!(diff(&top, c.untracked()[0], false).unwrap().contains("+x"));
        discard(&top, &c.untracked()[0].clone()).unwrap();
        assert!(!p.join("junk.txt").exists());

        // History + file at commit.
        std::fs::write(p.join("a b.txt"), "one\nthree\n").unwrap();
        stage_all(&top).unwrap();
        commit_staged(p, "second").unwrap();
        let log = file_log(p, "a b.txt").unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].subject, "second");
        assert_eq!(log[1].author, "T");
        assert_eq!(file_at(p, &log[1].hash, "a b.txt").unwrap(), "one\n");
        assert!(file_at(p, &log[1].hash, "nope.txt").is_err());
    }

    #[test]
    fn rename_detected_and_initial_commit() {
        let Some(d) = repo() else { return };
        let p = d.path();
        std::fs::write(p.join("old.txt"), "content content content\n").unwrap();
        initial_commit(p, "Initial commit").unwrap();
        run(p, &["mv", "old.txt", "new name.txt"]).unwrap();
        let c = changes(p);
        assert_eq!(c.items.len(), 1);
        assert_eq!(c.items[0].path, "new name.txt");
        assert_eq!(c.items[0].orig_path.as_deref(), Some("old.txt"));
    }

    #[test]
    fn nested_project_prefix() {
        let Some(d) = repo() else { return };
        let p = d.path();
        std::fs::create_dir_all(p.join("sub")).unwrap();
        std::fs::write(p.join("sub/f.txt"), "x").unwrap();
        let c = changes(&p.join("sub"));
        assert_eq!(c.prefix, "sub/");
        assert_eq!(c.project_rel(&c.items[0]).as_deref(), Some("f.txt"));
    }
}
