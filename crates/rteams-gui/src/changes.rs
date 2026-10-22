//! Change summaries computed off the UI thread: per task (kanban badge) and per agent (member
//! dialog). The original derives both from agent session logs; rteams does the same through
//! `runs::changed_files` (files written by Edit/Write tool calls) and diffs them against HEAD.
//! Results are cached under a fingerprint of HEAD and the touched files' mtimes and sizes, so
//! an unchanged repo costs no git work on later passes.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::git;
use crate::runs::{self, RunKind};

const MAX_FILES: usize = 200;
const REFRESH: Duration = Duration::from_secs(3);

/// Files changed and lines added/removed by one task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Summary {
    pub files: usize,
    pub added: usize,
    pub removed: usize,
}

/// `(+added, -removed)` lines of a unified diff, not counting the file headers.
pub fn diff_counts(diff: &str) -> (usize, usize) {
    let mut c = (0, 0);
    for l in diff.lines() {
        if l.starts_with('+') && !l.starts_with("+++") {
            c.0 += 1;
        } else if l.starts_with('-') && !l.starts_with("---") {
            c.1 += 1;
        }
    }
    c
}

/// Per-file counts for the paths that really differ from HEAD (unchanged files drop out).
pub fn file_counts(repo: &Path, paths: &[String]) -> Vec<(String, usize, usize)> {
    paths
        .iter()
        .take(MAX_FILES)
        .filter_map(|p| {
            let d = git::diff_file(repo, p).ok()?;
            let (a, r) = diff_counts(&d);
            (a + r > 0).then(|| (p.clone(), a, r))
        })
        .collect()
}

pub fn summarize(repo: &Path, paths: &[String]) -> Summary {
    let f = file_counts(repo, paths);
    Summary {
        files: f.len(),
        added: f.iter().map(|x| x.1).sum(),
        removed: f.iter().map(|x| x.2).sum(),
    }
}

fn is_edit(name: &str) -> bool {
    matches!(name, "Edit" | "Write" | "MultiEdit" | "NotebookEdit")
}

/// Fingerprint of everything a summary depends on.
fn fingerprint(repo: &Path, paths: &[String]) -> u64 {
    let mut h = DefaultHasher::new();
    git::head(repo).hash(&mut h);
    for p in paths {
        p.hash(&mut h);
        let full = if Path::new(p).is_absolute() {
            PathBuf::from(p)
        } else {
            repo.join(p)
        };
        match std::fs::metadata(&full) {
            Ok(m) => (m.len(), m.modified().ok()).hash(&mut h),
            Err(_) => 0u8.hash(&mut h),
        }
    }
    h.finish()
}

/// Summaries per task id, with the fingerprint each was computed under.
#[derive(Debug, Clone, Default)]
pub struct Cache {
    keys: HashMap<String, u64>,
    out: HashMap<String, Summary>,
}

impl Cache {
    pub fn get(&self, task: &str) -> Option<Summary> {
        self.out.get(task).copied().filter(|s| s.files > 0)
    }

    /// Recompute the tasks whose fingerprint moved; returns how many were recomputed.
    pub fn refresh(&mut self, home: &Path, team: &str, repo: &Path, tasks: &[String]) -> usize {
        let mut n = 0;
        for t in tasks {
            let paths = runs::changed_files(home, team, t);
            let key = fingerprint(repo, &paths);
            if self.keys.get(t) == Some(&key) {
                continue;
            }
            self.out.insert(t.clone(), summarize(repo, &paths));
            self.keys.insert(t.clone(), key);
            n += 1;
        }
        n
    }
}

type Paths = (PathBuf, String, PathBuf);

/// Background-refreshed summaries for the board. `poll` is cheap and never runs git itself.
#[derive(Debug, Default)]
pub struct ChangeSummaries {
    paths: Option<Paths>,
    shared: Arc<Mutex<Cache>>,
    busy: Arc<AtomicBool>,
    last: Option<Instant>,
}

impl ChangeSummaries {
    pub fn set_paths(&mut self, home: &Path, team: &str, repo: Option<&Path>) {
        let new = repo.map(|r| (home.to_path_buf(), team.to_string(), r.to_path_buf()));
        if new != self.paths {
            self.paths = new;
            self.last = None;
            *self.shared.lock().unwrap_or_else(|e| e.into_inner()) = Cache::default();
        }
    }

    /// Test hook: pretend a pass already produced `s` for `task` (and hold off real passes).
    #[cfg(test)]
    pub fn seed(&mut self, task: &str, s: Summary) {
        self.paths = Some(("/x".into(), "t".into(), "/x".into()));
        self.last = Some(Instant::now());
        self.shared.lock().unwrap().out.insert(task.to_string(), s);
    }

    pub fn get(&self, task: &str) -> Option<Summary> {
        self.paths.as_ref()?;
        self.shared.lock().ok()?.get(task)
    }

    /// Start a background pass over `tasks` when the last one is old enough.
    pub fn poll(&mut self, ctx: &eframe::egui::Context, tasks: Vec<String>) {
        let Some((home, team, repo)) = self.paths.clone() else {
            return;
        };
        ctx.request_repaint_after(REFRESH);
        if self.busy.load(Ordering::Acquire) || self.last.is_some_and(|l| l.elapsed() < REFRESH) {
            return;
        }
        self.last = Some(Instant::now());
        self.busy.store(true, Ordering::Release);
        let shared = Arc::clone(&self.shared);
        let busy = Arc::clone(&self.busy);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut cache = shared.lock().map(|c| c.clone()).unwrap_or_default();
            if cache.refresh(&home, &team, &repo, &tasks) > 0 {
                if let Ok(mut s) = shared.lock() {
                    *s = cache;
                }
                ctx.request_repaint();
            }
            busy.store(false, Ordering::Release);
        });
    }
}

/// One file an agent changed, with the tasks it was changed under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFile {
    pub path: String,
    pub tasks: Vec<String>,
    pub added: usize,
    pub removed: usize,
}

/// Paths written by `member`'s tool calls, with the task of each segment (first-touched order).
pub fn member_paths(home: &Path, team: &str, member: &str) -> Vec<(String, Vec<String>)> {
    let dir = home.join(team).join("runs").join(member);
    let mut order: Vec<String> = Vec::new();
    let mut tasks: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for seg in runs::task_segments(&dir) {
        for e in &seg.events {
            let RunKind::ToolUse { name, input } = &e.kind else {
                continue;
            };
            if !is_edit(name) {
                continue;
            }
            let p = ["file_path", "notebook_path", "path"]
                .iter()
                .find_map(|k| input.get(*k).and_then(Value::as_str));
            let Some(p) = p else { continue };
            let t = tasks.entry(p.to_string()).or_insert_with(|| {
                order.push(p.to_string());
                Vec::new()
            });
            if !seg.task.is_empty() && !t.contains(&seg.task) {
                t.push(seg.task.clone());
            }
        }
    }
    order
        .into_iter()
        .map(|p| {
            let t = tasks.remove(&p).unwrap_or_default();
            (p, t)
        })
        .collect()
}

/// All files `member` changed across their tasks that still differ from HEAD.
pub fn agent_changes(home: &Path, team: &str, repo: &Path, member: &str) -> Vec<AgentFile> {
    let found = member_paths(home, team, member);
    let paths: Vec<String> = found.iter().map(|f| f.0.clone()).collect();
    file_counts(repo, &paths)
        .into_iter()
        .map(|(path, added, removed)| {
            let tasks = found
                .iter()
                .find(|f| f.0 == path)
                .map(|f| f.1.clone())
                .unwrap_or_default();
            AgentFile {
                path,
                tasks,
                added,
                removed,
            }
        })
        .collect()
}

/// A value computed on a worker thread; `take` is non-blocking.
#[derive(Debug)]
pub struct Job<T> {
    slot: Arc<Mutex<Option<T>>>,
}

impl<T: Send + 'static> Job<T> {
    pub fn spawn(ctx: &eframe::egui::Context, f: impl FnOnce() -> T + Send + 'static) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let s = Arc::clone(&slot);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let v = f();
            if let Ok(mut g) = s.lock() {
                *g = Some(v);
            }
            ctx.request_repaint();
        });
        Job { slot }
    }

    pub fn take(&self) -> Option<T> {
        self.slot.lock().ok()?.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rteams-changes-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn repo(dir: &Path) -> PathBuf {
        let r = dir.join("proj");
        git::init_repo(&r).unwrap();
        std::fs::write(r.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(r.join("b.txt"), "x\n").unwrap();
        git::initial_commit(&r).unwrap();
        r
    }

    /// A run file whose segments edit `files` while on `task`.
    fn run(home: &Path, member: &str, task: &str, files: &[&str]) {
        let dir = home.join("t").join("runs").join(member);
        std::fs::create_dir_all(&dir).unwrap();
        let line = |name: &str, input: serde_json::Value| {
            serde_json::json!({"type":"assistant","message":{"content":[
                {"type":"tool_use","name":name,"input":input}]}})
            .to_string()
        };
        let mut lines = vec![line(
            "mcp__rteams__task_start",
            serde_json::json!({ "task": task }),
        )];
        for f in files {
            let p = home.join("proj").join(f).to_string_lossy().into_owned();
            lines.push(line("Edit", serde_json::json!({ "file_path": p })));
        }
        std::fs::write(dir.join(format!("{task}.jsonl")), lines.join("\n")).unwrap();
    }

    #[test]
    fn counts_skip_headers() {
        assert_eq!(
            diff_counts("--- a/x\n+++ b/x\n@@\n-a\n+b\n+c\n ctx\n"),
            (2, 1)
        );
    }

    #[test]
    fn summarizes_changed_files_only() {
        let d = tmp("sum");
        let r = repo(&d);
        std::fs::write(r.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        std::fs::write(r.join("new.txt"), "n1\nn2\n").unwrap();
        let paths: Vec<String> = ["a.txt", "b.txt", "new.txt", "gone.txt"]
            .map(String::from)
            .into();
        let s = summarize(&r, &paths);
        assert_eq!(
            s,
            Summary {
                files: 2,
                added: 4,
                removed: 1
            }
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn cache_reuses_until_files_or_head_move() {
        let d = tmp("cache");
        let r = repo(&d);
        let home = d.clone();
        run(&home, "ada", "1", &["a.txt"]);
        std::fs::write(r.join("a.txt"), "one\nchanged\n").unwrap();
        let mut c = Cache::default();
        let tasks = vec!["1".to_string()];
        assert_eq!(c.refresh(&home, "t", &r, &tasks), 1);
        assert_eq!(c.get("1").unwrap().files, 1);
        assert_eq!(c.refresh(&home, "t", &r, &tasks), 0, "unchanged: cached");
        std::fs::write(r.join("a.txt"), "one\nchanged\nmore lines\n").unwrap();
        assert_eq!(c.refresh(&home, "t", &r, &tasks), 1, "file size moved");
        assert_eq!(c.get("1").unwrap().added, 2);
        // Committing moves HEAD and clears the diff: the task no longer has changes.
        let ok = Command::new("git")
            .arg("-C")
            .arg(&r)
            .args(["commit", "-qam", "x"])
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap()
            .success();
        assert!(ok);
        assert_eq!(c.refresh(&home, "t", &r, &tasks), 1, "HEAD moved");
        assert!(c.get("1").is_none());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn aggregates_per_member_across_tasks() {
        let d = tmp("agent");
        let r = repo(&d);
        let home = d.clone();
        run(&home, "ada", "1", &["a.txt"]);
        run(&home, "ada", "2", &["a.txt", "b.txt"]);
        run(&home, "bob", "3", &["b.txt"]);
        std::fs::write(r.join("a.txt"), "z\n").unwrap();
        std::fs::write(r.join("b.txt"), "x\ny\n").unwrap();
        let ada = agent_changes(&home, "t", &r, "ada");
        let names: Vec<&str> = ada
            .iter()
            .map(|f| f.path.rsplit('/').next().unwrap())
            .collect();
        assert_eq!(names, ["a.txt", "b.txt"]);
        assert_eq!(ada[0].tasks, ["1", "2"]);
        assert_eq!(ada[1].tasks, ["2"]);
        let bob = agent_changes(&home, "t", &r, "bob");
        assert_eq!(bob.len(), 1);
        assert_eq!(bob[0].tasks, ["3"]);
        assert!(agent_changes(&home, "t", &r, "nobody").is_empty());
        let _ = std::fs::remove_dir_all(d);
    }
}
