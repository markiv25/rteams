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

/// Run `tmux -V`; None when tmux is missing or fails.
pub fn detect_tmux() -> Option<String> {
    let out = Command::new("tmux").arg("-V").output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_version(&String::from_utf8_lossy(&out.stdout))
}

/// Is `bin` found on PATH?
pub fn on_path(bin: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|d| d.join(bin).is_file())
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
    fn version_parse() {
        assert_eq!(parse_version("tmux 3.4\n").as_deref(), Some("3.4"));
        assert_eq!(parse_version("tmux next-3.5").as_deref(), Some("next-3.5"));
        assert_eq!(parse_version("nope"), None);
        assert_eq!(parse_version("tmux"), None);
    }
}
