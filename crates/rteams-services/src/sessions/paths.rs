//! Where session files live: Claude Code (`~/.claude/projects/<encoded>/*.jsonl`), Codex
//! (`~/.codex/sessions/**/rollout-*.jsonl`) and rteams' own runs
//! (`<home>/<team>/runs/<member>/<unix_ms>.jsonl`). Roots are injected so tests never touch the
//! real home directories.

use std::path::{Path, PathBuf};

/// Which tool wrote a session file.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Claude,
    Codex,
    Rteams,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Claude => "claude",
            Source::Codex => "codex",
            Source::Rteams => "rteams",
        }
    }

    pub fn parse(s: &str) -> Source {
        match s {
            "codex" => Source::Codex,
            "rteams" => Source::Rteams,
            _ => Source::Claude,
        }
    }
}

/// Directories to scan.
#[derive(Debug, Clone)]
pub struct Roots {
    /// `~/.claude`.
    pub claude: Option<PathBuf>,
    /// `~/.codex`.
    pub codex: Option<PathBuf>,
    /// The rteams home (teams live under it; the metadata index is stored in it).
    pub home: PathBuf,
}

impl Roots {
    /// The real `~/.claude` and `~/.codex`.
    pub fn system(home: &Path) -> Roots {
        Roots {
            claude: dirs::home_dir().map(|h| h.join(".claude")),
            codex: dirs::home_dir().map(|h| h.join(".codex")),
            home: home.to_path_buf(),
        }
    }

    /// Only rteams runs (and whatever explicit roots are given).
    pub fn new(home: &Path, claude: Option<PathBuf>, codex: Option<PathBuf>) -> Roots {
        Roots {
            claude,
            codex,
            home: home.to_path_buf(),
        }
    }
}

/// A session file found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFile {
    pub path: PathBuf,
    pub source: Source,
    /// Claude: encoded project dir name; rteams: `<team>/<member>`; Codex: empty.
    pub group: String,
}

/// Claude's directory-name encoding of a project path (`/` and `.` become `-`).
pub fn encode_dir(path: &str) -> String {
    path.chars()
        .map(|c| {
            if matches!(c, '/' | '.' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect()
}

fn jsonl_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    v.sort();
    v
}

fn walk_codex(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            if depth < 6 {
                walk_codex(&p, depth + 1, out);
            }
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            out.push(p);
        }
    }
}

/// Every session file under the roots. Subagent files (`<session>/subagents/*`) are not listed.
pub fn discover(roots: &Roots) -> Vec<SessionFile> {
    let mut out = Vec::new();
    if let Some(c) = &roots.claude {
        if let Ok(rd) = std::fs::read_dir(c.join("projects")) {
            let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            dirs.sort();
            for d in dirs.into_iter().filter(|d| d.is_dir()) {
                let group = d
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                for p in jsonl_files(&d) {
                    out.push(SessionFile {
                        path: p,
                        source: Source::Claude,
                        group: group.clone(),
                    });
                }
            }
        }
    }
    if let Some(c) = &roots.codex {
        let mut files = Vec::new();
        walk_codex(&c.join("sessions"), 0, &mut files);
        files.sort();
        out.extend(files.into_iter().map(|p| SessionFile {
            path: p,
            source: Source::Codex,
            group: String::new(),
        }));
    }
    out.extend(rteams_runs(&roots.home));
    out
}

/// `<home>/<team>/runs/<member>/*.jsonl`.
pub fn rteams_runs(home: &Path) -> Vec<SessionFile> {
    let mut out = Vec::new();
    let Ok(teams) = std::fs::read_dir(home) else {
        return out;
    };
    let mut teams: Vec<PathBuf> = teams.flatten().map(|e| e.path()).collect();
    teams.sort();
    for t in teams {
        let runs = t.join("runs");
        let Ok(members) = std::fs::read_dir(&runs) else {
            continue;
        };
        let team = t
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut members: Vec<PathBuf> = members.flatten().map(|e| e.path()).collect();
        members.sort();
        for m in members.into_iter().filter(|m| m.is_dir()) {
            let member = m
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            for p in jsonl_files(&m) {
                out.push(SessionFile {
                    path: p,
                    source: Source::Rteams,
                    group: format!("{team}/{member}"),
                });
            }
        }
    }
    out
}

/// `<project dir>/<session id>/subagents`, where Claude Code writes sidechain transcripts.
pub fn subagent_dir(session_file: &Path) -> PathBuf {
    let stem = session_file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    session_file
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(stem)
        .join("subagents")
}

/// Subagent transcript files of a session, sorted by name.
pub fn subagent_files(session_file: &Path) -> Vec<PathBuf> {
    jsonl_files(&subagent_dir(session_file))
}

/// Session id from a file path (the file stem; Codex `rollout-<ts>-<uuid>` keeps its stem too).
pub fn session_id_of(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_maps_separators() {
        assert_eq!(encode_dir("/Users/a/my.proj"), "-Users-a-my-proj");
    }

    #[test]
    fn discovers_all_three_sources() {
        let d = tempfile::tempdir().unwrap();
        let claude = d.path().join("claude");
        std::fs::create_dir_all(claude.join("projects/-tmp-x/s1/subagents")).unwrap();
        std::fs::write(claude.join("projects/-tmp-x/s1.jsonl"), "{}\n").unwrap();
        std::fs::write(
            claude.join("projects/-tmp-x/s1/subagents/agent-a1.jsonl"),
            "{}\n",
        )
        .unwrap();
        let codex = d.path().join("codex/sessions/2025/03/01");
        std::fs::create_dir_all(&codex).unwrap();
        std::fs::write(codex.join("rollout-1.jsonl"), "{}\n").unwrap();
        let home = d.path().join("home");
        std::fs::create_dir_all(home.join("t/runs/ada")).unwrap();
        std::fs::write(home.join("t/runs/ada/1700000000000.jsonl"), "{}\n").unwrap();
        let roots = Roots::new(&home, Some(claude.clone()), Some(d.path().join("codex")));
        let f = discover(&roots);
        assert_eq!(f.len(), 3);
        assert_eq!(f.iter().filter(|s| s.source == Source::Claude).count(), 1);
        assert_eq!(
            f.iter().find(|s| s.source == Source::Rteams).unwrap().group,
            "t/ada"
        );
        let s1 = claude.join("projects/-tmp-x/s1.jsonl");
        assert_eq!(subagent_files(&s1).len(), 1);
    }
}
