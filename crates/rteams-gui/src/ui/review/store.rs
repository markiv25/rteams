//! Persisted review state for one task: per-hunk and per-file decisions plus line comments,
//! stored at `<home>/<team>/reviews/<task>.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineComment {
    pub file: String,
    pub line: u32,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewState {
    /// `"<file>#<hunk key>"` -> decision.
    #[serde(default)]
    pub hunks: BTreeMap<String, Decision>,
    #[serde(default)]
    pub files: BTreeMap<String, Decision>,
    #[serde(default)]
    pub comments: Vec<LineComment>,
    /// Rejected renames: new path -> old path.
    #[serde(default)]
    pub renames: BTreeMap<String, String>,
}

pub fn hunk_id(file: &str, key: &str) -> String {
    format!("{file}#{key}")
}

pub fn path(home: &Path, team: &str, task: &str) -> PathBuf {
    let safe: String = task
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    home.join(team).join("reviews").join(format!("{safe}.json"))
}

impl ReviewState {
    pub fn load(home: &Path, team: &str, task: &str) -> Self {
        std::fs::read_to_string(path(home, team, task))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, home: &Path, team: &str, task: &str) -> Result<(), String> {
        let p = path(home, team, task);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("create {}: {e}", d.display()))?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&p, json).map_err(|e| format!("write {}: {e}", p.display()))
    }

    pub fn hunk(&self, file: &str, key: &str) -> Option<Decision> {
        self.hunks.get(&hunk_id(file, key)).copied()
    }

    pub fn comments_at<'a>(
        &'a self,
        file: &'a str,
        line: u32,
    ) -> impl Iterator<Item = &'a LineComment> {
        self.comments
            .iter()
            .filter(move |c| c.file == file && c.line == line)
    }

    pub fn count(&self, d: Decision) -> usize {
        self.hunks.values().filter(|x| **x == d).count()
    }

    /// Bullet-point summary for a task comment.
    pub fn summary(&self) -> String {
        let mut out = vec![format!(
            "- Review: {} hunks accepted, {} rejected",
            self.count(Decision::Accepted),
            self.count(Decision::Rejected)
        )];
        for c in &self.comments {
            out.push(format!("- {}:{}: {}", c.file, c.line, c.text.trim()));
        }
        out.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_summary() {
        let home = std::env::temp_dir().join(format!("rteams-review-{}", std::process::id()));
        let mut s = ReviewState::default();
        s.hunks.insert(hunk_id("a.rs", "k1"), Decision::Accepted);
        s.hunks.insert(hunk_id("a.rs", "k2"), Decision::Rejected);
        s.comments.push(LineComment {
            file: "a.rs".into(),
            line: 7,
            text: "why?".into(),
        });
        s.save(&home, "t", "1/../x").unwrap();
        let p = path(&home, "t", "1/../x");
        assert!(p.starts_with(home.join("t/reviews")));
        let back = ReviewState::load(&home, "t", "1/../x");
        assert_eq!(back, s);
        assert_eq!(back.hunk("a.rs", "k1"), Some(Decision::Accepted));
        let sum = back.summary();
        assert!(sum.contains("1 hunks accepted, 1 rejected") && sum.contains("- a.rs:7: why?"));
        assert_eq!(
            ReviewState::load(&home, "t", "none"),
            ReviewState::default()
        );
        let _ = std::fs::remove_dir_all(home);
    }
}
