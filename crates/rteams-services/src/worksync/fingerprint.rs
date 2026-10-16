//! Agenda fingerprint: canonical JSON of (task id, status, reason) per item, hashed. Subjects
//! and descriptions are excluded so cosmetic edits never count as a change.

use serde_json::json;

use super::agenda::AgendaItem;
use crate::ledger::stable_json::hash_str;

pub fn fingerprint(member: &str, items: &[AgendaItem]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let mut rows: Vec<_> = items
        .iter()
        .map(|i| json!({"t": i.task, "s": i.status, "r": i.reason}))
        .collect();
    rows.sort_by_key(|v| v.to_string());
    let canonical = json!({"m": member, "items": rows});
    hash_str(&crate::ledger::stable_json(&canonical))[..16].to_string()
}
