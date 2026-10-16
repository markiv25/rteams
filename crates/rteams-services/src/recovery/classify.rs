//! Classify why an agent run failed, from its last error line / output.

use std::sync::OnceLock;

use regex::Regex;

use super::redact::redact;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// Usage / rate limit; may carry a reset time.
    RateLimit,
    /// Network trouble, overloaded or 5xx: worth retrying soon.
    Transient,
    /// Credentials rejected: needs the user.
    Auth,
    /// Request rejected (400/404/413/422): retrying cannot help.
    Client,
    /// Non-zero exit or crash without a known cause.
    Crash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub class: FailureClass,
    /// Redacted, single-line excerpt.
    pub detail: String,
    /// Unix ms the limit resets (rate limits only).
    pub reset_at: Option<i64>,
    pub status_code: Option<u16>,
}

const BUFFER_MS: i64 = 30_000;

fn status_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)(?:http|status(?: code)?|error|api error)\D{0,8}\b([45]\d\d)\b")
            .expect("static regex")
    })
}

fn retry_after_ms(lower: &str) -> Option<i64> {
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| {
        Regex::new(r"(?:retry-after\D{0,4}|try again in |retry in |resets? in )(\d+)\s*(seconds?|secs?|s|minutes?|mins?|m|hours?|h)?")
            .expect("static regex")
    });
    let c = re.captures(lower)?;
    let n: i64 = c[1].parse().ok()?;
    let mult = match c.get(2).map(|m| m.as_str()) {
        Some(u) if u.starts_with('m') => 60_000,
        Some(u) if u.starts_with('h') => 3_600_000,
        _ => 1_000,
    };
    Some(n * mult)
}

fn one_line(s: &str, max: usize) -> String {
    let t = redact(s).split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > max {
        let h: String = t.chars().take(max).collect();
        format!("{h}...")
    } else {
        t
    }
}

/// Classify a failure. `text` is the last error / output; `rate_limited_until` is a reset
/// already extracted by the launcher (unix ms); `status_code` / `exit_code` come from the run
/// summary when known. `None` when nothing looks like a failure.
pub fn classify(
    text: &str,
    rate_limited_until: Option<i64>,
    status_code: Option<u16>,
    exit_code: Option<i32>,
    now_ms: i64,
) -> Option<Failure> {
    let lower = text.to_lowercase();
    let status = status_code.or_else(|| {
        status_re()
            .captures(text)
            .and_then(|c| c[1].parse::<u16>().ok())
    });
    let mk = |class, reset_at| Failure {
        class,
        detail: one_line(text, 200),
        reset_at,
        status_code: status,
    };
    let has = |keys: &[&str]| keys.iter().any(|k| lower.contains(k));

    let limited = rate_limited_until.is_some()
        || status == Some(429)
        || has(&[
            "rate limit",
            "rate_limit",
            "usage limit",
            "too many requests",
            "limit reached",
        ]);
    if limited {
        let reset =
            rate_limited_until.or_else(|| retry_after_ms(&lower).map(|d| now_ms + d + BUFFER_MS));
        return Some(mk(FailureClass::RateLimit, reset));
    }
    if matches!(status, Some(401 | 403))
        || has(&[
            "invalid_grant",
            "refresh_token_reused",
            "invalid api key",
            "invalid x-api-key",
            "authentication_error",
            "unauthorized",
            "not logged in",
            "please run /login",
        ])
    {
        return Some(mk(FailureClass::Auth, None));
    }
    if matches!(status, Some(500 | 502 | 503 | 504 | 529))
        || has(&[
            "econnreset",
            "epipe",
            "etimedout",
            "connection reset",
            "connection refused",
            "fetch failed",
            "overloaded_error",
            "overloaded",
            "service unavailable",
            "socket hang up",
            "network error",
            "timed out",
        ])
    {
        return Some(mk(FailureClass::Transient, None));
    }
    if matches!(status, Some(400 | 404 | 413 | 422)) {
        return Some(mk(FailureClass::Client, None));
    }
    if exit_code.is_some_and(|c| c != 0)
        || has(&["panicked", "segmentation fault", "killed", "core dumped"])
    {
        return Some(mk(FailureClass::Crash, None));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000;

    #[test]
    fn transient_markers() {
        for t in [
            "API Error: 503 service unavailable",
            "error: read ECONNRESET",
            "fetch failed",
            "{\"type\":\"overloaded_error\"}",
            "HTTP 529",
        ] {
            let f = classify(t, None, None, None, NOW).unwrap();
            assert_eq!(f.class, FailureClass::Transient, "{t}");
        }
    }

    #[test]
    fn rate_limit_with_and_without_reset() {
        let f = classify("rate limit hit, retry-after: 90", None, None, None, NOW).unwrap();
        assert_eq!(f.class, FailureClass::RateLimit);
        assert_eq!(f.reset_at, Some(NOW + 90_000 + BUFFER_MS));
        let f = classify("429 too many requests", None, None, None, NOW).unwrap();
        assert_eq!(f.reset_at, None);
        let f = classify("usage limit reached", Some(5_000_000), None, None, NOW).unwrap();
        assert_eq!(f.reset_at, Some(5_000_000));
        let f = classify("try again in 2 hours; rate limit", None, None, None, NOW).unwrap();
        assert_eq!(f.reset_at, Some(NOW + 7_200_000 + BUFFER_MS));
    }

    #[test]
    fn auth_client_crash_and_none() {
        assert_eq!(
            classify("invalid_grant", None, None, None, NOW)
                .unwrap()
                .class,
            FailureClass::Auth
        );
        assert_eq!(
            classify("HTTP 401", None, None, None, NOW).unwrap().class,
            FailureClass::Auth
        );
        assert_eq!(
            classify("error: status 422 bad body", None, None, None, NOW)
                .unwrap()
                .class,
            FailureClass::Client
        );
        assert_eq!(
            classify("boom", None, None, Some(137), NOW).unwrap().class,
            FailureClass::Crash
        );
        assert_eq!(classify("all good", None, None, Some(0), NOW), None);
        assert_eq!(classify("", None, None, None, NOW), None);
    }

    #[test]
    fn detail_is_redacted_and_clipped() {
        let f = classify(
            "503 service unavailable Authorization: Bearer abcdefghijkl12345",
            None,
            None,
            None,
            NOW,
        )
        .unwrap();
        assert!(!f.detail.contains("abcdefghijkl12345"), "{}", f.detail);
        let long = format!("HTTP 503 {}", "x ".repeat(500));
        assert!(
            classify(&long, None, None, None, NOW)
                .unwrap()
                .detail
                .chars()
                .count()
                <= 203
        );
    }
}
