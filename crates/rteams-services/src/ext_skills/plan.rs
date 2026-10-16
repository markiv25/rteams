//! A plan describes what creating / updating / importing a skill folder will change on disk:
//! added, updated and removed files with unified diffs. Nothing is written until `apply`.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Updated,
    Removed,
    Unchanged,
}

#[derive(Debug, Clone)]
pub struct FileChange {
    pub rel: String,
    pub kind: ChangeKind,
    pub binary: bool,
    pub old: Option<Vec<u8>>,
    pub new: Option<Vec<u8>>,
    /// Unified diff for text files (empty for binary / unchanged).
    pub diff: String,
    pub added_lines: usize,
    pub removed_lines: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Counts {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub binary: usize,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub target: PathBuf,
    pub changes: Vec<FileChange>,
}

fn is_binary(b: &[u8]) -> bool {
    b.contains(&0) || std::str::from_utf8(b).is_err()
}

/// A relative path that stays inside its folder.
pub fn safe_rel(rel: &str) -> bool {
    !rel.is_empty()
        && Path::new(rel)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

fn read_existing(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>, depth: usize) {
        if depth > 8 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() {
                walk(base, &p, out, depth + 1);
            } else if ft.is_file() {
                if let (Ok(r), Ok(b)) = (p.strip_prefix(base), std::fs::read(&p)) {
                    out.insert(r.to_string_lossy().replace('\\', "/"), b);
                }
            }
        }
    }
    walk(dir, dir, &mut out, 0);
    out
}

fn unified(rel: &str, old: &str, new: &str) -> (String, usize, usize) {
    use similar::{ChangeTag, TextDiff};
    let d = TextDiff::from_lines(old, new);
    let (mut a, mut r) = (0, 0);
    for c in d.iter_all_changes() {
        match c.tag() {
            ChangeTag::Insert => a += 1,
            ChangeTag::Delete => r += 1,
            ChangeTag::Equal => {}
        }
    }
    let text = d
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{rel}"), &format!("b/{rel}"))
        .to_string();
    (text, a, r)
}

impl Plan {
    /// Compare `new_files` with what is in `target` now. With `remove_missing`, files that exist
    /// on disk but not in `new_files` are planned for removal.
    pub fn build(
        target: &Path,
        new_files: &BTreeMap<String, Vec<u8>>,
        remove_missing: bool,
    ) -> Plan {
        let existing = read_existing(target);
        let mut changes = vec![];
        for (rel, new) in new_files {
            let old = existing.get(rel);
            let binary = is_binary(new) || old.is_some_and(|o| is_binary(o));
            let (kind, diff, a, r) = match old {
                None => {
                    let (d, a, r) = if binary {
                        (String::new(), 0, 0)
                    } else {
                        unified(rel, "", &String::from_utf8_lossy(new))
                    };
                    (ChangeKind::Added, d, a, r)
                }
                Some(o) if o == new => (ChangeKind::Unchanged, String::new(), 0, 0),
                Some(o) => {
                    let (d, a, r) = if binary {
                        (String::new(), 0, 0)
                    } else {
                        unified(
                            rel,
                            &String::from_utf8_lossy(o),
                            &String::from_utf8_lossy(new),
                        )
                    };
                    (ChangeKind::Updated, d, a, r)
                }
            };
            changes.push(FileChange {
                rel: rel.clone(),
                kind,
                binary,
                old: old.cloned(),
                new: Some(new.clone()),
                diff,
                added_lines: a,
                removed_lines: r,
            });
        }
        if remove_missing {
            for (rel, old) in &existing {
                if !new_files.contains_key(rel) {
                    let binary = is_binary(old);
                    let (d, a, r) = if binary {
                        (String::new(), 0, 0)
                    } else {
                        unified(rel, &String::from_utf8_lossy(old), "")
                    };
                    changes.push(FileChange {
                        rel: rel.clone(),
                        kind: ChangeKind::Removed,
                        binary,
                        old: Some(old.clone()),
                        new: None,
                        diff: d,
                        added_lines: a,
                        removed_lines: r,
                    });
                }
            }
        }
        changes.sort_by(|a, b| a.rel.cmp(&b.rel));
        Plan {
            target: target.to_path_buf(),
            changes,
        }
    }

    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for ch in &self.changes {
            match ch.kind {
                ChangeKind::Added => c.added += 1,
                ChangeKind::Updated => c.updated += 1,
                ChangeKind::Removed => c.removed += 1,
                ChangeKind::Unchanged => {}
            }
            if ch.binary && ch.kind != ChangeKind::Unchanged {
                c.binary += 1;
            }
        }
        c
    }

    /// No file would change.
    pub fn is_noop(&self) -> bool {
        self.changes.iter().all(|c| c.kind == ChangeKind::Unchanged)
    }

    pub fn change_count(&self) -> usize {
        self.changes
            .iter()
            .filter(|c| c.kind != ChangeKind::Unchanged)
            .count()
    }

    /// Write the plan to disk (atomic per file) and remove files planned for removal.
    pub fn apply(&self) -> Result<(), String> {
        for c in &self.changes {
            if !safe_rel(&c.rel) {
                return Err(format!("unsafe path in plan: {}", c.rel));
            }
        }
        std::fs::create_dir_all(&self.target).map_err(|e| e.to_string())?;
        for c in &self.changes {
            let path = self.target.join(&c.rel);
            match c.kind {
                ChangeKind::Added | ChangeKind::Updated => {
                    if let Some(b) = &c.new {
                        crate::atomic::write(&path, b).map_err(|e| format!("{}: {e}", c.rel))?;
                    }
                }
                ChangeKind::Removed => {
                    let _ = std::fs::remove_file(&path);
                }
                ChangeKind::Unchanged => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(v: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
        v.iter()
            .map(|(k, c)| (k.to_string(), c.as_bytes().to_vec()))
            .collect()
    }

    #[test]
    fn diff_added_updated_removed_unchanged() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("s");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "a\nb\nc\n").unwrap();
        std::fs::write(dir.join("gone.txt"), "x\n").unwrap();
        std::fs::write(dir.join("same.txt"), "s\n").unwrap();
        let new = files(&[
            ("SKILL.md", "a\nB\nc\n"),
            ("new.txt", "n\n"),
            ("same.txt", "s\n"),
        ]);
        let p = Plan::build(&dir, &new, true);
        let k = |r: &str| p.changes.iter().find(|c| c.rel == r).unwrap();
        assert_eq!(k("SKILL.md").kind, ChangeKind::Updated);
        assert!(k("SKILL.md").diff.contains("-b") && k("SKILL.md").diff.contains("+B"));
        assert_eq!(
            (k("SKILL.md").added_lines, k("SKILL.md").removed_lines),
            (1, 1)
        );
        assert_eq!(k("new.txt").kind, ChangeKind::Added);
        assert_eq!(k("gone.txt").kind, ChangeKind::Removed);
        assert_eq!(k("same.txt").kind, ChangeKind::Unchanged);
        let c = p.counts();
        assert_eq!((c.added, c.updated, c.removed), (1, 1, 1));
        assert!(!p.is_noop());
        // nothing written yet
        assert_eq!(
            std::fs::read_to_string(dir.join("SKILL.md")).unwrap(),
            "a\nb\nc\n"
        );
        p.apply().unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("SKILL.md")).unwrap(),
            "a\nB\nc\n"
        );
        assert!(dir.join("new.txt").exists() && !dir.join("gone.txt").exists());
        assert!(Plan::build(&dir, &new, true).is_noop());
    }

    #[test]
    fn binary_files_have_no_diff_and_unsafe_paths_rejected() {
        let t = tempfile::tempdir().unwrap();
        let mut new = BTreeMap::new();
        new.insert("assets/a.png".to_string(), vec![0u8, 1, 2, 255]);
        let p = Plan::build(t.path(), &new, false);
        assert!(p.changes[0].binary && p.changes[0].diff.is_empty());
        assert_eq!(p.counts().binary, 1);
        let mut bad = BTreeMap::new();
        bad.insert("../evil".to_string(), vec![1]);
        assert!(Plan::build(t.path(), &bad, false).apply().is_err());
        assert!(!safe_rel("/abs") && !safe_rel("a/../b") && safe_rel("a/b.txt"));
    }
}
