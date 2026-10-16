//! Queued-message model. Persisted in `<home>/<team>/queue.json`.

use serde::{Deserialize, Serialize};

pub const MAX_TEXT: usize = 8_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueState {
    Queued,
    Sending,
    /// The app died (or the send result was lost) while `Sending`.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: String,
    /// Bumped on every edit (optimistic concurrency for the UI).
    pub revision: u32,
    pub position: u64,
    pub created_at: i64,
    /// Recipient member; `None` = group chat (broadcast).
    pub to: Option<String>,
    pub text: String,
    /// `do` / `ask` / `delegate`.
    pub mode: Option<String>,
    pub state: QueueState,
    /// Stable id of the message this item will become; reused on every attempt.
    pub message_id: String,
    pub attempt_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueFile {
    pub version: u32,
    pub next_position: u64,
    pub items: Vec<QueueItem>,
}

impl Default for QueueFile {
    fn default() -> Self {
        QueueFile {
            version: 1,
            next_position: 1,
            items: vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QueueError {
    #[error("message is empty")]
    Empty,
    #[error("message is longer than {0} characters")]
    TooLong(usize),
    #[error("queued message not found")]
    NotFound,
    #[error("queued message changed; reload")]
    Conflict,
    #[error("message is being sent")]
    Busy,
    #[error("queue io: {0}")]
    Io(String),
}

/// Trim and bound-check message text.
pub fn validate_text(text: &str, max: usize) -> Result<String, QueueError> {
    let t = text.trim();
    if t.is_empty() {
        return Err(QueueError::Empty);
    }
    if t.chars().count() > max {
        return Err(QueueError::TooLong(max));
    }
    Ok(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation() {
        assert_eq!(validate_text("  hi \n", 10).unwrap(), "hi");
        assert_eq!(validate_text("  ", 10), Err(QueueError::Empty));
        assert_eq!(validate_text("abcdef", 3), Err(QueueError::TooLong(3)));
    }
}
