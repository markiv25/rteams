//! Agenda fingerprint (shared with `api`, so agent acks match scheduler observations). Subjects
//! and descriptions are excluded so cosmetic edits never count as a change.

use super::agenda::AgendaItem;

pub fn fingerprint(member: &str, items: &[AgendaItem]) -> String {
    let rows: Vec<_> = items
        .iter()
        .map(|i| (i.task.clone(), i.status.clone(), i.reason.clone()))
        .collect();
    super::api::fingerprint_rows(member, &rows)
}
