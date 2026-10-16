//! Announcement feed: parsing with size caps, fetch (https / file://) and the on-disk cache
//! `<home>/announcements/feed.json`.
//!
//! Feed JSON: `{"items":[{"id","title","summary","body"(markdown),"published_at","expires_at",
//! "min_app_version","max_app_version","show_to_new_users","cover_url","url"}]}`. Timestamps are
//! unix milliseconds or RFC 3339 strings.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_FEED_BYTES: usize = 1024 * 1024;
pub const MAX_ITEMS: usize = 100;
pub const MAX_BODY_BYTES: usize = 32 * 1024;
pub const MAX_TITLE_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Announcement {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub body: String,
    /// Unix ms; items without one are treated as published at 0.
    pub published_at: i64,
    pub expires_at: Option<i64>,
    pub min_app_version: Option<String>,
    pub max_app_version: Option<String>,
    pub show_to_new_users: bool,
    /// https cover image (not fetched by the core; the GUI may choose to).
    pub cover_url: Option<String>,
    /// https link to the full article.
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feed {
    pub items: Vec<Announcement>,
}

fn ts(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.timestamp_millis()),
        _ => None,
    }
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

fn https(v: Option<String>) -> Option<String> {
    v.filter(|u| u.starts_with("https://"))
}

fn clip(mut t: String, max: usize) -> String {
    if t.len() > max {
        let mut n = max;
        while !t.is_char_boundary(n) {
            n -= 1;
        }
        t.truncate(n);
    }
    t
}

/// Parse a feed. Invalid items (no id/title) and duplicate ids are skipped; sizes are capped.
pub fn parse(json: &str) -> Result<Feed, String> {
    if json.len() > MAX_FEED_BYTES {
        return Err("feed too large".into());
    }
    let v: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let arr = v
        .get("items")
        .and_then(Value::as_array)
        .or_else(|| v.as_array())
        .ok_or("feed has no items array")?;
    let mut items: Vec<Announcement> = Vec::new();
    for it in arr.iter().take(MAX_ITEMS) {
        let (Some(id), Some(title)) = (s(it, "id"), s(it, "title")) else {
            continue;
        };
        if id.is_empty() || title.is_empty() || items.iter().any(|x| x.id == id) {
            continue;
        }
        items.push(Announcement {
            id: clip(id, 128),
            title: clip(title, MAX_TITLE_CHARS),
            summary: clip(s(it, "summary").unwrap_or_default(), 1000),
            body: clip(s(it, "body").unwrap_or_default(), MAX_BODY_BYTES),
            published_at: ts(it.get("published_at")).unwrap_or(0),
            expires_at: ts(it.get("expires_at")),
            min_app_version: s(it, "min_app_version"),
            max_app_version: s(it, "max_app_version"),
            show_to_new_users: it
                .get("show_to_new_users")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            cover_url: https(s(it, "cover_url")),
            url: https(s(it, "url")),
        });
    }
    Ok(Feed { items })
}

pub fn cache_path(home: &Path) -> PathBuf {
    home.join("announcements").join("feed.json")
}

pub fn load_cache(home: &Path) -> Option<Feed> {
    serde_json::from_slice(&std::fs::read(cache_path(home)).ok()?).ok()
}

fn save_cache(home: &Path, feed: &Feed) {
    if let Ok(b) = serde_json::to_vec_pretty(feed) {
        let _ = crate::atomic::write(&cache_path(home), &b);
    }
}

/// Fetch and parse the feed at `url` (https, http or `file://`). Blocking.
pub fn fetch(url: &str) -> Result<Feed, String> {
    let bytes = if let Some(p) = url.strip_prefix("file://") {
        std::fs::read(p).map_err(|e| e.to_string())?
    } else if url.starts_with("https://") || url.starts_with("http://") {
        crate::http::get_bytes(url, Duration::from_secs(10)).map_err(|e| e.to_string())?
    } else {
        return Err("unsupported feed URL scheme".into());
    };
    parse(&String::from_utf8(bytes).map_err(|e| e.to_string())?)
}

/// Fetch and, on success, replace the cache. On failure the cached feed (if any) is still
/// readable with `load_cache`; the caller shows the offline notice.
pub fn refresh(home: &Path, url: &str) -> Result<Feed, String> {
    let feed = fetch(url.trim())?;
    save_cache(home, &feed);
    Ok(feed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_sanitizes() {
        let f = parse(
            r#"{"items":[
              {"id":"a","title":"A","published_at":"2026-01-01T00:00:00Z","cover_url":"http://x/y.png","url":"https://x/a"},
              {"id":"a","title":"dup"},
              {"title":"no id"},
              {"id":"b","title":"B","published_at":1700000000000,"show_to_new_users":false}
            ]}"#,
        )
        .unwrap();
        assert_eq!(f.items.len(), 2);
        assert_eq!(f.items[0].published_at, 1_767_225_600_000);
        assert_eq!(f.items[0].cover_url, None, "non-https dropped");
        assert_eq!(f.items[0].url.as_deref(), Some("https://x/a"));
        assert!(!f.items[1].show_to_new_users);
        assert!(parse("[1]").unwrap().items.is_empty());
        assert!(parse("{}").is_err());
    }

    #[test]
    fn caps_body_on_char_boundary() {
        let body = "é".repeat(MAX_BODY_BYTES);
        let f =
            parse(&serde_json::json!({"items":[{"id":"a","title":"t","body":body}]}).to_string())
                .unwrap();
        assert!(f.items[0].body.len() <= MAX_BODY_BYTES);
    }

    #[test]
    fn refresh_caches_and_failure_keeps_cache() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src.json");
        std::fs::write(&src, r#"{"items":[{"id":"a","title":"A"}]}"#).unwrap();
        let url = format!("file://{}", src.display());
        assert_eq!(refresh(d.path(), &url).unwrap().items.len(), 1);
        assert!(refresh(d.path(), "file:///nonexistent/x.json").is_err());
        assert_eq!(load_cache(d.path()).unwrap().items[0].id, "a");
    }
}
