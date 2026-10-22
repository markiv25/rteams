//! Session context helpers: which CLAUDE.md / rules / memory files apply to a project, the
//! project's `.claude/agents` configs, and `@path` mention validation / reading for prompts.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// Size of one context file (or a directory of `.md` files).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    pub path: PathBuf,
    pub exists: bool,
    pub char_count: usize,
    pub estimated_tokens: usize,
}

fn missing(path: PathBuf) -> FileInfo {
    FileInfo {
        path,
        exists: false,
        char_count: 0,
        estimated_tokens: 0,
    }
}

/// ~4 characters per token, as the original estimates.
pub fn count_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

fn info_of(path: &Path, text: &str) -> FileInfo {
    let chars = text.chars().count();
    FileInfo {
        path: path.to_path_buf(),
        exists: true,
        char_count: chars,
        estimated_tokens: count_tokens(text),
    }
}

pub fn read_file_info(path: &Path) -> FileInfo {
    match std::fs::read_to_string(path) {
        Ok(t) => info_of(path, &t),
        Err(_) => missing(path.to_path_buf()),
    }
}

fn collect_md(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            collect_md(&p, out);
        } else if p.extension().is_some_and(|x| x == "md") {
            out.push(p);
        }
    }
}

/// Combined size of every `.md` file under `dir` (recursive).
pub fn read_dir_md_info(dir: &Path) -> FileInfo {
    let mut files = Vec::new();
    collect_md(dir, &mut files);
    let texts: Vec<String> = files
        .iter()
        .filter_map(|f| std::fs::read_to_string(f).ok())
        .collect();
    if texts.is_empty() {
        return missing(dir.to_path_buf());
    }
    let chars = texts.iter().map(|t| t.chars().count()).sum();
    FileInfo {
        path: dir.to_path_buf(),
        exists: true,
        char_count: chars,
        estimated_tokens: count_tokens(&texts.join("\n")),
    }
}

/// Claude's auto-memory folder name for a project: the path with separators turned into `-`.
fn memory_slug(project: &Path) -> String {
    project
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The CLAUDE.md sources that apply to `project`, in the original's order and keys.
/// `claude_base` is the Claude root (`~/.claude`); `enterprise` the managed-policy file.
pub fn claude_md_files(
    project: &Path,
    claude_base: &Path,
    enterprise: &Path,
) -> Vec<(&'static str, FileInfo)> {
    vec![
        ("enterprise", read_file_info(enterprise)),
        ("user", read_file_info(&claude_base.join("CLAUDE.md"))),
        ("project", read_file_info(&project.join("CLAUDE.md"))),
        (
            "project-alt",
            read_file_info(&project.join(".claude").join("CLAUDE.md")),
        ),
        (
            "project-rules",
            read_dir_md_info(&project.join(".claude").join("rules")),
        ),
        (
            "project-local",
            read_file_info(&project.join("CLAUDE.local.md")),
        ),
        ("user-rules", read_dir_md_info(&claude_base.join("rules"))),
        (
            "auto-memory",
            read_file_info(
                &claude_base
                    .join("projects")
                    .join(memory_slug(project))
                    .join("memory")
                    .join("MEMORY.md"),
            ),
        ),
    ]
}

/// Platform path of the managed-policy CLAUDE.md.
pub fn enterprise_path() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/ClaudeCode/CLAUDE.md")
    } else if cfg!(windows) {
        PathBuf::from("C:\\Program Files\\ClaudeCode\\CLAUDE.md")
    } else {
        PathBuf::from("/etc/claude-code/CLAUDE.md")
    }
}

/// A project subagent definition from `.claude/agents/*.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConfig {
    pub name: String,
    pub color: Option<String>,
}

fn frontmatter_value(text: &str, key: &str) -> Option<String> {
    let rest = text.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    for line in rest[..end].lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim() == key {
                let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
                return (!v.is_empty()).then(|| v.to_string());
            }
        }
    }
    None
}

/// Agents defined under `<project>/.claude/agents`, keyed by name.
pub fn read_agent_configs(project: &Path) -> BTreeMap<String, AgentConfig> {
    let mut out = BTreeMap::new();
    let Ok(rd) = std::fs::read_dir(project.join(".claude").join("agents")) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "md") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = frontmatter_value(&text, "name").unwrap_or(stem);
        let color = frontmatter_value(&text, "color");
        out.insert(name.clone(), AgentConfig { name, color });
    }
    out
}

fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolve a mention against the project; `None` when it escapes the project root.
pub fn resolve_mention(project: &Path, value: &str) -> Option<PathBuf> {
    let v = Path::new(value);
    let full = if v.is_absolute() {
        lexical_normalize(v)
    } else {
        lexical_normalize(&project.join(v))
    };
    full.starts_with(lexical_normalize(project)).then_some(full)
}

/// `@path` mentions in prompt text (without the `@`). A mention starts a word and runs to
/// whitespace; trailing punctuation is trimmed. Bare `@name` without `/` or `.` is not a path.
pub fn extract_mentions(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut prev_ws = true;
    for (i, c) in text.char_indices() {
        if c == '@' && prev_ws {
            let rest = &text[i + 1..];
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let tok = rest[..end].trim_end_matches([',', ';', ':', '!', '?', ')', '"', '\'']);
            let tok = tok.trim_end_matches('.');
            if !tok.is_empty() && (tok.contains('/') || tok.contains('.')) {
                let t = tok.to_string();
                if !out.contains(&t) {
                    out.push(t);
                }
            }
        }
        prev_ws = c.is_whitespace();
    }
    out
}

/// For each mention (`@value` key), whether it points at something inside the project.
pub fn validate_mentions(project: &Path, mentions: &[String]) -> BTreeMap<String, bool> {
    mentions
        .iter()
        .map(|m| {
            let ok = resolve_mention(project, m).is_some_and(|p| p.exists());
            (format!("@{m}"), ok)
        })
        .collect()
}

/// Size of a mentioned file; `None` when missing, outside the project, not a file, or over
/// `max_tokens`.
pub fn read_mentioned_file(project: &Path, value: &str, max_tokens: usize) -> Option<FileInfo> {
    let p = resolve_mention(project, value)?;
    if !p.is_file() {
        return None;
    }
    let info = info_of(&p, &std::fs::read_to_string(&p).ok()?);
    (info.estimated_tokens <= max_tokens).then_some(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_md_and_rules_and_agents() {
        let d = tempfile::tempdir().unwrap();
        let proj = d.path().join("proj");
        let base = d.path().join("base");
        std::fs::create_dir_all(proj.join(".claude/rules/sub")).unwrap();
        std::fs::create_dir_all(proj.join(".claude/agents")).unwrap();
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(proj.join("CLAUDE.md"), "12345678").unwrap();
        std::fs::write(proj.join(".claude/rules/sub/a.md"), "abcd").unwrap();
        std::fs::write(base.join("CLAUDE.md"), "x").unwrap();
        std::fs::write(
            proj.join(".claude/agents/rev.md"),
            "---\nname: reviewer\ncolor: \"blue\"\n---\nbody",
        )
        .unwrap();
        std::fs::write(proj.join(".claude/agents/plain.md"), "no frontmatter").unwrap();
        let f = claude_md_files(&proj, &base, &d.path().join("none.md"));
        let get = |k: &str| f.iter().find(|(n, _)| *n == k).unwrap().1.clone();
        assert!(get("project").exists);
        assert_eq!(get("project").estimated_tokens, 2);
        assert!(get("project-rules").exists);
        assert!(get("user").exists);
        assert!(!get("enterprise").exists);
        assert!(!get("project-local").exists);
        let a = read_agent_configs(&proj);
        assert_eq!(a["reviewer"].color.as_deref(), Some("blue"));
        assert!(a.contains_key("plain"));
    }

    #[test]
    fn mentions_extract_validate_read() {
        assert_eq!(
            extract_mentions("see @src/a.rs, and @b.txt. @alice hi a@b.c"),
            vec!["src/a.rs", "b.txt"]
        );
        let d = tempfile::tempdir().unwrap();
        let proj = d.path().join("p");
        std::fs::create_dir_all(proj.join("src")).unwrap();
        std::fs::write(proj.join("src/a.rs"), "fn main(){}").unwrap();
        std::fs::write(d.path().join("secret.txt"), "s").unwrap();
        let m = vec![
            "src/a.rs".to_string(),
            "nope.rs".to_string(),
            "../secret.txt".to_string(),
        ];
        let v = validate_mentions(&proj, &m);
        assert!(v["@src/a.rs"]);
        assert!(!v["@nope.rs"]);
        assert!(!v["@../secret.txt"], "escape is rejected");
        assert!(read_mentioned_file(&proj, "src/a.rs", 100).is_some());
        assert!(read_mentioned_file(&proj, "src/a.rs", 1).is_none());
        assert!(read_mentioned_file(&proj, "src", 100).is_none());
    }
}
