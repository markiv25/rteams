//! Off-thread ledger refresh: the UI asks for a rescan, a worker thread runs it (with the mtime
//! cache) and publishes the result; the UI only ever reads the latest snapshot.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::budget::{self, BudgetAlert};
use super::ledger::{now_ms, Ledger, Scanner};

#[derive(Default)]
struct Inner {
    ledger: Arc<Ledger>,
    /// Bumped on every published scan.
    version: u64,
    scanning: bool,
    loaded: bool,
}

/// Cheap to clone; all clones share one state.
#[derive(Clone)]
pub struct UsageState {
    scanner: Scanner,
    inner: Arc<Mutex<Inner>>,
}

impl UsageState {
    pub fn new(scanner: Scanner) -> Self {
        UsageState {
            scanner,
            inner: Arc::default(),
        }
    }

    /// Scans `<home>` and the real `~/.claude` / `~/.codex` session directories.
    pub fn for_home(home: impl Into<PathBuf>) -> Self {
        Self::new(Scanner::with_default_importers(home))
    }

    pub fn home(&self) -> &std::path::Path {
        &self.scanner.home
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Start a background scan unless one is already running. Returns whether one was started.
    pub fn refresh(&self) -> bool {
        {
            let mut g = self.lock();
            if g.scanning {
                return false;
            }
            g.scanning = true;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            let ledger = me.scanner.scan();
            let mut g = me.lock();
            g.ledger = Arc::new(ledger);
            g.version += 1;
            g.scanning = false;
            g.loaded = true;
        });
        true
    }

    /// Refresh when never loaded or the last scan is older than `max_age_ms`.
    pub fn refresh_if_stale(&self, max_age_ms: u64) -> bool {
        let stale = {
            let g = self.lock();
            !g.loaded || now_ms().saturating_sub(g.ledger.scanned_at_ms) > max_age_ms
        };
        stale && self.refresh()
    }

    pub fn is_scanning(&self) -> bool {
        self.lock().scanning
    }

    pub fn is_loaded(&self) -> bool {
        self.lock().loaded
    }

    pub fn version(&self) -> u64 {
        self.lock().version
    }

    /// Latest published ledger (empty before the first scan).
    pub fn snapshot(&self) -> Arc<Ledger> {
        self.lock().ledger.clone()
    }

    /// Newly crossed budget thresholds on the latest ledger (each is returned once). The shell
    /// calls this after a refresh completes (see `version()`) and turns each into a notification.
    pub fn budget_alerts(&self) -> Vec<BudgetAlert> {
        let (loaded, ledger) = {
            let g = self.lock();
            (g.loaded, g.ledger.clone())
        };
        if !loaded {
            return Vec::new();
        }
        budget::budget_alerts(&self.scanner.home, &ledger.records, now_ms())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_runs_off_thread_and_publishes() {
        let dir = tempfile::tempdir().unwrap();
        let runs = dir.path().join("t/runs/ada");
        std::fs::create_dir_all(&runs).unwrap();
        std::fs::write(
            runs.join("1700000000000.jsonl"),
            r#"{"type":"result","total_cost_usd":1.0,"usage":{"input_tokens":5,"output_tokens":5}}"#,
        )
        .unwrap();
        let st = UsageState::new(Scanner::app_only(dir.path()));
        assert!(!st.is_loaded());
        assert!(st.budget_alerts().is_empty());
        assert!(st.refresh());
        for _ in 0..200 {
            if st.is_loaded() && !st.is_scanning() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(st.snapshot().records.len(), 1);
        assert_eq!(st.version(), 1);
        assert!(!st.refresh_if_stale(60_000));
    }
}
