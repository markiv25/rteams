//! Filesystem watcher: `notify` events debounced (300 ms) into a channel of `FsEvent`s.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

pub const DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsKind {
    Created,
    Changed,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsEvent {
    /// Absolute path.
    pub path: PathBuf,
    pub kind: FsKind,
}

pub struct FsWatcher {
    watcher: RecommendedWatcher,
    pub events: Receiver<FsEvent>,
}

fn classify(k: &EventKind) -> Option<FsKind> {
    match k {
        EventKind::Create(_) => Some(FsKind::Created),
        EventKind::Remove(_) => Some(FsKind::Deleted),
        EventKind::Access(_) => None,
        _ => Some(FsKind::Changed),
    }
}

/// Merge a new kind into a pending one (create then delete cancels to delete, etc.).
fn merge(old: FsKind, new: FsKind) -> FsKind {
    match (old, new) {
        (FsKind::Created, FsKind::Changed) => FsKind::Created,
        (_, k) => k,
    }
}

impl FsWatcher {
    /// `wake` is called after each debounced batch (e.g. `ctx.request_repaint`).
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> notify::Result<FsWatcher> {
        let (raw_tx, raw_rx) = mpsc::channel::<FsEvent>();
        let (tx, rx) = mpsc::channel::<FsEvent>();
        let watcher = RecommendedWatcher::new(
            move |r: notify::Result<Event>| {
                if let Ok(ev) = r {
                    if let Some(kind) = classify(&ev.kind) {
                        for path in ev.paths {
                            let _ = raw_tx.send(FsEvent { path, kind });
                        }
                    }
                }
            },
            notify::Config::default(),
        )?;
        let wake = Arc::new(wake);
        std::thread::spawn(move || debounce_loop(raw_rx, tx, wake));
        Ok(FsWatcher {
            watcher,
            events: rx,
        })
    }

    /// Watch a directory (non-recursive: the editor watches the dirs of open files and of
    /// expanded tree folders).
    pub fn watch_dir(&mut self, dir: &Path) {
        let _ = self.watcher.watch(dir, RecursiveMode::NonRecursive);
    }

    pub fn unwatch_dir(&mut self, dir: &Path) {
        let _ = self.watcher.unwatch(dir);
    }
}

fn debounce_loop(raw: Receiver<FsEvent>, out: Sender<FsEvent>, wake: Arc<dyn Fn() + Send + Sync>) {
    let mut pending: HashMap<PathBuf, FsKind> = HashMap::new();
    let mut last = Instant::now();
    loop {
        let wait = if pending.is_empty() {
            Duration::from_secs(3600)
        } else {
            DEBOUNCE
        };
        match raw.recv_timeout(wait) {
            Ok(ev) => {
                let k = pending
                    .get(&ev.path)
                    .map_or(ev.kind, |&o| merge(o, ev.kind));
                pending.insert(ev.path, k);
                last = Instant::now();
                if last.elapsed() < DEBOUNCE {
                    continue;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if !pending.is_empty() && last.elapsed() >= DEBOUNCE {
            let mut sent = false;
            for (path, kind) in pending.drain() {
                // Trust the filesystem over the raw kind (renames and atomic saves are noisy).
                let kind = match (path.exists(), kind) {
                    (false, _) => FsKind::Deleted,
                    (true, FsKind::Created) => FsKind::Created,
                    (true, _) => FsKind::Changed,
                };
                sent |= out.send(FsEvent { path, kind }).is_ok();
            }
            if sent {
                wake();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_changes_and_deletes() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().canonicalize().unwrap();
        let mut w = FsWatcher::new(|| {}).unwrap();
        w.watch_dir(&dir);
        let f = dir.join("a.txt");
        std::fs::write(&f, "1").unwrap();
        let ev = w
            .events
            .recv_timeout(Duration::from_secs(5))
            .expect("create event");
        assert_eq!(ev.path, f);
        std::thread::sleep(Duration::from_millis(500));
        std::fs::remove_file(&f).unwrap();
        let mut deleted = false;
        while let Ok(ev) = w.events.recv_timeout(Duration::from_secs(5)) {
            if ev.path == f && ev.kind == FsKind::Deleted {
                deleted = true;
                break;
            }
        }
        assert!(deleted);
    }
}
