//! Session discovery: lists projects and sessions of Claude Code, Codex and rteams runs,
//! backed by the incremental metadata index.

use std::collections::HashMap;

use super::metadata_index::{MetadataIndex, SessionMeta};
use super::paths::{discover, Roots, Source};
use super::worktrees::RepoCache;
use crate::projects::identity::canonical;

/// A working directory with sessions.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectInfo {
    /// Canonical cwd, or `rteams/<team>` for runs without a recorded cwd.
    pub id: String,
    pub cwd: String,
    pub label: String,
    pub repo_key: String,
    pub repo_label: String,
    pub is_worktree: bool,
    pub sources: Vec<Source>,
    pub session_count: usize,
    pub last_ms: i64,
}

#[derive(Debug, Clone, Default)]
pub struct ScanResult {
    pub projects: Vec<ProjectInfo>,
    /// Newest first.
    pub sessions: Vec<SessionMeta>,
    /// Files that had to be re-read (the rest came from the index).
    pub computed: usize,
}

/// Project id of a session.
pub fn project_id(m: &SessionMeta, canon: &mut HashMap<String, String>) -> String {
    if !m.cwd.is_empty() {
        return canon
            .entry(m.cwd.clone())
            .or_insert_with(|| canonical(&m.cwd))
            .clone();
    }
    match m.source {
        Source::Rteams => {
            let team = m.group.split('/').next().unwrap_or("");
            format!("rteams/{team}")
        }
        _ => format!("{}/{}", m.source.as_str(), m.group),
    }
}

/// Scan every source (call off the UI thread). Updates and saves `<home>/sessions-index.json`.
pub fn scan(roots: &Roots) -> ScanResult {
    let files = discover(roots);
    let mut index = MetadataIndex::load(&roots.home);
    let mut sessions = index.refresh_all(&files);
    let computed = index.computed;
    let _ = index.save(&roots.home);
    sessions.sort_by(|a, b| b.end_ms.cmp(&a.end_ms).then_with(|| a.path.cmp(&b.path)));

    let mut canon = HashMap::new();
    let mut repos = RepoCache::default();
    let mut by: HashMap<String, ProjectInfo> = HashMap::new();
    for s in &sessions {
        let id = project_id(s, &mut canon);
        let p = by.entry(id.clone()).or_insert_with(|| {
            let cwd = id.clone();
            let (root, label, wt) = if s.cwd.is_empty() {
                (id.clone(), id.clone(), false)
            } else {
                let r = repos.get(&cwd);
                (r.root, r.label, r.is_worktree)
            };
            let label_own = std::path::Path::new(&cwd)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| cwd.clone());
            ProjectInfo {
                label: label_own,
                cwd,
                repo_key: root,
                repo_label: label,
                is_worktree: wt,
                id,
                sources: Vec::new(),
                session_count: 0,
                last_ms: 0,
            }
        });
        p.session_count += 1;
        p.last_ms = p.last_ms.max(s.end_ms.max(s.mtime_ms));
        if !p.sources.contains(&s.source) {
            p.sources.push(s.source);
        }
    }
    let mut projects: Vec<ProjectInfo> = by.into_values().collect();
    projects.sort_by(|a, b| b.last_ms.cmp(&a.last_ms).then_with(|| a.id.cmp(&b.id)));
    ScanResult {
        projects,
        sessions,
        computed,
    }
}

/// Sessions of one project, in scan order.
pub fn sessions_of<'a>(r: &'a ScanResult, project_id_: &str) -> Vec<&'a SessionMeta> {
    let mut canon = HashMap::new();
    r.sessions
        .iter()
        .filter(|s| project_id(s, &mut canon) == project_id_)
        .collect()
}

/// Apply pinned / hidden lists (`AppConfig::pinned_sessions` / `hidden_sessions`): hidden
/// sessions are dropped unless `show_hidden`; pinned ones come first, each group newest first.
pub fn order_for_display<'a>(
    list: Vec<&'a SessionMeta>,
    pinned: &[String],
    hidden: &[String],
    show_hidden: bool,
) -> Vec<&'a SessionMeta> {
    let mut v: Vec<&SessionMeta> = list
        .into_iter()
        .filter(|s| show_hidden || !hidden.contains(&s.id))
        .collect();
    v.sort_by(|a, b| {
        let pa = pinned.contains(&a.id);
        let pb = pinned.contains(&b.id);
        pb.cmp(&pa)
            .then_with(|| b.end_ms.cmp(&a.end_ms))
            .then_with(|| a.id.cmp(&b.id))
    });
    v
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::chunks::fixtures;
    use super::*;

    pub(crate) fn make_roots(d: &std::path::Path) -> Roots {
        let claude = d.join("claude");
        let proj = claude.join("projects/-work-proj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("s1.jsonl"), fixtures::claude()).unwrap();
        let second = fixtures::claude().replace("Fix the login bug", "Another task");
        std::fs::write(
            proj.join("s2.jsonl"),
            second.replace("2025-03-01", "2025-03-05"),
        )
        .unwrap();
        let codex = d.join("codex/sessions/2025/03/02");
        std::fs::create_dir_all(&codex).unwrap();
        std::fs::write(
            codex.join("rollout-2025-03-02-cx1.jsonl"),
            fixtures::codex(),
        )
        .unwrap();
        let home = d.join("home");
        std::fs::create_dir_all(home.join("t/runs/ada")).unwrap();
        std::fs::write(
            home.join("t/runs/ada/1700000000000.jsonl"),
            [
                r#"{"type":"assistant","message":{"id":"x","model":"claude-sonnet-4-5","content":[{"type":"text","text":"hello from run"}],"usage":{"input_tokens":3,"output_tokens":4}}}"#,
                r#"{"type":"user","message":{"role":"user","content":"do the thing"}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        Roots::new(&home, Some(claude), Some(d.join("codex")))
    }

    #[test]
    fn scans_all_sources_and_groups_projects() {
        let d = tempfile::tempdir().unwrap();
        let roots = make_roots(d.path());
        let r = scan(&roots);
        assert_eq!(r.sessions.len(), 4);
        assert_eq!(r.computed, 4);
        // newest first: s2 (March 5) before s1; the run file has no clock so mtime (now) leads
        let pos = |id: &str| r.sessions.iter().position(|s| s.id == id).unwrap();
        assert!(pos("s2") < pos("s1"));
        assert_eq!(r.sessions[0].id, "1700000000000");
        let ids: Vec<&str> = r.projects.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"/work/proj"));
        assert!(ids.contains(&"/work/codexproj"));
        assert!(ids.contains(&"rteams/t"));
        let p = r.projects.iter().find(|p| p.id == "/work/proj").unwrap();
        assert_eq!((p.session_count, p.label.as_str()), (2, "proj"));
        assert_eq!(sessions_of(&r, "/work/proj").len(), 2);
        // second scan reuses the index
        assert_eq!(scan(&roots).computed, 0);
    }

    #[test]
    fn pinned_first_hidden_dropped() {
        let d = tempfile::tempdir().unwrap();
        let r = scan(&make_roots(d.path()));
        let all: Vec<&SessionMeta> = r.sessions.iter().collect();
        let v = order_for_display(all.clone(), &["s1".into()], &["s2".into()], false);
        assert_eq!(v[0].id, "s1");
        assert!(v.iter().all(|s| s.id != "s2"));
        let v = order_for_display(all, &[], &["s2".into()], true);
        assert!(v.iter().any(|s| s.id == "s2"));
    }
}
