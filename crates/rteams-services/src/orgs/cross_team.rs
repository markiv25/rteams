//! Cross-team messaging helpers. A message from team A's member lands in team B's log with
//! `from_team = A` (the `message_send` tool's existing convention); its display sender is the
//! prefixed `<team>/<member>`. Sending itself goes through `GuiCmd::CrossTeamMessage`.

use std::collections::BTreeMap;
use std::path::Path;

use rteams_core::{Event, EventKind};

/// Every `(team, member)` that can receive a cross-team message: active members of every
/// team under `home`, leads first within each team.
pub fn targets(home: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for team in super::projection::team_dirs(home) {
        let Some((state, _)) = super::projection::read_log(home, &team) else {
            continue;
        };
        let Some(t) = state.team.filter(|t| !t.deleted) else {
            continue;
        };
        let mut members: Vec<_> = t.members.iter().filter(|m| !m.removed).collect();
        members.sort_by_key(|m| m.role != "lead");
        out.extend(members.into_iter().map(|m| (team.clone(), m.name.clone())));
    }
    out
}

/// `<team>/<member>`.
pub fn sender_label(team: &str, member: &str) -> String {
    format!("{team}/{member}")
}

/// Split a `<team>/<member>` label.
pub fn parse_sender(s: &str) -> Option<(&str, &str)> {
    let (t, m) = s.split_once('/')?;
    (!t.is_empty() && !m.is_empty() && !m.contains('/')).then_some((t, m))
}

/// A parsed `@team/member` address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub team: String,
    pub member: String,
    /// The message with the address and its trailing whitespace removed.
    pub text: String,
}

fn name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '.')
}

/// Parse a leading `@team/member <text>` address. `None` when the text does not start with
/// one (a plain `@member` is a same-team mention, not a route).
pub fn route(text: &str) -> Option<Route> {
    let rest = text.trim_start().strip_prefix('@')?;
    let t_end = rest.find(|c: char| !name_char(c))?;
    let team = &rest[..t_end];
    let rest = rest[t_end..].strip_prefix('/')?;
    let m_end = rest.find(|c: char| !name_char(c)).unwrap_or(rest.len());
    let member = &rest[..m_end];
    if team.is_empty() || member.is_empty() {
        return None;
    }
    let tail = &rest[m_end..];
    if !tail.is_empty() && !tail.starts_with(char::is_whitespace) {
        return None;
    }
    Some(Route {
        team: team.into(),
        member: member.into(),
        text: tail.trim().to_string(),
    })
}

/// Sender team of a cross-team message event, if it is one.
fn cross_sender(e: &Event) -> Option<String> {
    let EventKind::MessageSent { from_team, .. } = &e.kind else {
        return None;
    };
    from_team
        .clone()
        .or_else(|| parse_sender(&e.actor).map(|(t, _)| t.to_string()))
}

/// Cross-team messages received, counted as `(from team, to team) -> n`, scanning every
/// team's log under `home`.
pub fn message_counts(home: &Path) -> BTreeMap<(String, String), usize> {
    let mut out = BTreeMap::new();
    for to in super::projection::team_dirs(home) {
        let Some((_, events)) = super::projection::read_log(home, &to) else {
            continue;
        };
        for e in &events {
            if let Some(from) = cross_sender(e).filter(|f| *f != to) {
                *out.entry((from, to.clone())).or_insert(0) += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_parsing() {
        let r = route("@ops/boss please deploy").unwrap();
        assert_eq!(
            (r.team.as_str(), r.member.as_str(), r.text.as_str()),
            ("ops", "boss", "please deploy")
        );
        assert_eq!(route("  @a-b/c_d").unwrap().text, "");
        assert_eq!(route("@a/b\nmulti\nline").unwrap().text, "multi\nline");
        assert!(route("@ada hello").is_none());
        assert!(route("hello @a/b").is_none());
        assert!(route("@/b hi").is_none());
        assert!(route("@a/ hi").is_none());
        assert!(route("@a/b,hi").is_none());
        assert!(route("").is_none());
    }

    #[test]
    fn sender_labels() {
        assert_eq!(sender_label("a", "b"), "a/b");
        assert_eq!(parse_sender("a/b"), Some(("a", "b")));
        assert_eq!(parse_sender("ada"), None);
        assert_eq!(parse_sender("a/b/c"), None);
    }
}
