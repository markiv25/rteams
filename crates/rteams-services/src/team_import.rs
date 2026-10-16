//! Team import (original `team-import`) plus the shared `TeamDraft` that templates, the import
//! dialog and the shell use. Everything here is plain data: the shell turns a `TeamDraft` into
//! `CreateTeam` / `ConfigureTeam` / `AddMember` commands.
//!
//! Sources: a folder with `.claude/agents/*.md`, `.claude/CLAUDE.md` / `CLAUDE.md` and
//! `.claude/skills/*`; a JSON or small YAML team-definition file; another rteams team's roster.

use std::fs;
use std::path::Path;

use rteams_core::{Event, State};

use crate::i18n::I18n;

const MAX_FILE: u64 = 256 * 1024;
const MAX_AGENT_FILES: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftMember {
    pub name: String,
    pub role: String,
    /// Runtime id (`claude`, `codex`, ...).
    pub runtime: String,
    /// Standing prompt (workflow).
    pub prompt: String,
}

/// What the shell creates: a team with a roster, a lead prompt and a project path.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TeamDraft {
    pub team_name: String,
    pub members: Vec<DraftMember>,
    pub lead_prompt: String,
    pub project_path: Option<String>,
}

/// A problem found while importing; `key` is an i18n key, `args` its `{{placeholders}}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportWarning {
    pub key: &'static str,
    pub args: Vec<(&'static str, String)>,
}

impl ImportWarning {
    fn new(key: &'static str, args: &[(&'static str, &str)]) -> Self {
        ImportWarning {
            key,
            args: args.iter().map(|(k, v)| (*k, v.to_string())).collect(),
        }
    }

    pub fn text(&self, tr: &I18n) -> String {
        let a: Vec<(&str, &str)> = self.args.iter().map(|(k, v)| (*k, v.as_str())).collect();
        tr.t_args(self.key, &a)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportPreview {
    pub suggested_name: String,
    pub members: Vec<DraftMember>,
    pub lead_prompt: String,
    pub skills: Vec<String>,
    pub warnings: Vec<ImportWarning>,
}

impl ImportPreview {
    pub fn into_draft(self, team_name: &str, project_path: Option<String>) -> TeamDraft {
        TeamDraft {
            team_name: team_name.trim().to_string(),
            members: self.members,
            lead_prompt: self.lead_prompt,
            project_path,
        }
    }
}

// ---- names -------------------------------------------------------------------------------

const RESERVED_TEAM_NAMES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    Required,
    InvalidFormat,
    Reserved,
}

impl NameError {
    /// i18n key of the message.
    pub fn key(self) -> &'static str {
        match self {
            NameError::Required => "team.teamImport.teamNameRequired",
            NameError::InvalidFormat => "team.teamImport.invalidTeamName",
            NameError::Reserved => "team.teamImport.teamNameReserved",
        }
    }
}

/// Kebab-case, at most 64 characters, not an OS-reserved device name.
pub fn validate_team_name(name: &str) -> Result<(), NameError> {
    let n = name.trim();
    if n.is_empty() {
        return Err(NameError::Required);
    }
    let kebab = n.len() <= 64
        && n.split('-').all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        });
    if !kebab {
        return Err(NameError::InvalidFormat);
    }
    if RESERVED_TEAM_NAMES.contains(&n) {
        return Err(NameError::Reserved);
    }
    Ok(())
}

/// Folder name -> valid team name.
pub fn suggest_name(folder_name: &str) -> String {
    let mut out = String::new();
    let mut dash = true;
    for c in folder_name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let out: String = out.trim_matches('-').chars().take(64).collect();
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "imported-team".into()
    } else if RESERVED_TEAM_NAMES.contains(&out.as_str()) {
        format!("team-{out}")
    } else {
        out
    }
}

fn member_name_problem(name: &str) -> Option<&'static str> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid {
        return Some("team.teamImport.warningMemberInvalid");
    }
    let l = name.to_lowercase();
    if matches!(l.as_str(), "user" | "team-lead" | "lead") {
        return Some("team.teamImport.warningMemberReserved");
    }
    if let Some((_, suf)) = l.rsplit_once('-') {
        if !suf.is_empty() && suf.chars().all(|c| c.is_ascii_digit()) {
            return Some("team.teamImport.warningMemberReservedSuffix");
        }
    }
    None
}

// ---- reading -----------------------------------------------------------------------------

/// Replay a team directory's `events.jsonl` into a `State` (`None` if there is no log).
pub fn load_state(team_dir: &Path) -> Option<State> {
    let text = fs::read_to_string(team_dir.join("events.jsonl")).ok()?;
    let mut s = State::default();
    for line in text.lines() {
        if let Ok(e) = serde_json::from_str::<Event>(line) {
            s.apply(&e);
        }
    }
    Some(s)
}

/// Read a regular file (no symlinks), at most 256 KiB.
fn safe_read(path: &Path) -> Option<String> {
    let m = fs::symlink_metadata(path).ok()?;
    if !m.is_file() || m.len() > MAX_FILE {
        return None;
    }
    fs::read_to_string(path).ok()
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Frontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub tools: Option<String>,
    pub model: Option<String>,
    pub skills: Vec<String>,
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let rest = content
        .strip_prefix("---\r\n")
        .or_else(|| content.strip_prefix("---\n"))?;
    let end = rest.find("\n---")?;
    let fm = &rest[..end];
    let after = &rest[end + 4..];
    Some((fm, after.trim_start_matches(['\r', '\n'])))
}

/// Parse the `name`, `description`, `tools`, `model` and `skills` frontmatter fields.
pub fn parse_frontmatter(content: &str) -> Frontmatter {
    let mut f = Frontmatter::default();
    let Some((fm, _)) = split_frontmatter(content) else {
        return f;
    };
    let mut in_skills = false;
    for line in fm.lines() {
        if in_skills {
            if let Some(item) = line.trim_start().strip_prefix("- ") {
                let v = unquote(item);
                if !v.is_empty() {
                    f.skills.push(v);
                }
                continue;
            }
            in_skills = false;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        let v = v.trim();
        match k.trim() {
            "name" if !v.is_empty() => f.name = Some(unquote(v)),
            "description" if !v.is_empty() => f.description = Some(unquote(v)),
            "tools" if !v.is_empty() => f.tools = Some(unquote(v)),
            "model" if !v.is_empty() => f.model = Some(unquote(v)),
            "skills" => {
                if let Some(list) = v.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
                    f.skills
                        .extend(list.split(',').map(unquote).filter(|s| !s.is_empty()));
                } else {
                    in_skills = true;
                }
            }
            _ => {}
        }
    }
    f
}

/// Markdown body without the frontmatter, trimmed.
pub fn markdown_body(content: &str) -> String {
    split_frontmatter(content)
        .map_or(content, |(_, b)| b)
        .trim()
        .to_string()
}

/// Scan an agent-team folder (read-only, no symlinks, size and count caps).
pub fn inspect(folder: &Path) -> ImportPreview {
    let mut p = ImportPreview {
        suggested_name: suggest_name(
            &folder
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
        ..Default::default()
    };
    // Lead prompt.
    let lead = [folder.join(".claude/CLAUDE.md"), folder.join("CLAUDE.md")]
        .iter()
        .find_map(|f| safe_read(f));
    match lead {
        Some(t) => p.lead_prompt = t.trim().to_string(),
        None => p.warnings.push(ImportWarning::new(
            "team.teamImport.warningMissingClaudeMd",
            &[],
        )),
    }
    // Members.
    let mut files: Vec<_> = fs::read_dir(folder.join(".claude/agents"))
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    files.retain(|f| f.extension().is_some_and(|x| x == "md"));
    files.sort();
    files.truncate(MAX_AGENT_FILES);
    for f in files {
        let Some(text) = safe_read(&f) else { continue };
        let file_name = f
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let fm = parse_frontmatter(&text);
        let name = fm.name.clone().unwrap_or_else(|| {
            f.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        if let Some(key) = member_name_problem(&name) {
            p.warnings.push(ImportWarning::new(
                key,
                &[("fileName", &file_name), ("name", &name)],
            ));
            continue;
        }
        if p.members.iter().any(|m| m.name.eq_ignore_ascii_case(&name)) {
            p.warnings.push(ImportWarning::new(
                "team.teamImport.warningDuplicateMember",
                &[("fileName", &file_name), ("name", &name)],
            ));
            continue;
        }
        let role = fm
            .description
            .as_deref()
            .and_then(|d| d.lines().next())
            .map(|l| l.chars().take(120).collect::<String>())
            .unwrap_or_else(|| "Imported agent".into());
        p.members.push(DraftMember {
            name,
            role,
            runtime: "claude".into(),
            prompt: markdown_body(&text),
        });
    }
    // Skills (directory names only).
    if let Ok(rd) = fs::read_dir(folder.join(".claude/skills")) {
        let mut s: Vec<String> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| !t.is_symlink()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        s.sort();
        p.skills = s;
    }
    p
}

/// Copy the roster, lead prompt and project path of another rteams team (members that are
/// removed or the lead are skipped; the new team gets its own lead).
pub fn from_team(home: &Path, team_dir_name: &str) -> Result<ImportPreview, String> {
    let state = load_state(&home.join(team_dir_name)).ok_or("team not found")?;
    let t = state.team.ok_or("team has no configuration yet")?;
    Ok(ImportPreview {
        suggested_name: suggest_name(&format!("{team_dir_name}-copy")),
        lead_prompt: t.lead_prompt.clone().unwrap_or_default(),
        members: t
            .members
            .iter()
            .filter(|m| !m.removed && m.role != "lead")
            .map(|m| DraftMember {
                name: m.name.clone(),
                role: m.role.clone(),
                runtime: m.runtime.clone().unwrap_or_else(|| "claude".into()),
                prompt: m.prompt.clone().unwrap_or_default(),
            })
            .collect(),
        skills: Vec::new(),
        warnings: Vec::new(),
    })
}

// ---- definition files --------------------------------------------------------------------

fn from_json(v: &serde_json::Value) -> Result<(String, ImportPreview, Option<String>), String> {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let name = s("name").or_else(|| s("team_name")).unwrap_or_default();
    let members = v
        .get("members")
        .and_then(|m| m.as_array())
        .ok_or("definition has no `members` list")?
        .iter()
        .filter_map(|m| {
            let g = |k: &str| m.get(k).and_then(|x| x.as_str()).map(str::to_string);
            Some(DraftMember {
                name: g("name")?,
                role: g("role").unwrap_or_else(|| "member".into()),
                runtime: g("runtime").unwrap_or_else(|| "claude".into()),
                prompt: g("prompt").or_else(|| g("workflow")).unwrap_or_default(),
            })
        })
        .collect();
    let preview = ImportPreview {
        suggested_name: if name.is_empty() {
            "imported-team".into()
        } else {
            suggest_name(&name)
        },
        lead_prompt: s("lead_prompt")
            .or_else(|| s("leadPrompt"))
            .unwrap_or_default(),
        members,
        ..Default::default()
    };
    Ok((name, preview, s("project_path").or_else(|| s("project"))))
}

/// A small YAML subset: top-level scalars (`name`, `project_path`), `lead_prompt: |` block,
/// and a `members:` list of `- name: x` items with `role`, `runtime`, `prompt` fields
/// (`prompt: |` blocks allowed).
fn yaml_to_json(text: &str) -> serde_json::Value {
    use serde_json::{json, Map, Value};
    let mut top = Map::new();
    let mut members: Vec<Map<String, Value>> = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    let mut in_members = false;
    // Collect an indented block scalar following `key: |`.
    let block = |i: &mut usize, parent_indent: usize| -> String {
        let mut out = Vec::new();
        let mut base: Option<usize> = None;
        while *i < lines.len() {
            let l = lines[*i];
            let ind = l.len() - l.trim_start().len();
            if !l.trim().is_empty() && ind <= parent_indent {
                break;
            }
            let b = *base.get_or_insert(ind);
            out.push(if l.len() >= b {
                l[b..].to_string()
            } else {
                String::new()
            });
            *i += 1;
        }
        out.join("\n").trim_end().to_string()
    };
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let (item_start, body) = match trimmed.strip_prefix("- ") {
            Some(r) if in_members => (true, r),
            _ => (false, trimmed),
        };
        let Some((k, v)) = body.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim().to_string(), v.trim());
        let value = if v == "|" || v == ">" {
            json!(block(&mut i, indent))
        } else {
            json!(unquote(v))
        };
        if indent == 0 && !item_start {
            in_members = k == "members";
            if !in_members {
                top.insert(k, value);
            }
        } else if in_members {
            if item_start {
                members.push(Map::new());
            }
            if let Some(m) = members.last_mut() {
                m.insert(k, value);
            }
        }
    }
    top.insert(
        "members".into(),
        Value::Array(members.into_iter().map(Value::Object).collect()),
    );
    Value::Object(top)
}

/// Parse a team-definition JSON or YAML-ish text.
pub fn parse_definition(text: &str) -> Result<(String, ImportPreview, Option<String>), String> {
    let t = text.trim_start();
    let v = if t.starts_with('{') {
        serde_json::from_str(t).map_err(|e| e.to_string())?
    } else {
        yaml_to_json(t)
    };
    from_json(&v)
}

/// Import from a path: a folder (`inspect`) or a definition file (`.json`, `.yaml`, `.yml`).
pub fn import_path(path: &Path) -> Result<ImportPreview, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if meta.is_dir() {
        return Ok(inspect(path));
    }
    let text = safe_read(path).ok_or("file is not readable or larger than 256 KiB")?;
    let (_, mut p, _) = parse_definition(&text)?;
    if p.members.is_empty() {
        p.warnings
            .push(ImportWarning::new("team.teamImport.selectPrompt", &[]));
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(p: &Path, text: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    #[test]
    fn team_names() {
        assert!(validate_team_name("my-team-2").is_ok());
        assert_eq!(validate_team_name(" "), Err(NameError::Required));
        assert_eq!(validate_team_name("My_Team"), Err(NameError::InvalidFormat));
        assert_eq!(validate_team_name("con"), Err(NameError::Reserved));
        assert_eq!(
            validate_team_name(&"a".repeat(65)),
            Err(NameError::InvalidFormat)
        );
        assert_eq!(suggest_name("My Cool_Project!"), "my-cool-project");
        assert_eq!(suggest_name("NUL"), "team-nul");
        assert_eq!(suggest_name("***"), "imported-team");
    }

    #[test]
    fn inspects_claude_folder() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path().join("My App");
        w(&r.join(".claude/CLAUDE.md"), "Lead does things\n");
        w(
            &r.join(".claude/agents/reviewer.md"),
            "---\nname: reviewer\ndescription: \"Reviews code\"\ntools: Read, Grep\nskills:\n  - lint\n---\nBe strict.\n",
        );
        w(&r.join(".claude/agents/dev.md"), "No frontmatter body");
        w(
            &r.join(".claude/agents/dev2.md"),
            "---\nname: dev\n---\nDup",
        );
        w(&r.join(".claude/agents/lead.md"), "reserved");
        w(&r.join(".claude/agents/bad name.md"), "x");
        w(&r.join(".claude/agents/qa-2.md"), "x");
        w(&r.join(".claude/skills/pdf/SKILL.md"), "x");
        let p = inspect(&r);
        assert_eq!(p.suggested_name, "my-app");
        assert_eq!(p.lead_prompt, "Lead does things");
        assert_eq!(p.skills, vec!["pdf"]);
        let names: Vec<_> = p.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["dev", "reviewer"]);
        let rev = &p.members[1];
        assert_eq!(rev.role, "Reviews code");
        assert_eq!(rev.prompt, "Be strict.");
        let keys: Vec<_> = p.warnings.iter().map(|w| w.key).collect();
        assert!(keys.contains(&"team.teamImport.warningDuplicateMember"));
        assert!(keys.contains(&"team.teamImport.warningMemberReserved"));
        assert!(keys.contains(&"team.teamImport.warningMemberInvalid"));
        assert!(keys.contains(&"team.teamImport.warningMemberReservedSuffix"));
        assert!(!keys.contains(&"team.teamImport.warningMissingClaudeMd"));
    }

    #[test]
    fn missing_claude_md_warns_and_text_renders() {
        let d = tempfile::tempdir().unwrap();
        let p = inspect(d.path());
        assert_eq!(p.warnings.len(), 1);
        let tr = I18n::default();
        assert_eq!(ImportWarning::new("k.x", &[("a", "1")]).text(&tr), "k.x");
    }

    #[test]
    fn parses_json_definition() {
        let (n, p, proj) = parse_definition(
            r#"{"name":"Alpha Team","project_path":"/p","lead_prompt":"L","members":[{"name":"a","role":"dev","runtime":"codex","prompt":"do"},{"role":"x"}]}"#,
        )
        .unwrap();
        assert_eq!(n, "Alpha Team");
        assert_eq!(p.suggested_name, "alpha-team");
        assert_eq!(proj.as_deref(), Some("/p"));
        assert_eq!(p.members.len(), 1);
        assert_eq!(p.members[0].runtime, "codex");
    }

    #[test]
    fn parses_yaml_definition() {
        let y = "name: alpha\nproject_path: /p\nlead_prompt: |\n  line one\n  line two\nmembers:\n  - name: ada\n    role: dev\n    prompt: |\n      write code\n      carefully\n  - name: bob\n    role: qa\n    runtime: codex\n";
        let (n, p, proj) = parse_definition(y).unwrap();
        assert_eq!(n, "alpha");
        assert_eq!(proj.as_deref(), Some("/p"));
        assert_eq!(p.lead_prompt, "line one\nline two");
        assert_eq!(p.members.len(), 2);
        assert_eq!(p.members[0].prompt, "write code\ncarefully");
        assert_eq!(p.members[1].runtime, "codex");
        assert_eq!(p.members[0].runtime, "claude");
    }

    #[test]
    fn import_path_dispatch_and_draft() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("t.json");
        fs::write(&f, r#"{"members":[{"name":"a","role":"r"}]}"#).unwrap();
        let p = import_path(&f).unwrap();
        let dr = p.into_draft(" x ", Some("/p".into()));
        assert_eq!(dr.team_name, "x");
        assert_eq!(dr.members.len(), 1);
        assert!(import_path(&d.path().join("nope")).is_err());
    }
}
