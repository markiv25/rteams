//! Team prompt management: validated update requests, revision history and the injection
//! policy (how much prompt text goes into an agent's context).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rteams_core::{Event, EventKind};
use serde::{Deserialize, Serialize};

/// Max characters of a stored prompt.
pub const MAX_PROMPT_CHARS: usize = 200_000;
/// Revisions kept per prompt.
pub const MAX_REVISIONS: usize = 50;
/// Token-frugal injection budgets (characters) for what is actually put into an agent run.
pub const LEAD_INJECT_CHARS: usize = 4_000;
pub const MEMBER_INJECT_CHARS: usize = 2_000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PromptError {
    #[error("prompt is empty")]
    Empty,
    #[error("prompt is longer than {0} characters")]
    TooLong(usize),
    #[error("invalid member name `{0}`")]
    BadName(String),
    #[error("unsupported field `{0}`")]
    UnsupportedField(String),
    #[error("configuration changed (expected revision {expected}, now {actual})")]
    Stale { expected: u64, actual: u64 },
    #[error("prompt io: {0}")]
    Io(String),
}

/// Trim, require non-empty and bound the text.
pub fn validate_prompt(text: &str) -> Result<String, PromptError> {
    let t = text.trim();
    if t.is_empty() {
        return Err(PromptError::Empty);
    }
    if t.chars().count() > MAX_PROMPT_CHARS {
        return Err(PromptError::TooLong(MAX_PROMPT_CHARS));
    }
    Ok(t.to_string())
}

/// Member names: kebab/snake style letters, digits, `-`, `_`, up to 64 chars, starting with
/// a letter; a pure `-N` numeric suffix is reserved for auto-renaming.
pub fn validate_member_name(name: &str) -> Result<(), PromptError> {
    let bad = || PromptError::BadName(name.to_string());
    let mut chars = name.chars();
    let first = chars.next().ok_or_else(bad)?;
    if !first.is_ascii_alphabetic() || name.len() > 64 {
        return Err(bad());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(bad());
    }
    if let Some((_, suffix)) = name.rsplit_once('-') {
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
            return Err(bad());
        }
    }
    Ok(())
}

/// Reject request objects carrying fields other than `allowed`.
pub fn reject_unsupported(
    request: &serde_json::Value,
    allowed: &[&str],
) -> Result<(), PromptError> {
    if let Some(m) = request.as_object() {
        if let Some(k) = m.keys().find(|k| !allowed.contains(&k.as_str())) {
            return Err(PromptError::UnsupportedField(k.clone()));
        }
    }
    Ok(())
}

/// Optimistic-concurrency token: seq of the last `TeamConfigured` / `MemberUpdated` event.
pub fn configuration_revision(events: &[Event]) -> u64 {
    events
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                EventKind::TeamConfigured { .. } | EventKind::MemberUpdated { .. }
            )
        })
        .map(|e| e.seq)
        .max()
        .unwrap_or(0)
}

pub fn check_revision(events: &[Event], expected: u64) -> Result<(), PromptError> {
    let actual = configuration_revision(events);
    if actual == expected {
        Ok(())
    } else {
        Err(PromptError::Stale { expected, actual })
    }
}

/// What gets injected into a run: the trimmed prompt clipped to `max` chars (at a line
/// boundary when possible) with a marker. Short prompts pass through unchanged.
pub fn injected(text: &str, max: usize) -> String {
    let t = text.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let cut: String = t.chars().take(max).collect();
    let cut = match cut.rfind('\n') {
        Some(i) if i > max / 2 => cut[..i].to_string(),
        _ => cut,
    };
    format!(
        "{}\n[prompt truncated: {} of {} chars]",
        cut.trim_end(),
        cut.chars().count(),
        t.chars().count()
    )
}

// ---- revision history ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    pub rev: u32,
    pub at: i64,
    pub author: String,
    pub text: String,
}

/// Which prompt: the lead's or a member's workflow.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PromptKey {
    Lead,
    Member(String),
}

impl PromptKey {
    fn name(&self) -> String {
        match self {
            PromptKey::Lead => "@lead".into(),
            PromptKey::Member(m) => m.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HistoryFile {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    prompts: BTreeMap<String, Vec<Revision>>,
}

#[derive(Debug)]
pub struct PromptHistory {
    path: PathBuf,
    file: HistoryFile,
}

pub fn history_path(home: &Path, team: &str) -> PathBuf {
    home.join(team).join("prompts.json")
}

impl PromptHistory {
    pub fn open(home: &Path, team: &str) -> PromptHistory {
        let path = history_path(home, team);
        let file = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        PromptHistory { path, file }
    }

    pub fn revisions(&self, key: &PromptKey) -> &[Revision] {
        self.file
            .prompts
            .get(&key.name())
            .map_or(&[], |v| v.as_slice())
    }

    pub fn head(&self, key: &PromptKey) -> Option<&Revision> {
        self.revisions(key).last()
    }

    /// Validate and record a new revision. An unchanged prompt adds nothing (returns the
    /// current head). Old revisions beyond `MAX_REVISIONS` are dropped.
    pub fn record(
        &mut self,
        key: &PromptKey,
        text: &str,
        author: &str,
        now: i64,
    ) -> Result<Revision, PromptError> {
        if let PromptKey::Member(m) = key {
            validate_member_name(m)?;
        }
        let text = validate_prompt(text)?;
        if let Some(h) = self.head(key) {
            if h.text == text {
                return Ok(h.clone());
            }
        }
        let v = self.file.prompts.entry(key.name()).or_default();
        let rev = Revision {
            rev: v.last().map_or(1, |r| r.rev + 1),
            at: now,
            author: author.to_string(),
            text,
        };
        v.push(rev.clone());
        if v.len() > MAX_REVISIONS {
            v.drain(..v.len() - MAX_REVISIONS);
        }
        self.save()?;
        Ok(rev)
    }

    /// Text of an older revision, to be re-applied through the normal update path.
    pub fn text_of(&self, key: &PromptKey, rev: u32) -> Option<&str> {
        self.revisions(key)
            .iter()
            .find(|r| r.rev == rev)
            .map(|r| r.text.as_str())
    }

    fn save(&mut self) -> Result<(), PromptError> {
        self.file.schema_version = 1;
        let b =
            serde_json::to_vec_pretty(&self.file).map_err(|e| PromptError::Io(e.to_string()))?;
        crate::atomic::write(&self.path, &b).map_err(|e| PromptError::Io(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validation_rules() {
        assert_eq!(validate_prompt("  hi "), Ok("hi".into()));
        assert_eq!(validate_prompt(" \n"), Err(PromptError::Empty));
        assert!(matches!(
            validate_prompt(&"x".repeat(MAX_PROMPT_CHARS + 1)),
            Err(PromptError::TooLong(_))
        ));
        assert!(validate_member_name("ada-dev").is_ok());
        assert!(validate_member_name("ada-2").is_err());
        assert!(validate_member_name("9ada").is_err());
        assert!(validate_member_name("a b").is_err());
        assert!(reject_unsupported(&json!({"prompt": "x", "evil": 1}), &["prompt"]).is_err());
        assert!(reject_unsupported(&json!({"prompt": "x"}), &["prompt"]).is_ok());
    }

    #[test]
    fn revision_is_last_config_event_seq() {
        let ev = |seq, kind| Event {
            seq,
            ts: 0,
            actor: "a".into(),
            kind,
        };
        let evs = vec![
            ev(
                1,
                EventKind::TeamConfigured {
                    project_path: None,
                    lead_prompt: Some("x".into()),
                },
            ),
            ev(2, EventKind::TeamCreated { name: "t".into() }),
        ];
        assert_eq!(configuration_revision(&evs), 1);
        assert!(check_revision(&evs, 1).is_ok());
        assert_eq!(
            check_revision(&evs, 0),
            Err(PromptError::Stale {
                expected: 0,
                actual: 1
            })
        );
    }

    #[test]
    fn injection_is_bounded() {
        assert_eq!(injected(" short ", 100), "short");
        let long = "line\n".repeat(1000);
        let out = injected(&long, 100);
        assert!(out.chars().count() < 160 && out.contains("[prompt truncated"));
    }

    #[test]
    fn history_dedups_caps_and_persists() {
        let d = tempfile::tempdir().unwrap();
        let mut h = PromptHistory::open(d.path(), "t");
        let k = PromptKey::Member("ada".into());
        let r1 = h.record(&k, "v1", "me", 1).unwrap();
        let r1b = h.record(&k, " v1 ", "me", 2).unwrap();
        assert_eq!(r1, r1b);
        h.record(&k, "v2", "me", 3).unwrap();
        for i in 0..60 {
            h.record(&PromptKey::Lead, &format!("l{i}"), "me", i)
                .unwrap();
        }
        let h2 = PromptHistory::open(d.path(), "t");
        assert_eq!(h2.head(&k).unwrap().rev, 2);
        assert_eq!(h2.text_of(&k, 1), Some("v1"));
        assert_eq!(h2.revisions(&PromptKey::Lead).len(), MAX_REVISIONS);
        assert!(h
            .record(&PromptKey::Member("bad name".into()), "x", "me", 1)
            .is_err());
    }
}
