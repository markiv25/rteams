//! Importer for Claude Code session logs (`~/.claude/projects/<enc>/<session>.jsonl`): one
//! record per assistant request, deduped by `message.id` + `requestId` (streaming writes the same
//! request several times; the last write has the final counts). Cost comes from the pricing table.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use super::ledger::{record_id, Billing, Source, SourceKind, UsageRecord};
use super::pricing;

/// Unix ms from an RFC 3339 timestamp.
pub(crate) fn parse_ts(s: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .and_then(|d| u64::try_from(d.timestamp_millis()).ok())
}

pub fn parse_session(raw: &str, path: &Path) -> Vec<UsageRecord> {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut by_req: BTreeMap<String, UsageRecord> = BTreeMap::new();
    for line in raw.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if v.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(u) = v.pointer("/message/usage") else {
            continue;
        };
        let model = v
            .pointer("/message/model")
            .and_then(Value::as_str)
            .unwrap_or("");
        if model == "<synthetic>" {
            continue;
        }
        let g = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
        let (input, output) = (g("input_tokens"), g("output_tokens"));
        let (cc, cr) = (
            g("cache_creation_input_tokens"),
            g("cache_read_input_tokens"),
        );
        let session = v
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or(&stem)
            .to_string();
        let key = format!(
            "{}:{}",
            v.pointer("/message/id")
                .and_then(Value::as_str)
                .unwrap_or(""),
            v.get("requestId").and_then(Value::as_str).unwrap_or("")
        );
        let ts_ms = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_ts)
            .unwrap_or(0);
        by_req.insert(
            key.clone(),
            UsageRecord {
                id: record_id(&session, &key),
                source: Source::Claude,
                team: String::new(),
                member: String::new(),
                runtime: "claude".into(),
                model: model.into(),
                run: session.clone(),
                session,
                ts_ms,
                input,
                output,
                cache_creation: cc,
                cache_read: cr,
                reasoning: 0,
                cost_usd: pricing::cost(model, input, output, cr, cc),
                kind: SourceKind::LogParsed,
                billing: Billing::Unknown,
            },
        );
    }
    by_req.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::ledger::Scanner;

    fn line(req: &str, out: u64) -> String {
        format!(
            r#"{{"type":"assistant","sessionId":"sess","requestId":"{req}","timestamp":"2025-03-01T10:00:00Z","message":{{"id":"m-{req}","model":"claude-sonnet-4-5","usage":{{"input_tokens":100,"output_tokens":{out},"cache_read_input_tokens":50}}}}}}"#
        )
    }

    #[test]
    fn dedupes_streamed_requests_keeping_last() {
        let raw = format!(
            "{}\n{}\n{}\n{}\n",
            line("r1", 1),
            line("r1", 40),
            line("r2", 7),
            r#"{"type":"user","message":{}}"#
        );
        let r = parse_session(&raw, Path::new("/x/sess.jsonl"));
        assert_eq!(r.len(), 2);
        assert_eq!(
            r.iter().find(|r| r.output == 40).unwrap().ts_ms,
            1_740_823_200_000
        );
        assert!(r.iter().all(|r| r.cost_usd > 0.0 && r.team.is_empty()));
    }

    #[test]
    fn scanner_imports_from_synthetic_dir() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("claude/-proj");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("sess.jsonl"), line("r1", 5)).unwrap();
        let sc = Scanner {
            home: dir.path().join("home"),
            claude_dir: Some(dir.path().join("claude")),
            codex_dir: None,
        };
        let l = sc.scan();
        assert_eq!(l.records.len(), 1);
        assert_eq!(l.records[0].source, Source::Claude);
    }
}
