//! Update check. Deliberately passive: it reads a configurable release feed, compares semver, and
//! reports. It never downloads or replaces a binary; the GUI offers "Download", which opens the
//! release page in the browser. Default feed URL is empty, which disables the check (this fork has
//! no release channel yet).
//!
//! Feed formats (JSON): `{"version","notes","url","published_at"}` or a GitHub
//! `releases/latest` document (`tag_name`, `body`, `html_url`). `url` must be https.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::AppConfig;

/// Minimum time between automatic checks.
pub const MIN_INTERVAL_MS: u64 = 24 * 60 * 60 * 1000;
const FEED_CAP: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    /// No feed URL configured.
    Disabled,
    Fetch(String),
    Parse(String),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Disabled => write!(f, "update feed is not configured"),
            UpdateError::Fetch(e) => write!(f, "could not fetch update feed: {e}"),
            UpdateError::Parse(e) => write!(f, "bad update feed: {e}"),
        }
    }
}

impl std::error::Error for UpdateError {}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub available: bool,
    pub notes: String,
    /// https release page to open in the browser (may be empty).
    pub url: String,
    pub published_at: String,
}

/// `general.update_feed_url` (kept in the config's passthrough fields; empty = disabled).
pub fn feed_url_from_config(cfg: &AppConfig) -> String {
    cfg.general
        .extra
        .get("update_feed_url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

fn parse_version(v: &str) -> Option<semver::Version> {
    semver::Version::parse(v.trim().trim_start_matches(['v', 'V'])).ok()
}

/// Is `candidate` a strictly newer semver than `current`? Unparsable versions are never newer.
/// Pre-releases of a newer core version count; `1.0.0-rc.1` is older than `1.0.0`.
pub fn is_newer(current: &str, candidate: &str) -> bool {
    match (parse_version(current), parse_version(candidate)) {
        (Some(c), Some(n)) => n > c,
        _ => false,
    }
}

fn str_of<'a>(v: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .unwrap_or("")
}

/// Parse a feed document and compare against `current`.
pub fn parse_feed(json: &str, current: &str) -> Result<UpdateInfo, UpdateError> {
    let v: Value = serde_json::from_str(json).map_err(|e| UpdateError::Parse(e.to_string()))?;
    let latest = str_of(&v, &["version", "tag_name"])
        .trim()
        .trim_start_matches(['v', 'V'])
        .to_string();
    if parse_version(&latest).is_none() {
        return Err(UpdateError::Parse(format!("invalid version {latest:?}")));
    }
    let url = str_of(&v, &["url", "html_url"]);
    Ok(UpdateInfo {
        current: current.to_string(),
        available: is_newer(current, &latest),
        notes: str_of(&v, &["notes", "body"]).to_string(),
        url: if url.starts_with("https://") {
            url.to_string()
        } else {
            String::new()
        },
        published_at: str_of(&v, &["published_at"]).to_string(),
        latest,
    })
}

/// Fetch the feed text: `file://` path (tests, air-gapped mirrors) or http(s) via curl.
pub fn fetch_feed(url: &str) -> Result<String, UpdateError> {
    let bytes = if let Some(p) = url.strip_prefix("file://") {
        std::fs::read(p).map_err(|e| UpdateError::Fetch(e.to_string()))?
    } else if url.starts_with("https://") || url.starts_with("http://") {
        crate::http::get_bytes(url, Duration::from_secs(10))
            .map_err(|e| UpdateError::Fetch(e.to_string()))?
    } else {
        return Err(UpdateError::Fetch("unsupported feed URL scheme".into()));
    };
    if bytes.len() > FEED_CAP {
        return Err(UpdateError::Fetch("feed too large".into()));
    }
    String::from_utf8(bytes).map_err(|e| UpdateError::Parse(e.to_string()))
}

/// Check `feed_url` against `current_version`. Blocking; call from a worker thread.
pub fn check(feed_url: &str, current_version: &str) -> Result<UpdateInfo, UpdateError> {
    if feed_url.trim().is_empty() {
        return Err(UpdateError::Disabled);
    }
    parse_feed(&fetch_feed(feed_url.trim())?, current_version)
}

// ---- persisted check state: <home>/updates/state.json ----

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub schema_version: u32,
    pub last_check_ms: u64,
    /// Version the user dismissed the banner for.
    pub dismissed_version: String,
}

fn state_path(home: &Path) -> PathBuf {
    home.join("updates").join("state.json")
}

pub fn load_state(home: &Path) -> State {
    std::fs::read(state_path(home))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_state(home: &Path, s: &State) {
    let mut s = s.clone();
    s.schema_version = 1;
    if let Ok(b) = serde_json::to_vec_pretty(&s) {
        let _ = crate::atomic::write(&state_path(home), &b);
    }
}

/// Automatic checks run at most once per 24 h.
pub fn should_check(home: &Path, now_ms: u64) -> bool {
    now_ms.saturating_sub(load_state(home).last_check_ms) >= MIN_INTERVAL_MS
}

pub fn record_check(home: &Path, now_ms: u64) {
    let mut s = load_state(home);
    s.last_check_ms = now_ms;
    save_state(home, &s);
}

pub fn dismiss(home: &Path, version: &str) {
    let mut s = load_state(home);
    s.dismissed_version = version.to_string();
    save_state(home, &s);
}

pub fn is_dismissed(home: &Path, version: &str) -> bool {
    load_state(home).dismissed_version == version
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_compare() {
        assert!(is_newer("0.1.0", "0.1.1"));
        assert!(is_newer("0.9.0", "v0.10.0"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("1.0.0", "1.0.0-rc.1"));
        assert!(is_newer("1.0.0-rc.1", "1.0.0"));
        assert!(!is_newer("1.0.0", "garbage"));
    }

    #[test]
    fn parses_both_feed_shapes() {
        let a = parse_feed(
            r#"{"version":"0.3.0","notes":"- fast","url":"https://x.test/r","published_at":"2026-01-01"}"#,
            "0.2.0",
        )
        .unwrap();
        assert!(a.available);
        assert_eq!(
            (a.latest.as_str(), a.notes.as_str(), a.url.as_str()),
            ("0.3.0", "- fast", "https://x.test/r")
        );
        let g = parse_feed(
            r#"{"tag_name":"v0.2.0","body":"b","html_url":"https://github.com/o/r/releases/tag/v0.2.0"}"#,
            "0.2.0",
        )
        .unwrap();
        assert!(!g.available);
        assert_eq!(g.latest, "0.2.0");
    }

    #[test]
    fn rejects_non_https_release_url_and_bad_versions() {
        let i = parse_feed(
            r#"{"version":"9.0.0","url":"javascript:alert(1)"}"#,
            "1.0.0",
        )
        .unwrap();
        assert_eq!(i.url, "");
        assert!(matches!(
            parse_feed(r#"{"version":"x"}"#, "1.0.0"),
            Err(UpdateError::Parse(_))
        ));
        assert!(matches!(
            parse_feed("nope", "1.0.0"),
            Err(UpdateError::Parse(_))
        ));
    }

    #[test]
    fn check_disabled_and_file_feed() {
        assert_eq!(check("", "1.0.0"), Err(UpdateError::Disabled));
        assert_eq!(check("  ", "1.0.0"), Err(UpdateError::Disabled));
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("feed.json");
        std::fs::write(&f, r#"{"version":"2.0.0","url":"https://x.test/r"}"#).unwrap();
        let i = check(&format!("file://{}", f.display()), "1.0.0").unwrap();
        assert!(i.available);
        assert!(matches!(
            check("ftp://x", "1.0.0"),
            Err(UpdateError::Fetch(_))
        ));
    }

    #[test]
    fn state_interval_and_dismiss() {
        let d = tempfile::tempdir().unwrap();
        assert!(should_check(d.path(), 1_000_000_000));
        record_check(d.path(), 1_000_000_000);
        assert!(!should_check(d.path(), 1_000_000_000 + 1000));
        assert!(should_check(d.path(), 1_000_000_000 + MIN_INTERVAL_MS));
        dismiss(d.path(), "2.0.0");
        assert!(is_dismissed(d.path(), "2.0.0"));
        assert!(!is_dismissed(d.path(), "2.0.1"));
        assert_eq!(
            load_state(d.path()).last_check_ms,
            1_000_000_000,
            "dismiss keeps last_check"
        );
    }
}
