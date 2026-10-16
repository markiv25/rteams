use crate::lifecycle::Status;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub name: String,
    pub role: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Team {
    pub name: String,
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub owner: Option<String>,
    pub status: Status,
    /// Tasks that should be done first. Only a relation: nothing transitions automatically.
    /// The inverse ("blocks") is derived, see `State::dependents`.
    pub blocked_by: Vec<String>,
    /// Set by the last review request.
    pub reviewer: Option<String>,
    /// While `Blocked`: the status to return to on unblock.
    pub blocked_from: Option<Status>,
    pub blocked_reason: Option<String>,
    /// The only deletion mechanism: hidden from every projection.
    pub archived: bool,
    /// Seq of the creating event; gives stable ordering.
    pub created_seq: u64,
}

/// Where a message lives. Task comments, direct messages and broadcasts are one stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum Scope {
    Task { task: String },
    Direct { to: String },
    Broadcast,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Seq of the event that produced it; doubles as the inbox cursor.
    pub seq: u64,
    pub ts: i64,
    pub from: String,
    #[serde(flatten)]
    pub scope: Scope,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessEntry {
    pub pid: u32,
    pub label: String,
    pub owner: String,
    pub port: Option<u16>,
    pub url: Option<String>,
    pub command: Option<String>,
    pub running: bool,
}
