//! Strip secrets from failure text before it is stored or shown.

use std::sync::OnceLock;

use regex::Regex;

fn rules() -> &'static [(Regex, &'static str)] {
    static R: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    R.get_or_init(|| {
        let mk = |p: &str| Regex::new(p).expect("static regex");
        vec![
            (mk(r"(?i)(authorization\s*[:=]\s*(?:bearer|basic)\s+)[^\s,;'\x22]+"), "${1}[redacted]"),
            (mk(r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]{8,}"), "Bearer [redacted]"),
            (mk(r"\bsk-[A-Za-z0-9_-]{8,}"), "sk-[redacted]"),
            (
                mk(r"(?i)\b([A-Z0-9_]*(?:API_?KEY|TOKEN|SECRET|PASSWORD|PASSWD))(\s*[=:]\s*)[^\s,;'\x22]+"),
                "${1}${2}[redacted]",
            ),
        ]
    })
}

/// Replace bearer tokens, `sk-` keys and `*_API_KEY|TOKEN|SECRET|PASSWORD=` values.
pub fn redact(text: &str) -> String {
    let mut out = text.to_string();
    for (re, to) in rules() {
        out = re.replace_all(&out, *to).into_owned();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_known_secret_shapes() {
        let r = redact("Authorization: Bearer abc123def456ghi and key sk-ant-api03-ABCDEFG123");
        assert!(
            !r.contains("abc123def456ghi") && !r.contains("ABCDEFG123"),
            "{r}"
        );
        let r = redact("ANTHROPIC_API_KEY=hunter2 GITHUB_TOKEN: ghp_x DB_PASSWORD=p@ss ok=fine");
        assert!(
            !r.contains("hunter2") && !r.contains("ghp_x") && !r.contains("p@ss"),
            "{r}"
        );
        assert!(r.contains("ok=fine"));
    }

    #[test]
    fn leaves_plain_text() {
        assert_eq!(
            redact("HTTP 503 service unavailable"),
            "HTTP 503 service unavailable"
        );
    }
}
