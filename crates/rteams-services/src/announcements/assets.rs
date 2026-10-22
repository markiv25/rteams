//! Cover-image cache: `<home>/announcements/covers/<sha256(url)>.<ext>`. Each https cover is
//! downloaded once (size-capped, magic-byte checked) and reused; the cache only ever holds files
//! for URLs the current feed lists.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// Largest cover accepted (bytes).
pub const MAX_COVER_BYTES: usize = 5 * 1024 * 1024;
/// Covers prefetched per refresh.
pub const MAX_PREFETCH: usize = 20;

pub fn covers_dir(home: &Path) -> PathBuf {
    home.join("announcements").join("covers")
}

fn kind(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else {
        None
    }
}

fn stem(url: &str) -> String {
    Sha256::digest(url.as_bytes())
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The cached file for `url`, if it was downloaded before. No network.
pub fn cached_cover(home: &Path, url: &str) -> Option<PathBuf> {
    let s = stem(url);
    ["png", "jpg"]
        .iter()
        .map(|e| covers_dir(home).join(format!("{s}.{e}")))
        .find(|p| p.is_file())
}

/// Store already-fetched `bytes` for `url` (validates type and size).
pub fn store_cover(home: &Path, url: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    if bytes.len() > MAX_COVER_BYTES {
        return Err("cover too large".into());
    }
    let ext = kind(bytes).ok_or("cover is not a PNG or JPEG")?;
    let p = covers_dir(home).join(format!("{}.{ext}", stem(url)));
    crate::atomic::write(&p, bytes).map_err(|e| e.to_string())?;
    Ok(p)
}

/// Cached cover for `url`, downloading it (https only, or `file://` for mirrors/tests) when
/// missing. Blocking.
pub fn ensure_cover(home: &Path, url: &str) -> Result<PathBuf, String> {
    if let Some(p) = cached_cover(home, url) {
        return Ok(p);
    }
    let bytes = if let Some(p) = url.strip_prefix("file://") {
        std::fs::read(p).map_err(|e| e.to_string())?
    } else if url.starts_with("https://") {
        crate::http::get_bytes(url, Duration::from_secs(15)).map_err(|e| e.to_string())?
    } else {
        return Err("cover URL must be https".into());
    };
    store_cover(home, url, &bytes)
}

/// Download (once) the covers of `urls`, skipping failures, and delete cached covers no longer
/// referenced. Returns how many were newly fetched.
pub fn sync_covers(home: &Path, urls: &[&str]) -> usize {
    let mut fetched = 0;
    for u in urls.iter().take(MAX_PREFETCH) {
        if cached_cover(home, u).is_none() && ensure_cover(home, u).is_ok() {
            fetched += 1;
        }
    }
    let keep: Vec<String> = urls.iter().map(|u| stem(u)).collect();
    if let Ok(rd) = std::fs::read_dir(covers_dir(home)) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let file_stem = name.split('.').next().unwrap_or("");
            if !keep.iter().any(|k| k == file_stem) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    fetched
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 1, 2, 3];

    #[test]
    fn downloads_once_and_reuses() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("c.png");
        std::fs::write(&src, PNG).unwrap();
        let url = format!("file://{}", src.display());
        assert!(cached_cover(d.path(), &url).is_none());
        let p = ensure_cover(d.path(), &url).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), PNG);
        std::fs::remove_file(&src).unwrap();
        assert_eq!(
            ensure_cover(d.path(), &url).unwrap(),
            p,
            "served from cache"
        );
    }

    #[test]
    fn rejects_bad_type_size_and_scheme() {
        let d = tempfile::tempdir().unwrap();
        assert!(store_cover(d.path(), "u", b"<html>").is_err());
        let big = [vec![0xFF, 0xD8, 0xFF], vec![0; MAX_COVER_BYTES]].concat();
        assert!(store_cover(d.path(), "u", &big).is_err());
        assert!(ensure_cover(d.path(), "http://x.test/a.png").is_err());
    }

    #[test]
    fn sync_prunes_unreferenced() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.png");
        let b = d.path().join("b.png");
        std::fs::write(&a, PNG).unwrap();
        std::fs::write(&b, PNG).unwrap();
        let (ua, ub) = (
            format!("file://{}", a.display()),
            format!("file://{}", b.display()),
        );
        assert_eq!(sync_covers(d.path(), &[&ua, &ub]), 2);
        assert_eq!(sync_covers(d.path(), &[&ua]), 0);
        assert!(cached_cover(d.path(), &ua).is_some() && cached_cover(d.path(), &ub).is_none());
    }
}
