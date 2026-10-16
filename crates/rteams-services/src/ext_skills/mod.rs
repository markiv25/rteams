//! ext_skills: discover, validate, scaffold, import and edit Claude/Codex skills
//! (`<root>/<name>/SKILL.md`) and `.claude/agents/*.md`. Headless: every function takes its roots
//! explicitly so tests never touch the real home directory.

pub mod import;
pub mod metadata;
pub mod plan;
pub mod review;
pub mod roots;
pub mod scaffold;
pub mod scanner;
pub mod validator;
pub mod watch;

use std::path::{Path, PathBuf};

pub use metadata::{Frontmatter, SkillMeta};
pub use plan::{ChangeKind, FileChange, Plan};
pub use roots::{RootKind, Scope, SkillRoot};
pub use scanner::{AgentDef, Skill};
pub use validator::{Issue, Severity};

/// Everything the Skills tab lists.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub skills: Vec<Skill>,
    pub agents: Vec<AgentDef>,
}

/// Scan all roots for a user home and optional project.
pub fn scan_all(user_home: Option<&Path>, project: Option<&Path>) -> Catalog {
    let roots = roots::skill_roots(user_home, project);
    Catalog {
        skills: scanner::scan(&roots),
        agents: scanner::scan_agents(&roots::agent_dirs(user_home, project)),
    }
}

/// Move a skill folder to the trash. Refuses anything that is not a direct child of a writable root.
pub fn delete_skill(roots: &[SkillRoot], dir: &Path) -> Result<(), String> {
    let ok = roots
        .iter()
        .any(|r| !r.read_only && dir.parent() == Some(r.dir.as_path()));
    if !ok {
        return Err("not a skill folder inside a writable skill root".into());
    }
    trash::delete(dir).map_err(|e| e.to_string())
}

/// Default user home (`$HOME`).
pub fn default_user_home() -> Option<PathBuf> {
    dirs::home_dir()
}
