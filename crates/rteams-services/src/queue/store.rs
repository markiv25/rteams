//! Persistent queue with enqueue / edit / reorder / cancel.

use std::path::{Path, PathBuf};

use super::model::*;

#[derive(Debug)]
pub struct Queue {
    path: PathBuf,
    pub file: QueueFile,
}

pub fn queue_path(home: &Path, team: &str) -> PathBuf {
    home.join(team).join("queue.json")
}

impl Queue {
    /// Load; a missing or corrupt file yields an empty queue (the corrupt one is kept aside).
    pub fn open(home: &Path, team: &str) -> Queue {
        let path = queue_path(home, team);
        let file = match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice::<QueueFile>(&b).unwrap_or_else(|_| {
                let _ = std::fs::rename(&path, path.with_extension("json.corrupt"));
                QueueFile::default()
            }),
            Err(_) => QueueFile::default(),
        };
        Queue { path, file }
    }

    /// Re-read from disk (another process may have edited).
    pub fn reload(&mut self) {
        if let Ok(b) = std::fs::read(&self.path) {
            if let Ok(f) = serde_json::from_slice::<QueueFile>(&b) {
                self.file = f;
            }
        }
    }

    pub fn save(&self) -> Result<(), QueueError> {
        let bytes =
            serde_json::to_vec_pretty(&self.file).map_err(|e| QueueError::Io(e.to_string()))?;
        crate::atomic::write(&self.path, &bytes).map_err(|e| QueueError::Io(e.to_string()))
    }

    pub fn len(&self) -> usize {
        self.file.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.file.items.is_empty()
    }

    /// Items in send order.
    pub fn items(&self) -> Vec<&QueueItem> {
        let mut v: Vec<&QueueItem> = self.file.items.iter().collect();
        v.sort_by_key(|i| i.position);
        v
    }

    pub fn for_member<'a>(&'a self, to: Option<&str>) -> Vec<&'a QueueItem> {
        self.items()
            .into_iter()
            .filter(|i| i.to.as_deref() == to)
            .collect()
    }

    pub fn enqueue(
        &mut self,
        to: Option<&str>,
        text: &str,
        mode: Option<&str>,
        now: i64,
    ) -> Result<QueueItem, QueueError> {
        let text = validate_text(text, MAX_TEXT)?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let item = QueueItem {
            message_id: format!("q-{id}"),
            id,
            revision: 1,
            position: self.file.next_position,
            created_at: now,
            to: to.map(str::to_string),
            text,
            mode: mode.map(str::to_string),
            state: QueueState::Queued,
            attempt_at: None,
        };
        self.file.next_position += 1;
        self.file.items.push(item.clone());
        self.save()?;
        Ok(item)
    }

    fn idx(&self, id: &str) -> Result<usize, QueueError> {
        self.file
            .items
            .iter()
            .position(|i| i.id == id)
            .ok_or(QueueError::NotFound)
    }

    pub fn edit(&mut self, id: &str, text: &str, expected_revision: u32) -> Result<(), QueueError> {
        let text = validate_text(text, MAX_TEXT)?;
        let i = self.idx(id)?;
        let it = &mut self.file.items[i];
        if it.state != QueueState::Queued {
            return Err(QueueError::Busy);
        }
        if it.revision != expected_revision {
            return Err(QueueError::Conflict);
        }
        it.text = text;
        it.revision += 1;
        self.save()
    }

    pub fn cancel(&mut self, id: &str) -> Result<QueueItem, QueueError> {
        let i = self.idx(id)?;
        if self.file.items[i].state == QueueState::Sending {
            return Err(QueueError::Busy);
        }
        let it = self.file.items.remove(i);
        self.save()?;
        Ok(it)
    }

    /// Drop every queued (not sending) item for a recipient; returns how many.
    pub fn discard_for(&mut self, to: Option<&str>) -> Result<usize, QueueError> {
        let before = self.file.items.len();
        self.file
            .items
            .retain(|i| i.to.as_deref() != to || i.state == QueueState::Sending);
        let n = before - self.file.items.len();
        if n > 0 {
            self.save()?;
        }
        Ok(n)
    }

    /// Move one step earlier (`up`) or later among items of the same recipient.
    pub fn move_item(&mut self, id: &str, up: bool) -> Result<(), QueueError> {
        let i = self.idx(id)?;
        let to = self.file.items[i].to.clone();
        let pos = self.file.items[i].position;
        let neighbour = self
            .file
            .items
            .iter()
            .enumerate()
            .filter(|(_, o)| o.to == to && o.state == QueueState::Queued)
            .filter(|(_, o)| {
                if up {
                    o.position < pos
                } else {
                    o.position > pos
                }
            })
            .min_by_key(|(_, o)| {
                if up {
                    u64::MAX - o.position
                } else {
                    o.position
                }
            })
            .map(|(j, _)| j);
        let Some(j) = neighbour else { return Ok(()) };
        let (a, b) = (self.file.items[i].position, self.file.items[j].position);
        self.file.items[i].position = b;
        self.file.items[j].position = a;
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_edit_cancel_persist() {
        let d = tempfile::tempdir().unwrap();
        let mut q = Queue::open(d.path(), "t");
        let a = q.enqueue(Some("ada"), " one ", Some("do"), 1).unwrap();
        let b = q.enqueue(Some("ada"), "two", None, 2).unwrap();
        assert!(q.enqueue(Some("ada"), "  ", None, 3).is_err());
        q.edit(&a.id, "uno", 1).unwrap();
        assert_eq!(q.edit(&a.id, "x", 1), Err(QueueError::Conflict));
        q.cancel(&b.id).unwrap();
        let q2 = Queue::open(d.path(), "t");
        assert_eq!(q2.len(), 1);
        assert_eq!(q2.items()[0].text, "uno");
        assert_eq!(q2.items()[0].revision, 2);
    }

    #[test]
    fn reorder_only_within_recipient() {
        let d = tempfile::tempdir().unwrap();
        let mut q = Queue::open(d.path(), "t");
        let a = q.enqueue(Some("ada"), "a", None, 1).unwrap();
        let _x = q.enqueue(Some("bob"), "x", None, 2).unwrap();
        let b = q.enqueue(Some("ada"), "b", None, 3).unwrap();
        q.move_item(&b.id, true).unwrap();
        let texts: Vec<_> = q
            .for_member(Some("ada"))
            .iter()
            .map(|i| i.text.clone())
            .collect();
        assert_eq!(texts, ["b", "a"]);
        q.move_item(&b.id, true).unwrap(); // already first: no-op
        q.move_item(&a.id, false).unwrap(); // already last: no-op
        assert_eq!(q.for_member(Some("ada"))[0].text, "b");
    }

    #[test]
    fn corrupt_file_is_set_aside() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("t")).unwrap();
        std::fs::write(queue_path(d.path(), "t"), b"{nope").unwrap();
        let q = Queue::open(d.path(), "t");
        assert!(q.is_empty());
        assert!(d.path().join("t/queue.json.corrupt").exists());
    }

    #[test]
    fn discard_keeps_sending() {
        let d = tempfile::tempdir().unwrap();
        let mut q = Queue::open(d.path(), "t");
        let a = q.enqueue(Some("ada"), "a", None, 1).unwrap();
        q.enqueue(Some("ada"), "b", None, 1).unwrap();
        q.file
            .items
            .iter_mut()
            .find(|i| i.id == a.id)
            .unwrap()
            .state = QueueState::Sending;
        assert_eq!(q.discard_for(Some("ada")).unwrap(), 1);
        assert_eq!(q.len(), 1);
    }
}
