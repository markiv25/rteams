//! App-wide and per-team file locations. App-wide files live directly under the rteams home
//! (`config.json`, `recent-projects.json`, `usage-budgets.json`, `editor/`, `schedules/` ...);
//! per-team files under `<home>/<team>/` next to the event log.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub home: PathBuf,
}

impl AppPaths {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        AppPaths { home: home.into() }
    }

    /// `<home>/<name>` for an app-wide file or directory.
    pub fn app(&self, name: &str) -> PathBuf {
        self.home.join(name)
    }

    pub fn team_dir(&self, team: &str) -> PathBuf {
        self.home.join(team)
    }

    /// `<home>/<team>/<name>` for a per-team file.
    pub fn team_file(&self, team: &str, name: &str) -> PathBuf {
        self.team_dir(team).join(name)
    }

    pub fn home(&self) -> &Path {
        &self.home
    }
}
