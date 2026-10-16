//! Pure scheduler: decides which schedules fire now. The shell calls `due` (or `evaluate`)
//! every ~15 s with the team's schedules and `history::last_scheduled`, turns each result into
//! a wake order for the member, and records a `history::RunRecord` for it (which moves the
//! "last run" forward so the same fire is never returned twice).

use std::collections::HashMap;

use rteams_core::Schedule;

use super::cron;

/// A missed cron fire is still run (once) when it is at most this old; older ones are
/// reported as interrupted.
pub const MISSED_GRACE_MS: i64 = 3_600_000;
/// With no history, cron fires only count when they fell within this window.
const FIRST_LOOKBACK_MS: i64 = 30_000;
const MAX_CATCHUP: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fire {
    pub schedule_id: String,
    pub member: String,
    pub prompt: String,
    /// The instant the schedule was meant to fire (unix ms).
    pub scheduled_for: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warmup {
    pub schedule_id: String,
    pub member: String,
    /// The fire this warm-up prepares for.
    pub fire_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Evaluation {
    pub fires: Vec<Fire>,
    /// Missed fires too old to run: `(schedule_id, scheduled_for)`.
    pub interrupted: Vec<(String, i64)>,
    pub warmups: Vec<Warmup>,
}

fn cron_of(s: &Schedule) -> Option<&str> {
    s.cron.as_deref().map(str::trim).filter(|c| !c.is_empty())
}

/// The next time `s` will fire after `now_ms` (unix ms); `None` if disabled or invalid.
pub fn next_run(s: &Schedule, last: Option<i64>, now_ms: i64) -> Option<i64> {
    if !s.enabled {
        return None;
    }
    if let Some(expr) = cron_of(s) {
        return cron::next_after(expr, s.timezone.as_deref(), now_ms);
    }
    if s.every_minutes == 0 {
        return None;
    }
    let next = last.map_or(now_ms, |l| l + i64::from(s.every_minutes) * 60_000);
    Some(next.max(now_ms))
}

/// Full evaluation: fires, too-old missed fires and warm-ups to start.
pub fn evaluate(
    schedules: &[Schedule],
    last_runs: &HashMap<String, i64>,
    now_ms: i64,
) -> Evaluation {
    let mut ev = Evaluation::default();
    for s in schedules.iter().filter(|s| s.enabled) {
        let last = last_runs.get(&s.id).copied();
        let mut fire = |at: i64| {
            ev.fires.push(Fire {
                schedule_id: s.id.clone(),
                member: s.member.clone(),
                prompt: s.prompt.clone(),
                scheduled_for: at,
            })
        };
        let upcoming;
        if let Some(expr) = cron_of(s) {
            let tz = s.timezone.as_deref();
            let anchor = last.unwrap_or(now_ms - FIRST_LOOKBACK_MS);
            let Ok(times) = cron::next_n(expr, tz, anchor, MAX_CATCHUP) else {
                continue;
            };
            let past: Vec<i64> = times.iter().copied().filter(|t| *t <= now_ms).collect();
            if let Some(&latest) = past.last() {
                for &t in &past[..past.len() - 1] {
                    ev.interrupted.push((s.id.clone(), t));
                }
                if now_ms - latest <= MISSED_GRACE_MS {
                    fire(latest);
                } else {
                    ev.interrupted.push((s.id.clone(), latest));
                }
            }
            upcoming = if past.len() == times.len() {
                cron::next_after(expr, tz, now_ms)
            } else {
                times.get(past.len()).copied()
            };
        } else if s.every_minutes > 0 {
            let next = last.map_or(now_ms, |l| l + i64::from(s.every_minutes) * 60_000);
            if next <= now_ms {
                fire(now_ms);
                upcoming = Some(now_ms + i64::from(s.every_minutes) * 60_000);
            } else {
                upcoming = Some(next);
            }
        } else {
            continue;
        }
        if let (Some(w), Some(at)) = (s.warmup_minutes.filter(|w| *w > 0), upcoming) {
            if at - i64::from(w) * 60_000 <= now_ms && now_ms < at {
                ev.warmups.push(Warmup {
                    schedule_id: s.id.clone(),
                    member: s.member.clone(),
                    fire_at: at,
                });
            }
        }
    }
    ev
}

/// `(schedule_id, member, prompt)` of every schedule that should fire now.
pub fn due(
    schedules: &[Schedule],
    last_runs: &HashMap<String, i64>,
    now_ms: i64,
) -> Vec<(String, String, String)> {
    evaluate(schedules, last_runs, now_ms)
        .fires
        .into_iter()
        .map(|f| (f.schedule_id, f.member, f.prompt))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn sched(id: &str, every: u32, cron: Option<&str>) -> Schedule {
        Schedule {
            id: id.into(),
            member: "bob".into(),
            prompt: "check".into(),
            every_minutes: every,
            enabled: true,
            cron: cron.map(String::from),
            timezone: Some("UTC".into()),
            warmup_minutes: None,
        }
    }

    fn utc(h: u32, m: u32) -> i64 {
        Utc.with_ymd_and_hms(2026, 5, 4, h, m, 0)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn interval_first_run_then_waits() {
        let s = [sched("a", 10, None)];
        let now = utc(8, 0);
        assert_eq!(due(&s, &HashMap::new(), now).len(), 1);
        let last = HashMap::from([("a".to_string(), now)]);
        assert!(due(&s, &last, now + 9 * 60_000).is_empty());
        assert_eq!(due(&s, &last, now + 10 * 60_000).len(), 1);
    }

    #[test]
    fn disabled_and_invalid_never_fire() {
        let mut off = sched("a", 1, None);
        off.enabled = false;
        let bad = sched("b", 0, Some("nope"));
        assert!(due(&[off, bad], &HashMap::new(), utc(8, 0)).is_empty());
    }

    #[test]
    fn cron_fires_once_per_instant() {
        let s = [sched("c", 0, Some("0 9 * * *"))];
        let last = HashMap::from([("c".to_string(), utc(9, 0) - 86_400_000)]);
        assert!(due(&s, &last, utc(8, 59)).is_empty());
        let d = evaluate(&s, &last, utc(9, 0) + 15_000);
        assert_eq!(d.fires.len(), 1);
        assert_eq!(d.fires[0].scheduled_for, utc(9, 0));
        // Once recorded, the same instant is not returned again.
        let last = HashMap::from([("c".to_string(), utc(9, 0))]);
        assert!(due(&s, &last, utc(9, 0) + 30_000).is_empty());
    }

    #[test]
    fn cron_without_history_does_not_catch_up() {
        let s = [sched("c", 0, Some("0 9 * * *"))];
        assert!(due(&s, &HashMap::new(), utc(12, 0)).is_empty());
        assert_eq!(due(&s, &HashMap::new(), utc(9, 0) + 10_000).len(), 1);
    }

    #[test]
    fn missed_fires_run_once_within_grace_else_interrupted() {
        let s = [sched("c", 0, Some("*/30 * * * *"))];
        let last = HashMap::from([("c".to_string(), utc(8, 0))]);
        let ev = evaluate(&s, &last, utc(9, 10));
        assert_eq!(ev.fires.len(), 1);
        assert_eq!(ev.fires[0].scheduled_for, utc(9, 0));
        assert_eq!(ev.interrupted, vec![(("c".to_string()), utc(8, 30))]);
        let ev = evaluate(&s, &last, utc(12, 0) + 1);
        assert_eq!(ev.fires[0].scheduled_for, utc(12, 0));
        assert!(!ev.interrupted.is_empty());
        let ev = evaluate(
            &[sched("c", 0, Some("0 9 * * *"))],
            &HashMap::from([("c".to_string(), utc(0, 0) - 86_400_000)]),
            utc(15, 0),
        );
        assert!(ev.fires.is_empty());
        assert_eq!(ev.interrupted.len(), 2);
    }

    #[test]
    fn warmup_window() {
        let mut s = sched("c", 0, Some("0 9 * * *"));
        s.warmup_minutes = Some(10);
        let last = HashMap::from([("c".to_string(), utc(9, 0) - 86_400_000)]);
        assert!(evaluate(&[s.clone()], &last, utc(8, 49)).warmups.is_empty());
        let w = evaluate(&[s.clone()], &last, utc(8, 51)).warmups;
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].fire_at, utc(9, 0));
        // Interval schedules warm up too.
        let mut i = sched("i", 30, None);
        i.warmup_minutes = Some(5);
        let last = HashMap::from([("i".to_string(), utc(8, 0))]);
        assert_eq!(evaluate(&[i], &last, utc(8, 26)).warmups.len(), 1);
    }

    #[test]
    fn next_run_values() {
        let c = sched("c", 0, Some("0 9 * * *"));
        assert_eq!(next_run(&c, None, utc(8, 0)), Some(utc(9, 0)));
        let i = sched("i", 15, None);
        assert_eq!(next_run(&i, Some(utc(8, 0)), utc(8, 5)), Some(utc(8, 15)));
        let mut off = i.clone();
        off.enabled = false;
        assert_eq!(next_run(&off, None, 0), None);
    }
}
