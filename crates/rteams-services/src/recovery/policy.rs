//! Retry policy per failure class, driven by `AppConfig.recovery`.

use crate::config::Recovery;

use super::classify::{Failure, FailureClass};

/// Job lifetime from creation.
pub const JOB_TTL_MS: i64 = 2 * 3_600_000;
/// Longest backoff for transient retries.
pub const MAX_DELAY_MS: i64 = 15 * 60_000;
/// Rate limits further out than this are left to the user.
pub const RATE_HORIZON_MS: i64 = 12 * 3_600_000;
pub const RATE_BUFFER_MS: i64 = 30_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub transient: bool,
    pub rate_limits: bool,
    pub initial_delay_s: u64,
    pub max_attempts: u32,
}

impl Policy {
    pub fn from_config(r: &Recovery) -> Self {
        Policy {
            transient: r.transient,
            rate_limits: r.rate_limits,
            initial_delay_s: r.delay_s.clamp(15, 900),
            max_attempts: r.attempts.clamp(1, 5),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Retry at this unix-ms time; `reason` is `retry_transient` or `retry_at_reset`.
    Retry { at_ms: i64, reason: &'static str },
    /// Needs the user; the text says why.
    Manual(String),
}

/// FNV-1a 64-bit.
pub fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Stable multiplier in 0.8..=1.2 seeded by the job id.
pub fn jitter(job_id: &str) -> f64 {
    0.8 + (fnv1a(job_id) % 10_001) as f64 / 10_000.0 * 0.4
}

/// `initial * 2^attempt`, capped at 15 min, times the jitter (attempt is 0-based).
pub fn backoff_ms(initial_s: u64, attempt: u32, job_id: &str) -> i64 {
    let base = (initial_s as i64 * 1000).saturating_mul(1i64 << attempt.min(20));
    (base.min(MAX_DELAY_MS) as f64 * jitter(job_id)).round() as i64
}

/// Decide what to do about `failure` after `attempt` retries already happened.
pub fn decide(p: &Policy, failure: &Failure, attempt: u32, job_id: &str, now_ms: i64) -> Decision {
    if attempt >= p.max_attempts {
        return Decision::Manual("maximum recovery attempts reached".into());
    }
    match failure.class {
        FailureClass::Transient if p.transient => Decision::Retry {
            at_ms: now_ms + backoff_ms(p.initial_delay_s, attempt, job_id),
            reason: "retry_transient",
        },
        FailureClass::Transient => Decision::Manual("transient recovery is off".into()),
        FailureClass::RateLimit if !p.rate_limits => {
            Decision::Manual("rate limit recovery is off".into())
        }
        FailureClass::RateLimit => match failure.reset_at {
            Some(at) if at - now_ms <= RATE_HORIZON_MS => Decision::Retry {
                at_ms: at.max(now_ms + RATE_BUFFER_MS),
                reason: "retry_at_reset",
            },
            Some(_) => Decision::Manual("rate limit resets too far away".into()),
            None => Decision::Manual("rate limit without a reset time".into()),
        },
        FailureClass::Auth => Decision::Manual("authentication needs attention".into()),
        FailureClass::Client => {
            Decision::Manual("request was rejected; retrying cannot help".into())
        }
        FailureClass::Crash => Decision::Manual("the agent crashed".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fail(class: FailureClass, reset: Option<i64>) -> Failure {
        Failure {
            class,
            detail: String::new(),
            reset_at: reset,
            status_code: None,
        }
    }

    fn pol() -> Policy {
        Policy::from_config(&Recovery {
            delay_s: 60,
            attempts: 2,
            ..Recovery::default()
        })
    }

    #[test]
    fn backoff_doubles_with_cap_and_bounded_jitter() {
        let j = jitter("job-1");
        assert!((0.8..=1.2).contains(&j));
        assert_eq!(jitter("job-1"), j);
        let b0 = backoff_ms(60, 0, "job-1");
        let b1 = backoff_ms(60, 1, "job-1");
        assert!((b1 as f64 / b0 as f64 - 2.0).abs() < 0.01);
        assert!(backoff_ms(900, 5, "x") <= (MAX_DELAY_MS as f64 * 1.2) as i64);
        assert!((48_000..=72_000).contains(&b0));
    }

    #[test]
    fn transient_retries_until_attempts_run_out() {
        let p = pol();
        let f = fail(FailureClass::Transient, None);
        assert!(matches!(
            decide(&p, &f, 0, "j", 0),
            Decision::Retry {
                reason: "retry_transient",
                ..
            }
        ));
        assert!(matches!(decide(&p, &f, 1, "j", 0), Decision::Retry { .. }));
        assert!(matches!(decide(&p, &f, 2, "j", 0), Decision::Manual(_)));
    }

    #[test]
    fn rate_limit_waits_for_reset_within_horizon() {
        let p = pol();
        let d = decide(&p, &fail(FailureClass::RateLimit, Some(600_000)), 0, "j", 0);
        assert_eq!(
            d,
            Decision::Retry {
                at_ms: 600_000,
                reason: "retry_at_reset"
            }
        );
        assert!(matches!(
            decide(
                &p,
                &fail(FailureClass::RateLimit, Some(13 * 3_600_000)),
                0,
                "j",
                0
            ),
            Decision::Manual(_)
        ));
        assert!(matches!(
            decide(&p, &fail(FailureClass::RateLimit, None), 0, "j", 0),
            Decision::Manual(_)
        ));
    }

    #[test]
    fn toggles_and_terminal_classes() {
        let mut p = pol();
        p.transient = false;
        p.rate_limits = false;
        assert!(matches!(
            decide(&p, &fail(FailureClass::Transient, None), 0, "j", 0),
            Decision::Manual(_)
        ));
        assert!(matches!(
            decide(&p, &fail(FailureClass::RateLimit, Some(10)), 0, "j", 0),
            Decision::Manual(_)
        ));
        for c in [
            FailureClass::Auth,
            FailureClass::Client,
            FailureClass::Crash,
        ] {
            assert!(matches!(
                decide(&pol(), &fail(c, None), 0, "j", 0),
                Decision::Manual(_)
            ));
        }
    }

    #[test]
    fn config_is_clamped() {
        let p = Policy::from_config(&Recovery {
            delay_s: 1,
            attempts: 99,
            ..Recovery::default()
        });
        assert_eq!((p.initial_delay_s, p.max_attempts), (15, 5));
    }
}
