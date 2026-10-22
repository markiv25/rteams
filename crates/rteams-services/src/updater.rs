//! Update check. It reads a configurable release feed, compares semver, and reports. On an
//! explicit user click it can also download the release asset matching this OS/arch into the
//! Downloads folder (progress, size check, SHA-256) so the user can reveal/open it; it NEVER
//! replaces the running binary. Default feed URL is empty, which disables the check: this fork
//! publishes no feed by default, so set `general.update_feed_url` (for example
//! `https://api.github.com/repos/markiv25/rteams/releases/latest`) to enable it.
//!
//! Feed formats (JSON): `{"version","notes","url","published_at","assets"}` or a GitHub
//! `releases/latest` document (`tag_name`, `body`, `html_url`, `assets[]`). `url` and asset URLs
//! must be https.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// One downloadable file of a release.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReleaseAsset {
    pub name: String,
    /// https download URL.
    pub url: String,
    /// Expected size in bytes (0 = unknown).
    pub size: u64,
    /// Expected lowercase SHA-256 hex when the feed publishes a `sha256:` digest.
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub available: bool,
    pub notes: String,
    /// https release page to open in the browser (may be empty).
    pub url: String,
    pub published_at: String,
    pub assets: Vec<ReleaseAsset>,
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
        assets: parse_assets(&v),
        latest,
    })
}

fn parse_assets(v: &Value) -> Vec<ReleaseAsset> {
    v.get("assets")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    let name = x.get("name")?.as_str()?.to_string();
                    let url = str_of(x, &["browser_download_url", "url"]);
                    if !url.starts_with("https://") || name.is_empty() {
                        return None;
                    }
                    Some(ReleaseAsset {
                        name,
                        url: url.to_string(),
                        size: x.get("size").and_then(Value::as_u64).unwrap_or(0),
                        sha256: x
                            .get("digest")
                            .and_then(Value::as_str)
                            .and_then(|d| d.strip_prefix("sha256:"))
                            .map(str::to_lowercase),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The asset that fits `os` (`std::env::consts::OS`) and `arch` (`ARCH`): installers/archives
/// only (checksums and signatures are skipped); an asset naming a different arch is excluded and
/// one naming ours (or "universal") is preferred.
pub fn pick_asset<'a>(
    assets: &'a [ReleaseAsset],
    os: &str,
    arch: &str,
) -> Option<&'a ReleaseAsset> {
    let (os_tokens, exts): (&[&str], &[&str]) = match os {
        "macos" => (&["macos", "darwin", "osx", "apple"], &[".dmg", ".pkg"]),
        "windows" => (&["windows", "win64", "win32"], &[".msi", ".exe"]),
        "linux" => (&["linux"], &[".appimage", ".deb", ".rpm"]),
        _ => return None,
    };
    let (mine, other): (&[&str], &[&str]) = match arch {
        "aarch64" => (&["aarch64", "arm64"], &["x86_64", "x64", "amd64", "x86"]),
        "x86_64" => (&["x86_64", "x64", "amd64"], &["aarch64", "arm64"]),
        _ => (&[], &[]),
    };
    let skip = [
        ".sha256", ".sha512", ".sig", ".asc", ".txt", ".json", ".sbom", ".md5",
    ];
    assets
        .iter()
        .filter_map(|a| {
            let n = a.name.to_lowercase();
            if skip.iter().any(|e| n.ends_with(e)) {
                return None;
            }
            let os_hit =
                os_tokens.iter().any(|t| n.contains(t)) || exts.iter().any(|e| n.ends_with(e));
            if !os_hit {
                return None;
            }
            let mine_hit = mine.iter().any(|t| n.contains(t)) || n.contains("universal");
            if !mine_hit && other.iter().any(|t| n.contains(t)) {
                return None;
            }
            Some((u8::from(mine_hit), a))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, a)| a)
}

/// Hex SHA-256 of a file (streamed).
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// A finished, verified download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Downloaded {
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
}

/// `~/Downloads` (falls back to the home dir).
pub fn downloads_dir() -> Option<PathBuf> {
    dirs::download_dir().or_else(dirs::home_dir)
}

/// File name safe to join onto a directory (no separators, not hidden/empty).
fn safe_name(name: &str) -> Option<String> {
    let n = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    (!n.is_empty() && !n.starts_with('.') && n != "..").then(|| n.to_string())
}

/// Download `asset` into `dest_dir`. Blocking: call from a worker thread. `progress` receives the
/// bytes written so far; setting `cancel` aborts and removes the partial file. Verifies the size
/// (when the feed gave one) and the SHA-256 (when published). Only https URLs are accepted.
pub fn download_asset(
    asset: &ReleaseAsset,
    dest_dir: &Path,
    progress: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<Downloaded, UpdateError> {
    if !asset.url.starts_with("https://") {
        return Err(UpdateError::Fetch("download URL must be https".into()));
    }
    download_from(&asset.url, asset, dest_dir, progress, cancel)
}

fn download_from(
    src: &str,
    asset: &ReleaseAsset,
    dest_dir: &Path,
    progress: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<Downloaded, UpdateError> {
    let name = safe_name(&asset.name).ok_or_else(|| UpdateError::Fetch("bad file name".into()))?;
    std::fs::create_dir_all(dest_dir).map_err(|e| UpdateError::Fetch(e.to_string()))?;
    let part = dest_dir.join(format!("{name}.part"));
    let fin = dest_dir.join(&name);
    let _ = std::fs::remove_file(&part);
    progress.store(0, Ordering::Relaxed);
    let mut child = Command::new("curl")
        .args(["-fsSL", "--max-time", "3600", "-A"])
        .arg(concat!("rteams/", env!("CARGO_PKG_VERSION")))
        .arg("-o")
        .arg(&part)
        .args(["--", src])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| UpdateError::Fetch(format!("curl unavailable: {e}")))?;
    let status = loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&part);
            return Err(UpdateError::Fetch("cancelled".into()));
        }
        if let Ok(m) = std::fs::metadata(&part) {
            progress.store(m.len(), Ordering::Relaxed);
        }
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => std::thread::sleep(Duration::from_millis(80)),
            Err(e) => return Err(UpdateError::Fetch(e.to_string())),
        }
    };
    if !status.success() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut e, &mut err);
        }
        let _ = std::fs::remove_file(&part);
        let err = err.trim();
        return Err(UpdateError::Fetch(if err.is_empty() {
            format!("download failed ({status})")
        } else {
            err.to_string()
        }));
    }
    let fail = |msg: String| {
        let _ = std::fs::remove_file(&part);
        Err(UpdateError::Fetch(msg))
    };
    let size = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    progress.store(size, Ordering::Relaxed);
    if asset.size > 0 && size != asset.size {
        return fail(format!(
            "size mismatch: expected {} bytes, got {size}",
            asset.size
        ));
    }
    let sha = sha256_file(&part).map_err(|e| UpdateError::Fetch(e.to_string()))?;
    if let Some(want) = &asset.sha256 {
        if *want != sha {
            return fail("SHA-256 mismatch: the download is corrupt or was tampered with".into());
        }
    }
    std::fs::rename(&part, &fin).map_err(|e| UpdateError::Fetch(e.to_string()))?;
    Ok(Downloaded {
        path: fin,
        size,
        sha256: sha,
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

    fn asset(name: &str) -> ReleaseAsset {
        ReleaseAsset {
            name: name.into(),
            url: format!("https://x.test/{name}"),
            ..Default::default()
        }
    }

    const RELEASE: &str = r#"{"tag_name":"v0.4.0","body":"b","html_url":"https://github.com/markiv25/rteams/releases/tag/v0.4.0",
      "assets":[
       {"name":"rteams-0.4.0-macos-aarch64.dmg","browser_download_url":"https://github.com/a/aarch64.dmg","size":10,"digest":"sha256:ABCD"},
       {"name":"rteams-0.4.0-macos-x86_64.dmg","browser_download_url":"https://github.com/a/x64.dmg","size":11},
       {"name":"rteams-0.4.0-macos-aarch64.dmg.sha256","browser_download_url":"https://github.com/a/s","size":1},
       {"name":"rteams-0.4.0-linux-x86_64.AppImage","browser_download_url":"https://github.com/a/l","size":12},
       {"name":"rteams-0.4.0-windows-x64.msi","browser_download_url":"http://insecure/w","size":13}]}"#;

    #[test]
    fn parses_release_assets_and_picks_per_platform() {
        let i = parse_feed(RELEASE, "0.3.0").unwrap();
        assert_eq!(i.assets.len(), 4, "non-https asset dropped");
        assert_eq!(i.assets[0].sha256.as_deref(), Some("abcd"));
        let pick = |os, arch| pick_asset(&i.assets, os, arch).map(|a| a.name.as_str());
        assert_eq!(
            pick("macos", "aarch64"),
            Some("rteams-0.4.0-macos-aarch64.dmg")
        );
        assert_eq!(
            pick("macos", "x86_64"),
            Some("rteams-0.4.0-macos-x86_64.dmg")
        );
        assert_eq!(
            pick("linux", "x86_64"),
            Some("rteams-0.4.0-linux-x86_64.AppImage")
        );
        assert_eq!(pick("linux", "aarch64"), None);
        assert_eq!(pick("windows", "x86_64"), None);
        assert_eq!(pick("freebsd", "x86_64"), None);
        assert!(pick_asset(&[asset("app-universal-macos.dmg")], "macos", "aarch64").is_some());
    }

    #[cfg(unix)]
    #[test]
    fn downloads_verifies_size_and_sha_from_a_local_file() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src.bin");
        std::fs::write(&src, b"hello").unwrap();
        let sha = sha256_file(&src).unwrap();
        assert_eq!(
            sha,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        let out = d.path().join("dl");
        let (p, c) = (AtomicU64::new(0), AtomicBool::new(false));
        let mut a = asset("../evil/app.dmg");
        a.size = 5;
        a.sha256 = Some(sha.clone());
        let got = download_from(&format!("file://{}", src.display()), &a, &out, &p, &c).unwrap();
        assert_eq!(got.path, out.join("app.dmg"), "path components stripped");
        assert_eq!((got.size, got.sha256.as_str()), (5, sha.as_str()));
        assert_eq!(p.load(Ordering::Relaxed), 5);
        // wrong sha: file removed, error
        a.sha256 = Some("00".into());
        a.name = "bad.dmg".into();
        let e = download_from(&format!("file://{}", src.display()), &a, &out, &p, &c);
        assert!(matches!(e, Err(UpdateError::Fetch(m)) if m.contains("SHA-256")));
        assert!(!out.join("bad.dmg").exists() && !out.join("bad.dmg.part").exists());
        // wrong size
        a.sha256 = None;
        a.size = 9;
        let e = download_from(&format!("file://{}", src.display()), &a, &out, &p, &c);
        assert!(matches!(e, Err(UpdateError::Fetch(m)) if m.contains("size mismatch")));
        // http refused by the public entry point (no network attempted)
        a.url = "http://x.test/a".into();
        assert!(download_asset(&a, &out, &p, &c).is_err());
    }
}
