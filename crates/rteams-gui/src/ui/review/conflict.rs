//! Conflict detection for review decisions: the file changed on disk since the diff was
//! loaded, the hunk no longer applies, or a saved decision no longer matches the diff.

use std::collections::BTreeMap;

use super::store::{hunk_id, Decision, ReviewState};
use crate::git::FileDiff;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The file's content differs from when the diff was loaded.
    FileChanged,
    /// The hunk can no longer be reverse-applied.
    NoLongerApplies,
    /// A saved decision that does not match the current diff.
    SavedStale,
    /// Restoring onto a file that changed since the recorded mutation.
    RestoreChanged,
}

/// What "keep mine" would carry out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    RejectHunk {
        key: String,
    },
    RejectFile,
    SaveEdit {
        text: String,
    },
    Restore {
        id: String,
    },
    /// Keep a saved decision as it is.
    Nothing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub file: String,
    pub reason: Reason,
    pub pending: Pending,
}

/// Pure: has the file changed since it was hashed at load time?
pub fn changed_since(loaded: Option<u64>, now: Option<u64>) -> bool {
    loaded != now
}

/// Saved decisions that no longer line up with `files` (the current diff).
/// An accepted hunk that vanished, or a rejected hunk that is back, is a candidate.
pub fn stale_decisions(state: &ReviewState, files: &[FileDiff]) -> Vec<Candidate> {
    let mut present: BTreeMap<String, (&str, &str)> = BTreeMap::new();
    let keys: Vec<(String, String)> = files
        .iter()
        .flat_map(|f| f.hunks.iter().map(|h| (f.path.clone(), h.key())))
        .collect();
    for (p, k) in &keys {
        present.insert(hunk_id(p, k), (p, k));
    }
    let mut out = Vec::new();
    for (id, d) in &state.hunks {
        let Some((file, key)) = id.rsplit_once('#') else {
            continue;
        };
        let here = present.contains_key(id);
        match d {
            Decision::Accepted if !here => out.push(Candidate {
                file: file.to_string(),
                reason: Reason::SavedStale,
                pending: Pending::Nothing,
            }),
            Decision::Rejected if here => out.push(Candidate {
                file: file.to_string(),
                reason: Reason::SavedStale,
                pending: Pending::RejectHunk {
                    key: key.to_string(),
                },
            }),
            _ => {}
        }
    }
    out
}

/// The saved-decision ids behind stale candidates (to drop on "take theirs").
pub fn stale_ids(state: &ReviewState, files: &[FileDiff]) -> Vec<String> {
    let here: std::collections::BTreeSet<String> = files
        .iter()
        .flat_map(|f| f.hunks.iter().map(|h| hunk_id(&f.path, &h.key())))
        .collect();
    state
        .hunks
        .iter()
        .filter(|(id, d)| match d {
            Decision::Accepted => !here.contains(*id),
            Decision::Rejected => here.contains(*id),
        })
        .map(|(id, _)| id.clone())
        .collect()
}
