//! External-change detection (mtime + content hash) and a line diff for the conflict dialog.

use std::path::Path;

use similar::{ChangeTag, TextDiff};

use super::files::{self, FileContent};

/// What the editor remembers about a buffer's on-disk base.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BaseStamp {
    pub mtime_ms: u64,
    pub hash: String,
}

impl From<&FileContent> for BaseStamp {
    fn from(c: &FileContent) -> Self {
        BaseStamp {
            mtime_ms: c.mtime_ms,
            hash: c.hash.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskState {
    Unchanged,
    /// Content differs from the base; `content` is the disk text.
    Changed {
        content: FileContent,
    },
    Deleted,
}

/// Compare disk with `base`. A newer mtime with the same hash (touch, save without changes) is
/// `Unchanged`; the hash is only computed when mtime moved.
pub fn check(path: &Path, base: &BaseStamp) -> DiskState {
    let Some(m) = files::mtime_ms(path) else {
        return DiskState::Deleted;
    };
    if m == base.mtime_ms {
        return DiskState::Unchanged;
    }
    match files::read_path(path) {
        Ok(c) if c.hash == base.hash => DiskState::Unchanged,
        Ok(c) => DiskState::Changed { content: c },
        Err(_) => DiskState::Deleted,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Same,
    /// Only in "mine" (the buffer).
    Mine,
    /// Only on disk.
    Disk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

/// Line diff of the buffer against the disk text. Long unchanged runs are collapsed to a
/// 3-line context so the dialog stays small.
pub fn diff(mine: &str, disk: &str) -> Vec<DiffLine> {
    let d = TextDiff::from_lines(mine, disk);
    let mut out = Vec::new();
    for group in d.grouped_ops(3) {
        if !out.is_empty() {
            out.push(DiffLine {
                kind: DiffKind::Same,
                text: "...".into(),
            });
        }
        for op in group {
            for ch in d.iter_changes(&op) {
                let kind = match ch.tag() {
                    ChangeTag::Equal => DiffKind::Same,
                    ChangeTag::Delete => DiffKind::Mine,
                    ChangeTag::Insert => DiffKind::Disk,
                };
                out.push(DiffLine {
                    kind,
                    text: ch.value().trim_end_matches('\n').to_string(),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detects_change_touch_and_delete() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f.txt");
        fs::write(&p, "one").unwrap();
        let base = BaseStamp::from(&files::read_path(&p).unwrap());
        assert_eq!(check(&p, &base), DiskState::Unchanged);
        // same content, different mtime
        let stale = BaseStamp {
            mtime_ms: base.mtime_ms.saturating_sub(5000),
            ..base.clone()
        };
        assert_eq!(check(&p, &stale), DiskState::Unchanged);
        fs::write(&p, "two").unwrap();
        let stale = BaseStamp {
            mtime_ms: base.mtime_ms.saturating_sub(5000),
            ..base.clone()
        };
        assert!(
            matches!(check(&p, &stale), DiskState::Changed { content } if content.text == "two")
        );
        fs::remove_file(&p).unwrap();
        assert_eq!(check(&p, &base), DiskState::Deleted);
    }

    #[test]
    fn diff_marks_sides() {
        let v = diff("a\nb\nc\n", "a\nB\nc\n");
        assert!(v.contains(&DiffLine {
            kind: DiffKind::Mine,
            text: "b".into()
        }));
        assert!(v.contains(&DiffLine {
            kind: DiffKind::Disk,
            text: "B".into()
        }));
    }
}
