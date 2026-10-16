//! Turns failure signals into recovery jobs and jobs into wake actions. The shell calls
//! `observe_failure` when an agent run ends badly, `note_success` when one ends well, and
//! `due_actions` on its tick; for each action it wakes the member with `action.prompt` and
//! then calls `mark_dispatched(job_id)`.

use std::path::Path;

use crate::config::Recovery;

use super::classify::{classify, Failure};
use super::policy::{decide, Decision, Policy, JOB_TTL_MS};
use super::store::{self, Job, JobState, Jobs};

/// What a failed run looks like to the recovery code.
#[derive(Debug, Clone, Default)]
pub struct FailureSignal {
    pub member: String,
    pub observed_at: i64,
    /// Last error line / output tail.
    pub detail: String,
    pub status_code: Option<u16>,
    pub exit_code: Option<i32>,
    /// `AgentView.rate_limited_until`.
    pub rate_limited_until: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryAction {
    pub job_id: String,
    pub member: String,
    pub at_ms: i64,
    /// `retry_transient` | `retry_at_reset`.
    pub reason: String,
    pub prompt: String,
}

fn prompt_for(reason: &str) -> String {
    if reason == "retry_at_reset" {
        "Your rate limit has reset. Please resume the work you were doing before it was interrupted.".into()
    } else {
        "A temporary error interrupted your last run. Please resume the work where you left off."
            .into()
    }
}

fn open_job<'a>(jobs: &'a mut Jobs, member: &str) -> Option<&'a mut Job> {
    jobs.jobs
        .iter_mut()
        .find(|j| j.member == member && j.is_open())
}

/// Record a failure. Returns the job created or updated (`None` if the text is no failure).
pub fn observe_failure(
    home: &Path,
    team: &str,
    sig: &FailureSignal,
    cfg: &Recovery,
) -> Option<Job> {
    let failure = classify(
        &sig.detail,
        sig.rate_limited_until,
        sig.status_code,
        sig.exit_code,
        sig.observed_at,
    )?;
    let mut jobs = store::load(home, team);
    let job = apply(
        &mut jobs,
        &sig.member,
        sig.observed_at,
        &failure,
        &Policy::from_config(cfg),
    );
    let _ = store::save(home, team, &jobs);
    Some(job)
}

/// Pure core of `observe_failure`.
pub fn apply(
    jobs: &mut Jobs,
    member: &str,
    now_ms: i64,
    failure: &Failure,
    policy: &Policy,
) -> Job {
    let prior = open_job(jobs, member).map(|j| {
        j.state = JobState::Cancelled;
        j.attempt
    });
    // A failure after a dispatched retry means that retry was used up.
    let attempt = prior.unwrap_or(0);
    let id = format!("{member}-{now_ms}");
    let decision = decide(policy, failure, attempt, &id, now_ms);
    let (state, reason, next_at) = match decision {
        Decision::Retry { at_ms, reason } => (JobState::Pending, reason.to_string(), at_ms),
        Decision::Manual(why) => (JobState::Manual, why, now_ms),
    };
    let expires_at = next_at.max(now_ms) + JOB_TTL_MS;
    let job = Job {
        id,
        member: member.to_string(),
        reason,
        attempt,
        next_at,
        expires_at,
        state,
        class: Some(failure.class),
        detail: failure.detail.clone(),
    };
    jobs.jobs.push(job.clone());
    jobs.jobs
        .retain(|j| j.is_open() || j.expires_at > now_ms - JOB_TTL_MS);
    job
}

fn expire(jobs: &mut Jobs, now_ms: i64) {
    for j in jobs
        .jobs
        .iter_mut()
        .filter(|j| j.is_open() && j.expires_at <= now_ms)
    {
        j.state = JobState::Cancelled;
    }
}

/// Open jobs sorted by time: what recovery will do next, including future ones.
pub fn next_actions(home: &Path, team: &str, now_ms: i64) -> Vec<RecoveryAction> {
    let mut jobs = store::load(home, team);
    expire(&mut jobs, now_ms);
    let mut v: Vec<RecoveryAction> = jobs
        .jobs
        .iter()
        .filter(|j| j.state == JobState::Pending)
        .map(|j| RecoveryAction {
            job_id: j.id.clone(),
            member: j.member.clone(),
            at_ms: j.next_at,
            reason: j.reason.clone(),
            prompt: prompt_for(&j.reason),
        })
        .collect();
    v.sort_by_key(|a| a.at_ms);
    v
}

/// Actions whose time has come.
pub fn due_actions(home: &Path, team: &str, now_ms: i64) -> Vec<RecoveryAction> {
    next_actions(home, team, now_ms)
        .into_iter()
        .filter(|a| a.at_ms <= now_ms)
        .collect()
}

fn update(home: &Path, team: &str, f: impl FnOnce(&mut Jobs)) {
    let mut jobs = store::load(home, team);
    f(&mut jobs);
    let _ = store::save(home, team, &jobs);
}

/// The wake order for `job_id` went out; the next failure counts as the following attempt.
pub fn mark_dispatched(home: &Path, team: &str, job_id: &str) {
    update(home, team, |jobs| {
        if let Some(j) = jobs.jobs.iter_mut().find(|j| j.id == job_id) {
            j.state = JobState::Dispatched;
            j.attempt += 1;
        }
    });
}

/// The member finished a run successfully: its open jobs are recovered.
pub fn note_success(home: &Path, team: &str, member: &str) {
    update(home, team, |jobs| {
        for j in jobs
            .jobs
            .iter_mut()
            .filter(|j| j.member == member && j.is_open())
        {
            j.state = JobState::Recovered;
        }
    });
}

/// The user stopped the member: drop its open jobs.
pub fn cancel(home: &Path, team: &str, member: &str) {
    update(home, team, |jobs| {
        for j in jobs
            .jobs
            .iter_mut()
            .filter(|j| j.member == member && j.is_open())
        {
            j.state = JobState::Cancelled;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(detail: &str, at: i64) -> FailureSignal {
        FailureSignal {
            member: "bob".into(),
            observed_at: at,
            detail: detail.into(),
            ..Default::default()
        }
    }

    fn cfg() -> Recovery {
        Recovery {
            delay_s: 60,
            attempts: 2,
            ..Recovery::default()
        }
    }

    #[test]
    fn transient_flow_retries_then_gives_up() {
        let d = tempfile::tempdir().unwrap();
        let (h, t) = (d.path(), "t");
        let j = observe_failure(h, t, &sig("503 service unavailable", 1_000), &cfg()).unwrap();
        assert_eq!(j.state, JobState::Pending);
        assert!(next_actions(h, t, 1_000)[0].at_ms > 1_000);
        assert!(due_actions(h, t, 1_000).is_empty());
        let at = next_actions(h, t, 1_000)[0].at_ms;
        let due = due_actions(h, t, at);
        assert_eq!(due.len(), 1);
        mark_dispatched(h, t, &due[0].job_id);
        assert!(due_actions(h, t, at + 1).is_empty());
        // Fails again -> attempt 1 -> retried; again -> attempt 2 -> manual.
        let j2 = observe_failure(h, t, &sig("HTTP 503", at + 10), &cfg()).unwrap();
        assert_eq!((j2.state, j2.attempt), (JobState::Pending, 1));
        mark_dispatched(h, t, &j2.id);
        let j3 = observe_failure(h, t, &sig("HTTP 503", at + 20), &cfg()).unwrap();
        assert_eq!(j3.state, JobState::Manual);
        assert!(next_actions(h, t, at + 21).is_empty());
    }

    #[test]
    fn rate_limit_waits_and_success_cancels() {
        let d = tempfile::tempdir().unwrap();
        let (h, t) = (d.path(), "t");
        let mut s = sig("usage limit reached", 0);
        s.rate_limited_until = Some(600_000);
        observe_failure(h, t, &s, &cfg()).unwrap();
        let a = &next_actions(h, t, 0)[0];
        assert_eq!((a.at_ms, a.reason.as_str()), (600_000, "retry_at_reset"));
        assert!(a.prompt.starts_with("Your rate limit has reset"));
        note_success(h, t, "bob");
        assert!(next_actions(h, t, 0).is_empty());
    }

    #[test]
    fn auth_is_manual_and_jobs_expire() {
        let d = tempfile::tempdir().unwrap();
        let (h, t) = (d.path(), "t");
        assert_eq!(
            observe_failure(h, t, &sig("invalid_grant", 0), &cfg())
                .unwrap()
                .state,
            JobState::Manual
        );
        assert!(observe_failure(h, t, &sig("all fine", 0), &cfg()).is_none());
        observe_failure(h, t, &sig("econnreset", 0), &cfg()).unwrap();
        assert_eq!(next_actions(h, t, 0).len(), 1);
        assert!(next_actions(h, t, JOB_TTL_MS + 10 * 60_000).is_empty());
    }

    #[test]
    fn cancel_drops_open_jobs() {
        let d = tempfile::tempdir().unwrap();
        observe_failure(d.path(), "t", &sig("econnreset", 0), &cfg()).unwrap();
        cancel(d.path(), "t", "bob");
        assert!(next_actions(d.path(), "t", 0).is_empty());
    }
}
