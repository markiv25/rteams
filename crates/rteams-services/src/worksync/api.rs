//! Agent-facing work-sync API. SELF-CONTAINED on purpose (only std, serde_json, rteams-core,
//! rteams-store): `rteams-services` depends on `rteams-server`, so the server cannot call into
//! services; it includes this file instead:
//! `#[path = "../../rteams-services/src/worksync/api.rs"] mod worksync;` then `worksync::status(..)`.
//! Record format = `store::MemberSync` JSON at `<home>/<team>/work-sync/<member>.json`.

use std::path::Path;

use rteams_core::projection::briefing;
use rteams_core::State;
use serde_json::{json, Value};

const SETTLE_MS: i64 = 2 * 60_000;
const GRACE_MS: i64 = 10 * 60_000;

fn lead_like(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "team-lead" | "lead" | "orchestrator"
    )
}

fn s(v: impl serde::Serialize) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// (task, subject, status, reason) of the member's actionable work.
fn items(state: &State, member: &str) -> Vec<(String, String, String, String)> {
    let removed = state
        .team
        .as_ref()
        .is_some_and(|t| t.members.iter().any(|m| m.name == member && m.removed));
    if lead_like(member) || removed {
        return vec![];
    }
    briefing(state, member)
        .actionable
        .into_iter()
        .map(|i| (i.task, i.subject, s(i.status), s(i.reason)))
        .collect()
}

/// Stable 64-bit FNV-1a, hex. Empty agenda -> "".
pub fn fingerprint_rows(member: &str, rows: &[(String, String, String)]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut v: Vec<String> = rows
        .iter()
        .map(|(t, st, r)| json!({"t": t, "s": st, "r": r}).to_string())
        .collect();
    v.sort();
    let canon = format!("{member}\n{}", v.join("\n"));
    let mut h: u64 = 0xcbf29ce484222325;
    for b in canon.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn fp(state: &State, member: &str) -> (Vec<(String, String, String, String)>, String) {
    let it = items(state, member);
    let rows: Vec<_> = it
        .iter()
        .map(|(t, _, st, r)| (t.clone(), st.clone(), r.clone()))
        .collect();
    let f = fingerprint_rows(member, &rows);
    (it, f)
}

fn rec_path(home: &Path, team: &str, member: &str) -> std::path::PathBuf {
    let safe: String = member
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    home.join(team)
        .join("work-sync")
        .join(format!("{safe}.json"))
}

fn load_rec(home: &Path, team: &str, member: &str) -> Value {
    std::fs::read(rec_path(home, team, member))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

fn load_state(home: &Path, team: &str) -> Result<State, String> {
    rteams_store::FileStore::open_read_only(home, team)
        .map(|st| st.loaded_state().clone())
        .map_err(|e| e.to_string())
}

fn status_of(state: &State, home: &Path, team: &str, member: &str, now: i64) -> Value {
    let (it, f) = fp(state, member);
    let rec = load_rec(home, team, member);
    let g = |k: &str| rec.get(k).and_then(Value::as_i64);
    let same = rec.get("fingerprint").and_then(Value::as_str) == Some(f.as_str());
    let first_seen = if same {
        g("first_seen_at").unwrap_or(now)
    } else {
        now
    };
    let acked = same && g("last_running_at").is_some_and(|r| r + SETTLE_MS >= first_seen);
    let state_word = if it.is_empty() {
        "idle"
    } else if rec.get("auto_resume_stopped").and_then(Value::as_bool) == Some(true) {
        "stopped"
    } else if acked {
        "in_sync"
    } else if now < first_seen + GRACE_MS {
        "pending"
    } else {
        "needs_attention"
    };
    json!({
        "member": member,
        "state": state_word,
        "fingerprint": f,
        "acknowledged": acked,
        "actionable": it.iter().map(|(t, sub, st, r)| json!({"task": t, "subject": sub, "status": st, "reason": r})).collect::<Vec<_>>(),
        "attempts": if same { rec.get("attempts").and_then(Value::as_u64).unwrap_or(0) } else { 0 },
        "last_nudge_at": rec.get("last_nudge_at").cloned().unwrap_or(Value::Null),
        "auto_resume_stopped": rec.get("auto_resume_stopped").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// What work-sync currently thinks of `member` (actionable items, fingerprint, state:
/// idle|pending|in_sync|needs_attention|stopped). Errors are returned as `{"error": ..}`.
pub fn status(home: &Path, team: &str, member: &str) -> Value {
    match load_state(home, team) {
        Ok(st) => status_of(&st, home, team, member, now_ms()),
        Err(e) => json!({"error": e}),
    }
}

/// The member confirms it has seen its current agenda: no work-sync nudge is sent for this
/// fingerprint until it changes. Returns the new status (or `{"error": ..}`).
pub fn ack(home: &Path, team: &str, member: &str, now: i64) -> Value {
    let st = match load_state(home, team) {
        Ok(s) => s,
        Err(e) => return json!({"error": e}),
    };
    let (_, f) = fp(&st, member);
    let mut rec = load_rec(home, team, member);
    if rec.get("fingerprint").and_then(Value::as_str) != Some(f.as_str()) {
        rec["fingerprint"] = json!(f);
        rec["first_seen_at"] = json!(now);
        rec["attempts"] = json!(0);
    }
    rec["last_running_at"] = json!(now);
    rec["schema_version"] = json!(1);
    let path = rec_path(home, team, member);
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = path.with_extension("json.tmp");
    let wrote = serde_json::to_vec_pretty(&rec)
        .map_err(|e| e.to_string())
        .and_then(|b| std::fs::write(&tmp, b).map_err(|e| e.to_string()))
        .and_then(|_| std::fs::rename(&tmp, &path).map_err(|e| e.to_string()));
    if let Err(e) = wrote {
        return json!({"error": e});
    }
    status_of(&st, home, team, member, now)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_team_is_error_and_fingerprint_stable() {
        let d = tempfile::tempdir().unwrap();
        assert!(status(d.path(), "nope", "ada").get("error").is_some());
        let a = fingerprint_rows("ada", &[("1".into(), "pending".into(), "start".into())]);
        assert_eq!(a.len(), 16);
        assert_eq!(
            a,
            fingerprint_rows("ada", &[("1".into(), "pending".into(), "start".into())])
        );
        assert_eq!(fingerprint_rows("ada", &[]), "");
    }

    #[test]
    fn ack_then_status_in_sync_for_empty_agenda_record() {
        let d = tempfile::tempdir().unwrap();
        let team = "t";
        let mut st = rteams_store::FileStore::open(d.path(), team).unwrap();
        let _ = &mut st;
        let out = ack(d.path(), team, "ada", 5_000);
        assert!(out.get("error").is_none(), "{out}");
        assert_eq!(out["state"], "idle");
        assert!(rec_path(d.path(), team, "ada").exists());
    }
}
