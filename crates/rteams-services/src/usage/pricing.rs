//! Model pricing (USD per million tokens), matched by lowercase model-id prefix/substring.
//! The original used a LiteLLM table; this keeps the families that matter for the CLIs we drive.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

const fn r(input: f64, output: f64, cache_read: f64, cache_write: f64) -> Rates {
    Rates {
        input,
        output,
        cache_read,
        cache_write,
    }
}

/// First match wins, so more specific prefixes come first.
const TABLE: &[(&str, Rates)] = &[
    ("claude-opus-4-5", r(5.0, 25.0, 0.5, 6.25)),
    ("claude-opus-4-6", r(5.0, 25.0, 0.5, 6.25)),
    ("claude-opus-4-7", r(5.0, 25.0, 0.5, 6.25)),
    ("claude-opus-4", r(15.0, 75.0, 1.5, 18.75)),
    ("claude-3-opus", r(15.0, 75.0, 1.5, 18.75)),
    ("claude-sonnet-4", r(3.0, 15.0, 0.3, 3.75)),
    ("claude-3-7-sonnet", r(3.0, 15.0, 0.3, 3.75)),
    ("claude-3-5-sonnet", r(3.0, 15.0, 0.3, 3.75)),
    ("claude-haiku-4", r(1.0, 5.0, 0.1, 1.25)),
    ("claude-3-5-haiku", r(0.8, 4.0, 0.08, 1.0)),
    ("claude-3-haiku", r(0.25, 1.25, 0.03, 0.3)),
    ("opus", r(5.0, 25.0, 0.5, 6.25)),
    ("sonnet", r(3.0, 15.0, 0.3, 3.75)),
    ("haiku", r(1.0, 5.0, 0.1, 1.25)),
    ("gpt-5-mini", r(0.25, 2.0, 0.025, 0.0)),
    ("gpt-5-nano", r(0.05, 0.4, 0.005, 0.0)),
    ("gpt-5.1-codex-mini", r(0.25, 2.0, 0.025, 0.0)),
    ("gpt-5", r(1.25, 10.0, 0.125, 0.0)),
    ("codex-mini", r(1.5, 6.0, 0.375, 0.0)),
    ("o4-mini", r(1.1, 4.4, 0.275, 0.0)),
    ("o3", r(2.0, 8.0, 0.5, 0.0)),
    ("gpt-4.1-mini", r(0.4, 1.6, 0.1, 0.0)),
    ("gpt-4.1", r(2.0, 8.0, 0.5, 0.0)),
    ("gpt-4o-mini", r(0.15, 0.6, 0.075, 0.0)),
    ("gpt-4o", r(2.5, 10.0, 1.25, 0.0)),
];

/// Rates for a model id (case-insensitive, provider prefix like `anthropic/` ignored).
pub fn rates_for(model: &str) -> Option<Rates> {
    let m = model.to_ascii_lowercase();
    let m = m.rsplit('/').next().unwrap_or(&m);
    if m.is_empty() || m == "<synthetic>" {
        return None;
    }
    if let Some((_, r)) = TABLE.iter().find(|(p, _)| m.starts_with(p)) {
        return Some(*r);
    }
    TABLE
        .iter()
        .find(|(p, _)| !p.contains('-') && m.contains(p))
        .map(|(_, r)| *r)
}

/// API-equivalent USD for the given token counts; 0 when the model is unknown.
pub fn cost(model: &str, input: u64, output: u64, cache_read: u64, cache_write: u64) -> f64 {
    let Some(r) = rates_for(model) else {
        return 0.0;
    };
    (input as f64 * r.input
        + output as f64 * r.output
        + cache_read as f64 * r.cache_read
        + cache_write as f64 * r.cache_write)
        / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_and_family_lookup() {
        assert_eq!(rates_for("claude-sonnet-4-5-20250929").unwrap().input, 3.0);
        assert_eq!(rates_for("claude-opus-4-1").unwrap().output, 75.0);
        assert_eq!(rates_for("claude-opus-4-5").unwrap().output, 25.0);
        assert_eq!(rates_for("anthropic/Claude-Haiku-4-5").unwrap().input, 1.0);
        assert_eq!(rates_for("sonnet").unwrap().output, 15.0);
        assert!(rates_for("<synthetic>").is_none());
        assert!(rates_for("mystery").is_none());
    }

    #[test]
    fn cost_math() {
        // 1M in + 1M out + 1M cache read + 1M cache write on sonnet 4.
        let c = cost(
            "claude-sonnet-4-5",
            1_000_000,
            1_000_000,
            1_000_000,
            1_000_000,
        );
        assert!((c - (3.0 + 15.0 + 0.3 + 3.75)).abs() < 1e-9);
        assert_eq!(cost("unknown", 5, 5, 5, 5), 0.0);
    }
}
