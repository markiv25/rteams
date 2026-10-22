//! Read / dismissed state: `<home>/announcements/state.json`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Cap on remembered ids so the file cannot grow without bound.
pub const MAX_STATE_IDS: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub schema_version: u32,
    pub read: Vec<String>,
    pub dismissed: Vec<String>,
    /// Ids already offered by the startup auto-open (never offered twice).
    pub auto_claimed: Vec<String>,
    /// Unix ms of the first time the app saw the feed; gates `show_to_new_users`.
    pub first_app_opened_at: i64,
}

impl Default for State {
    fn default() -> Self {
        State {
            schema_version: 1,
            read: Vec::new(),
            dismissed: Vec::new(),
            auto_claimed: Vec::new(),
            first_app_opened_at: 0,
        }
    }
}

fn push_capped(v: &mut Vec<String>, id: &str) {
    if !v.iter().any(|x| x == id) {
        v.push(id.to_string());
        if v.len() > MAX_STATE_IDS {
            v.remove(0);
        }
    }
}

impl State {
    pub fn path(home: &Path) -> PathBuf {
        home.join("announcements").join("state.json")
    }

    /// Load, stamping `first_app_opened_at` with `now_ms` the first time (persisted).
    pub fn load(home: &Path, now_ms: i64) -> State {
        let mut s: State = std::fs::read(Self::path(home))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        if s.first_app_opened_at == 0 {
            s.first_app_opened_at = now_ms;
            s.save(home);
        }
        s
    }

    pub fn save(&self, home: &Path) {
        if let Ok(b) = serde_json::to_vec_pretty(self) {
            let _ = crate::atomic::write(&Self::path(home), &b);
        }
    }

    pub fn is_read(&self, id: &str) -> bool {
        self.read.iter().any(|x| x == id)
    }

    pub fn is_dismissed(&self, id: &str) -> bool {
        self.dismissed.iter().any(|x| x == id)
    }

    pub fn is_auto_claimed(&self, id: &str) -> bool {
        self.auto_claimed.iter().any(|x| x == id)
    }

    pub fn claim_auto(&mut self, id: &str) {
        push_capped(&mut self.auto_claimed, id);
    }

    pub fn mark_read(&mut self, id: &str) {
        push_capped(&mut self.read, id);
    }

    pub fn dismiss(&mut self, id: &str) {
        push_capped(&mut self.dismissed, id);
        push_capped(&mut self.read, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_open_is_stamped_once_and_state_round_trips() {
        let d = tempfile::tempdir().unwrap();
        let mut s = State::load(d.path(), 1000);
        assert_eq!(s.first_app_opened_at, 1000);
        s.mark_read("a");
        s.dismiss("b");
        s.save(d.path());
        let s2 = State::load(d.path(), 9999);
        assert_eq!(s2.first_app_opened_at, 1000);
        assert!(
            s2.is_read("a") && s2.is_read("b") && s2.is_dismissed("b") && !s2.is_dismissed("a")
        );
    }

    #[test]
    fn ids_are_capped() {
        let mut s = State::default();
        for i in 0..MAX_STATE_IDS + 10 {
            s.mark_read(&i.to_string());
        }
        assert_eq!(s.read.len(), MAX_STATE_IDS);
        assert!(!s.is_read("0") && s.is_read(&(MAX_STATE_IDS + 9).to_string()));
    }
}
