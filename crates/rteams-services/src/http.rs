//! Tiny blocking HTTP GET helper for catalogs, announcements and update checks.
//!
//! Implemented on top of the system `curl` binary (`curl -fsSL --max-time N`) instead of an
//! embedded TLS client: this keeps the binary small and avoids a rustls/ring build dependency, and
//! curl ships with macOS, most Linux distributions and Windows 10+. Call it from a worker thread.

use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    /// `curl` is not installed or could not be started.
    NoCurl(String),
    /// curl exited non-zero (HTTP >= 400, DNS, timeout ...); message is its stderr.
    Failed(String),
    /// Body was not valid UTF-8 / JSON.
    Decode(String),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::NoCurl(e) => write!(f, "curl unavailable: {e}"),
            HttpError::Failed(e) => write!(f, "request failed: {e}"),
            HttpError::Decode(e) => write!(f, "bad response: {e}"),
        }
    }
}

impl std::error::Error for HttpError {}

/// Arguments for a GET (exposed for tests). `headers` are `"Name: value"` strings.
pub fn curl_args(url: &str, timeout: Duration, headers: &[&str]) -> Vec<String> {
    let mut a = vec![
        "-fsSL".to_string(),
        "--max-time".into(),
        timeout.as_secs().max(1).to_string(),
        "-A".into(),
        concat!("rteams/", env!("CARGO_PKG_VERSION")).into(),
    ];
    for h in headers {
        a.push("-H".into());
        a.push((*h).into());
    }
    a.push("--".into());
    a.push(url.into());
    a
}

/// GET `url` and return the body bytes.
pub fn get_bytes(url: &str, timeout: Duration) -> Result<Vec<u8>, HttpError> {
    get_bytes_with(url, timeout, &[])
}

pub fn get_bytes_with(
    url: &str,
    timeout: Duration,
    headers: &[&str],
) -> Result<Vec<u8>, HttpError> {
    let out = Command::new("curl")
        .args(curl_args(url, timeout, headers))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| HttpError::NoCurl(e.to_string()))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(HttpError::Failed(if err.is_empty() {
            format!("exit {}", out.status)
        } else {
            err
        }))
    }
}

/// GET `url` as UTF-8 text.
pub fn get_text(url: &str, timeout: Duration) -> Result<String, HttpError> {
    String::from_utf8(get_bytes(url, timeout)?).map_err(|e| HttpError::Decode(e.to_string()))
}

/// GET `url` and parse the body as JSON.
pub fn get_json(url: &str, timeout: Duration) -> Result<serde_json::Value, HttpError> {
    serde_json::from_slice(&get_bytes(url, timeout)?).map_err(|e| HttpError::Decode(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_args() {
        let a = curl_args(
            "https://x.test/a",
            Duration::from_secs(7),
            &["Accept: application/json"],
        );
        assert_eq!(&a[..3], ["-fsSL", "--max-time", "7"]);
        assert_eq!(a.last().unwrap(), "https://x.test/a");
        assert!(a.contains(&"Accept: application/json".to_string()));
    }

    #[test]
    fn unreachable_host_is_an_error_not_a_panic() {
        // Port 1 on loopback refuses immediately; if curl is missing we get NoCurl.
        assert!(get_json("http://127.0.0.1:1/", Duration::from_secs(2)).is_err());
    }
}
