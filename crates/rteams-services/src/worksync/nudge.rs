//! Tick entry point: which members should get a work-sync wake right now.

use std::path::{Path, PathBuf};

use rteams_core::State;

use super::agenda::{build_agenda, Agenda};
use super::reconcile::{decide, mark_nudged, observe, Decision};
use super::store::{self, MemberSync};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nudge {
    pub member: String,
    /// Short wake reason for `WakeOrder::reasons` (the shell builds the prompt with
    /// `wake::build_prompt`, which already lists the actionable tasks).
    pub reason: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone)]
pub struct WorkSync {
    home: PathBuf,
    team: String,
}

fn reason(a: &Agenda) -> String {
    let mut s = format!("work-sync: {} actionable", a.items.len());
    for i in a.items.iter().take(3) {
        s.push_str(&format!(" #{} {}", i.task, i.reason));
    }
    s
}

impl WorkSync {
    pub fn new(home: &Path, team: &str) -> Self {
        WorkSync {
            home: home.to_path_buf(),
            team: team.to_string(),
        }
    }

    pub fn load(&self, member: &str) -> MemberSync {
        store::load(&self.home, &self.team, member)
    }

    /// Call every ~60 s and when events arrive. `eligible` = members that can be launched as
    /// agents; `running` = those currently running. Returns the members to wake (usually
    /// none); records the nudge before returning so a crash cannot duplicate it.
    pub fn nudges(
        &self,
        state: &State,
        eligible: &[String],
        running: &[String],
        now: i64,
    ) -> Vec<Nudge> {
        let mut out = vec![];
        for m in eligible {
            let agenda = build_agenda(state, m);
            let is_running = running.iter().any(|r| r == m);
            let mut rec = self.load(m);
            // Skip the disk write for members with nothing to do and nothing recorded.
            let mut dirty = observe(&mut rec, &agenda, is_running, now);
            if decide(&rec, &agenda, is_running, now) == Decision::Nudge {
                mark_nudged(&mut rec, now);
                dirty = true;
                out.push(Nudge {
                    member: m.clone(),
                    reason: reason(&agenda),
                    fingerprint: agenda.fingerprint.clone(),
                });
            }
            if dirty {
                let _ = store::save(&self.home, &self.team, m, &rec);
            }
        }
        out
    }

    /// `Stop auto-resume` / `Resume auto-resume`.
    pub fn set_auto_resume_stopped(&self, member: &str, stopped: bool) {
        let mut rec = self.load(member);
        rec.auto_resume_stopped = stopped;
        let _ = store::save(&self.home, &self.team, member, &rec);
    }

    /// `Continue`: forget the attempts and rate window so the next tick may nudge now (still
    /// only if the member has work and is idle).
    pub fn continue_now(&self, member: &str) {
        let mut rec = self.load(member);
        rec.attempts = 0;
        rec.last_nudge_at = None;
        rec.last_running_at = None;
        rec.first_seen_at = i64::MIN / 2;
        rec.auto_resume_stopped = false;
        let _ = store::save(&self.home, &self.team, member, &rec);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worksync::reconcile::{GRACE_MS, MIN_GAP_MS};
    use rteams_core::{Command, MemStore};

    fn state() -> State {
        let mut s = MemStore::new();
        s.execute("boss", 1, Command::CreateTeam { name: "t".into() })
            .unwrap();
        s.execute(
            "boss",
            2,
            Command::AddMember {
                name: "ada".into(),
                role: "dev".into(),
                runtime: None,
            },
        )
        .unwrap();
        s.execute(
            "boss",
            3,
            Command::CreateTask {
                id: "1".into(),
                subject: "s".into(),
                description: String::new(),
                owner: Some("ada".into()),
                blocked_by: vec![],
            },
        )
        .unwrap();
        s.state().clone()
    }

    #[test]
    fn rare_and_deduplicated() {
        let d = tempfile::tempdir().unwrap();
        let ws = WorkSync::new(d.path(), "t");
        let st = state();
        let el = vec!["ada".to_string()];
        let t0 = 5_000_000;
        assert!(ws.nudges(&st, &el, &[], t0).is_empty());
        assert!(ws.nudges(&st, &el, &[], t0 + GRACE_MS - 1).is_empty());
        let n = ws.nudges(&st, &el, &[], t0 + GRACE_MS);
        assert_eq!(n.len(), 1);
        assert!(n[0].reason.starts_with("work-sync: 1 actionable"));
        // immediately again: nothing (also across a fresh WorkSync = restart)
        let ws2 = WorkSync::new(d.path(), "t");
        assert!(ws2.nudges(&st, &el, &[], t0 + GRACE_MS + 1000).is_empty());
        assert!(ws2
            .nudges(&st, &el, &[], t0 + GRACE_MS + MIN_GAP_MS / 2)
            .is_empty());
        // running member is never nudged
        assert!(ws2
            .nudges(&st, &el, &el, t0 + GRACE_MS + 10 * MIN_GAP_MS)
            .is_empty());
    }

    #[test]
    fn stop_and_continue() {
        let d = tempfile::tempdir().unwrap();
        let ws = WorkSync::new(d.path(), "t");
        let st = state();
        let el = vec!["ada".to_string()];
        ws.nudges(&st, &el, &[], 1_000);
        ws.set_auto_resume_stopped("ada", true);
        assert!(ws.nudges(&st, &el, &[], 1_000 + 10 * GRACE_MS).is_empty());
        ws.continue_now("ada");
        assert_eq!(ws.nudges(&st, &el, &[], 2_000).len(), 1);
    }

    #[test]
    fn lead_like_and_idle_members_untouched() {
        let d = tempfile::tempdir().unwrap();
        let ws = WorkSync::new(d.path(), "t");
        let st = state();
        let el = vec!["team-lead".to_string(), "nobody".to_string()];
        assert!(ws.nudges(&st, &el, &[], 1).is_empty());
        assert!(ws.nudges(&st, &el, &[], 1 << 40).is_empty());
        assert!(
            !store::dir(d.path(), "t").exists(),
            "no files for members without work"
        );
    }
}
