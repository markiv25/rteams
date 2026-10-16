//! JSONL parsing and chunk building. Claude Code, rteams runs (Claude stream-json) and Codex
//! rollouts are normalised into `Msg`s; a chunk is one user turn plus the AI response group
//! that followed it (steps: text, thinking, tool calls, subagents).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::paths::{session_id_of, Source};
use super::steps::{close_steps, Step, StepKind};
use super::subagents::{self, Subagent};
use super::tools::ToolExec;
use crate::usage::pricing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    /// Bookkeeping records (init, result, token counts); never shown as messages.
    Meta,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    pub fn add(&mut self, o: &Usage) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
    }

    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text(String),
    Thinking(String),
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        id: String,
        content: String,
        is_error: bool,
    },
}

/// One normalised transcript record.
#[derive(Debug, Clone)]
pub struct Msg {
    pub role: Role,
    pub uuid: String,
    pub parent: Option<String>,
    pub ts_ms: Option<i64>,
    pub sidechain: bool,
    /// Injected context (meta, command output, environment), not something the user typed.
    pub synthetic: bool,
    pub model: String,
    /// Provider message id; repeated records of one message share it (usage counted once).
    pub msg_id: Option<String>,
    pub usage: Option<Usage>,
    pub blocks: Vec<Block>,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub session_id: Option<String>,
    /// Subagent id reported next to a tool result.
    pub agent_id: Option<String>,
    pub reported_cost: Option<f64>,
    pub duration_ms: Option<u64>,
}

impl Msg {
    fn new(role: Role) -> Msg {
        Msg {
            role,
            uuid: String::new(),
            parent: None,
            ts_ms: None,
            sidechain: false,
            synthetic: false,
            model: String::new(),
            msg_id: None,
            usage: None,
            blocks: Vec::new(),
            cwd: None,
            branch: None,
            session_id: None,
            agent_id: None,
            reported_cost: None,
            duration_ms: None,
        }
    }

    /// Text blocks joined.
    pub fn text(&self) -> String {
        let v: Vec<&str> = self
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        v.join("\n")
    }

    /// A user turn that starts a chunk.
    pub fn is_prompt(&self) -> bool {
        self.role == Role::User
            && !self.sidechain
            && !self.synthetic
            && self
                .blocks
                .iter()
                .any(|b| matches!(b, Block::Text(t) if !t.trim().is_empty()))
    }
}

pub fn parse_ts(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis())
}

fn ts_of(v: &Value) -> Option<i64> {
    match v.get("timestamp")? {
        Value::String(s) => parse_ts(s),
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

fn str_of(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

/// Tool result payload as text: a string, or the `text` parts of an array.
fn result_text(c: &Value) -> String {
    match c {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .map(|p| match p.get("type").and_then(Value::as_str) {
                Some("text") => p
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                Some("image") => "[image]".to_string(),
                _ => p.as_str().map(str::to_string).unwrap_or_default(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn is_synthetic_text(t: &str) -> bool {
    let t = t.trim_start();
    t.starts_with("<command-name>")
        || t.starts_with("<command-message>")
        || t.starts_with("<local-command")
        || t.starts_with("<system-reminder>")
        || t.starts_with("<environment_context>")
        || t.starts_with("<user_instructions>")
        || t.starts_with("Caveat: The messages below")
}

fn claude_blocks(content: &Value) -> Vec<Block> {
    match content {
        Value::String(s) => vec![Block::Text(s.clone())],
        Value::Array(a) => a
            .iter()
            .filter_map(|b| match b.get("type").and_then(Value::as_str)? {
                "text" => Some(Block::Text(str_of(b, "text")?)),
                "thinking" => Some(Block::Thinking(str_of(b, "thinking").unwrap_or_default())),
                "tool_use" => Some(Block::ToolUse {
                    id: str_of(b, "id").unwrap_or_default(),
                    name: str_of(b, "name").unwrap_or_default(),
                    input: b.get("input").cloned().unwrap_or(Value::Null),
                }),
                "tool_result" => Some(Block::ToolResult {
                    id: str_of(b, "tool_use_id").unwrap_or_default(),
                    content: result_text(b.get("content").unwrap_or(&Value::Null)),
                    is_error: b.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                }),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn claude_usage(u: &Value) -> Usage {
    let g = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    Usage {
        input: g("input_tokens"),
        output: g("output_tokens"),
        cache_read: g("cache_read_input_tokens"),
        cache_write: g("cache_creation_input_tokens"),
    }
}

/// Line-by-line normaliser (Codex needs state across lines).
pub struct Normalizer {
    source: Source,
    model: String,
    last_total: Option<u64>,
    n: u64,
}

impl Normalizer {
    pub fn new(source: Source) -> Normalizer {
        Normalizer {
            source,
            model: String::new(),
            last_total: None,
            n: 0,
        }
    }

    /// Records for one JSONL line (empty for lines that are not transcript records).
    pub fn push(&mut self, line: &str) -> Vec<Msg> {
        let line = line.trim();
        if line.is_empty() {
            return Vec::new();
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        match self.source {
            Source::Codex => self.codex(&v),
            _ => claude(&v).into_iter().collect(),
        }
    }

    fn id(&mut self) -> String {
        self.n += 1;
        format!("c{}", self.n)
    }

    fn codex(&mut self, v: &Value) -> Vec<Msg> {
        let ts = ts_of(v);
        let p = v.get("payload").unwrap_or(&Value::Null);
        let pt = p.get("type").and_then(Value::as_str).unwrap_or("");
        let mut out = Vec::new();
        match v.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                let mut m = Msg::new(Role::Meta);
                m.ts_ms = ts;
                m.session_id = str_of(p, "id");
                m.cwd = str_of(p, "cwd");
                m.branch = p
                    .pointer("/git/branch")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                out.push(m);
            }
            Some("turn_context") => {
                if let Some(m) = str_of(p, "model") {
                    self.model = m;
                }
                let mut m = Msg::new(Role::Meta);
                m.ts_ms = ts;
                m.cwd = str_of(p, "cwd");
                m.model = self.model.clone();
                out.push(m);
            }
            Some("event_msg") if pt == "token_count" => {
                let Some(info) = p.get("info").filter(|i| !i.is_null()) else {
                    return out;
                };
                let total = info
                    .pointer("/total_token_usage/total_tokens")
                    .and_then(Value::as_u64);
                if total.is_some() && total == self.last_total {
                    return out;
                }
                self.last_total = total;
                let g = |k: &str| {
                    info.pointer(&format!("/last_token_usage/{k}"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                };
                let cached = g("cached_input_tokens");
                let mut m = Msg::new(Role::Meta);
                m.ts_ms = ts;
                m.model = self.model.clone();
                m.uuid = self.id();
                m.usage = Some(Usage {
                    input: g("input_tokens").saturating_sub(cached),
                    output: g("output_tokens"),
                    cache_read: cached,
                    cache_write: 0,
                });
                out.push(m);
            }
            Some("response_item") => match pt {
                "message" => {
                    let role = match p.get("role").and_then(Value::as_str) {
                        Some("user") => Role::User,
                        Some("assistant") => Role::Assistant,
                        _ => return out,
                    };
                    let text: Vec<String> = p
                        .get("content")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|c| c.get("text").and_then(Value::as_str))
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default();
                    let text = text.join("\n");
                    let mut m = Msg::new(role);
                    m.uuid = self.id();
                    m.ts_ms = ts;
                    m.model = self.model.clone();
                    m.synthetic = role == Role::User && is_synthetic_text(&text);
                    m.blocks = vec![Block::Text(text)];
                    out.push(m);
                }
                "reasoning" => {
                    let text: Vec<&str> = p
                        .get("summary")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|c| c.get("text").and_then(Value::as_str))
                                .collect()
                        })
                        .unwrap_or_default();
                    if !text.is_empty() {
                        let mut m = Msg::new(Role::Assistant);
                        m.uuid = self.id();
                        m.ts_ms = ts;
                        m.model = self.model.clone();
                        m.blocks = vec![Block::Thinking(text.join("\n"))];
                        out.push(m);
                    }
                }
                "function_call" | "custom_tool_call" | "local_shell_call" => {
                    let id = str_of(p, "call_id").unwrap_or_else(|| self.id());
                    let name = str_of(p, "name").unwrap_or_else(|| "shell".into());
                    let input = match p.get("arguments").or_else(|| p.get("input")) {
                        Some(Value::String(s)) => serde_json::from_str(s)
                            .unwrap_or_else(|_| serde_json::json!({ "input": s })),
                        Some(o) => o.clone(),
                        None => p.get("action").cloned().unwrap_or(Value::Null),
                    };
                    let mut m = Msg::new(Role::Assistant);
                    m.uuid = self.id();
                    m.ts_ms = ts;
                    m.model = self.model.clone();
                    m.blocks = vec![Block::ToolUse { id, name, input }];
                    out.push(m);
                }
                "function_call_output" | "custom_tool_call_output" => {
                    let id = str_of(p, "call_id").unwrap_or_default();
                    let raw = match p.get("output") {
                        Some(Value::String(s)) => s.clone(),
                        Some(o) => o.to_string(),
                        None => String::new(),
                    };
                    let (content, is_error) = match serde_json::from_str::<Value>(&raw) {
                        Ok(j) if j.is_object() => {
                            let text = j
                                .get("output")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                .unwrap_or_else(|| raw.clone());
                            let bad = j
                                .pointer("/metadata/exit_code")
                                .and_then(Value::as_i64)
                                .is_some_and(|c| c != 0);
                            (text, bad)
                        }
                        _ => (raw, false),
                    };
                    let mut m = Msg::new(Role::User);
                    m.uuid = self.id();
                    m.ts_ms = ts;
                    m.synthetic = true;
                    m.blocks = vec![Block::ToolResult {
                        id,
                        content,
                        is_error,
                    }];
                    out.push(m);
                }
                _ => {}
            },
            _ => {}
        }
        out
    }
}

fn claude(v: &Value) -> Option<Msg> {
    let ty = v.get("type").and_then(Value::as_str)?;
    let role = match ty {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "system" | "result" => Role::Meta,
        _ => return None,
    };
    let mut m = Msg::new(role);
    m.uuid = str_of(v, "uuid").unwrap_or_default();
    m.parent = str_of(v, "parentUuid");
    m.ts_ms = ts_of(v);
    m.sidechain = v
        .get("isSidechain")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    m.cwd = str_of(v, "cwd");
    m.branch = str_of(v, "gitBranch");
    m.session_id = str_of(v, "sessionId").or_else(|| str_of(v, "session_id"));
    if ty == "system" {
        m.model = str_of(v, "model").unwrap_or_default();
        return Some(m);
    }
    if ty == "result" {
        m.reported_cost = v.get("total_cost_usd").and_then(Value::as_f64);
        m.duration_ms = v.get("duration_ms").and_then(Value::as_u64);
        return Some(m);
    }
    let msg = v.get("message")?;
    m.blocks = claude_blocks(msg.get("content").unwrap_or(&Value::Null));
    m.synthetic = v.get("isMeta").and_then(Value::as_bool).unwrap_or(false)
        || v.get("isCompactSummary")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || (role == Role::User
            && m.blocks
                .iter()
                .any(|b| matches!(b, Block::Text(t) if is_synthetic_text(t))));
    if role == Role::Assistant {
        let model = str_of(msg, "model").unwrap_or_default();
        m.model = if model == "<synthetic>" {
            String::new()
        } else {
            model
        };
        m.msg_id = str_of(msg, "id");
        m.usage = msg.get("usage").filter(|u| u.is_object()).map(claude_usage);
    }
    m.agent_id = v
        .pointer("/toolUseResult/agentId")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(m)
}

/// Parse a whole transcript.
pub fn parse_str(raw: &str, source: Source) -> Vec<Msg> {
    let mut n = Normalizer::new(source);
    raw.lines().flat_map(|l| n.push(l)).collect()
}

/// Stream a file through `f`, one normalised record at a time (bad lines are skipped).
pub fn parse_file(path: &Path, source: Source, mut f: impl FnMut(Msg)) -> std::io::Result<()> {
    use std::io::BufRead;
    let rd = std::io::BufReader::with_capacity(256 * 1024, std::fs::File::open(path)?);
    let mut n = Normalizer::new(source);
    for line in rd.split(b'\n') {
        let line = line?;
        for m in n.push(&String::from_utf8_lossy(&line)) {
            f(m);
        }
    }
    Ok(())
}

// ---- aggregation ----

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelTotals {
    pub usage: Usage,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Totals {
    pub usage: Usage,
    pub cost_usd: f64,
    pub by_model: BTreeMap<String, ModelTotals>,
}

impl Totals {
    pub fn add(&mut self, o: &Totals) {
        self.usage.add(&o.usage);
        self.cost_usd += o.cost_usd;
        for (k, v) in &o.by_model {
            let e = self.by_model.entry(k.clone()).or_default();
            e.usage.add(&v.usage);
            e.cost_usd += v.cost_usd;
        }
    }
}

/// Folds records into per-session facts; used for the metadata index (streaming) and sessions.
#[derive(Debug, Clone, Default)]
pub struct Aggregate {
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub model: String,
    pub first_prompt: String,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub message_count: u64,
    pub prompt_count: u64,
    pub tool_count: u64,
    pub error_count: u64,
    pub reported_cost: f64,
    usage_by_msg: HashMap<String, (String, Usage)>,
    loose: Vec<(String, Usage)>,
    seen_msgs: std::collections::HashSet<String>,
}

impl Aggregate {
    pub fn push(&mut self, m: &Msg) {
        if m.sidechain {
            return;
        }
        if self.session_id.is_none() {
            self.session_id = m.session_id.clone();
        }
        if self.cwd.is_none() {
            self.cwd = m.cwd.clone().filter(|c| !c.is_empty());
        }
        if m.branch.is_some() {
            self.branch = m.branch.clone();
        }
        if let Some(t) = m.ts_ms {
            self.start_ms = Some(self.start_ms.map_or(t, |s| s.min(t)));
            self.end_ms = Some(self.end_ms.map_or(t, |s| s.max(t)));
        }
        if let Some(c) = m.reported_cost {
            self.reported_cost = self.reported_cost.max(c);
        }
        if !m.model.is_empty() {
            self.model = m.model.clone();
        }
        if m.is_prompt() {
            self.prompt_count += 1;
            self.message_count += 1;
            if self.first_prompt.is_empty() {
                self.first_prompt = m.text().trim().chars().take(300).collect();
            }
        }
        if m.role == Role::Assistant {
            let first = match &m.msg_id {
                Some(id) => self.seen_msgs.insert(id.clone()),
                None => true,
            };
            if first {
                self.message_count += 1;
            }
        }
        for b in &m.blocks {
            match b {
                Block::ToolUse { .. } => self.tool_count += 1,
                Block::ToolResult { is_error: true, .. } => self.error_count += 1,
                _ => {}
            }
        }
        if let Some(u) = m.usage {
            match &m.msg_id {
                Some(id) => {
                    self.usage_by_msg.insert(id.clone(), (m.model.clone(), u));
                }
                None => self.loose.push((m.model.clone(), u)),
            }
        }
    }

    /// Token and cost totals (cost from `usage::pricing`; the run's own reported cost when the
    /// model is unknown to the table).
    pub fn totals(&self) -> Totals {
        let mut t = Totals::default();
        let fallback = if self.model.is_empty() {
            "unknown"
        } else {
            &self.model
        };
        let all = self.usage_by_msg.values().chain(self.loose.iter());
        for (model, u) in all {
            let model = if model.is_empty() {
                fallback
            } else {
                model.as_str()
            };
            let c = pricing::cost(model, u.input, u.output, u.cache_read, u.cache_write);
            t.usage.add(u);
            t.cost_usd += c;
            let e = t.by_model.entry(model.to_string()).or_default();
            e.usage.add(u);
            e.cost_usd += c;
        }
        if t.cost_usd == 0.0 && self.reported_cost > 0.0 {
            t.cost_usd = self.reported_cost;
        }
        t
    }
}

// ---- chunks ----

/// One user turn and the response steps that followed.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub index: usize,
    pub prompt: Option<String>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub steps: Vec<Step>,
}

impl Chunk {
    fn new(index: usize, prompt: Option<String>, ts: Option<i64>) -> Chunk {
        Chunk {
            index,
            prompt,
            start_ms: ts,
            end_ms: ts,
            steps: Vec::new(),
        }
    }

    pub fn duration_ms(&self) -> Option<i64> {
        match (self.start_ms, self.end_ms) {
            (Some(a), Some(b)) if b >= a => Some(b - a),
            _ => None,
        }
    }
}

/// Build chunks. `include_sidechain` is for subagent transcripts, whose records are all
/// flagged as sidechain.
pub fn build_chunks(msgs: &[Msg], include_sidechain: bool) -> Vec<Chunk> {
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut pending: HashMap<String, (usize, usize)> = HashMap::new();
    for m in msgs {
        if m.sidechain && !include_sidechain {
            continue;
        }
        let touch = |c: &mut Chunk, ts: Option<i64>| {
            if let Some(t) = ts {
                c.start_ms = Some(c.start_ms.map_or(t, |s| s.min(t)));
                c.end_ms = Some(c.end_ms.map_or(t, |s| s.max(t)));
            }
        };
        match m.role {
            Role::Meta => {}
            Role::User => {
                for b in &m.blocks {
                    if let Block::ToolResult {
                        id,
                        content,
                        is_error,
                    } = b
                    {
                        if let Some(&(ci, si)) = pending.get(id) {
                            if let Some(t) = chunks[ci].steps[si].tool.as_mut() {
                                t.result = Some(content.clone());
                                t.is_error = *is_error;
                                t.end_ms = m.ts_ms;
                                if t.is_task() {
                                    t.agent_id = m.agent_id.clone();
                                }
                            }
                            chunks[ci].steps[si].end_ms = m.ts_ms;
                            touch(&mut chunks[ci], m.ts_ms);
                        }
                    }
                }
                if m.is_prompt()
                    || (include_sidechain
                        && chunks.is_empty()
                        && m.role == Role::User
                        && !m.text().trim().is_empty())
                {
                    let n = chunks.len();
                    chunks.push(Chunk::new(n, Some(m.text()), m.ts_ms));
                }
            }
            Role::Assistant => {
                if chunks.is_empty() {
                    chunks.push(Chunk::new(0, None, m.ts_ms));
                }
                let ci = chunks.len() - 1;
                touch(&mut chunks[ci], m.ts_ms);
                for b in &m.blocks {
                    let step = match b {
                        Block::Text(t) if !t.trim().is_empty() => {
                            Step::text(StepKind::Text, t.clone(), m.ts_ms)
                        }
                        Block::Thinking(t) => Step::text(StepKind::Thinking, t.clone(), m.ts_ms),
                        Block::ToolUse { id, name, input } => {
                            let mut s = Step::call(ToolExec {
                                id: id.clone(),
                                name: name.clone(),
                                input: input.clone(),
                                result: None,
                                is_error: false,
                                start_ms: m.ts_ms,
                                end_ms: None,
                                agent_id: None,
                            });
                            s.end_ms = None;
                            pending.insert(id.clone(), (ci, chunks[ci].steps.len()));
                            s
                        }
                        _ => continue,
                    };
                    chunks[ci].steps.push(step);
                }
            }
        }
    }
    for c in &mut chunks {
        let tail = c.end_ms;
        close_steps(&mut c.steps, tail);
    }
    chunks
}

/// A fully parsed session.
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub path: PathBuf,
    pub source: Source,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub model: String,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub message_count: u64,
    pub chunks: Vec<Chunk>,
    pub subagents: Vec<Subagent>,
    /// Parent conversation only.
    pub totals: Totals,
    pub msgs: Vec<Msg>,
}

impl Session {
    pub fn from_msgs(path: &Path, source: Source, msgs: Vec<Msg>) -> Session {
        let mut agg = Aggregate::default();
        for m in &msgs {
            agg.push(m);
        }
        let mut chunks = build_chunks(&msgs, false);
        let mut subagents = if source == Source::Claude {
            subagents::load_all(path, source)
        } else {
            Vec::new()
        };
        subagents::link(&mut chunks, &mut subagents);
        Session {
            id: agg
                .session_id
                .clone()
                .unwrap_or_else(|| session_id_of(path)),
            path: path.to_path_buf(),
            source,
            cwd: agg.cwd.clone(),
            branch: agg.branch.clone(),
            model: agg.model.clone(),
            start_ms: agg.start_ms,
            end_ms: agg.end_ms,
            message_count: agg.message_count,
            totals: agg.totals(),
            chunks,
            subagents,
            msgs,
        }
    }

    /// Parent conversation plus subagents.
    pub fn grand_totals(&self) -> Totals {
        let mut t = self.totals.clone();
        for s in &self.subagents {
            t.add(&s.totals);
        }
        t
    }

    pub fn duration_ms(&self) -> Option<i64> {
        match (self.start_ms, self.end_ms) {
            (Some(a), Some(b)) if b >= a => Some(b - a),
            _ => None,
        }
    }

    /// Every tool execution in the parent conversation, in order.
    pub fn tools(&self) -> impl Iterator<Item = &ToolExec> {
        self.chunks
            .iter()
            .flat_map(|c| c.steps.iter())
            .filter_map(|s| s.tool.as_ref())
    }
}

/// Read and parse a session file.
pub fn load_session(path: &Path, source: Source) -> std::io::Result<Session> {
    let mut msgs = Vec::new();
    parse_file(path, source, |m| msgs.push(m))?;
    Ok(Session::from_msgs(path, source, msgs))
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! Synthetic transcripts in the Claude Code and Codex formats.
    pub fn claude() -> String {
        [
            r#"{"type":"user","uuid":"u1","sessionId":"s1","cwd":"/work/proj","gitBranch":"main","timestamp":"2025-03-01T10:00:00.000Z","message":{"role":"user","content":"Fix the login bug please"}}"#,
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2025-03-01T10:00:02.000Z","message":{"id":"m1","model":"claude-sonnet-4-5","role":"assistant","content":[{"type":"thinking","thinking":"look at auth"},{"type":"text","text":"Let me **read** the file."},{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/work/proj/src/auth.rs"}}],"usage":{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":1000,"cache_creation_input_tokens":50}}}"#,
            r#"{"type":"user","uuid":"u2","parentUuid":"a1","timestamp":"2025-03-01T10:00:03.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"fn login() {}"}]}}"#,
            r#"{"type":"assistant","uuid":"a2","timestamp":"2025-03-01T10:00:10.000Z","message":{"id":"m2","model":"claude-sonnet-4-5","role":"assistant","content":[{"type":"tool_use","id":"t2","name":"Edit","input":{"file_path":"/work/proj/src/auth.rs","old_string":"a","new_string":"b"}},{"type":"tool_use","id":"t3","name":"Bash","input":{"command":"cargo test"}}],"usage":{"input_tokens":200,"output_tokens":40,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
            r#"{"type":"user","uuid":"u3","timestamp":"2025-03-01T10:00:12.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t2","content":"ok"}]}}"#,
            r#"{"type":"user","uuid":"u4","timestamp":"2025-03-01T10:00:20.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t3","content":[{"type":"text","text":"error: test failed"}],"is_error":true}]}}"#,
            r#"{"type":"assistant","uuid":"a3","timestamp":"2025-03-01T10:00:25.000Z","message":{"id":"m2","model":"claude-sonnet-4-5","role":"assistant","content":[{"type":"text","text":"The test failed."}],"usage":{"input_tokens":200,"output_tokens":60,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
            r#"{"type":"user","uuid":"u5","timestamp":"2025-03-01T10:01:00.000Z","message":{"role":"user","content":"thanks, run git commit"}}"#,
            r#"{"type":"assistant","uuid":"a4","timestamp":"2025-03-01T10:01:05.000Z","message":{"id":"m3","model":"claude-sonnet-4-5","role":"assistant","content":[{"type":"tool_use","id":"t4","name":"Bash","input":{"command":"git commit -am fix"}}],"usage":{"input_tokens":10,"output_tokens":5}}}"#,
            r#"{"type":"user","uuid":"u6","timestamp":"2025-03-01T10:01:06.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t4","content":"[main abc] fix"}]}}"#,
            "not json",
        ]
        .join("\n")
    }

    pub fn codex() -> String {
        [
            r#"{"timestamp":"2025-03-02T09:00:00.000Z","type":"session_meta","payload":{"id":"cx1","cwd":"/work/codexproj","git":{"branch":"dev"}}}"#,
            r#"{"timestamp":"2025-03-02T09:00:00.100Z","type":"turn_context","payload":{"model":"gpt-5-codex","cwd":"/work/codexproj"}}"#,
            r#"{"timestamp":"2025-03-02T09:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>x</environment_context>"}]}}"#,
            r#"{"timestamp":"2025-03-02T09:00:02.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"list files"}]}}"#,
            r#"{"timestamp":"2025-03-02T09:00:03.000Z","type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"plan ls"}]}}"#,
            r#"{"timestamp":"2025-03-02T09:00:04.000Z","type":"response_item","payload":{"type":"function_call","name":"shell","call_id":"c9","arguments":"{\"command\":[\"ls\",\"-la\"]}"}}"#,
            r#"{"timestamp":"2025-03-02T09:00:06.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c9","output":"{\"output\":\"a\\nb\",\"metadata\":{\"exit_code\":1}}"}}"#,
            r#"{"timestamp":"2025-03-02T09:00:07.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":150},"last_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":50}}}}"#,
            r#"{"timestamp":"2025-03-02T09:00:07.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":150},"last_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":50}}}}"#,
            r#"{"timestamp":"2025-03-02T09:00:08.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Here are the files."}]}}"#,
        ]
        .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures;
    use super::*;

    #[test]
    fn claude_chunks_and_pairing() {
        let msgs = parse_str(&fixtures::claude(), Source::Claude);
        let chunks = build_chunks(&msgs, false);
        assert_eq!(chunks.len(), 2);
        assert_eq!(
            chunks[0].prompt.as_deref(),
            Some("Fix the login bug please")
        );
        let kinds: Vec<_> = chunks[0].steps.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            vec![
                StepKind::Thinking,
                StepKind::Text,
                StepKind::ToolCall,
                StepKind::ToolCall,
                StepKind::ToolCall,
                StepKind::Text
            ]
        );
        let read = chunks[0].steps[2].tool.as_ref().unwrap();
        assert_eq!(read.result.as_deref(), Some("fn login() {}"));
        assert_eq!(read.duration_ms(), Some(1000));
        let bash = chunks[0].steps[4].tool.as_ref().unwrap();
        assert!(bash.is_error);
        assert_eq!(bash.result.as_deref(), Some("error: test failed"));
        assert_eq!(chunks[1].steps.len(), 1);
    }

    #[test]
    fn claude_totals_dedupe_message_ids() {
        let msgs = parse_str(&fixtures::claude(), Source::Claude);
        let mut a = Aggregate::default();
        msgs.iter().for_each(|m| a.push(m));
        let t = a.totals();
        // m1 + m2 (last record wins: output 60) + m3
        assert_eq!(t.usage.input, 310);
        assert_eq!(t.usage.output, 20 + 60 + 5);
        assert_eq!(t.usage.cache_read, 1000);
        assert_eq!(t.usage.cache_write, 50);
        assert!(t.cost_usd > 0.0);
        assert_eq!(a.cwd.as_deref(), Some("/work/proj"));
        assert_eq!(a.branch.as_deref(), Some("main"));
        assert_eq!(a.first_prompt, "Fix the login bug please");
        assert_eq!(a.tool_count, 4);
        assert_eq!(a.error_count, 1);
        assert_eq!(a.prompt_count, 2);
    }

    #[test]
    fn codex_normalises() {
        let msgs = parse_str(&fixtures::codex(), Source::Codex);
        let chunks = build_chunks(&msgs, false);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].prompt.as_deref(), Some("list files"));
        let tool = chunks[0]
            .steps
            .iter()
            .find_map(|s| s.tool.as_ref())
            .unwrap();
        assert_eq!(tool.name, "shell");
        assert!(tool.is_error);
        assert_eq!(tool.result.as_deref(), Some("a\nb"));
        let mut a = Aggregate::default();
        msgs.iter().for_each(|m| a.push(m));
        let t = a.totals();
        assert_eq!(
            (t.usage.input, t.usage.cache_read, t.usage.output),
            (60, 40, 50)
        );
        assert_eq!(a.cwd.as_deref(), Some("/work/codexproj"));
        assert_eq!(a.model, "gpt-5-codex");
    }

    #[test]
    fn run_stream_json_uses_reported_cost() {
        let raw = [
            r#"{"type":"system","subtype":"init","session_id":"r1","cwd":"/w","model":"mystery"}"#,
            r#"{"type":"assistant","message":{"id":"x","model":"mystery","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":1,"output_tokens":2}}}"#,
            r#"{"type":"result","total_cost_usd":0.25,"duration_ms":1500,"session_id":"r1"}"#,
        ]
        .join("\n");
        let msgs = parse_str(&raw, Source::Rteams);
        let mut a = Aggregate::default();
        msgs.iter().for_each(|m| a.push(m));
        assert_eq!(a.totals().cost_usd, 0.25);
        assert_eq!(a.cwd.as_deref(), Some("/w"));
    }
}
