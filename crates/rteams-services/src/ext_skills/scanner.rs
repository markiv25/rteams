//! Scan skill roots for `<folder>/SKILL.md` and `.claude/agents/*.md`.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use super::metadata::{self, SkillMeta};
use super::roots::{RootKind, Scope, SkillRoot};
use super::validator::{self, Issue};

const SKILL_FILE_NAMES: [&str; 3] = ["SKILL.md", "Skill.md", "skill.md"];

#[derive(Debug, Clone)]
pub struct Skill {
    pub folder_name: String,
    pub dir: PathBuf,
    pub skill_file: PathBuf,
    pub scope: Scope,
    pub kind: RootKind,
    pub root_label: String,
    pub read_only: bool,
    pub meta: SkillMeta,
    pub body: String,
    /// Relative paths of every file in the folder (sorted).
    pub files: Vec<String>,
    pub has_scripts: bool,
    pub has_references: bool,
    pub has_assets: bool,
    pub modified_ms: i64,
    pub issues: Vec<Issue>,
}

impl Skill {
    pub fn display_name(&self) -> &str {
        self.meta
            .name
            .as_deref()
            .filter(|n| !n.is_empty())
            .unwrap_or(&self.folder_name)
    }
    pub fn needs_attention(&self) -> bool {
        validator::has_errors(&self.issues)
    }
    pub fn manual_only(&self) -> bool {
        self.meta.disable_model_invocation
    }
}

#[derive(Debug, Clone)]
pub struct AgentDef {
    pub name: String,
    pub description: String,
    pub tools: Vec<String>,
    pub model: Option<String>,
    pub path: PathBuf,
    pub scope: Scope,
}

/// Find the skill file in `dir` (case variants), if any.
pub fn find_skill_file(dir: &Path) -> Option<PathBuf> {
    SKILL_FILE_NAMES
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.is_file())
}

fn list_files(dir: &Path) -> Vec<String> {
    let mut out = vec![];
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>, depth: usize) {
        if depth > 8 || out.len() > 2000 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                walk(base, &p, out, depth + 1);
            } else if ft.is_file() {
                if let Ok(r) = p.strip_prefix(base) {
                    out.push(r.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    walk(dir, dir, &mut out, 0);
    out.sort();
    out
}

fn mtime_ms(p: &Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

/// Read one skill folder.
pub fn read_skill(dir: &Path, root: &SkillRoot) -> Option<Skill> {
    let skill_file = find_skill_file(dir)?;
    let text = std::fs::read_to_string(&skill_file).unwrap_or_default();
    let (meta, body, fm) = metadata::parse(&text);
    let files = list_files(dir);
    let has = |d: &str| files.iter().any(|f| f.starts_with(&format!("{d}/")));
    let (has_scripts, has_references, has_assets) =
        (has("scripts"), has("references"), has("assets"));
    let folder_name = dir.file_name()?.to_string_lossy().into_owned();
    let issues = validator::validate(&meta, &fm, &body, Some(&folder_name), has_scripts);
    let modified_ms = files
        .iter()
        .map(|f| mtime_ms(&dir.join(f)))
        .max()
        .unwrap_or_else(|| mtime_ms(&skill_file));
    Some(Skill {
        folder_name,
        dir: dir.to_path_buf(),
        skill_file,
        scope: root.scope,
        kind: root.kind,
        root_label: root.label.clone(),
        read_only: root.read_only,
        meta,
        body,
        files,
        has_scripts,
        has_references,
        has_assets,
        modified_ms,
        issues,
    })
}

/// Scan every root; missing roots are skipped. Sorted by display name.
pub fn scan(roots: &[SkillRoot]) -> Vec<Skill> {
    let mut out = vec![];
    for r in roots {
        let Ok(rd) = std::fs::read_dir(&r.dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            // follow symlinked skill folders, like the runtimes do
            if !p.is_dir() {
                continue;
            }
            if p.file_name()
                .is_none_or(|n| n.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            if let Some(s) = read_skill(&p, r) {
                out.push(s);
            }
        }
    }
    out.sort_by(|a, b| {
        a.display_name()
            .to_lowercase()
            .cmp(&b.display_name().to_lowercase())
            .then(a.dir.cmp(&b.dir))
    });
    out
}

/// Scan `.claude/agents/*.md`.
pub fn scan_agents(dirs: &[(Scope, PathBuf)]) -> Vec<AgentDef> {
    let mut out = vec![];
    for (scope, dir) in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "md") || !p.is_file() {
                continue;
            }
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            let (_, _, fm) = metadata::parse(&text);
            let stem = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let tools = match fm.get("tools") {
                Some(metadata::Value::List(l)) => l.clone(),
                Some(metadata::Value::Str(s)) => s
                    .split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect(),
                None => vec![],
            };
            out.push(AgentDef {
                name: fm.get_str("name").filter(|n| !n.is_empty()).unwrap_or(stem),
                description: fm.get_str("description").unwrap_or_default(),
                tools,
                model: fm.get_str("model"),
                path: p,
                scope: *scope,
            });
        }
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ext_skills::roots::skill_roots;

    #[test]
    fn scans_skills_and_agents() {
        let h = tempfile::tempdir().unwrap();
        let p = tempfile::tempdir().unwrap();
        let s = h.path().join(".claude/skills/alpha");
        std::fs::create_dir_all(s.join("scripts")).unwrap();
        std::fs::write(
            s.join("SKILL.md"),
            "---\nname: alpha\ndescription: A skill\nallowed-tools: Read\n---\nDo it",
        )
        .unwrap();
        std::fs::write(s.join("scripts/run.sh"), "echo hi").unwrap();
        let q = p.path().join(".codex/skills/beta");
        std::fs::create_dir_all(&q).unwrap();
        std::fs::write(q.join("skill.md"), "no frontmatter").unwrap();
        std::fs::create_dir_all(p.path().join(".claude/agents")).unwrap();
        std::fs::write(
            p.path().join(".claude/agents/rev.md"),
            "---\nname: rev\ndescription: Reviews\ntools: Read, Grep\nmodel: opus\n---\nx",
        )
        .unwrap();

        let roots = skill_roots(Some(h.path()), Some(p.path()));
        let skills = scan(&roots);
        assert_eq!(skills.len(), 2);
        let a = &skills[0];
        assert_eq!(a.display_name(), "alpha");
        assert!(a.has_scripts && !a.needs_attention());
        assert_eq!(a.meta.allowed_tools, vec!["Read"]);
        assert_eq!(a.files, vec!["SKILL.md", "scripts/run.sh"]);
        let b = &skills[1];
        assert!(b.needs_attention() && b.kind.is_codex_only() && b.scope == Scope::Project);

        let ag = scan_agents(&crate::ext_skills::roots::agent_dirs(
            Some(h.path()),
            Some(p.path()),
        ));
        assert_eq!(ag.len(), 1);
        assert_eq!(ag[0].tools, vec!["Read", "Grep"]);
        assert_eq!(ag[0].model.as_deref(), Some("opus"));
    }
}
