//! Live refresh: watch skill roots with `notify` and report when something changed.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

pub struct SkillWatcher {
    _watcher: RecommendedWatcher,
    rx: Receiver<()>,
    pending_since: Option<Instant>,
    pub watched: Vec<PathBuf>,
}

const DEBOUNCE: Duration = Duration::from_millis(300);

impl SkillWatcher {
    /// Watch every existing directory in `dirs` recursively. Missing dirs are skipped.
    pub fn start(dirs: &[PathBuf]) -> Result<SkillWatcher, String> {
        let (tx, rx) = channel();
        let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                if !matches!(ev.kind, notify::EventKind::Access(_)) {
                    let _ = tx.send(());
                }
            }
        })
        .map_err(|e| e.to_string())?;
        let mut watched = vec![];
        for d in dirs {
            if d.is_dir() && w.watch(d, RecursiveMode::Recursive).is_ok() {
                watched.push(d.clone());
            }
        }
        Ok(SkillWatcher {
            _watcher: w,
            rx,
            pending_since: None,
            watched,
        })
    }

    /// True once per burst of changes, after they settle for 300 ms. Call every frame.
    pub fn poll(&mut self) -> bool {
        while self.rx.try_recv().is_ok() {
            self.pending_since = Some(Instant::now());
        }
        match self.pending_since {
            Some(t) if t.elapsed() >= DEBOUNCE => {
                self.pending_since = None;
                true
            }
            _ => false,
        }
    }

    pub fn has_pending(&self) -> bool {
        self.pending_since.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_changes_in_temp_dir() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().to_path_buf();
        let mut w = SkillWatcher::start(&[dir.clone(), dir.join("missing")]).unwrap();
        assert_eq!(w.watched.len(), 1);
        std::fs::create_dir_all(dir.join("s")).unwrap();
        std::fs::write(dir.join("s/SKILL.md"), "x").unwrap();
        let start = Instant::now();
        let mut seen = false;
        while start.elapsed() < Duration::from_secs(5) {
            if w.poll() {
                seen = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(seen, "no change event");
    }
}
