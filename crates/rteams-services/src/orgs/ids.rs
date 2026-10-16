//! Id helpers for organizations, groups and relations (kebab-case, like the original's
//! `normalizeOrganizationId`).

/// Lowercase kebab id from a display name; `item` when nothing usable is left.
pub fn normalize_id(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in name.trim().chars() {
        if c.is_alphanumeric() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.extend(c.to_lowercase());
        } else {
            dash = true;
        }
    }
    if out.is_empty() {
        "item".into()
    } else {
        out
    }
}

/// `base`, or `base-2`, `base-3`, ... the first one `taken` does not claim.
pub fn unique_id(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !taken(id))
        .expect("unbounded range")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kebab_and_unique() {
        assert_eq!(normalize_id("  Product  Area! "), "product-area");
        assert_eq!(normalize_id("---"), "item");
        assert_eq!(normalize_id("Ünï Team"), "ünï-team");
        assert_eq!(unique_id("a", |_| false), "a");
        assert_eq!(unique_id("a", |s| s == "a" || s == "a-2"), "a-3");
    }
}
