use crate::lifecycle::Status;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub name: String,
    pub role: String,
    /// Standing instructions for this member.
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Soft removal: kept so history renders, but cannot act or be assigned.
    #[serde(default)]
    pub removed: bool,
    /// AI runtime id: "claude" (default when absent), "codex", "opencode", "cursor", "gemini", ...
    #[serde(default)]
    pub runtime: Option<String>,
    /// Works in its own git worktree.
    #[serde(default)]
    pub worktree: bool,
    /// Avatar icon (an emoji) picked for this member; `None` uses the GUI's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Reasoning effort: low | medium | high.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Fast mode.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fast: bool,
    /// Context window limit (tokens) for this member's runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_limit: Option<u32>,
    /// Extra MCP server names this member may use.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_allow: Vec<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A user-created subset chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupChat {
    pub id: String,
    pub name: String,
    pub members: Vec<String>,
    #[serde(default)]
    pub created_by: String,
}

/// Last heartbeat reported by a member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeartbeatInfo {
    /// starting | ready | working | idle | stuck
    pub status: String,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    pub ts: i64,
}

/// A recurring prompt for a member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub id: String,
    pub member: String,
    pub prompt: String,
    pub every_minutes: u32,
    pub enabled: bool,
    /// Cron expression (5 fields); when set it wins over `every_minutes`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    /// IANA timezone for `cron` (default: local time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Minutes before a fire at which the runtime is probed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warmup_minutes: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Team {
    pub name: String,
    pub members: Vec<Member>,
    #[serde(default)]
    pub project_path: Option<String>,
    /// Standing instructions from the lead, shown to every member.
    #[serde(default)]
    pub lead_prompt: Option<String>,
    /// Soft delete of the whole team: while set, every write except un-deleting is rejected.
    #[serde(default)]
    pub deleted: bool,
    /// Manual kanban order per column (status in snake_case -> task ids).
    #[serde(default)]
    pub column_order: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub schedules: Vec<Schedule>,
    #[serde(default)]
    pub group_chats: Vec<GroupChat>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reaction {
    pub from: String,
    pub emoji: String,
}

/// A tool-approval request from an agent (`--permission-prompt-tool`), decided by a lead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub id: String,
    pub member: String,
    pub tool: String,
    pub input: serde_json::Value,
    pub decided: Option<bool>,
    pub note: Option<String>,
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
    /// Extra reviewers besides `reviewer`.
    #[serde(default)]
    pub reviewers: Vec<String>,
    /// While `Blocked`: the status to return to on unblock.
    pub blocked_from: Option<Status>,
    pub blocked_reason: Option<String>,
    /// The only deletion mechanism: hidden from every projection.
    pub archived: bool,
    /// Seq of the creating event; gives stable ordering.
    pub created_seq: u64,
    /// A question to the user/lead; the task waits for an answer while this is set.
    #[serde(default)]
    pub clarification: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// Symmetric "see also" links to other task ids.
    #[serde(default)]
    pub related: Vec<String>,
    /// The reviewer has picked the task up (reset on every new review request).
    #[serde(default)]
    pub review_started: bool,
}

/// Where a message lives. Task comments, direct messages and broadcasts are one stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum Scope {
    Task {
        task: String,
    },
    Direct {
        to: String,
    },
    Broadcast,
    /// A user-created subset group chat.
    Group {
        chat: String,
    },
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
    /// Intent of a direct message / broadcast: do, ask or delegate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Sender's team for a cross-team message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_team: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<Reaction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
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
