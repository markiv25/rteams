//! Where the local Claude data root (`~/.claude`) lives: `CLAUDE_CONFIG_DIR`, a user override
//! saved in the app config, or the default under the user's home. Pure functions over explicit
//! inputs so tests never touch the real `~/.claude`.

use std::path::{Path, PathBuf};

/// The user's real home directory (`None` when unknown).
pub fn user_home() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Unix seconds of the next local midnight after `now` (settings snooze "until tomorrow").
pub fn next_local_midnight(now: i64) -> i64 {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(now, 0)
        .single()
        .and_then(|t| (t.date_naive() + chrono::Days::new(1)).and_hms_opt(0, 0, 0))
        .and_then(|n| chrono::Local.from_local_datetime(&n).earliest())
        .map(|t| t.timestamp())
        .unwrap_or(now + 24 * 3600)
}

/// `YYYY-MM-DD HH:MM` in local time.
pub fn format_local(ts: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

pub fn now_unix() -> i64 {
    chrono::Local::now().timestamp()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootSource {
    /// Chosen by the user in Settings.
    Override,
    /// `CLAUDE_CONFIG_DIR` environment variable.
    Env,
    /// `<home>/.claude`.
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeRoot {
    pub path: PathBuf,
    pub source: RootSource,
}

/// Precedence: override, then `env` (the value of `CLAUDE_CONFIG_DIR`), then `<user_home>/.claude`.
/// Empty strings count as unset; a leading `~` is expanded against `user_home`.
pub fn resolve(override_path: Option<&str>, env: Option<&str>, user_home: &Path) -> ClaudeRoot {
    let clean = |s: Option<&str>| {
        s.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| expand(s, user_home))
    };
    if let Some(p) = clean(override_path) {
        return ClaudeRoot {
            path: p,
            source: RootSource::Override,
        };
    }
    if let Some(p) = clean(env) {
        return ClaudeRoot {
            path: p,
            source: RootSource::Env,
        };
    }
    ClaudeRoot {
        path: user_home.join(".claude"),
        source: RootSource::Default,
    }
}

/// `resolve` using the process environment and the real home directory.
pub fn resolve_system(override_path: Option<&str>) -> ClaudeRoot {
    let home = dirs::home_dir().unwrap_or_default();
    resolve(
        override_path,
        std::env::var("CLAUDE_CONFIG_DIR").ok().as_deref(),
        &home,
    )
}

fn expand(s: &str, user_home: &Path) -> PathBuf {
    if s == "~" {
        user_home.to_path_buf()
    } else if let Some(rest) = s.strip_prefix("~/") {
        user_home.join(rest)
    } else {
        PathBuf::from(s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validation {
    pub exists: bool,
    pub is_dir: bool,
    /// Folder name is `.claude`.
    pub named_claude: bool,
    pub has_projects: bool,
    pub folder_name: String,
}

impl Validation {
    /// Usable without any confirmation prompt.
    pub fn is_clean(&self) -> bool {
        self.exists && self.is_dir && self.named_claude && self.has_projects
    }
}

pub fn validate(path: &Path) -> Validation {
    let folder_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Validation {
        exists: path.exists(),
        is_dir: path.is_dir(),
        named_claude: folder_name == ".claude",
        has_projects: path.join("projects").is_dir(),
        folder_name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midnight_is_within_a_day() {
        let now = now_unix();
        let m = next_local_midnight(now);
        assert!(m > now && m <= now + 25 * 3600);
        assert_eq!(format_local(m).len(), 16);
    }

    #[test]
    fn precedence_override_env_default() {
        let h = Path::new("/u");
        assert_eq!(
            resolve(None, None, h),
            ClaudeRoot {
                path: "/u/.claude".into(),
                source: RootSource::Default
            }
        );
        let e = resolve(None, Some("/e"), h);
        assert_eq!((e.path, e.source), (PathBuf::from("/e"), RootSource::Env));
        let o = resolve(Some("/o"), Some("/e"), h);
        assert_eq!(
            (o.path, o.source),
            (PathBuf::from("/o"), RootSource::Override)
        );
    }

    #[test]
    fn blank_is_unset_and_tilde_expands() {
        let h = Path::new("/u");
        assert_eq!(resolve(Some("  "), Some(""), h).source, RootSource::Default);
        assert_eq!(
            resolve(Some("~/x/.claude"), None, h).path,
            PathBuf::from("/u/x/.claude")
        );
        assert_eq!(resolve(Some("~"), None, h).path, PathBuf::from("/u"));
    }

    #[test]
    fn validation_flags() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join(".claude");
        assert!(!validate(&root).exists);
        std::fs::create_dir_all(root.join("projects")).unwrap();
        assert!(validate(&root).is_clean());
        let other = d.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        let v = validate(&other);
        assert!(v.is_dir && !v.named_claude && !v.has_projects && !v.is_clean());
        assert_eq!(v.folder_name, "other");
    }
}
