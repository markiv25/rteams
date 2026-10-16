//! Tolerant reads for files that may be mid-write or damaged: bounded UTF-8 reads with retry on
//! transient faults, torn last lines of append-only logs, and rebuilding GUI state after a
//! corrupted file.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use serde::de::DeserializeOwned;

/// Backoff between retries of a transient read fault.
pub const RETRY_MS: [u64; 3] = [50, 200, 800];

fn transient(k: ErrorKind) -> bool {
    matches!(
        k,
        ErrorKind::Interrupted | ErrorKind::WouldBlock | ErrorKind::UnexpectedEof
    )
}

/// Run `op`, retrying transient errors with `RETRY_MS` backoff.
pub fn with_retry<T>(mut op: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut tries = RETRY_MS.iter();
    loop {
        match op() {
            Err(e) if transient(e.kind()) => match tries.next() {
                Some(ms) => sleep(Duration::from_millis(*ms)),
                None => return Err(e),
            },
            other => return other,
        }
    }
}

/// Read at most `max` bytes as UTF-8 (lossy at a cut multi-byte tail). Returns
/// `(text, truncated)`.
pub fn read_bounded_utf8(path: &Path, max: usize) -> std::io::Result<(String, bool)> {
    with_retry(|| {
        let f = std::fs::File::open(path)?;
        let mut buf = Vec::new();
        f.take(max as u64 + 1).read_to_end(&mut buf)?;
        let truncated = buf.len() > max;
        buf.truncate(max);
        Ok((String::from_utf8_lossy(&buf).into_owned(), truncated))
    })
}

/// Parse a JSON-lines text, tolerating a torn (unparsable, unterminated) last line. Unparsable
/// lines elsewhere are skipped and counted. Returns `(items, skipped)`.
pub fn parse_jsonl_tolerant<T: DeserializeOwned>(text: &str) -> (Vec<T>, usize) {
    let mut items = vec![];
    let mut skipped = 0;
    let terminated = text.ends_with('\n');
    let lines: Vec<&str> = text.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        if l.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<T>(l) {
            Ok(v) => items.push(v),
            Err(_) if i + 1 == lines.len() && !terminated => {} // torn tail: not counted
            Err(_) => skipped += 1,
        }
    }
    (items, skipped)
}

/// Read a JSON file; if missing -> `Ok(None)`; if corrupt, move it to `<name>.corrupt-<ts>`
/// and return `Err(path of the moved file)` so the caller can rebuild.
pub fn load_json_or_quarantine<T: DeserializeOwned>(
    path: &Path,
    now_ms: i64,
) -> Result<Option<T>, PathBuf> {
    let (text, _) = match read_bounded_utf8(path, 16 << 20) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(path.to_path_buf()),
    };
    match serde_json::from_str::<T>(&text) {
        Ok(v) => Ok(Some(v)),
        Err(_) => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let moved = path.with_file_name(format!("{name}.corrupt-{now_ms}"));
            let _ = std::fs::rename(path, &moved);
            Err(moved)
        }
    }
}

/// Rebuild read cursors after a corrupted GUI state file: every member is marked as having
/// read everything up to `max_seq`, so a rebuild never replays old messages as unread or
/// triggers wakes/notifications.
pub fn rebuild_read_cursors(members: &[String], max_seq: u64) -> BTreeMap<String, u64> {
    members.iter().map(|m| (m.clone(), max_seq)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn bounded_read_truncates() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        std::fs::write(&p, "hello world").unwrap();
        assert_eq!(
            read_bounded_utf8(&p, 100).unwrap(),
            ("hello world".into(), false)
        );
        assert_eq!(read_bounded_utf8(&p, 5).unwrap(), ("hello".into(), true));
        assert!(read_bounded_utf8(&d.path().join("no"), 5).is_err());
    }

    #[test]
    fn retry_only_transient() {
        let n = Cell::new(0);
        let r = with_retry(|| {
            n.set(n.get() + 1);
            if n.get() < 3 {
                Err(ErrorKind::Interrupted.into())
            } else {
                Ok(n.get())
            }
        });
        assert_eq!(r.unwrap(), 3);
        let m = Cell::new(0);
        let r: std::io::Result<()> = with_retry(|| {
            m.set(m.get() + 1);
            Err(ErrorKind::PermissionDenied.into())
        });
        assert!(r.is_err());
        assert_eq!(m.get(), 1);
    }

    #[test]
    fn torn_tail_is_tolerated() {
        let (v, skipped): (Vec<serde_json::Value>, _) =
            parse_jsonl_tolerant("{\"a\":1}\nbad\n{\"a\":2}\n{\"a\":");
        assert_eq!(v.len(), 2);
        assert_eq!(skipped, 1);
    }

    #[test]
    fn corrupt_json_is_quarantined() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("gui-state.json");
        assert!(matches!(
            load_json_or_quarantine::<BTreeMap<String, u64>>(&p, 1),
            Ok(None)
        ));
        std::fs::write(&p, "{broken").unwrap();
        let r = load_json_or_quarantine::<BTreeMap<String, u64>>(&p, 7);
        let moved = r.unwrap_err();
        assert!(moved.exists() && !p.exists());
        std::fs::write(&p, "{\"a\":3}").unwrap();
        assert_eq!(
            load_json_or_quarantine::<BTreeMap<String, u64>>(&p, 8)
                .unwrap()
                .unwrap()["a"],
            3
        );
        assert_eq!(rebuild_read_cursors(&["a".into()], 9)["a"], 9);
    }
}
