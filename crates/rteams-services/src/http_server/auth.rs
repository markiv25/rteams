//! Bearer-token auth for the HTTP API. Every route requires `Authorization: Bearer <token>`;
//! the comparison is constant-time so response timing does not leak a prefix match.

/// A fresh 256-bit random token (two v4 UUIDs, hex, no dashes).
pub fn generate_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Constant-time byte equality (length is not secret; content is).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Does an `Authorization` header value carry exactly `token` as a Bearer credential?
/// An empty configured token never authorizes anything.
pub fn bearer_ok(header: Option<&str>, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    let Some(h) = header else { return false };
    let h = h.trim();
    let Some((scheme, cred)) = h.split_once(' ') else {
        return false;
    };
    scheme.eq_ignore_ascii_case("bearer") && ct_eq(cred.trim().as_bytes(), token.as_bytes())
}

/// Reject DNS-rebinding style requests: the `Host` must be a loopback name.
pub fn host_ok(host: Option<&str>) -> bool {
    let Some(h) = host else { return false };
    let name = if let Some(rest) = h.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        h.split(':').next().unwrap_or("")
    };
    matches!(name, "127.0.0.1" | "localhost" | "::1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_and_unique() {
        let a = generate_token();
        assert_eq!(a.len(), 64);
        assert_ne!(a, generate_token());
    }

    #[test]
    fn bearer_checks() {
        assert!(bearer_ok(Some("Bearer abc"), "abc"));
        assert!(bearer_ok(Some("bearer abc"), "abc"));
        assert!(!bearer_ok(Some("Bearer abd"), "abc"));
        assert!(!bearer_ok(Some("Bearer abcd"), "abc"));
        assert!(!bearer_ok(Some("Basic abc"), "abc"));
        assert!(!bearer_ok(None, "abc"));
        assert!(!bearer_ok(Some("Bearer "), ""));
    }

    #[test]
    fn host_checks() {
        assert!(host_ok(Some("127.0.0.1:7860")));
        assert!(host_ok(Some("localhost")));
        assert!(host_ok(Some("[::1]:1")));
        assert!(!host_ok(Some("evil.example:80")));
        assert!(!host_ok(None));
    }
}
