//! Announcements ("News"): a configurable JSON feed, cached under `<home>/announcements/`, with
//! read/dismissed state and a show policy (publish time, expiry, app-version window). Disabled
//! when no feed URL is configured (`general.announcements_feed_url`, default empty).

pub mod feed;
pub mod policy;
pub mod state;

use std::path::Path;

use serde_json::Value;

use crate::config::AppConfig;
use feed::Announcement;
use state::State;

/// `general.announcements_feed_url` from the config's passthrough fields; empty = disabled.
pub fn feed_url_from_config(cfg: &AppConfig) -> String {
    cfg.general
        .extra
        .get("announcements_feed_url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Everything the news button/dialog render from.
#[derive(Debug, Clone, Default)]
pub struct Model {
    pub items: Vec<Announcement>,
    pub state: State,
    /// The last refresh failed; `items` come from the cache.
    pub offline: bool,
    pub enabled: bool,
}

impl Model {
    /// Load the cache and state only (no network).
    pub fn load(home: &Path, feed_url: &str, now_ms: i64) -> Model {
        Model {
            items: feed::load_cache(home).map(|f| f.items).unwrap_or_default(),
            state: State::load(home, now_ms),
            offline: false,
            enabled: !feed_url.trim().is_empty(),
        }
    }

    /// Fetch the feed (blocking; worker thread). A failure keeps the cached items and sets
    /// `offline`.
    pub fn refresh(home: &Path, feed_url: &str, now_ms: i64) -> Model {
        let mut m = Model::load(home, feed_url, now_ms);
        if !m.enabled {
            return m;
        }
        match feed::refresh(home, feed_url) {
            Ok(f) => m.items = f.items,
            Err(_) => m.offline = true,
        }
        m
    }

    pub fn unread(&self, now_ms: i64, app_version: &str) -> usize {
        if !self.enabled {
            return 0;
        }
        policy::unread_count(&self.items, &self.state, now_ms, app_version)
    }

    pub fn listed(&self, now_ms: i64, app_version: &str) -> Vec<&Announcement> {
        policy::listed(&self.items, now_ms, app_version)
    }

    pub fn mark_read(&mut self, home: &Path, id: &str) {
        self.state.mark_read(id);
        self.state.save(home);
    }

    pub fn mark_all_read(&mut self, home: &Path, now_ms: i64, app_version: &str) {
        let ids: Vec<String> = policy::listed(&self.items, now_ms, app_version)
            .iter()
            .map(|a| a.id.clone())
            .collect();
        for id in ids {
            self.state.mark_read(&id);
        }
        self.state.save(home);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_without_url_and_offline_fallback() {
        let d = tempfile::tempdir().unwrap();
        let m = Model::refresh(d.path(), "", 1000);
        assert!(!m.enabled && m.unread(2000, "1.0.0") == 0);

        let src = d.path().join("f.json");
        std::fs::write(
            &src,
            r#"{"items":[{"id":"a","title":"A","published_at":1}]}"#,
        )
        .unwrap();
        let url = format!("file://{}", src.display());
        let mut m = Model::refresh(d.path(), &url, 1000);
        assert_eq!((m.offline, m.unread(2000, "1.0.0")), (false, 1));
        m.mark_all_read(d.path(), 2000, "1.0.0");
        assert_eq!(Model::load(d.path(), &url, 3000).unread(3000, "1.0.0"), 0);

        let off = Model::refresh(d.path(), "file:///nonexistent.json", 4000);
        assert!(
            off.offline && off.items.len() == 1,
            "cache survives failure"
        );
    }
}
