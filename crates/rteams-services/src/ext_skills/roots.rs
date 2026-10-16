//! Where skills live: `<base>/.claude|.cursor|.agents|.codex/skills` under the user home and the
//! project, plus read-only plugin skills under `~/.claude/plugins`.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    User,
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RootKind {
    Claude,
    Cursor,
    Agents,
    Codex,
    Plugin,
}

impl RootKind {
    /// `.claude`, `.cursor` and `.agents` skills are shared between runtimes.
    pub fn is_shared(self) -> bool {
        matches!(self, RootKind::Claude | RootKind::Cursor | RootKind::Agents)
    }
    pub fn is_codex_only(self) -> bool {
        self == RootKind::Codex
    }
    pub fn dot_dir(self) -> &'static str {
        match self {
            RootKind::Claude | RootKind::Plugin => ".claude",
            RootKind::Cursor => ".cursor",
            RootKind::Agents => ".agents",
            RootKind::Codex => ".codex",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRoot {
    pub scope: Scope,
    pub kind: RootKind,
    pub dir: PathBuf,
    /// Human label such as `~/.claude/skills` or `.claude/skills`.
    pub label: String,
    pub read_only: bool,
}

const KINDS: [RootKind; 4] = [
    RootKind::Claude,
    RootKind::Cursor,
    RootKind::Agents,
    RootKind::Codex,
];

/// All skill roots (directories need not exist). User roots first.
pub fn skill_roots(user_home: Option<&Path>, project: Option<&Path>) -> Vec<SkillRoot> {
    let mut out = vec![];
    if let Some(h) = user_home {
        for k in KINDS {
            out.push(SkillRoot {
                scope: Scope::User,
                kind: k,
                dir: h.join(k.dot_dir()).join("skills"),
                label: format!("~/{}/skills", k.dot_dir()),
                read_only: false,
            });
        }
        for dir in plugin_skill_dirs(&h.join(".claude").join("plugins")) {
            let label = dir
                .strip_prefix(h)
                .map(|p| format!("~/{}", p.display()))
                .unwrap_or_else(|_| dir.display().to_string());
            out.push(SkillRoot {
                scope: Scope::User,
                kind: RootKind::Plugin,
                dir,
                label,
                read_only: true,
            });
        }
    }
    if let Some(p) = project {
        for k in KINDS {
            out.push(SkillRoot {
                scope: Scope::Project,
                kind: k,
                dir: p.join(k.dot_dir()).join("skills"),
                label: format!("{}/skills", k.dot_dir()),
                read_only: false,
            });
        }
    }
    out
}

/// `.claude/agents` directories (user, project).
pub fn agent_dirs(user_home: Option<&Path>, project: Option<&Path>) -> Vec<(Scope, PathBuf)> {
    let mut v = vec![];
    if let Some(h) = user_home {
        v.push((Scope::User, h.join(".claude").join("agents")));
    }
    if let Some(p) = project {
        v.push((Scope::Project, p.join(".claude").join("agents")));
    }
    v
}

/// Directories named `skills` below `plugins` (bounded depth), e.g. `cache/<mkt>/<plugin>/<ver>/skills`.
pub fn plugin_skill_dirs(plugins: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth == 0 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut subs: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .collect();
        subs.sort();
        for p in subs {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name == "node_modules" || name.starts_with('.') {
                continue;
            }
            if name == "skills" {
                out.push(p);
            } else {
                walk(&p, depth - 1, out);
            }
        }
    }
    walk(plugins, 6, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_for_user_and_project() {
        let r = skill_roots(Some(Path::new("/h")), Some(Path::new("/p")));
        assert_eq!(r.len(), 8);
        assert_eq!(r[0].dir, Path::new("/h/.claude/skills"));
        assert_eq!(r[4].scope, Scope::Project);
        assert!(r[3].kind.is_codex_only());
        assert!(skill_roots(None, None).is_empty());
    }

    #[test]
    fn finds_plugin_skills() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join(".claude/plugins/cache/m/p/1.0/skills/s");
        std::fs::create_dir_all(&d).unwrap();
        let r = skill_roots(Some(t.path()), None);
        assert!(r.iter().any(|x| x.kind == RootKind::Plugin && x.read_only));
    }
}
