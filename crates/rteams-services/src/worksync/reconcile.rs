//! Pure decision: should this member be nudged right now? Token-frugal by construction.
//!
//! A nudge (one agent run) happens only when ALL hold:
//! * the member has actionable work (non-empty agenda),
//! * the agenda fingerprint changed and has stayed unhandled for `GRACE_MS` (normal event
//!   wakes get the first chance),
//! * the member is idle and was not seen running since the change (a run after the change
//!   already received the agenda in its prompt; repeating it would burn tokens for nothing),
//! * no nudge in the last `MIN_GAP_MS` (1 per hour) and fewer than `MAX_ATTEMPTS` nudges for
//!   this fingerprint,
//! * auto-resume was not stopped by the user.

use super::agenda::Agenda;
use super::store::MemberSync;

pub const GRACE_MS: i64 = 10 * 60_000;
pub const MIN_GAP_MS: i64 = 60 * 60_000;
pub const MAX_ATTEMPTS: u32 = 3;
/// A run that ended this shortly before the change is treated as having caused it.
pub const SETTLE_MS: i64 = 2 * 60_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// No actionable work.
    Idle,
    /// Auto-resume is stopped.
    Stopped,
    /// The member is running.
    Running,
    /// The member ran since the agenda changed; nothing to add.
    Settled,
    /// Not before this time (grace or rate window).
    Wait {
        until: i64,
    },
    /// Gave up on this fingerprint.
    Exhausted,
    Nudge,
}

/// Fold the observation into the record (fingerprint change / running tracking). Returns true
/// if the record changed.
pub fn observe(rec: &mut MemberSync, agenda: &Agenda, running: bool, now: i64) -> bool {
    let before = rec.clone();
    if rec.fingerprint != agenda.fingerprint {
        rec.fingerprint = agenda.fingerprint.clone();
        rec.first_seen_at = now;
        rec.attempts = 0;
    }
    if running {
        rec.last_running_at = Some(now);
    }
    *rec != before
}

/// Decide from an already-observed record. Does not mutate.
pub fn decide(rec: &MemberSync, agenda: &Agenda, running: bool, now: i64) -> Decision {
    if agenda.items.is_empty() {
        return Decision::Idle;
    }
    if rec.auto_resume_stopped {
        return Decision::Stopped;
    }
    if running {
        return Decision::Running;
    }
    if rec
        .last_running_at
        .is_some_and(|r| r + SETTLE_MS >= rec.first_seen_at)
    {
        return Decision::Settled;
    }
    if rec.attempts >= MAX_ATTEMPTS {
        return Decision::Exhausted;
    }
    let mut until = rec.first_seen_at + GRACE_MS;
    if let Some(last) = rec.last_nudge_at {
        until = until.max(last + MIN_GAP_MS);
    }
    if now < until {
        return Decision::Wait { until };
    }
    Decision::Nudge
}

/// Record that a nudge was sent.
pub fn mark_nudged(rec: &mut MemberSync, now: i64) {
    rec.attempts += 1;
    rec.last_nudge_at = Some(now);
    rec.last_nudged_fingerprint = rec.fingerprint.clone();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worksync::agenda::AgendaItem;

    fn agenda(fp: &str) -> Agenda {
        Agenda {
            member: "ada".into(),
            items: if fp.is_empty() {
                vec![]
            } else {
                vec![AgendaItem {
                    task: "1".into(),
                    subject: "s".into(),
                    status: "pending".into(),
                    reason: "start".into(),
                }]
            },
            fingerprint: fp.into(),
        }
    }

    #[test]
    fn grace_then_nudge_then_rate_limit_then_exhausted() {
        let mut rec = MemberSync::default();
        let a = agenda("f1");
        let t0 = 1_000_000;
        observe(&mut rec, &a, false, t0);
        assert_eq!(
            decide(&rec, &a, false, t0),
            Decision::Wait {
                until: t0 + GRACE_MS
            }
        );
        assert_eq!(decide(&rec, &a, false, t0 + GRACE_MS), Decision::Nudge);
        mark_nudged(&mut rec, t0 + GRACE_MS);
        let t1 = t0 + GRACE_MS + 30 * 60_000;
        assert!(matches!(decide(&rec, &a, false, t1), Decision::Wait { .. }));
        let t2 = t0 + GRACE_MS + MIN_GAP_MS;
        assert_eq!(decide(&rec, &a, false, t2), Decision::Nudge);
        mark_nudged(&mut rec, t2);
        let t3 = t2 + MIN_GAP_MS;
        mark_nudged(&mut rec, t3);
        assert_eq!(
            decide(&rec, &a, false, t3 + 10 * MIN_GAP_MS),
            Decision::Exhausted
        );
    }

    #[test]
    fn same_fingerprint_does_not_reset_but_change_does() {
        let mut rec = MemberSync::default();
        observe(&mut rec, &agenda("f1"), false, 0);
        mark_nudged(&mut rec, GRACE_MS);
        assert!(!observe(&mut rec, &agenda("f1"), false, GRACE_MS + 1));
        assert_eq!(rec.attempts, 1);
        observe(&mut rec, &agenda("f2"), false, GRACE_MS + 5);
        assert_eq!(rec.attempts, 0);
        // rate window still applies across fingerprints
        assert!(matches!(
            decide(&rec, &agenda("f2"), false, GRACE_MS + 5 + GRACE_MS),
            Decision::Wait { .. }
        ));
    }

    #[test]
    fn empty_running_stopped_and_settled_never_nudge() {
        let mut rec = MemberSync::default();
        assert_eq!(decide(&rec, &agenda(""), false, 1 << 40), Decision::Idle);
        let a = agenda("f1");
        observe(&mut rec, &a, true, 0);
        assert_eq!(decide(&rec, &a, true, 1 << 40), Decision::Running);
        // ran right when the fingerprint appeared -> settled even much later while idle
        assert_eq!(decide(&rec, &a, false, 1 << 40), Decision::Settled);
        rec.auto_resume_stopped = true;
        assert_eq!(decide(&rec, &a, false, 1 << 40), Decision::Stopped);
    }

    #[test]
    fn change_long_after_a_run_can_nudge() {
        let mut rec = MemberSync {
            last_running_at: Some(0),
            ..Default::default()
        };
        let a = agenda("f9");
        observe(&mut rec, &a, false, 3_600_000);
        assert_eq!(
            decide(&rec, &a, false, 3_600_000 + GRACE_MS),
            Decision::Nudge
        );
    }
}
