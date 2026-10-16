//! Cron parsing and next-fire computation (5-field expressions, optional IANA timezone),
//! the interval check and a human description for the editor.

use chrono::{DateTime, Local, TimeZone, Utc};
use chrono_tz::Tz;
use croner::Cron;

/// Editor presets: `(i18n key, English label, expression)`.
pub const PRESETS: &[(&str, &str, &str)] = &[
    (
        "team.schedule.cron.presets.everyThirtyMinutes",
        "Every 30 min",
        "*/30 * * * *",
    ),
    (
        "team.schedule.cron.presets.everyHour",
        "Every hour",
        "0 * * * *",
    ),
    (
        "team.schedule.cron.presets.everySixHours",
        "Every 6 hours",
        "0 */6 * * *",
    ),
    (
        "team.schedule.cron.presets.dailyAtNine",
        "Daily at 9am",
        "0 9 * * *",
    ),
    (
        "team.schedule.cron.presets.weekdaysAtNine",
        "Weekdays at 9am",
        "0 9 * * 1-5",
    ),
    (
        "team.schedule.cron.presets.mondayAtNine",
        "Monday at 9am",
        "0 9 * * 1",
    ),
];

/// Warm-up choices in minutes (`0` = none).
pub const WARMUP_CHOICES: &[u32] = &[0, 5, 10, 15, 30];

/// Interval below which the editor warns about a high-frequency schedule.
pub const HIGH_FREQUENCY_MINUTES: i64 = 5;

/// Common IANA zones offered by the editor.
pub const COMMON_TIMEZONES: &[&str] = &[
    "UTC",
    "America/Los_Angeles",
    "America/Denver",
    "America/Chicago",
    "America/New_York",
    "America/Sao_Paulo",
    "Europe/London",
    "Europe/Berlin",
    "Europe/Moscow",
    "Asia/Dubai",
    "Asia/Kolkata",
    "Asia/Shanghai",
    "Asia/Tokyo",
    "Australia/Sydney",
];

/// Format a unix-ms instant as `Mon 2026-05-04 09:00` in `tz` (local time when `None`).
pub fn format_in(ms: i64, tz: Option<&str>) -> String {
    let Some(utc) = DateTime::<Utc>::from_timestamp_millis(ms) else {
        return String::new();
    };
    const F: &str = "%a %Y-%m-%d %H:%M";
    match parse_tz(tz) {
        Ok(Some(z)) => utc.with_timezone(&z).format(F).to_string(),
        _ => utc.with_timezone(&Local).format(F).to_string(),
    }
}

/// Parse a 5-field cron expression.
pub fn parse(expr: &str) -> Result<Cron, String> {
    let e = expr.trim();
    if e.is_empty() {
        return Err("Enter a cron expression".into());
    }
    if e.split_whitespace().count() != 5 {
        return Err("Invalid cron expression".into());
    }
    Cron::new(e)
        .parse()
        .map_err(|_| "Invalid cron expression".to_string())
}

/// Parse an IANA timezone name; `None` or empty means local time.
pub fn parse_tz(tz: Option<&str>) -> Result<Option<Tz>, String> {
    match tz.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(name) => name
            .parse::<Tz>()
            .map(Some)
            .map_err(|_| format!("Unknown timezone: {name}")),
    }
}

fn next_n_in<Z: TimeZone>(cron: &Cron, zone: &Z, after_ms: i64, n: usize) -> Vec<i64> {
    let Some(utc) = DateTime::<Utc>::from_timestamp_millis(after_ms) else {
        return vec![];
    };
    let mut cur = utc.with_timezone(zone);
    let mut out = Vec::new();
    while out.len() < n {
        match cron.find_next_occurrence(&cur, false) {
            Ok(t) => {
                out.push(t.timestamp_millis());
                cur = t;
            }
            Err(_) => break,
        }
    }
    out
}

/// The next `n` fire times (unix ms) strictly after `after_ms`.
pub fn next_n(expr: &str, tz: Option<&str>, after_ms: i64, n: usize) -> Result<Vec<i64>, String> {
    let cron = parse(expr)?;
    Ok(match parse_tz(tz)? {
        Some(z) => next_n_in(&cron, &z, after_ms, n),
        None => next_n_in(&cron, &Local, after_ms, n),
    })
}

/// The next fire strictly after `after_ms`.
pub fn next_after(expr: &str, tz: Option<&str>, after_ms: i64) -> Option<i64> {
    next_n(expr, tz, after_ms, 1).ok()?.first().copied()
}

/// Smallest gap in minutes between consecutive fires among the next few runs.
pub fn min_interval_minutes(expr: &str, tz: Option<&str>, from_ms: i64) -> Option<i64> {
    let v = next_n(expr, tz, from_ms, 12).ok()?;
    v.windows(2).map(|w| (w[1] - w[0]) / 60_000).min()
}

/// True when the schedule would fire more often than every five minutes.
pub fn is_high_frequency(expr: &str, tz: Option<&str>, from_ms: i64) -> bool {
    min_interval_minutes(expr, tz, from_ms).is_some_and(|m| m < HIGH_FREQUENCY_MINUTES)
}

/// `*/N` -> `N`.
fn step(field: &str) -> Option<u32> {
    field.strip_prefix("*/")?.parse().ok().filter(|n| *n > 0)
}

fn num(field: &str) -> Option<u32> {
    field.parse().ok()
}

fn clock(h: u32, m: u32) -> String {
    format!("{h:02}:{m:02}")
}

fn weekday_name(d: u32) -> &'static str {
    [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ][(d % 8) as usize]
}

/// Plain-English description of a cron expression (common shapes; otherwise the expression).
pub fn describe(expr: &str) -> String {
    let f: Vec<&str> = expr.split_whitespace().collect();
    if f.len() != 5 {
        return expr.trim().to_string();
    }
    let (min, hour, dom, mon, dow) = (f[0], f[1], f[2], f[3], f[4]);
    let when = if mon != "*" {
        None
    } else {
        match (dom, dow) {
            ("*", "*") => Some(String::new()),
            ("*", "1-5") => Some(", Monday through Friday".into()),
            ("*", "0,6") | ("*", "6,0") => Some(", on weekends".into()),
            ("*", d) => num(d)
                .filter(|d| *d <= 7)
                .map(|d| format!(", only on {}", weekday_name(d))),
            (d, "*") => num(d).map(|d| format!(", on day {d} of the month")),
            _ => None,
        }
    };
    let Some(suffix) = when else {
        return format!("Cron: {}", expr.trim());
    };
    let base = if let (Some(n), "*") = (step(min), hour) {
        format!("Every {n} minutes")
    } else if min == "*" && hour == "*" {
        "Every minute".to_string()
    } else if let (Some(m), "*") = (num(min), hour) {
        if m == 0 {
            "Every hour".to_string()
        } else {
            format!("Every hour at minute {m}")
        }
    } else if let (Some(m), Some(n)) = (num(min), step(hour)) {
        if m == 0 {
            format!("Every {n} hours")
        } else {
            format!("Every {n} hours at minute {m}")
        }
    } else if let (Some(m), Some(h)) = (num(min), num(hour)) {
        format!("At {}", clock(h, m))
    } else {
        return format!("Cron: {}", expr.trim());
    };
    format!("{base}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn validates() {
        assert!(parse("*/5 * * * *").is_ok());
        assert_eq!(parse("").unwrap_err(), "Enter a cron expression");
        assert_eq!(parse("* * *").unwrap_err(), "Invalid cron expression");
        assert!(parse("61 * * * *").is_err());
        assert!(parse_tz(Some("Mars/Base")).is_err());
        assert!(parse_tz(Some("")).unwrap().is_none());
    }

    #[test]
    fn next_fire_respects_timezone() {
        // New York is UTC-5 in January and UTC-4 in July.
        let w = next_after(
            "0 9 * * *",
            Some("America/New_York"),
            utc(2026, 1, 15, 0, 0),
        );
        assert_eq!(w, Some(utc(2026, 1, 15, 14, 0)));
        let s = next_after(
            "0 9 * * *",
            Some("America/New_York"),
            utc(2026, 7, 15, 0, 0),
        );
        assert_eq!(s, Some(utc(2026, 7, 15, 13, 0)));
        let t = next_after("0 9 * * *", Some("Asia/Tokyo"), utc(2026, 7, 15, 0, 0));
        assert_eq!(t, Some(utc(2026, 7, 16, 0, 0)));
    }

    #[test]
    fn strictly_after_and_five_fires() {
        let from = utc(2026, 1, 1, 0, 0);
        let v = next_n("*/30 * * * *", Some("UTC"), from, 5).unwrap();
        assert_eq!(v.len(), 5);
        assert_eq!(v[0], from + 30 * 60_000);
        assert!(v.windows(2).all(|w| w[1] - w[0] == 30 * 60_000));
    }

    #[test]
    fn dst_transitions_keep_wall_clock() {
        // 2026-03-08 US spring forward: 9:00 local moves from 14:00Z to 13:00Z.
        let v = next_n(
            "0 9 * * *",
            Some("America/New_York"),
            utc(2026, 3, 6, 20, 0),
            3,
        )
        .unwrap();
        assert_eq!(
            v,
            vec![
                utc(2026, 3, 7, 14, 0),
                utc(2026, 3, 8, 13, 0),
                utc(2026, 3, 9, 13, 0)
            ]
        );
        // 2026-11-01 fall back.
        let v = next_n(
            "0 9 * * *",
            Some("America/New_York"),
            utc(2026, 10, 31, 20, 0),
            2,
        )
        .unwrap();
        assert_eq!(v, vec![utc(2026, 11, 1, 14, 0), utc(2026, 11, 2, 14, 0)]);
        // A time inside the skipped hour still yields increasing fires.
        let v = next_n(
            "30 2 * * *",
            Some("America/New_York"),
            utc(2026, 3, 6, 12, 0),
            4,
        )
        .unwrap();
        assert_eq!(v.len(), 4);
        assert!(v.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn frequency_warning() {
        let now = utc(2026, 1, 1, 0, 0);
        assert!(is_high_frequency("*/2 * * * *", Some("UTC"), now));
        assert!(!is_high_frequency("*/30 * * * *", Some("UTC"), now));
        assert!(!is_high_frequency("0 9 * * 1", Some("UTC"), now));
    }

    #[test]
    fn descriptions() {
        assert_eq!(describe("*/30 * * * *"), "Every 30 minutes");
        assert_eq!(describe("0 * * * *"), "Every hour");
        assert_eq!(describe("0 */6 * * *"), "Every 6 hours");
        assert_eq!(describe("0 9 * * *"), "At 09:00");
        assert_eq!(describe("0 9 * * 1-5"), "At 09:00, Monday through Friday");
        assert_eq!(describe("0 9 * * 1"), "At 09:00, only on Monday");
        assert_eq!(describe("5 4 1 6 *"), "Cron: 5 4 1 6 *");
    }

    #[test]
    fn presets_parse() {
        for (_, _, e) in PRESETS {
            assert!(parse(e).is_ok(), "{e}");
        }
    }
}
