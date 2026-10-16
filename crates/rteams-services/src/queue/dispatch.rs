//! Dispatch rule: when a recipient is idle, its next queued item goes out (one per tick per
//! recipient, in position order). The shell sends it as `GuiCmd::Message` carrying
//! `message_id`, then calls `ack` (or `nack` if the send failed).

use rteams_core::{Scope, State};

use super::model::*;
use super::store::Queue;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub id: String,
    pub message_id: String,
    pub to: Option<String>,
    pub text: String,
    pub mode: Option<String>,
}

fn outgoing(i: &QueueItem) -> Outgoing {
    Outgoing {
        id: i.id.clone(),
        message_id: i.message_id.clone(),
        to: i.to.clone(),
        text: i.text.clone(),
        mode: i.mode.clone(),
    }
}

/// Take the next item for each idle recipient and mark it `Sending`. Group-chat items
/// (`to == None`) go out whenever `group_ready` (normally always true: a broadcast does not
/// need an idle agent). Persists before returning, so a crash leaves `Sending` (-> `Unknown`).
pub fn take_due(
    q: &mut Queue,
    idle_members: &[String],
    group_ready: bool,
    now: i64,
) -> Result<Vec<Outgoing>, QueueError> {
    let mut out = vec![];
    let mut seen: Vec<Option<String>> = vec![];
    let order: Vec<(String, Option<String>)> = q
        .items()
        .iter()
        .filter(|i| i.state == QueueState::Queued)
        .map(|i| (i.id.clone(), i.to.clone()))
        .collect();
    // A recipient with an item already sending is skipped until it is acked.
    let busy: Vec<Option<String>> = q
        .file
        .items
        .iter()
        .filter(|i| i.state != QueueState::Queued)
        .map(|i| i.to.clone())
        .collect();
    for (id, to) in order {
        if seen.contains(&to) || busy.contains(&to) {
            continue;
        }
        let ready = match &to {
            Some(m) => idle_members.iter().any(|x| x == m),
            None => group_ready,
        };
        if !ready {
            continue;
        }
        seen.push(to);
        if let Some(it) = q.file.items.iter_mut().find(|i| i.id == id) {
            it.state = QueueState::Sending;
            it.attempt_at = Some(now);
            out.push(outgoing(it));
        }
    }
    if !out.is_empty() {
        q.save()?;
    }
    Ok(out)
}

/// "Send now" on one item regardless of the recipient's state.
pub fn take_now(q: &mut Queue, id: &str, now: i64) -> Result<Outgoing, QueueError> {
    let it = q
        .file
        .items
        .iter_mut()
        .find(|i| i.id == id)
        .ok_or(QueueError::NotFound)?;
    if it.state == QueueState::Sending {
        return Err(QueueError::Busy);
    }
    it.state = QueueState::Sending;
    it.attempt_at = Some(now);
    let o = outgoing(it);
    q.save()?;
    Ok(o)
}

/// The message was written: drop the item.
pub fn ack(q: &mut Queue, id: &str) -> Result<(), QueueError> {
    q.file.items.retain(|i| i.id != id);
    q.save()
}

/// The send failed: back to the queue.
pub fn nack(q: &mut Queue, id: &str) -> Result<(), QueueError> {
    if let Some(it) = q.file.items.iter_mut().find(|i| i.id == id) {
        it.state = QueueState::Queued;
    }
    q.save()
}

/// On startup: `Sending` items of a dead process become `Unknown`, then each is reconciled:
/// `already_sent(item)` true -> dropped (the message exists), false -> re-queued. Returns
/// (dropped, requeued).
pub fn reconcile(
    q: &mut Queue,
    already_sent: impl Fn(&QueueItem) -> bool,
) -> Result<(usize, usize), QueueError> {
    let (mut dropped, mut requeued) = (0, 0);
    for it in q.file.items.iter_mut() {
        if it.state == QueueState::Sending {
            it.state = QueueState::Unknown;
        }
    }
    let mut keep = vec![];
    for mut it in std::mem::take(&mut q.file.items) {
        if it.state == QueueState::Unknown {
            if already_sent(&it) {
                dropped += 1;
                continue;
            }
            it.state = QueueState::Queued;
            requeued += 1;
        }
        keep.push(it);
    }
    q.file.items = keep;
    if dropped + requeued > 0 {
        q.save()?;
    }
    Ok((dropped, requeued))
}

/// Heuristic used when events carry no message id: a message from `from` with the same text
/// and recipient exists at or after the attempt time.
pub fn sent_in_state(state: &State, from: &str, item: &QueueItem) -> bool {
    let Some(at) = item.attempt_at else {
        return false;
    };
    state.messages.iter().any(|m| {
        m.from == from
            && m.text.trim() == item.text
            && m.ts >= at
            && match (&m.scope, &item.to) {
                (Scope::Direct { to }, Some(t)) => to == t,
                (Scope::Broadcast, None) => true,
                _ => false,
            }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q() -> (tempfile::TempDir, Queue) {
        let d = tempfile::tempdir().unwrap();
        let q = Queue::open(d.path(), "t");
        (d, q)
    }

    #[test]
    fn dispatch_order_one_per_idle_member() {
        let (_d, mut q) = q();
        q.enqueue(Some("ada"), "first", None, 1).unwrap();
        q.enqueue(Some("bob"), "bob1", None, 2).unwrap();
        q.enqueue(Some("ada"), "second", None, 3).unwrap();
        let out = take_due(&mut q, &["ada".into()], false, 10).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "first");
        // second is held until the first is acked
        assert!(take_due(&mut q, &["ada".into()], false, 11)
            .unwrap()
            .is_empty());
        ack(&mut q, &out[0].id).unwrap();
        let out = take_due(&mut q, &["ada".into(), "bob".into()], false, 12).unwrap();
        let texts: Vec<_> = out.iter().map(|o| o.text.as_str()).collect();
        assert_eq!(texts, ["bob1", "second"]);
    }

    #[test]
    fn busy_member_is_not_dispatched() {
        let (_d, mut q) = q();
        q.enqueue(Some("ada"), "x", None, 1).unwrap();
        assert!(take_due(&mut q, &[], true, 1).unwrap().is_empty());
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn nack_requeues_and_keeps_message_id() {
        let (_d, mut q) = q();
        q.enqueue(Some("ada"), "x", None, 1).unwrap();
        let o = take_due(&mut q, &["ada".into()], false, 1)
            .unwrap()
            .remove(0);
        nack(&mut q, &o.id).unwrap();
        let o2 = take_due(&mut q, &["ada".into()], false, 2)
            .unwrap()
            .remove(0);
        assert_eq!(o.message_id, o2.message_id);
    }

    #[test]
    fn reconcile_after_crash() {
        let d = tempfile::tempdir().unwrap();
        {
            let mut q = Queue::open(d.path(), "t");
            q.enqueue(Some("ada"), "sent", None, 1).unwrap();
            q.enqueue(Some("bob"), "lost", None, 1).unwrap();
            take_due(&mut q, &["ada".into(), "bob".into()], false, 5).unwrap();
        }
        let mut q = Queue::open(d.path(), "t");
        let (dropped, requeued) = reconcile(&mut q, |i| i.text == "sent").unwrap();
        assert_eq!((dropped, requeued), (1, 1));
        assert_eq!(q.len(), 1);
        assert_eq!(q.items()[0].state, QueueState::Queued);
    }
}
