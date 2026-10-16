//! Create a new skill from a template (the "Create Skill" form).

use std::collections::BTreeMap;
use std::path::Path;

use super::metadata::{self, SkillMeta};
use super::plan::Plan;
use super::validator;

#[derive(Debug, Clone, Default)]
pub struct Draft {
    pub name: String,
    pub description: String,
    pub when_to_use: String,
    pub steps: String,
    pub notes: String,
    pub license: String,
    pub compatibility: String,
    pub manual_only: bool,
    pub allowed_tools: Vec<String>,
    /// Starter folders to create (`scripts`, `references`, `assets`), each with a README placeholder.
    pub extra_dirs: Vec<String>,
}

/// Suggest a folder name from a free-form skill name: `"My Cool Skill!"` -> `my-cool-skill`.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !out.is_empty() && !dash {
            out.push('-');
            dash = true;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    out.chars()
        .take(validator::MAX_NAME)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string()
}

pub fn meta_for(d: &Draft) -> SkillMeta {
    SkillMeta {
        name: Some(d.name.trim().to_string()),
        description: Some(d.description.trim().to_string()),
        license: Some(d.license.trim().to_string()).filter(|s| !s.is_empty()),
        compatibility: Some(d.compatibility.trim().to_string()).filter(|s| !s.is_empty()),
        disable_model_invocation: d.manual_only,
        allowed_tools: d.allowed_tools.clone(),
        extra: vec![],
    }
}

/// The SKILL.md body generated from the structured fields.
pub fn body_for(d: &Draft) -> String {
    let title = d.name.trim().replace('-', " ");
    let mut b = format!("# {title}\n");
    if !d.when_to_use.trim().is_empty() {
        b.push_str(&format!("\n## When to use\n\n{}\n", d.when_to_use.trim()));
    }
    b.push_str("\n## Steps\n\n");
    if d.steps.trim().is_empty() {
        b.push_str("1. Describe the first step.\n");
    } else {
        b.push_str(d.steps.trim());
        b.push('\n');
    }
    if !d.notes.trim().is_empty() {
        b.push_str(&format!("\n## Notes\n\n{}\n", d.notes.trim()));
    }
    b
}

pub fn render_skill_md(d: &Draft) -> String {
    metadata::render(&meta_for(d), &body_for(d))
}

/// All files for a new skill.
pub fn files_for(d: &Draft) -> BTreeMap<String, Vec<u8>> {
    let mut f = BTreeMap::new();
    f.insert("SKILL.md".to_string(), render_skill_md(d).into_bytes());
    for dir in &d.extra_dirs {
        if matches!(dir.as_str(), "scripts" | "references" | "assets") {
            f.insert(
                format!("{dir}/README.md"),
                format!("# {dir}\n\nAdd {dir} for this skill here.\n").into_bytes(),
            );
        }
    }
    f
}

/// Plan for creating `<root>/<folder>`; errors if the folder name is invalid or already exists.
pub fn plan_create(root: &Path, folder: &str, d: &Draft) -> Result<Plan, String> {
    if !validator::is_valid_folder_name(folder) {
        return Err(
            "Pick a simpler folder name using letters, numbers, dots, dashes, or underscores."
                .into(),
        );
    }
    let target = root.join(folder);
    if target.exists() {
        return Err(format!("A skill folder named \"{folder}\" already exists."));
    }
    Ok(Plan::build(&target, &files_for(d), false))
}

/// Like `plan_create` for an already-built file set (e.g. hand-edited SKILL.md).
pub fn plan_create_with(
    root: &Path,
    folder: &str,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<Plan, String> {
    if !validator::is_valid_folder_name(folder) {
        return Err(
            "Pick a simpler folder name using letters, numbers, dots, dashes, or underscores."
                .into(),
        );
    }
    let target = root.join(folder);
    if target.exists() {
        return Err(format!("A skill folder named \"{folder}\" already exists."));
    }
    Ok(Plan::build(&target, files, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ext_skills::{
        metadata::parse,
        validator::{has_errors, validate},
    };

    fn draft() -> Draft {
        Draft {
            name: "code-review".into(),
            description: "Review code for bugs.".into(),
            when_to_use: "Use for PR review.".into(),
            steps: "1. Read the diff.\n2. Report.".into(),
            extra_dirs: vec!["scripts".into(), "bogus".into()],
            manual_only: true,
            ..Default::default()
        }
    }

    #[test]
    fn slug_rules() {
        assert_eq!(slug("My Cool Skill!"), "my-cool-skill");
        assert_eq!(slug("  --a__b "), "a-b");
        assert_eq!(slug("!!!"), "");
        assert!(validator::is_valid_name(&slug("Hello World 2")));
    }

    #[test]
    fn generated_skill_validates() {
        let d = draft();
        let text = render_skill_md(&d);
        let (m, b, fm) = parse(&text);
        assert!(m.disable_model_invocation);
        assert!(b.contains("## When to use") && b.contains("1. Read the diff."));
        assert!(!has_errors(&validate(
            &m,
            &fm,
            &b,
            Some("code-review"),
            false
        )));
        let f = files_for(&d);
        assert!(f.contains_key("scripts/README.md") && !f.contains_key("bogus/README.md"));
    }

    #[test]
    fn plan_create_checks_target() {
        let t = tempfile::tempdir().unwrap();
        let p = plan_create(t.path(), "code-review", &draft()).unwrap();
        assert_eq!(p.counts().added, 2);
        p.apply().unwrap();
        assert!(t.path().join("code-review/SKILL.md").exists());
        assert!(plan_create(t.path(), "code-review", &draft()).is_err());
        assert!(plan_create(t.path(), "../x", &draft()).is_err());
    }
}
