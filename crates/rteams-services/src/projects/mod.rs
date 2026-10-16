//! Recent projects, project identity and project-folder checks (original features
//! `recent-projects` and `project-folder`).

pub mod claude_source;
pub mod codex_source;
pub mod folder;
pub mod identity;
pub mod recent;

pub use recent::{RecentProject, RecentProjects};

/// A project directory as the pickers show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRef {
    pub path: String,
    pub label: String,
    pub last_used_ms: i64,
    pub exists: bool,
}

/// One raw sighting of a project path by a source (before identity merging).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: String,
    pub last_ms: i64,
    /// `claude`, `codex`, `rteams` or `opened`.
    pub provider: String,
    /// rteams team (directory name) that owns this path, for the `rteams` source.
    pub team: Option<String>,
}

pub(crate) fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

/// First line of `path` (at most 128 KiB read), for sources that only need a header record.
pub(crate) fn read_head(path: &std::path::Path, max_bytes: u64) -> Option<String> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(max_bytes)
        .read_to_end(&mut buf)
        .ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}
