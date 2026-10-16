//! Validation rules for a skill (name format, description length, required fields, scripts).

use super::metadata::{Frontmatter, SkillMeta};

pub const MAX_NAME: usize = 64;
pub const MAX_DESCRIPTION: usize = 1024;
pub const MAX_COMPATIBILITY: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub severity: Severity,
    /// Stable code, e.g. `name.missing`.
    pub code: &'static str,
    pub message: String,
}

fn err(code: &'static str, message: impl Into<String>) -> Issue {
    Issue {
        severity: Severity::Error,
        code,
        message: message.into(),
    }
}
fn warn(code: &'static str, message: impl Into<String>) -> Issue {
    Issue {
        severity: Severity::Warning,
        code,
        message: message.into(),
    }
}

/// Kebab-case: lowercase letters/digits separated by single dashes.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Folder names: letters, digits, dots, dashes and underscores, not starting with a dot.
pub fn is_valid_folder_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && !s.starts_with('.')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// Validate parsed metadata. `folder` is the on-disk folder name when known.
pub fn validate(
    meta: &SkillMeta,
    fm: &Frontmatter,
    body: &str,
    folder: Option<&str>,
    has_scripts: bool,
) -> Vec<Issue> {
    let mut v = vec![];
    if !fm.present {
        v.push(err(
            "frontmatter.missing",
            "SKILL.md needs a frontmatter block with name and description.",
        ));
    } else if fm.malformed {
        v.push(err(
            "frontmatter.malformed",
            "The frontmatter block could not be parsed.",
        ));
    }
    match meta.name.as_deref().map(str::trim) {
        None | Some("") => v.push(err("name.missing", "A skill name is required.")),
        Some(n) => {
            if n.len() > MAX_NAME {
                v.push(err(
                    "name.tooLong",
                    format!("The name must be at most {MAX_NAME} characters."),
                ));
            } else if !is_valid_name(n) {
                v.push(err(
                    "name.format",
                    "Use kebab-case letters, numbers, and dashes for the name.",
                ));
            }
            if let Some(f) = folder {
                if f != n {
                    v.push(warn(
                        "name.folderMismatch",
                        format!("The name \"{n}\" does not match the folder \"{f}\"."),
                    ));
                }
            }
        }
    }
    match meta.description.as_deref().map(str::trim) {
        None | Some("") => v.push(err(
            "description.missing",
            "A description is required so the runtime knows when to use this skill.",
        )),
        Some(d) => {
            if d.chars().count() > MAX_DESCRIPTION {
                v.push(err(
                    "description.tooLong",
                    format!("The description must be at most {MAX_DESCRIPTION} characters."),
                ));
            }
            if d.contains('<') && d.contains('>') {
                v.push(warn(
                    "description.markup",
                    "The description should be plain text without angle brackets.",
                ));
            }
        }
    }
    if let Some(c) = &meta.compatibility {
        if c.chars().count() > MAX_COMPATIBILITY {
            v.push(err(
                "compatibility.tooLong",
                format!("Compatibility must be at most {MAX_COMPATIBILITY} characters."),
            ));
        }
    }
    if body.trim().is_empty() {
        v.push(warn("body.empty", "The skill has no instructions yet."));
    }
    if has_scripts {
        v.push(warn(
            "scripts.bundled",
            "This skill includes bundled scripts.",
        ));
    }
    v
}

pub fn has_errors(issues: &[Issue]) -> bool {
    issues.iter().any(|i| i.severity == Severity::Error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ext_skills::metadata::parse;

    fn check(text: &str, folder: &str) -> Vec<&'static str> {
        let (m, b, fm) = parse(text);
        validate(&m, &fm, &b, Some(folder), false)
            .iter()
            .map(|i| i.code)
            .collect()
    }

    #[test]
    fn valid_skill_is_clean() {
        assert!(check(
            "---\nname: good-one\ndescription: Does a thing.\n---\nBody",
            "good-one"
        )
        .is_empty());
    }

    #[test]
    fn name_rules() {
        assert!(is_valid_name("a1-b2"));
        for bad in ["", "Bad", "a--b", "-a", "a-", "a_b", "a b"] {
            assert!(!is_valid_name(bad), "{bad}");
        }
        assert!(check("---\nname: Bad_Name\ndescription: x\n---\nb", "x").contains(&"name.format"));
        assert!(check("---\nname: other\ndescription: x\n---\nb", "folder")
            .contains(&"name.folderMismatch"));
        let long = "a".repeat(65);
        assert!(
            check(&format!("---\nname: {long}\ndescription: x\n---\nb"), "x")
                .contains(&"name.tooLong")
        );
    }

    #[test]
    fn required_fields_and_lengths() {
        let c = check("---\nname: a\n---\nb", "a");
        assert!(c.contains(&"description.missing"));
        let c = check("no frontmatter", "a");
        assert!(c.contains(&"frontmatter.missing") && c.contains(&"name.missing"));
        let d = "x".repeat(1025);
        assert!(
            check(&format!("---\nname: a\ndescription: {d}\n---\nb"), "a")
                .contains(&"description.tooLong")
        );
    }

    #[test]
    fn folder_names() {
        assert!(is_valid_folder_name("my.skill_1-x"));
        assert!(
            !is_valid_folder_name("../x")
                && !is_valid_folder_name(".hid")
                && !is_valid_folder_name("a b")
        );
    }

    #[test]
    fn scripts_warn() {
        let (m, b, fm) = parse("---\nname: a\ndescription: d\n---\nb");
        let v = validate(&m, &fm, &b, Some("a"), true);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].severity, Severity::Warning);
        assert!(!has_errors(&v));
    }
}
