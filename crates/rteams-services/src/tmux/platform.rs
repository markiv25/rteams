//! tmux::platform: detect the OS family and whether tmux is installed.

use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Os {
    MacOs,
    /// Linux with the `ID` / `ID_LIKE` values of /etc/os-release (lowercased).
    Linux {
        ids: Vec<String>,
    },
    Windows,
    Other(String),
}

impl Os {
    pub fn label(&self) -> String {
        match self {
            Os::MacOs => "macOS".into(),
            Os::Linux { ids } => format!(
                "Linux ({})",
                ids.first().map(String::as_str).unwrap_or("unknown")
            ),
            Os::Windows => "Windows".into(),
            Os::Other(s) => s.clone(),
        }
    }
}

/// Parse `ID` and `ID_LIKE` out of os-release text.
pub fn parse_os_release(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches('"').trim_matches('\'');
        match k.trim() {
            "ID" => ids.insert(0, v.to_lowercase()),
            "ID_LIKE" => ids.extend(v.split_whitespace().map(|s| s.to_lowercase())),
            _ => {}
        }
    }
    ids
}

/// The running OS; `os_release` is the contents of /etc/os-release when on Linux.
pub fn detect_os(os_release: Option<&str>) -> Os {
    match std::env::consts::OS {
        "macos" => Os::MacOs,
        "windows" => Os::Windows,
        "linux" => Os::Linux {
            ids: parse_os_release(os_release.unwrap_or("")),
        },
        other => Os::Other(other.into()),
    }
}

/// `detect_os` reading the real /etc/os-release.
pub fn current_os() -> Os {
    let text = std::fs::read_to_string("/etc/os-release").ok();
    detect_os(text.as_deref())
}

/// Extract the version from `tmux -V` output ("tmux 3.4" -> "3.4").
pub fn parse_version(out: &str) -> Option<String> {
    let s = out.trim().strip_prefix("tmux")?.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Tool dirs a GUI app started from Finder/launchd does not have on PATH.
const EXTRA_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"];

/// PATH plus the usual package-manager dirs (deduplicated, only existing dirs added).
pub fn search_path() -> Vec<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for d in EXTRA_DIRS {
        let d = std::path::PathBuf::from(d);
        if d.is_dir() && !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    dirs
}

/// `PATH` value for child processes (see `search_path`).
pub fn child_path() -> Option<std::ffi::OsString> {
    std::env::join_paths(search_path()).ok()
}

/// Does this installer output line mean it wants interactive input (a password)?
pub fn needs_input(line: &str) -> bool {
    let l = line.to_lowercase();
    l.contains("password")
        || l.contains("a terminal is required")
        || l.contains("no tty present")
        || l.contains("askpass")
}

/// Run `tmux -V`; None when tmux is missing or fails.
pub fn detect_tmux() -> Option<String> {
    let mut cmd = Command::new("tmux");
    if let Some(p) = child_path() {
        cmd.env("PATH", p);
    }
    let out = cmd.arg("-V").output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_version(&String::from_utf8_lossy(&out.stdout))
}

/// Is `bin` found on PATH?
pub fn on_path(bin: &str) -> bool {
    search_path().iter().any(|d| d.join(bin).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_release_parse() {
        let t = "NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=\"debian\"\n";
        assert_eq!(parse_os_release(t), vec!["ubuntu", "debian"]);
        assert_eq!(parse_os_release("ID=arch"), vec!["arch"]);
        assert!(parse_os_release("").is_empty());
    }

    #[test]
    fn detects_input_prompts() {
        assert!(needs_input("Password:"));
        assert!(needs_input(
            "sudo: a terminal is required to read the password"
        ));
        assert!(!needs_input("==> Downloading tmux"));
    }

    #[test]
    fn search_path_keeps_existing_path_entries() {
        let p = search_path();
        for d in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
            assert!(p.contains(&d));
        }
    }

    #[test]
    fn version_parse() {
        assert_eq!(parse_version("tmux 3.4\n").as_deref(), Some("3.4"));
        assert_eq!(parse_version("tmux next-3.5").as_deref(), Some("next-3.5"));
        assert_eq!(parse_version("nope"), None);
        assert_eq!(parse_version("tmux"), None);
    }
}
