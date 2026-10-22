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

/// Full HEAD commit id; `None` outside a repo or before the first commit.
pub fn head(path: &Path) -> Option<String> {
    git(path, &["rev-parse", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
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

/// One line of a hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Line number in the old file (context and deletions).
    pub old_no: Option<u32>,
    /// Line number in the new file (context and additions).
    pub new_no: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Del,
}

/// A `@@ -a,b +c,d @@` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub header: String,
    pub old_start: u32,
    pub old_len: u32,
    pub new_start: u32,
    pub new_len: u32,
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    pub fn added(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| l.kind == LineKind::Add)
            .count()
    }
    pub fn removed(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| l.kind == LineKind::Del)
            .count()
    }
    /// Stable id from the changed lines only, so it survives other hunks being rejected.
    pub fn key(&self) -> String {
        let mut h: u64 = 0xcbf29ce484222325;
        for l in self.lines.iter().filter(|l| l.kind != LineKind::Context) {
            let tag = if l.kind == LineKind::Add { b'+' } else { b'-' };
            for b in std::iter::once(tag)
                .chain(l.text.bytes())
                .chain(std::iter::once(b'\n'))
            {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x100000001b3);
            }
        }
        format!("{h:016x}")
    }
}

/// One file of a unified diff.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileDiff {
    /// New path (old path for a deletion).
    pub path: String,
    /// Header lines before the first hunk (`diff --git`, `index`, `---`, `+++`).
    pub header: Vec<String>,
    pub hunks: Vec<Hunk>,
    pub binary: bool,
    pub is_new: bool,
    pub is_deleted: bool,
    /// Old path when git reports a rename (`rename from`).
    pub renamed_from: Option<String>,
}

impl FileDiff {
    pub fn added(&self) -> usize {
        self.hunks.iter().map(Hunk::added).sum()
    }
    pub fn removed(&self) -> usize {
        self.hunks.iter().map(Hunk::removed).sum()
    }
}

fn parse_range(s: &str) -> (u32, u32) {
    let s = s.trim_start_matches(['-', '+']);
    match s.split_once(',') {
        Some((a, b)) => (a.parse().unwrap_or(0), b.parse().unwrap_or(0)),
        None => (s.parse().unwrap_or(0), 1),
    }
}

fn parse_hunk_header(l: &str) -> Option<(u32, u32, u32, u32)> {
    let rest = l.strip_prefix("@@ ")?;
    let (ranges, _) = rest.split_once(" @@")?;
    let (o, n) = ranges.split_once(' ')?;
    let (os, ol) = parse_range(o);
    let (ns, nl) = parse_range(n);
    Some((os, ol, ns, nl))
}

fn strip_ab(p: &str) -> String {
    p.strip_prefix("a/")
        .or_else(|| p.strip_prefix("b/"))
        .unwrap_or(p)
        .to_string()
}

/// Parse unified diff text (`git diff`) into files and hunks with line numbers.
pub fn parse_diff(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let (mut old_no, mut new_no) = (0u32, 0u32);
    for l in text.lines() {
        if let Some(rest) = l.strip_prefix("diff --git ") {
            // `a/x b/x`: take the second half (paths with spaces are rare; the `+++` line wins).
            let path = rest
                .rsplit_once(" b/")
                .map(|(_, b)| b.to_string())
                .unwrap_or_else(|| strip_ab(rest));
            files.push(FileDiff {
                path,
                header: vec![l.to_string()],
                ..Default::default()
            });
            continue;
        }
        if files.is_empty() && (l.starts_with("--- ") || l.starts_with("@@")) {
            files.push(FileDiff::default());
        }
        let Some(f) = files.last_mut() else { continue };
        if let Some((os, ol, ns, nl)) = parse_hunk_header(l) {
            old_no = os;
            new_no = ns;
            f.hunks.push(Hunk {
                header: l.to_string(),
                old_start: os,
                old_len: ol,
                new_start: ns,
                new_len: nl,
                lines: Vec::new(),
            });
            continue;
        }
        if let Some(h) = f.hunks.last_mut() {
            let (kind, body) = match l.chars().next() {
                Some('+') => (LineKind::Add, &l[1..]),
                Some('-') => (LineKind::Del, &l[1..]),
                Some(' ') => (LineKind::Context, &l[1..]),
                // "\ No newline at end of file" and anything else.
                _ => continue,
            };
            let (o, n) = match kind {
                LineKind::Add => (None, Some(new_no)),
                LineKind::Del => (Some(old_no), None),
                LineKind::Context => (Some(old_no), Some(new_no)),
            };
            if kind != LineKind::Add {
                old_no += 1;
            }
            if kind != LineKind::Del {
                new_no += 1;
            }
            h.lines.push(DiffLine {
                kind,
                old_no: o,
                new_no: n,
                text: body.to_string(),
            });
            continue;
        }
        // Header area.
        if l.starts_with("new file mode") {
            f.is_new = true;
        } else if l.starts_with("deleted file mode") {
            f.is_deleted = true;
        } else if let Some(p) = l.strip_prefix("rename from ") {
            f.renamed_from = Some(p.to_string());
        } else if l.starts_with("Binary files") || l.starts_with("GIT binary patch") {
            f.binary = true;
        } else if let Some(p) = l.strip_prefix("+++ ") {
            if p != "/dev/null" {
                f.path = strip_ab(p);
            }
        } else if let Some(p) = l.strip_prefix("--- ") {
            if f.path.is_empty() && p != "/dev/null" {
                f.path = strip_ab(p);
            }
        }
        f.header.push(l.to_string());
    }
    files
}

/// Diff of `path` parsed into a [`FileDiff`] (`None` when unchanged). `path` stays the
/// repo-relative path even for untracked files.
pub fn file_hunks(repo: &Path, path: &str) -> Result<Option<FileDiff>, String> {
    let rel = relative(repo, path);
    let text = diff_file(repo, &rel)?;
    Ok(parse_diff(&text).into_iter().next().map(|mut f| {
        f.path = rel;
        f
    }))
}

/// A one-hunk patch for `git apply`.
pub fn hunk_patch(file: &FileDiff, hunk: usize) -> Option<String> {
    let h = file.hunks.get(hunk)?;
    let mut s = String::new();
    for l in &file.header {
        s.push_str(l);
        s.push('\n');
    }
    s.push_str(&h.header);
    s.push('\n');
    for l in &h.lines {
        s.push(match l.kind {
            LineKind::Add => '+',
            LineKind::Del => '-',
            LineKind::Context => ' ',
        });
        s.push_str(&l.text);
        s.push('\n');
    }
    Some(s)
}

fn apply_patch(repo: &Path, patch: &str, reverse: bool) -> Result<(), String> {
    use std::io::Write;
    let mut args = vec!["apply", "--recount", "--whitespace=nowarn"];
    if reverse {
        args.push("-R");
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("git: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(patch.as_bytes())
            .map_err(|e| format!("git apply: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git apply: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Reject one hunk: reverse-apply it to the working tree. `file` is a fresh parse of the
/// current diff.
pub fn reject_hunk(repo: &Path, file: &FileDiff, hunk: usize) -> Result<(), String> {
    let patch = hunk_patch(file, hunk).ok_or_else(|| "no such hunk".to_string())?;
    if file.path.split('/').any(|c| c == "..") || Path::new(&file.path).is_absolute() {
        return Err(format!("{} is outside the repository", file.path));
    }
    apply_patch(repo, &patch, true)
}

/// FNV-1a hash of the file's bytes; `None` when it does not exist.
pub fn content_hash(repo: &Path, path: &str) -> Option<u64> {
    let bytes = std::fs::read(repo.join(relative(repo, path))).ok()?;
    Some(hash_bytes(&bytes))
}

pub fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn safe_rel(repo: &Path, path: &str) -> Result<PathBuf, String> {
    let rel = relative(repo, path);
    if rel.split('/').any(|c| c == "..") || Path::new(&rel).is_absolute() {
        return Err(format!("{path} is outside the repository"));
    }
    Ok(repo.join(rel))
}

/// UTF-8 text of a file in the repo (`None` when missing or not text).
pub fn read_text(repo: &Path, path: &str) -> Option<String> {
    std::fs::read_to_string(safe_rel(repo, path).ok()?).ok()
}

/// Write `text` to a repo file, creating parent directories.
pub fn write_text(repo: &Path, path: &str, text: &str) -> Result<(), String> {
    let full = safe_rel(repo, path)?;
    if let Some(d) = full.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("create {}: {e}", d.display()))?;
    }
    std::fs::write(&full, text).map_err(|e| format!("write {}: {e}", full.display()))
}

/// Delete a repo file (missing is fine).
pub fn remove_text(repo: &Path, path: &str) -> Result<(), String> {
    let full = safe_rel(repo, path)?;
    match std::fs::remove_file(&full) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove {}: {e}", full.display())),
    }
}

/// Pure: the file text after reverse-applying `h` to `current`. The hunk's new-side lines
/// (context + additions) are located nearest to where the diff said they were; `None` when
/// they are not there any more.
pub fn reverse_hunk_text(current: &str, h: &Hunk) -> Option<String> {
    let lines: Vec<&str> = current.split_inclusive('\n').collect();
    let strip = |s: &str| s.strip_suffix('\n').unwrap_or(s).to_string();
    let new_side: Vec<&str> = h
        .lines
        .iter()
        .filter(|l| l.kind != LineKind::Del)
        .map(|l| l.text.as_str())
        .collect();
    let old_side: Vec<&str> = h
        .lines
        .iter()
        .filter(|l| l.kind != LineKind::Add)
        .map(|l| l.text.as_str())
        .collect();
    let want = (h.new_start as usize).saturating_sub(1);
    let at = if new_side.is_empty() {
        (h.new_start as usize).min(lines.len())
    } else {
        let n = new_side.len();
        (0..=lines.len().checked_sub(n)?)
            .filter(|&i| {
                lines[i..i + n]
                    .iter()
                    .zip(&new_side)
                    .all(|(a, b)| strip(a) == *b)
            })
            .min_by_key(|&i| i.abs_diff(want))?
    };
    let n = new_side.len();
    let mut out: String = lines[..at].concat();
    let tail_no_nl = n > 0 && !lines[at + n - 1].ends_with('\n');
    for (i, l) in old_side.iter().enumerate() {
        out.push_str(l);
        if !(tail_no_nl && i + 1 == old_side.len()) {
            out.push('\n');
        }
    }
    out.push_str(&lines[at + n..].concat());
    Some(out)
}

/// The file text that rejecting `hunk` would leave behind (empty for a new file rejected).
pub fn preview_reject(repo: &Path, file: &FileDiff, hunk: usize) -> Result<String, String> {
    let h = file.hunks.get(hunk).ok_or("no such hunk")?;
    let cur =
        read_text(repo, &file.path).ok_or_else(|| format!("{} is not readable", file.path))?;
    reverse_hunk_text(&cur, h).ok_or_else(|| "the hunk no longer applies".to_string())
}

/// Whether rejecting `hunk` would apply cleanly to the working tree right now.
pub fn reject_applies(repo: &Path, file: &FileDiff, hunk: usize) -> bool {
    use std::io::Write;
    let Some(patch) = hunk_patch(file, hunk) else {
        return false;
    };
    let Ok(mut child) = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["apply", "-R", "--check", "--recount", "--whitespace=nowarn"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut si) = child.stdin.take() {
        let _ = si.write_all(patch.as_bytes());
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

/// Staged renames (`git mv`) against HEAD, as parsed diffs with `renamed_from` set.
pub fn renames(repo: &Path) -> Vec<FileDiff> {
    git(repo, &["diff", "HEAD", "-M", "--diff-filter=R"])
        .map(|t| {
            parse_diff(&t)
                .into_iter()
                .filter(|f| f.renamed_from.is_some())
                .collect()
        })
        .unwrap_or_default()
}

fn move_path(repo: &Path, src: &str, dst: &str) -> Result<(), String> {
    let (s, d) = (safe_rel(repo, src)?, safe_rel(repo, dst)?);
    if !s.exists() {
        return Err(format!("{src} does not exist"));
    }
    if d.exists() {
        return Err(format!("{dst} already exists"));
    }
    if let Some(p) = d.parent() {
        std::fs::create_dir_all(p).map_err(|e| format!("create {}: {e}", p.display()))?;
    }
    std::fs::rename(&s, &d).map_err(|e| format!("rename {src}: {e}"))?;
    let _ = git(repo, &["add", "-A", "--", src, dst]);
    Ok(())
}

/// Reject a rename: move `to` back to `from`.
pub fn reject_rename(repo: &Path, from: &str, to: &str) -> Result<(), String> {
    move_path(repo, to, from)
}

/// Re-apply a rejected rename: move `from` to `to` again.
pub fn reapply_rename(repo: &Path, from: &str, to: &str) -> Result<(), String> {
    move_path(repo, from, to)
}

/// Rows for a side-by-side view: deletions pair with the additions that follow them.
pub fn side_by_side(h: &Hunk) -> Vec<(Option<&DiffLine>, Option<&DiffLine>)> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < h.lines.len() {
        match h.lines[i].kind {
            LineKind::Context => {
                rows.push((Some(&h.lines[i]), Some(&h.lines[i])));
                i += 1;
            }
            _ => {
                let mut dels = Vec::new();
                let mut adds = Vec::new();
                while i < h.lines.len() && h.lines[i].kind != LineKind::Context {
                    if h.lines[i].kind == LineKind::Del {
                        dels.push(&h.lines[i]);
                    } else {
                        adds.push(&h.lines[i]);
                    }
                    i += 1;
                }
                for k in 0..dels.len().max(adds.len()) {
                    rows.push((dels.get(k).copied(), adds.get(k).copied()));
                }
            }
        }
    }
    rows
}

/// Repo-relative form of `path` (unchanged when already relative or outside the repo).
/// Repo-relative form of `path`.
pub fn relative_path(repo: &Path, path: &str) -> String {
    relative(repo, path)
}

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
    fn hunks_parse_and_reject() {
        let r = repo("hunks");
        let base: String = (1..=30).map(|i| format!("line{i}\n")).collect();
        std::fs::write(r.join("b.txt"), &base).unwrap();
        initial_commit(&r).unwrap();
        let edited = base
            .replace("line3\n", "THREE\n")
            .replace("line25\n", "TWENTYFIVE\nextra\n");
        std::fs::write(r.join("b.txt"), &edited).unwrap();
        let f = file_hunks(&r, "b.txt").unwrap().unwrap();
        assert_eq!(f.path, "b.txt");
        assert_eq!(f.hunks.len(), 2, "{f:?}");
        assert_eq!((f.added(), f.removed()), (3, 2));
        let h0 = &f.hunks[0];
        assert_eq!(h0.old_start, 1);
        let del = h0.lines.iter().find(|l| l.kind == LineKind::Del).unwrap();
        assert_eq!((del.old_no, del.text.as_str()), (Some(3), "line3"));
        let add = h0.lines.iter().find(|l| l.kind == LineKind::Add).unwrap();
        assert_eq!(add.new_no, Some(3));
        assert_ne!(f.hunks[0].key(), f.hunks[1].key());
        let rows = side_by_side(&f.hunks[1]);
        assert!(rows
            .iter()
            .any(|(l, r)| l.is_some() && r.is_some() && l != r));
        // Reject the second hunk only.
        reject_hunk(&r, &f, 1).unwrap();
        let now = std::fs::read_to_string(r.join("b.txt")).unwrap();
        assert!(now.contains("THREE") && now.contains("line25") && !now.contains("extra"));
        let f2 = file_hunks(&r, "b.txt").unwrap().unwrap();
        assert_eq!(f2.hunks.len(), 1);
        assert_eq!(f2.hunks[0].key(), f.hunks[0].key());
        reject_hunk(&r, &f2, 0).unwrap();
        assert_eq!(std::fs::read_to_string(r.join("b.txt")).unwrap(), base);
        assert!(file_hunks(&r, "b.txt").unwrap().is_none());
        let _ = std::fs::remove_dir_all(r);
    }

    #[test]
    fn rejecting_new_file_hunk_removes_it() {
        let r = repo("hunknew");
        std::fs::write(r.join("n.txt"), "x\ny\n").unwrap();
        let f = file_hunks(&r, "n.txt").unwrap().unwrap();
        assert!(f.is_new && f.hunks.len() == 1, "{f:?}");
        reject_hunk(&r, &f, 0).unwrap();
        assert!(!r.join("n.txt").exists());
        let _ = std::fs::remove_dir_all(r);
    }

    #[test]
    fn reverse_text_preview_and_applies() {
        let r = repo("prev");
        let base: String = (1..=30).map(|i| format!("line{i}\n")).collect();
        std::fs::write(r.join("b.txt"), &base).unwrap();
        initial_commit(&r).unwrap();
        let edited = base
            .replace("line3\n", "THREE\n")
            .replace("line25\n", "TWENTYFIVE\nextra\n");
        std::fs::write(r.join("b.txt"), &edited).unwrap();
        let f = file_hunks(&r, "b.txt").unwrap().unwrap();
        let p = preview_reject(&r, &f, 1).unwrap();
        assert!(p.contains("THREE") && p.contains("line25\n") && !p.contains("extra"));
        assert_eq!(std::fs::read_to_string(r.join("b.txt")).unwrap(), edited);
        assert!(reject_applies(&r, &f, 0));
        // The pure preview matches what git really produces.
        reject_hunk(&r, &f, 1).unwrap();
        assert_eq!(std::fs::read_to_string(r.join("b.txt")).unwrap(), p);
        // Hunk text vanishes from disk: no longer applies, preview errors.
        std::fs::write(r.join("b.txt"), &base).unwrap();
        assert!(!reject_applies(&r, &f, 0));
        assert!(preview_reject(&r, &f, 0).is_err());
        assert_ne!(content_hash(&r, "b.txt"), content_hash(&r, "a.txt"));
        assert_eq!(content_hash(&r, "nope"), None);
        let _ = std::fs::remove_dir_all(r);
    }

    #[test]
    fn rename_parse_reject_and_reapply() {
        let r = repo("ren");
        let body: String = (1..=10).map(|i| format!("row{i}\n")).collect();
        std::fs::write(r.join("old.txt"), &body).unwrap();
        initial_commit(&r).unwrap();
        git(&r, &["mv", "old.txt", "new.txt"]).unwrap();
        let rs = renames(&r);
        assert_eq!(rs.len(), 1, "{rs:?}");
        assert_eq!(rs[0].renamed_from.as_deref(), Some("old.txt"));
        assert_eq!(rs[0].path, "new.txt");
        reject_rename(&r, "old.txt", "new.txt").unwrap();
        assert!(r.join("old.txt").exists() && !r.join("new.txt").exists());
        assert!(renames(&r).is_empty());
        reapply_rename(&r, "old.txt", "new.txt").unwrap();
        assert!(r.join("new.txt").exists() && !r.join("old.txt").exists());
        assert_eq!(renames(&r).len(), 1);
        assert!(reject_rename(&r, "../x", "new.txt").is_err());
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
