//! A member's actionable work, derived from `State` (no tokens): the core briefing's actionable
//! items. Lead-like members and removed members have no agenda.

use rteams_core::projection::briefing;
use rteams_core::{State, Status};

use super::fingerprint::fingerprint;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgendaItem {
    pub task: String,
    pub subject: String,
    pub status: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agenda {
    pub member: String,
    pub items: Vec<AgendaItem>,
    /// Empty string when there is nothing to do.
    pub fingerprint: String,
}

pub fn is_lead_like(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "team-lead" | "lead" | "orchestrator"
    )
}

fn status_str(s: Status) -> String {
    serde_json::to_value(s)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

pub fn build_agenda(state: &State, member: &str) -> Agenda {
    let removed = state
        .team
        .as_ref()
        .is_some_and(|t| t.members.iter().any(|m| m.name == member && m.removed));
    let items: Vec<AgendaItem> = if is_lead_like(member) || removed {
        vec![]
    } else {
        briefing(state, member)
            .actionable
            .into_iter()
            .map(|i| AgendaItem {
                task: i.task,
                subject: i.subject,
                status: status_str(i.status),
                reason: serde_json::to_value(i.reason)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default(),
            })
            .collect()
    };
    let fingerprint = fingerprint(member, &items);
    Agenda {
        member: member.to_string(),
        items,
        fingerprint,
    }
}
