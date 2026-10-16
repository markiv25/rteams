//! Importer for Codex rollout logs (`~/.codex/sessions/**/rollout-*.jsonl`): one record per
//! `token_count` event using `last_token_usage` (cached input is a subset of input, so it is moved
//! to the cache column). The model comes from the latest `turn_context`.

use std::path::Path;

use serde_json::Value;

use super::claude_import::parse_ts;
use super::ledger::{record_id, Billing, Source, SourceKind, UsageRecord};
use super::pricing;

pub fn parse_rollout(raw: &str, path: &Path) -> Vec<UsageRecord> {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut session = stem.clone();
    let mut model = String::new();
    let mut last_total: Option<u64> = None;
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let p = v.get("payload").unwrap_or(&Value::Null);
        match v.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                if let Some(id) = p.get("id").and_then(Value::as_str) {
                    session = id.to_string();
                }
            }
            Some("turn_context") => {
                if let Some(m) = p.get("model").and_then(Value::as_str) {
                    model = m.to_string();
                }
            }
            Some("event_msg") if p.get("type").and_then(Value::as_str) == Some("token_count") => {
                let Some(info) = p.get("info").filter(|i| !i.is_null()) else {
                    continue;
                };
                // The same event is often emitted twice; the running total tells repeats apart.
                let total = info
                    .pointer("/total_token_usage/total_tokens")
                    .and_then(Value::as_u64);
                if total.is_some() && total == last_total {
                    continue;
                }
                last_total = total;
                let g = |k: &str| {
                    info.pointer(&format!("/last_token_usage/{k}"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                };
                let cached = g("cached_input_tokens");
                let input = g("input_tokens").saturating_sub(cached);
                let output = g("output_tokens");
                let reasoning = g("reasoning_output_tokens");
                out.push(UsageRecord {
                    id: record_id(&session, &format!("{i}")),
                    source: Source::Codex,
                    team: String::new(),
                    member: String::new(),
                    runtime: "codex".into(),
                    model: model.clone(),
                    run: session.clone(),
                    session: session.clone(),
                    ts_ms: v
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .and_then(parse_ts)
                        .unwrap_or(0),
                    input,
                    output,
                    cache_creation: 0,
                    cache_read: cached,
                    reasoning,
                    cost_usd: pricing::cost(&model, input, output, cached, 0),
                    kind: SourceKind::LogParsed,
                    billing: Billing::Unknown,
                });
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::ledger::Scanner;

    fn tc(ts: &str, total: u64, input: u64, cached: u64, out: u64) -> String {
        format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"total_tokens":{total}}},"last_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":{out},"reasoning_output_tokens":2}}}}}}}}"#
        )
    }

    fn rollout() -> String {
        [
            r#"{"type":"session_meta","payload":{"id":"abc"}}"#.to_string(),
            r#"{"type":"turn_context","payload":{"model":"gpt-5-codex"}}"#.to_string(),
            r#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#.to_string(),
            tc("2025-03-01T10:00:00Z", 150, 100, 40, 50),
            tc("2025-03-01T10:00:00Z", 150, 100, 40, 50),
            tc("2025-03-01T10:01:00Z", 400, 200, 0, 50),
        ]
        .join("\n")
    }

    #[test]
    fn parses_token_counts_skipping_repeats() {
        let r = parse_rollout(&rollout(), Path::new("rollout-1.jsonl"));
        assert_eq!(r.len(), 2);
        assert_eq!(
            (r[0].input, r[0].cache_read, r[0].output, r[0].reasoning),
            (60, 40, 50, 2)
        );
        assert_eq!(r[0].run, "abc");
        assert_eq!(r[0].runtime, "codex");
        assert!(r[0].cost_usd > 0.0);
    }

    #[test]
    fn scanner_imports_codex_dir() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("codex/2025/03/01");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("rollout-1.jsonl"), rollout()).unwrap();
        let sc = Scanner {
            home: dir.path().join("home"),
            claude_dir: None,
            codex_dir: Some(dir.path().join("codex")),
        };
        assert_eq!(sc.scan().records.len(), 2);
    }
}
