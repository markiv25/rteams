//! Idempotent execution on top of the ledger.

use serde_json::Value;

use super::store::{Begin, Ledger, LedgerError};

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Ran now.
    Executed(Value),
    /// Returned the stored result; nothing ran.
    Replayed(Value),
    /// Executor failed (recorded; a later retry with the same id runs again).
    Failed(String),
    /// Started elsewhere and not finished; use `replay_unacked` after a crash.
    InFlight,
}

/// Run `exec` at most once per `id` (until it fails).
pub fn run_once(
    ledger: &mut Ledger,
    id: &str,
    payload: &Value,
    now: i64,
    exec: impl FnOnce(&Value) -> Result<Value, String>,
) -> Result<Outcome, LedgerError> {
    match ledger.begin(id, payload, now)? {
        Begin::Replay(v) => Ok(Outcome::Replayed(v)),
        Begin::Unfinished => Ok(Outcome::InFlight),
        Begin::Execute => finish(ledger, id, payload, now, exec),
    }
}

fn finish(
    ledger: &mut Ledger,
    id: &str,
    payload: &Value,
    now: i64,
    exec: impl FnOnce(&Value) -> Result<Value, String>,
) -> Result<Outcome, LedgerError> {
    match exec(payload) {
        Ok(v) => {
            ledger.complete(id, v.clone(), now)?;
            Ok(Outcome::Executed(v))
        }
        Err(e) => {
            ledger.fail(id, &e, now)?;
            Ok(Outcome::Failed(e))
        }
    }
}

/// After a crash: re-run every started-but-unacked entry with its stored payload. The executor
/// must itself be safe to repeat (e.g. check whether the event with that message id exists).
pub fn replay_unacked(
    ledger: &mut Ledger,
    now: i64,
    mut exec: impl FnMut(&str, &Value) -> Result<Value, String>,
) -> Vec<(String, Outcome)> {
    let mut out = vec![];
    for e in ledger.unacked() {
        let Some(payload) = e.payload.clone() else {
            let _ = ledger.fail(&e.id, "missing payload", now);
            out.push((e.id, Outcome::Failed("missing payload".into())));
            continue;
        };
        let id = e.id.clone();
        if let Ok(o) = finish(ledger, &id, &payload, now, |p| exec(&id, p)) {
            out.push((id, o));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;

    #[test]
    fn executes_once_then_replays() {
        let d = tempfile::tempdir().unwrap();
        let mut l = Ledger::open(d.path(), "t");
        let n = Cell::new(0);
        let run = |l: &mut Ledger| {
            run_once(l, "m1", &json!({"t": "hi"}), 1, |_| {
                n.set(n.get() + 1);
                Ok(json!(7))
            })
            .unwrap()
        };
        assert_eq!(run(&mut l), Outcome::Executed(json!(7)));
        assert_eq!(run(&mut l), Outcome::Replayed(json!(7)));
        assert_eq!(n.get(), 1);
    }

    #[test]
    fn crash_then_replay_is_idempotent() {
        let d = tempfile::tempdir().unwrap();
        {
            let mut l = Ledger::open(d.path(), "t");
            // crash: begin recorded, executor never finished
            l.begin("m2", &json!({"x": 1}), 1).unwrap();
        }
        let mut l = Ledger::open(d.path(), "t");
        let n = Cell::new(0);
        let res = replay_unacked(&mut l, 5, |_, p| {
            n.set(n.get() + 1);
            Ok(p.clone())
        });
        assert_eq!(res.len(), 1);
        let res2 = replay_unacked(&mut l, 6, |_, _| {
            n.set(n.get() + 1);
            Ok(json!(null))
        });
        assert!(res2.is_empty());
        assert_eq!(n.get(), 1);
        assert_eq!(l.get("m2").unwrap().result, Some(json!({"x": 1})));
    }

    #[test]
    fn failure_is_retryable() {
        let d = tempfile::tempdir().unwrap();
        let mut l = Ledger::open(d.path(), "t");
        let o = run_once(&mut l, "f", &json!(1), 1, |_| Err("no".into())).unwrap();
        assert_eq!(o, Outcome::Failed("no".into()));
        let o = run_once(&mut l, "f", &json!(1), 2, |_| Ok(json!(2))).unwrap();
        assert_eq!(o, Outcome::Executed(json!(2)));
    }
}
