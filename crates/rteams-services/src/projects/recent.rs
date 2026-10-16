//! Recent projects: merge candidates from rteams teams, Claude and Codex sessions and the
//! app's own open history (`<home>/recent-projects.json`). Latest activity wins, providers are
//! unioned, paths are merged by identity (git root / canonical path).

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::identity::{self, Identity};
use super::{claude_source, codex_source, Candidate, ProjectRef};
use crate::{atomic, paths::AppPaths};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentProject {
    pub identity: String,
    pub path: String,
    pub label: String,
    pub last_activity_ms: i64,
    /// Union of `claude`, `codex`, `rteams`, `opened`.
    pub providers: Vec<String>,
    pub exists: bool,
    /// An rteams team (directory name) whose project path is this project.
    pub team: Option<String>,
}

impl RecentProject {
    pub fn to_ref(&self) -> ProjectRef {
        ProjectRef {
            path: self.path.clone(),
            label: self.label.clone(),
            last_used_ms: self.last_activity_ms,
            exists: self.exists,
        }
    }
}

/// Merge candidates (`mergeRecentProjectCandidates`): group by `resolve(path).key`, newest
/// activity wins (preferring paths that still exist), providers unioned. Newest first.
pub fn merge(candidates: &[Candidate], resolve: &dyn Fn(&str) -> Identity) -> Vec<RecentProject> {
    let mut groups: BTreeMap<String, Vec<(&Candidate, Identity)>> = BTreeMap::new();
    for c in candidates {
        if c.path.trim().is_empty() || c.last_ms <= 0 {
            continue;
        }
        let id = resolve(&c.path);
        groups.entry(id.key.clone()).or_default().push((c, id));
    }
    let mut out: Vec<RecentProject> = groups
        .into_iter()
        .map(|(key, group)| {
            let pool: Vec<&(&Candidate, Identity)> = {
                let live: Vec<_> = group.iter().filter(|(_, i)| i.exists).collect();
                if live.is_empty() {
                    group.iter().collect()
                } else {
                    live
                }
            };
            let best_id = pool
                .iter()
                .max_by_key(|(c, _)| c.last_ms)
                .map(|(_, i)| i)
                .expect("non-empty group");
            let mut providers: Vec<String> = Vec::new();
            for (c, _) in &group {
                if !providers.contains(&c.provider) {
                    providers.push(c.provider.clone());
                }
            }
            providers.sort();
            RecentProject {
                identity: key,
                path: best_id.path.clone(),
                label: best_id.label.clone(),
                last_activity_ms: group.iter().map(|(c, _)| c.last_ms).max().unwrap_or(0),
                providers,
                exists: best_id.exists,
                team: group.iter().find_map(|(c, _)| c.team.clone()),
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.last_activity_ms
            .cmp(&a.last_activity_ms)
            .then_with(|| a.label.cmp(&b.label))
    });
    out
}

/// Candidates from rteams teams: each team's `project_path`, activity = log mtime.
pub fn rteams_source(home: &Path) -> Vec<Candidate> {
    let Ok(rd) = std::fs::read_dir(home) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let log = e.path().join("events.jsonl");
        let Ok(meta) = std::fs::metadata(&log) else {
            continue;
        };
        let team = e.file_name().to_string_lossy().into_owned();
        let Some(state) = crate::team_import::load_state(&e.path()) else {
            continue;
        };
        let Some(t) = state.team else { continue };
        if t.deleted {
            continue;
        }
        if let Some(p) = t.project_path.filter(|p| !p.trim().is_empty()) {
            out.push(Candidate {
                path: p,
                last_ms: super::mtime_ms(&meta),
                provider: "rteams".into(),
                team: Some(team),
            });
        }
    }
    out
}

fn history_path(paths: &AppPaths) -> std::path::PathBuf {
    paths.app("recent-projects.json")
}

/// Projects the user opened through the app (`<home>/recent-projects.json`).
pub fn load_history(paths: &AppPaths) -> Vec<Candidate> {
    let Some(v) = atomic::read_json(&history_path(paths)) else {
        return Vec::new();
    };
    v.get("projects")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    Some(Candidate {
                        path: p.get("path")?.as_str()?.to_string(),
                        last_ms: p.get("last_used_ms")?.as_i64()?,
                        provider: "opened".into(),
                        team: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Record that the user opened `path` at `now_ms` (bumps it to the top). Keeps 100 entries.
pub fn touch(paths: &AppPaths, path: &str, now_ms: i64) -> std::io::Result<()> {
    let key = identity::canonical(path);
    let mut entries: Vec<(String, i64)> = load_history(paths)
        .into_iter()
        .map(|c| (c.path, c.last_ms))
        .filter(|(p, _)| *p != key)
        .collect();
    entries.push((key, now_ms));
    entries.sort_by_key(|e| std::cmp::Reverse(e.1));
    entries.truncate(100);
    let projects: Vec<Value> = entries
        .iter()
        .map(|(p, t)| json!({"path": p, "last_used_ms": t}))
        .collect();
    atomic::write_json(
        &history_path(paths),
        &json!({"schema_version": 1, "projects": projects}),
    )
}

/// Where the sources live; `None` skips that source.
#[derive(Debug, Clone, Default)]
pub struct Sources {
    pub claude_home: Option<std::path::PathBuf>,
    pub codex_home: Option<std::path::PathBuf>,
}

impl Sources {
    pub fn from_env() -> Self {
        Sources {
            claude_home: claude_source::default_home(),
            codex_home: codex_source::default_home(),
        }
    }
}

/// Everything merged, newest first (blocking: touches the disk and runs `git`).
pub fn collect(paths: &AppPaths, sources: &Sources) -> Vec<RecentProject> {
    let mut all = rteams_source(paths.home());
    all.extend(load_history(paths));
    if let Some(h) = &sources.claude_home {
        all.extend(claude_source::scan(h));
    }
    if let Some(h) = &sources.codex_home {
        all.extend(codex_source::scan(h));
    }
    merge(&all, &identity::resolve)
}

/// `collect` with a 60 s in-memory cache; `invalidate` after the user opens something.
#[derive(Debug, Default)]
pub struct RecentProjects {
    cache: Option<(Instant, Vec<RecentProject>)>,
}

impl RecentProjects {
    const TTL: Duration = Duration::from_secs(60);

    pub fn cached(&self) -> Option<&[RecentProject]> {
        self.cache
            .as_ref()
            .filter(|(t, _)| t.elapsed() < Self::TTL)
            .map(|(_, v)| v.as_slice())
    }

    pub fn load(&mut self, paths: &AppPaths, sources: &Sources) -> &[RecentProject] {
        if self.cached().is_none() {
            self.cache = Some((Instant::now(), collect(paths, sources)));
        }
        &self.cache.as_ref().expect("just set").1
    }

    /// Store a result computed elsewhere (a background thread).
    pub fn put(&mut self, v: Vec<RecentProject>) {
        self.cache = Some((Instant::now(), v));
    }

    pub fn invalidate(&mut self) {
        self.cache = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(p: &str, ms: i64, prov: &str) -> Candidate {
        Candidate {
            path: p.into(),
            last_ms: ms,
            provider: prov.into(),
            team: None,
        }
    }

    fn fake(path: &str) -> Identity {
        let key = path.trim_end_matches('/').replace("/wt", "");
        Identity {
            key,
            path: path.into(),
            label: "x".into(),
            exists: true,
        }
    }

    #[test]
    fn merge_unions_providers_and_latest_wins() {
        let c = vec![
            cand("/a", 10, "claude"),
            cand("/a/wt", 30, "codex"),
            cand("/b", 20, "claude"),
            cand("/zero", 0, "claude"),
        ];
        let m = merge(&c, &fake);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].identity, "/a");
        assert_eq!(m[0].last_activity_ms, 30);
        assert_eq!(m[0].providers, vec!["claude", "codex"]);
        assert_eq!(m[1].path, "/b");
    }

    #[test]
    fn history_touch_round_trips_and_bumps() {
        let d = tempfile::tempdir().unwrap();
        let p = AppPaths::new(d.path());
        touch(&p, "/x/one", 1).unwrap();
        touch(&p, "/x/two", 2).unwrap();
        touch(&p, "/x/one", 3).unwrap();
        let h = load_history(&p);
        assert_eq!(h.len(), 2);
        assert_eq!((h[0].path.as_str(), h[0].last_ms), ("/x/one", 3));
    }

    #[test]
    fn collect_merges_all_sources() {
        let d = tempfile::tempdir().unwrap();
        let proj = d.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let claude = d.path().join("claude/projects/-enc");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join("s.jsonl"),
            format!("{{\"cwd\":{:?}}}\n", proj.to_string_lossy()),
        )
        .unwrap();
        let home = d.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let paths = AppPaths::new(&home);
        touch(&paths, &proj.to_string_lossy(), 5).unwrap();
        let src = Sources {
            claude_home: Some(d.path().join("claude")),
            codex_home: Some(d.path().join("none")),
        };
        let r = collect(&paths, &src);
        assert_eq!(r.len(), 1);
        assert!(r[0].providers.contains(&"claude".to_string()));
        assert!(r[0].providers.contains(&"opened".to_string()));
        assert!(r[0].exists);
    }

    #[test]
    fn cache_serves_until_invalidated() {
        let mut c = RecentProjects::default();
        assert!(c.cached().is_none());
        c.put(vec![]);
        assert!(c.cached().is_some());
        c.invalidate();
        assert!(c.cached().is_none());
    }
}
