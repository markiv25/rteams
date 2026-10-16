//! Read-only view of work-sync state for the panel (never mutates or wakes anything).

use rteams_core::State;

use super::agenda::{build_agenda, AgendaItem};
use super::reconcile::{decide, observe, Decision};
use super::store;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberDiagnostic {
    pub member: String,
    pub items: Vec<AgendaItem>,
    pub fingerprint: String,
    pub decision: Decision,
    pub attempts: u32,
    pub last_nudge_at: Option<i64>,
    pub auto_resume_stopped: bool,
}

impl MemberDiagnostic {
    /// Human line, e.g. `Diagnostics: waiting`.
    pub fn status_text(&self) -> &'static str {
        match self.decision {
            Decision::Idle => "no actionable work",
            Decision::Stopped => "auto-resume stopped",
            Decision::Running => "running",
            Decision::Settled => "up to date (ran since last change)",
            Decision::Wait { .. } => "waiting",
            Decision::Exhausted => "nudge limit reached",
            Decision::Nudge => "would nudge",
        }
    }
}

pub fn diagnose(
    home: &std::path::Path,
    team: &str,
    state: &State,
    members: &[String],
    running: &[String],
    now: i64,
) -> Vec<MemberDiagnostic> {
    members
        .iter()
        .map(|m| {
            let agenda = build_agenda(state, m);
            let is_running = running.iter().any(|r| r == m);
            let mut rec = store::load(home, team, m);
            observe(&mut rec, &agenda, is_running, now);
            MemberDiagnostic {
                member: m.clone(),
                decision: decide(&rec, &agenda, is_running, now),
                fingerprint: agenda.fingerprint,
                items: agenda.items,
                attempts: rec.attempts,
                last_nudge_at: rec.last_nudge_at,
                auto_resume_stopped: rec.auto_resume_stopped,
            }
        })
        .collect()
}
