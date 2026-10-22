//! Supervisor for Claude Code agent processes, one per team member.
//!
//! `build_command` is pure; `Supervisor` spawns it (each agent in its own process group), reaps
//! exits on `poll()`, keeps a ring of human-readable lines per agent summarised from the
//! `stream-json` output, records every raw run to disk and remembers each member's Claude
//! session id so the next launch can `--resume` it. std threads only.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

#[cfg(test)]
use crate::diagnostics::SpawnState;
use crate::diagnostics::{self, Cause, DiagEnv, LaunchDiag, LaunchRecord};
use crate::runtimes::{self, AgentRuntime, LaunchCtx};
use crate::task_logs;

/// Max lines kept per agent.
const RING: usize = 200;

/// What the agent may do this run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Normal work: rteams tools plus file tools.
    #[default]
    Do,
    /// Read-only: rteams plus Read/Grep/Glob; never edits.
    Ask,
    /// Only rteams tools: break work into tasks and assign them.
    Delegate,
}

/// Tool permission policy, set per launch config.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Permissions {
    /// `acceptEdits` permission mode; only rteams tools are pre-allowed.
    #[default]
    EditsOnly,
    /// `--dangerously-skip-permissions` (mode restrictions still deny edit tools).
    SkipAll,
    /// Every permission request goes to the lead through the rteams `permission_request` tool.
    Manual,
}

impl Permissions {
    fn as_str(&self) -> &'static str {
        match self {
            Permissions::EditsOnly => "edits",
            Permissions::SkipAll => "skip",
            Permissions::Manual => "manual",
        }
    }
}

/// One agent to launch.
#[derive(Clone, Debug)]
pub struct AgentSpec {
    pub member: String,
    pub role: String,
    pub workdir: PathBuf,
    pub prompt: String,
    pub mode: Mode,
    /// `--model` value.
    pub model: Option<String>,
    /// Extra standing instructions appended to the system prompt.
    pub instructions: Option<String>,
    /// Resume the member's saved session when there is one (default true).
    pub resume: bool,
    /// Session to resume. Filled in by `Supervisor::launch`; leave `None` when constructing.
    pub session: Option<String>,
    /// Runtime id (`claude`, `codex`, ...); `None` = Claude Code.
    pub runtime: Option<String>,
    /// Run in a git worktree of `project` (needs `project`).
    pub worktree: bool,
    pub project: Option<PathBuf>,
    /// The only agent on the team: owns every task.
    pub solo: bool,
    /// Extra environment for the agent process (API keys for its runtime).
    pub env: Vec<(String, String)>,
    /// `low|medium|high|xhigh|max`; mapped to each runtime's effort flag where one exists.
    pub effort: Option<String>,
    /// Fast mode (Claude `fastMode` setting, Codex `service_tier=fast`).
    pub fast: bool,
    /// Context window cap in tokens (Codex `model_context_window`; Claude drops `[1m]`).
    pub context_limit: Option<u32>,
    /// Per-launch permissions override of the team-wide setting.
    pub permissions: Option<Permissions>,
    /// Extra MCP server names (from the user's MCP config) the agent may use beside `rteams`.
    pub mcp_allow: Vec<String>,
    /// User CLI flags appended to the Claude command (validated by `validate_extra_args`).
    pub extra_args: Vec<String>,
}

impl Default for AgentSpec {
    fn default() -> Self {
        AgentSpec {
            member: String::new(),
            role: String::new(),
            workdir: PathBuf::new(),
            prompt: String::new(),
            mode: Mode::Do,
            model: None,
            instructions: None,
            resume: true,
            session: None,
            runtime: None,
            worktree: false,
            project: None,
            solo: false,
            env: Vec::new(),
            effort: None,
            fast: false,
            context_limit: None,
            permissions: None,
            mcp_allow: Vec::new(),
            extra_args: Vec::new(),
        }
    }
}

/// How to reach `claude`, `rteams` and the team.
pub struct LaunchConfig {
    pub claude_bin: String,
    pub rteams_bin: String,
    pub home: PathBuf,
    pub team: String,
    pub permissions: Permissions,
}

/// Where `claude` commonly lives when the GUI was started with a minimal PATH.
pub fn claude_candidates(user_home: Option<&Path>) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = user_home {
        v.push(h.join(".local/bin/claude"));
        v.push(h.join(".claude/local/claude"));
    }
    v.push(PathBuf::from("/opt/homebrew/bin/claude"));
    v.push(PathBuf::from("/usr/local/bin/claude"));
    v
}

/// First `claude` found on `path_var`, else the first existing candidate.
pub fn find_claude(path_var: Option<&OsStr>, candidates: &[PathBuf]) -> Option<PathBuf> {
    let on_path = path_var.and_then(|p| {
        std::env::split_paths(p)
            .map(|d| d.join("claude"))
            .find(|f| f.is_file())
    });
    on_path.or_else(|| candidates.iter().find(|c| c.is_file()).cloned())
}

impl LaunchConfig {
    /// Defaults: `claude` discovered (PATH, then well-known locations; plain `claude` if none);
    /// `rteams` next to the running exe if present, else on PATH.
    pub fn new(home: PathBuf, team: &str) -> Self {
        let sibling = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("rteams")))
            .filter(|p| p.is_file());
        let user_home = std::env::var_os("HOME").map(PathBuf::from);
        let claude = find_claude(
            std::env::var_os("PATH").as_deref(),
            &claude_candidates(user_home.as_deref()),
        );
        LaunchConfig {
            claude_bin: claude
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| "claude".into()),
            rteams_bin: sibling
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| "rteams".into()),
            home,
            team: team.to_string(),
            permissions: Permissions::default(),
        }
    }
}

/// Lifecycle of one agent process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentStatus {
    Running { pid: u32 },
    Exited { code: Option<i32> },
    Failed(String),
}

/// Renderable snapshot of one agent.
#[derive(Clone, Debug)]
pub struct AgentView {
    pub member: String,
    pub status: AgentStatus,
    pub last_lines: Vec<String>,
    /// Unix ms until which the agent's provider is rate limited (from its output).
    pub rate_limited_until: Option<i64>,
    /// Text of the last `stderr:` / `error:` line.
    pub last_error: Option<String>,
    /// Runtime id this agent runs on (`claude` by default).
    pub runtime: String,
}

fn system_prompt(spec: &AgentSpec) -> String {
    let mut s = format!(
        "You are team member `{m}` (role: {r}) using the `rteams` MCP tools. Pass `{m}` as \
         `actor`/`member` on every call. Work in {d}. Be token-frugal:\n\
         1. Call member_briefing once, then task_briefing; take the FIRST actionable task. When \
         it is done, take the next actionable one (one task_briefing per task), at most 3 tasks \
         per run, then stop.\n\
         2. task_start, do the work, add ONE concise task_add_comment with the result (what \
         changed, where), then task_complete (or review_request if your role/flow needs review).\n\
         3. Do NOT send messages, greetings, acknowledgements or status updates. Exceptions: \
         when a person messaged you (direct or group chat) with a question or request, answer \
         them ONCE with message_send to that sender, short and concrete; when blocked \
         (with task_block), message the lead; and you may message the lead privately (direct, \
         not group chat) about a real concern you'd rather not raise in front of the team.\n\
         4. Never re-call briefings or the inbox in loops. With no actionable task, stop \
         immediately.\n\
         5. Keep tool inputs and comments short.\n\
         6. Everything a person will read (messages, task comments) is ONLY bullet points: \
         each on its own line starting with \"- \", one idea, <= 15 words, at most 6; no \
         intro, recap, filler or inline \"1) 2)\" lists. They will ask if they need more.\n\
         7. Call runtime_bootstrap_checkin once at start; call runtime_heartbeat with status \
         \"stuck\" and a note when blocked.",
        m = spec.member,
        r = spec.role,
        d = spec.workdir.display()
    );
    if spec.solo {
        s.push_str(
            "\nYou are the only agent on this team: own every task on the board yourself; \
             create tasks for multi-step work.",
        );
    } else if spec.role == "lead" {
        s.push_str(
            "\nYOU LEAD THIS TEAM (overrides rules 1, 3 and 4): when a message gives you work, act \
             on it even with no task. Read the full request, break it into rteams tasks \
             (task_create, task_set_dependencies), assign each to the best-fitting member from \
             team_get, then send the requester ONE short message listing the tasks and owners. \
             Review work sent to you (review_approve / review_request_changes). Do work yourself \
             only when no member fits it.",
        );
    }
    match spec.mode {
        Mode::Do => {}
        Mode::Ask => s.push_str(
            "\nMODE ASK (overrides the rules above): answer only, change nothing. Do not edit \
             files, run commands or change the board; reply with the answer.",
        ),
        Mode::Delegate => s.push_str(
            "\nMODE DELEGATE (overrides the rules above): do not do the work yourself; break it \
             into rteams tasks and assign them to members.",
        ),
    }
    if let Some(i) = spec.instructions.as_deref().map(str::trim) {
        if !i.is_empty() {
            s.push('\n');
            s.push_str(i);
        }
    }
    s
}

/// MCP servers named in `names`, looked up in the user's Claude MCP config (project + user
/// scopes), as `{command,args,env}` / `{url}` JSON. Unknown or disabled names are skipped.
pub fn resolve_mcp_allow(names: &[String], project: Option<&Path>) -> Vec<(String, Value)> {
    use rteams_services::ext::mcp_config as mc;
    if names.is_empty() {
        return Vec::new();
    }
    let paths = mc::ClientPaths::real(project.map(Path::to_path_buf));
    let (installed, _) = mc::list(&paths, mc::Runtime::Claude);
    let mut out: Vec<(String, Value)> = Vec::new();
    for n in names.iter().filter(|n| n.as_str() != "rteams") {
        if out.iter().any(|(k, _)| k == n) {
            continue;
        }
        let Some(s) = installed.iter().find(|s| &s.name == n && s.enabled) else {
            continue;
        };
        let e = &s.spec;
        let v = if matches!(e.transport, mc::Transport::Stdio) {
            json!({"command": e.command, "args": e.args, "env": e.env})
        } else {
            json!({"url": e.url, "headers": e.headers})
        };
        out.push((n.clone(), v));
    }
    out
}

/// Tools denied outright for the restricted modes.
const NO_EDIT: &str = "Edit,Write,NotebookEdit,Bash";

/// The `claude` command for one agent. Pure: spawns and writes nothing.
///
/// `claude -p <prompt> --output-format=stream-json --verbose --mcp-config=<json>
/// --strict-mcp-config --allowedTools=<..> [--disallowedTools=<..>]
/// (--permission-mode <m> | --dangerously-skip-permissions) [--model m] [--resume id]
/// --append-system-prompt <text>`. The prompt comes right after `-p` and variadic options use
/// `=` so they cannot swallow it. The spawned process leads its own process group.
pub fn build_command(cfg: &LaunchConfig, spec: &AgentSpec) -> Command {
    let extras = resolve_mcp_allow(&spec.mcp_allow, spec.project.as_deref());
    let mut servers = serde_json::Map::new();
    for (k, v) in &extras {
        servers.insert(k.clone(), v.clone());
    }
    servers.insert(
        "rteams".into(),
        json!({
            "command": cfg.rteams_bin,
            "args": ["serve", "--home", cfg.home.to_string_lossy(), "--team", cfg.team],
            "env": {"RTEAMS_MEMBER": spec.member},
        }),
    );
    let mcp = json!({ "mcpServers": servers });
    let mut allowed = String::from("mcp__rteams");
    for (k, _) in &extras {
        allowed.push_str(",mcp__");
        allowed.push_str(k);
    }
    match spec.mode {
        Mode::Do => {}
        Mode::Ask => allowed.push_str(",Read,Grep,Glob"),
        Mode::Delegate => {}
    }
    let mut cmd = Command::new(&cfg.claude_bin);
    cmd.arg("-p")
        .arg(&spec.prompt)
        .arg("--output-format=stream-json")
        .arg("--verbose")
        .arg(format!("--mcp-config={mcp}"))
        .arg("--strict-mcp-config")
        .arg(format!("--allowedTools={allowed}"));
    if spec.mode != Mode::Do {
        cmd.arg(format!("--disallowedTools={NO_EDIT}"));
    }
    match spec.permissions.as_ref().unwrap_or(&cfg.permissions) {
        Permissions::SkipAll => {
            cmd.arg("--dangerously-skip-permissions");
        }
        Permissions::Manual => {
            cmd.arg("--permission-mode")
                .arg("default")
                .arg("--permission-prompt-tool")
                .arg("mcp__rteams__permission_request");
        }
        Permissions::EditsOnly => {
            cmd.arg("--permission-mode").arg(if spec.mode == Mode::Do {
                "acceptEdits"
            } else {
                "default"
            });
        }
    }
    if let Some(m) = spec.model.as_deref().filter(|m| !m.trim().is_empty()) {
        cmd.arg("--model")
            .arg(claude_model(m.trim(), spec.context_limit));
    }
    if let Some(e) = spec
        .effort
        .as_deref()
        .filter(|e| matches!(*e, "low" | "medium" | "high" | "xhigh" | "max"))
    {
        cmd.arg("--effort").arg(e);
    }
    if spec.fast {
        cmd.arg("--settings").arg(r#"{"fastMode":true}"#);
    }
    if let Some(id) = spec.session.as_deref().filter(|_| spec.resume) {
        cmd.arg("--resume").arg(id);
    }
    cmd.args(&spec.extra_args);
    cmd.arg("--append-system-prompt").arg(system_prompt(spec));
    finish_command(&mut cmd, &spec.workdir);
    cmd
}

/// Flags rteams sets itself; user extra args may not override them.
const PROTECTED_FLAGS: [&str; 16] = [
    "-p",
    "--print",
    "--output-format",
    "--input-format",
    "--verbose",
    "--mcp-config",
    "--strict-mcp-config",
    "--allowedTools",
    "--allowed-tools",
    "--disallowedTools",
    "--disallowed-tools",
    "--dangerously-skip-permissions",
    "--permission-mode",
    "--permission-prompt-tool",
    "--resume",
    "--append-system-prompt",
];

/// Splits `raw` like a shell would (whitespace, single/double quotes).
pub fn split_args(raw: &str) -> Result<Vec<String>, String> {
    let (mut out, mut cur, mut quote, mut any) = (Vec::new(), String::new(), None, false);
    for c in raw.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            (None, c) => cur.push(c),
        }
    }
    if quote.is_some() {
        return Err("unclosed quote".into());
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

/// Checks user extra CLI args: max 2048 chars, no flag rteams sets itself, and (when `help`
/// is given, i.e. `claude --help` output) only flags the CLI knows. Returns the split args.
pub fn validate_extra_args(raw: &str, help: Option<&str>) -> Result<Vec<String>, String> {
    if raw.len() > 2048 {
        return Err("too long (max 2048)".into());
    }
    let args = split_args(raw)?;
    let flag = |a: &str| a.split('=').next().unwrap_or(a).to_string();
    let flags: Vec<String> = args
        .iter()
        .filter(|a| a.starts_with('-'))
        .map(|a| flag(a))
        .collect();
    let protected: Vec<&String> = flags
        .iter()
        .filter(|f| PROTECTED_FLAGS.contains(&f.as_str()))
        .collect();
    if !protected.is_empty() {
        let l: Vec<&str> = protected.iter().map(|s| s.as_str()).collect();
        return Err(format!("set by rteams: {}", l.join(", ")));
    }
    if let Some(help) = help {
        let known = |f: &str| {
            help.match_indices(f).any(|(i, _)| {
                let next = help[i + f.len()..].chars().next();
                !next.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-')
            })
        };
        let unknown: Vec<&str> = flags
            .iter()
            .map(String::as_str)
            .filter(|f| !known(f))
            .collect();
        if !unknown.is_empty() {
            return Err(format!("unknown flags: {}", unknown.join(", ")));
        }
    }
    Ok(args)
}

/// `claude --help` output, run once per process; `None` if the CLI could not be run.
pub fn claude_help() -> Option<&'static str> {
    static HELP: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    HELP.get_or_init(|| {
        let bin = LaunchConfig::new(PathBuf::new(), "").claude_bin;
        let out = Command::new(bin).arg("--help").output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    })
    .as_deref()
}

/// A context cap of 200k or less drops the `[1m]` long-context suffix from a Claude model.
fn claude_model(m: &str, limit: Option<u32>) -> String {
    match limit {
        Some(l) if l <= 200_000 => m.replace("[1m]", ""),
        _ => m.to_string(),
    }
}

/// cwd, piped stdio and a process group of its own, for any runtime's command.
fn finish_command(cmd: &mut Command, workdir: &Path) {
    cmd.current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
}

/// The command for one agent on its runtime; Claude's own builder when no runtime is registered.
fn command_for(cfg: &LaunchConfig, spec: &AgentSpec, rt: Option<&dyn AgentRuntime>) -> Command {
    let Some(rt) = rt else {
        return build_command(cfg, spec);
    };
    let bin = if rt.id() == "claude" {
        PathBuf::from(&cfg.claude_bin)
    } else {
        rt.detect().path.unwrap_or_else(|| PathBuf::from(rt.id()))
    };
    let ctx = LaunchCtx {
        bin,
        prompt: spec.prompt.clone(),
        system_prompt: system_prompt(spec),
        mcp_command: cfg.rteams_bin.clone(),
        mcp_args: [
            "serve",
            "--home",
            &cfg.home.to_string_lossy(),
            "--team",
            &cfg.team,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
        mcp_env: vec![("RTEAMS_MEMBER".into(), spec.member.clone())],
        model: spec
            .model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(|m| claude_model(m, spec.context_limit)),
        mode: match spec.mode {
            Mode::Do => "do",
            Mode::Ask => "ask",
            Mode::Delegate => "delegate",
        }
        .into(),
        permissions: spec
            .permissions
            .as_ref()
            .unwrap_or(&cfg.permissions)
            .as_str()
            .into(),
        resume: spec
            .session
            .clone()
            .filter(|_| spec.resume && rt.supports_resume()),
        workdir: spec.workdir.clone(),
        effort: spec.effort.clone(),
        fast: spec.fast,
        context_limit: spec.context_limit,
        extra_mcp: resolve_mcp_allow(&spec.mcp_allow, spec.project.as_deref()),
    };
    let mut cmd = rt.build_command(&ctx);
    finish_command(&mut cmd, &spec.workdir);
    cmd
}

const RATE_BUFFER_MS: i64 = 30_000;
const RATE_DEFAULT_MS: i64 = 3_600_000;
const RATE_CAP_MS: i64 = 12 * 3_600_000;

fn local_offset_secs() -> i64 {
    static OFF: OnceLock<i64> = OnceLock::new();
    *OFF.get_or_init(|| {
        let out = Command::new("date").arg("+%z").output().ok();
        let t = out
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let b = t.as_bytes();
        if b.len() == 5 && (b[0] == b'+' || b[0] == b'-') {
            let h: i64 = t[1..3].parse().unwrap_or(0);
            let m: i64 = t[3..5].parse().unwrap_or(0);
            let v = h * 3600 + m * 60;
            if b[0] == b'-' {
                -v
            } else {
                v
            }
        } else {
            0
        }
    })
}

fn find_reset_ts(v: &Value) -> Option<i64> {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if matches!(
                    k.as_str(),
                    "resetsAt" | "reset_at" | "resets_at" | "resetAt"
                ) {
                    let n = x
                        .as_i64()
                        .or_else(|| x.as_f64().map(|f| f as i64))
                        .or_else(|| x.as_str().and_then(|s| s.trim().parse().ok()));
                    if let Some(n) = n {
                        // Seconds or milliseconds.
                        return Some(if n < 100_000_000_000 { n * 1000 } else { n });
                    }
                }
            }
            m.values().find_map(find_reset_ts)
        }
        Value::Array(a) => a.iter().find_map(find_reset_ts),
        _ => None,
    }
}

fn find_status(v: &Value) -> Option<&str> {
    match v {
        Value::Object(m) => m
            .get("status")
            .and_then(Value::as_str)
            .or_else(|| m.values().find_map(find_status)),
        _ => None,
    }
}

/// "in 2 hours 30 minutes" / "2h 30m" after one of the markers -> milliseconds.
fn relative_ms(lower: &str) -> Option<i64> {
    const MARKERS: [&str; 6] = [
        "try again in",
        "retry in",
        "retry after",
        "resets in",
        "reset in",
        "available again in",
    ];
    let rest = MARKERS
        .iter()
        .find_map(|m| lower.find(m).map(|i| &lower[i + m.len()..]))?;
    let mut total = 0i64;
    let mut pending: Option<i64> = None;
    let mut parsed = false;
    for tok in rest.split(|c: char| !c.is_ascii_alphanumeric()) {
        if tok.is_empty() {
            continue;
        }
        let digits: String = tok.chars().take_while(|c| c.is_ascii_digit()).collect();
        let unit = &tok[digits.len()..];
        let n = if digits.is_empty() {
            pending.take()
        } else {
            let d = digits.parse().ok();
            if unit.is_empty() {
                pending = d;
                continue;
            }
            d
        };
        let mult = match unit {
            "d" | "day" | "days" => 86_400_000,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3_600_000,
            "m" | "min" | "mins" | "minute" | "minutes" => 60_000,
            "s" | "sec" | "secs" | "second" | "seconds" => 1000,
            "and" => continue,
            _ => break,
        };
        let Some(n) = n else { break };
        total += n * mult;
        parsed = true;
    }
    parsed.then_some(total)
}

/// `3pm`, `3:30 pm`, `15:30` -> seconds into the day.
fn parse_clock(s: &str) -> Option<i64> {
    let s = s.trim_start();
    let d1: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if d1.is_empty() || d1.len() > 2 {
        return None;
    }
    let mut h: i64 = d1.parse().ok()?;
    let mut rest = &s[d1.len()..];
    let mut min = 0i64;
    let mut colon = false;
    if let Some(r) = rest.strip_prefix(':') {
        let d2: String = r.chars().take_while(|c| c.is_ascii_digit()).collect();
        if d2.len() != 2 {
            return None;
        }
        min = d2.parse().ok()?;
        rest = &r[2..];
        colon = true;
    }
    let rest = rest.trim_start();
    let ampm = if rest.starts_with("pm") {
        Some(true)
    } else if rest.starts_with("am") {
        Some(false)
    } else {
        None
    };
    if !colon && ampm.is_none() {
        return None;
    }
    match ampm {
        Some(pm) => {
            if !(1..=12).contains(&h) {
                return None;
            }
            h = h % 12 + if pm { 12 } else { 0 };
        }
        None if h > 23 => return None,
        None => {}
    }
    (min < 60).then_some(h * 3600 + min * 60)
}

fn clock_ms(lower: &str, now_ms: i64, offset: i64) -> Option<i64> {
    let i = lower.find("reset")?;
    let mut rest = &lower[i + 5..];
    rest = rest.strip_prefix('s').unwrap_or(rest).trim_start();
    rest = rest.strip_prefix("at").unwrap_or(rest);
    let sod = parse_clock(rest)?;
    let now_s = now_ms.div_euclid(1000);
    let day = (now_s + offset).div_euclid(86_400) * 86_400;
    let mut t = day + sod - offset;
    if t <= now_s {
        t += 86_400;
    }
    Some(t * 1000)
}

fn rate_limit_at(line: &str, now_ms: i64, offset: i64) -> Option<i64> {
    let json = serde_json::from_str::<Value>(line).ok();
    let status = json.as_ref().and_then(find_status);
    if matches!(status, Some("allowed" | "allowed_warning")) {
        return None;
    }
    let lower = line.to_lowercase();
    let ts = json.as_ref().and_then(find_reset_ts);
    let phrase = [
        "rate limit",
        "rate_limit",
        "ratelimit",
        "usage limit",
        "limit reached",
        "limit exceeded",
        "too many requests",
        "quota exceeded",
        "hit your limit",
    ]
    .iter()
    .any(|k| lower.contains(k));
    if !(phrase || status == Some("rejected") && ts.is_some()) {
        return None;
    }
    // Legacy "...usage limit reached|1760000000".
    let piped = lower.rsplit_once('|').and_then(|(_, t)| {
        let t = t.trim();
        (t.len() == 10 && t.bytes().all(|b| b.is_ascii_digit()))
            .then(|| t.parse::<i64>().ok().map(|n| n * 1000))
            .flatten()
    });
    let target = ts
        .or(piped)
        .or_else(|| relative_ms(&lower).map(|d| now_ms + d))
        .or_else(|| clock_ms(&lower, now_ms, offset))
        .unwrap_or(now_ms + RATE_DEFAULT_MS);
    Some((target + RATE_BUFFER_MS).clamp(now_ms + RATE_BUFFER_MS, now_ms + RATE_CAP_MS))
}

/// If `line` reports a Claude/Codex usage or rate limit, the unix-ms time it resets: the stated
/// time (JSON `resetsAt`/`reset_at`, "resets at 3pm", "try again in 2 hours") plus 30 s, one hour
/// from `now_ms` when no time is given, never more than 12 h out.
pub fn rate_limit_reset(line: &str, now_ms: i64) -> Option<i64> {
    rate_limit_at(line, now_ms, local_offset_secs())
}

/// Claude's `session_id` on a stream-json line.
fn claude_session_id(line: &str) -> Option<String> {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|v| v["session_id"].as_str().map(str::to_string))
        .filter(|s| !s.is_empty())
}

fn clip(s: &str, max: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        one
    } else {
        let head: String = one.chars().take(max).collect();
        format!("{head}...")
    }
}

/// Turn one stream-json line into zero or more short display lines. Unparseable lines are
/// kept raw; events with nothing worth showing yield nothing.
pub fn summarize_line(line: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return if line.trim().is_empty() {
            vec![]
        } else {
            vec![line.to_string()]
        };
    };
    let mut out = Vec::new();
    match v["type"].as_str() {
        Some("system") if v["subtype"].as_str() == Some("init") => {
            out.push("session started".to_string());
        }
        Some("assistant") => {
            for part in v["message"]["content"].as_array().into_iter().flatten() {
                match part["type"].as_str() {
                    Some("text") => {
                        let t = clip(part["text"].as_str().unwrap_or(""), 300);
                        if !t.is_empty() {
                            out.push(t);
                        }
                    }
                    Some("tool_use") => {
                        let name = part["name"].as_str().unwrap_or("?");
                        let name = name.strip_prefix("mcp__rteams__").unwrap_or(name);
                        out.push(format!(
                            "tool: {name} {}",
                            clip(&part["input"].to_string(), 120)
                        ));
                    }
                    _ => {}
                }
            }
        }
        Some("result") => {
            let r = clip(v["result"].as_str().unwrap_or(""), 300);
            let err = v["is_error"].as_bool().unwrap_or(false);
            out.push(format!("{}: {r}", if err { "error" } else { "result" }));
        }
        _ => {}
    }
    out
}

type Ring = Arc<Mutex<VecDeque<String>>>;
type Sessions = Arc<Mutex<BTreeMap<String, String>>>;

fn push(ring: &Ring, line: String) {
    let mut r = ring.lock().unwrap_or_else(|e| e.into_inner());
    if r.len() >= RING {
        r.pop_front();
    }
    r.push_back(line);
}

/// Assistant/user/tool JSON lines carry model text, which may merely talk about limits.
fn is_conversation_line(line: &str) -> bool {
    serde_json::from_str::<Value>(line).is_ok_and(|v| {
        matches!(
            v["type"].as_str(),
            Some("assistant" | "user" | "system" | "tool_use" | "tool_result")
        )
    })
}

fn note_limit(slot: &Arc<Mutex<Option<i64>>>, line: &str) {
    if let Some(t) = rate_limit_reset(line, unix_ms() as i64) {
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(t);
    }
}

fn set_last(slot: &Arc<Mutex<Option<String>>>, text: &str) {
    let t = text.trim();
    if !t.is_empty() {
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(t.to_string());
    }
}

fn load_sessions(path: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_sessions(path: &Path, map: &BTreeMap<String, String>) {
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(s) = serde_json::to_string_pretty(map) {
        let _ = std::fs::write(path, s);
    }
}

fn safe_name(member: &str) -> String {
    member
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Send `sig` (e.g. "TERM") to the whole process group `pgid`. True if the signal was delivered.
/// Process groups of agents that are still running, so a SIGTERM / Ctrl+C handler can stop
/// them even when the app is killed and `Drop` never runs.
static LIVE_GROUPS: std::sync::Mutex<std::collections::BTreeSet<u32>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

fn track_group(pgid: u32, live: bool) {
    if let Ok(mut g) = LIVE_GROUPS.lock() {
        if live {
            g.insert(pgid);
        } else {
            g.remove(&pgid);
        }
    }
}

/// Kill every running agent's process group. For signal handlers: no allocation-heavy work
/// beyond spawning `kill`, and safe to call more than once.
pub fn kill_all_agents() {
    let groups: Vec<u32> = LIVE_GROUPS
        .lock()
        .map(|g| g.iter().copied().collect())
        .unwrap_or_default();
    for pgid in groups {
        signal_group("KILL", pgid);
        track_group(pgid, false);
    }
}

fn signal_group(sig: &str, pgid: u32) -> bool {
    Command::new("kill")
        .arg(format!("-{sig}"))
        .arg("--")
        .arg(format!("-{pgid}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

struct Entry {
    child: Option<Child>,
    status: AgentStatus,
    lines: Ring,
    readers: Vec<thread::JoinHandle<()>>,
    spec: AgentSpec,
    resumed: bool,
    events: Arc<AtomicUsize>,
    runtime: String,
    limited: Arc<Mutex<Option<i64>>>,
    last_error: Arc<Mutex<Option<String>>>,
    /// Launch facts and diagnosis (also published for the team page).
    rec: Arc<Mutex<LaunchRecord>>,
    /// Stopped on purpose: its exit is not a launch failure.
    stopped: bool,
}

/// Key of the member's session in `sessions.json`: plain member for Claude, `member@runtime`
/// for other runtimes (their session ids are not interchangeable).
fn session_key(member: &str, runtime: &str) -> String {
    if runtime == "claude" {
        member.to_string()
    } else {
        format!("{member}@{runtime}")
    }
}

/// Launches and tracks agent processes by member name.
pub struct Supervisor {
    cfg: LaunchConfig,
    agents: BTreeMap<String, Entry>,
    sessions: Sessions,
}

impl Supervisor {
    pub fn new(cfg: LaunchConfig) -> Self {
        let sessions = load_sessions(&Self::sessions_path(&cfg));
        {
            // Earlier runs feed the per-task logs; runs from now on come in live.
            let (home, team, before) = (cfg.home.clone(), cfg.team.clone(), unix_ms() as i64);
            thread::spawn(move || task_logs::load_history(&home, &team, before));
        }
        Supervisor {
            cfg,
            agents: BTreeMap::new(),
            sessions: Arc::new(Mutex::new(sessions)),
        }
    }

    fn team_dir(cfg: &LaunchConfig) -> PathBuf {
        cfg.home.join(&cfg.team)
    }

    fn sessions_path(cfg: &LaunchConfig) -> PathBuf {
        Self::team_dir(cfg).join("sessions.json")
    }

    /// Saved Claude session id for the member, if any.
    #[cfg(test)]
    pub fn session(&self, member: &str) -> Option<String> {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(member)
            .cloned()
    }

    fn clear_key(&mut self, key: &str) {
        let mut m = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        if m.remove(key).is_some() {
            save_sessions(&Self::sessions_path(&self.cfg), &m);
        }
    }

    #[cfg(test)]
    /// Recorded run files for the member (`<home>/<team>/runs/<member>/<unix_ms>.jsonl`),
    /// newest first.
    pub fn runs(&self, member: &str) -> Vec<PathBuf> {
        let dir = Self::team_dir(&self.cfg)
            .join("runs")
            .join(safe_name(member));
        let mut v: Vec<(u128, PathBuf)> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .filter_map(|p| {
                let ms = p.file_stem()?.to_str()?.parse().ok()?;
                Some((ms, p))
            })
            .collect();
        v.sort_by_key(|x| std::cmp::Reverse(x.0));
        v.into_iter().map(|(_, p)| p).collect()
    }

    /// Number of agents currently running.
    /// Permission policy for launches from now on (running agents keep theirs).
    pub fn set_permissions(&mut self, p: Permissions) {
        self.cfg.permissions = p;
    }

    pub fn running_count(&self) -> usize {
        self.agents
            .values()
            .filter(|e| matches!(e.status, AgentStatus::Running { .. }))
            .count()
    }

    fn open_run_file(&self, member: &str) -> Option<File> {
        let dir = Self::team_dir(&self.cfg)
            .join("runs")
            .join(safe_name(member));
        std::fs::create_dir_all(&dir).ok()?;
        let mut ms = unix_ms();
        loop {
            match File::options()
                .write(true)
                .create_new(true)
                .open(dir.join(format!("{ms}.jsonl")))
            {
                Ok(f) => return Some(f),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => ms += 1,
                Err(_) => return None,
            }
        }
    }

    /// Spawn the agent. Refused while that member is running; an exited entry is replaced.
    /// With `spec.resume` the member's saved session (if any) is passed as `--resume`.
    pub fn launch(&mut self, mut spec: AgentSpec) -> Result<(), String> {
        if let Some(e) = self.agents.get(&spec.member) {
            if matches!(e.status, AgentStatus::Running { .. }) {
                return Err(format!("{} is already running", spec.member));
            }
        }
        if spec.worktree {
            if let Some(project) = spec.project.clone() {
                if let Some(why) = diagnostics::readiness(&project).blocker() {
                    let msg = format!("worktree not ready: {why}");
                    return Err(self.reject(spec, msg, Cause::WorktreeNotReady));
                }
                match crate::git::ensure_worktree(
                    &project,
                    &self.cfg.home,
                    &self.cfg.team,
                    &spec.member,
                ) {
                    Ok(w) => spec.workdir = w,
                    Err(e) => return Err(self.reject(spec, e, Cause::WorktreeNotReady)),
                }
            }
        }
        if !Path::new(&spec.workdir).is_dir() {
            let msg = format!("workdir {} is not a directory", spec.workdir.display());
            return Err(self.reject(spec, msg, Cause::WorkdirMissing));
        }
        let rt: Option<Arc<dyn AgentRuntime>> =
            runtimes::get(spec.runtime.as_deref()).map(Arc::from);
        let rt_id = rt.as_ref().map_or("claude", |r| r.id()).to_string();
        let key = session_key(&spec.member, &rt_id);
        let can_resume = rt.as_ref().is_none_or(|r| r.supports_resume());
        spec.session = if spec.resume && can_resume {
            self.sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&key)
                .cloned()
        } else {
            None
        };
        let resumed = spec.session.is_some();
        let bin_name = if rt_id == "claude" {
            self.cfg.claude_bin.clone()
        } else {
            rt_id.clone()
        };
        let mut cmd = command_for(&self.cfg, &spec, rt.as_deref());
        cmd.envs(spec.env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let cause = if e.kind() == std::io::ErrorKind::NotFound {
                    Cause::CliNotFound
                } else {
                    Cause::Unknown
                };
                let msg = format!("spawn {bin_name}: {e}");
                return Err(self.reject(spec, msg, cause));
            }
        };
        track_group(child.id(), true);
        task_logs::reset_member(&self.cfg.team, &spec.member);
        crate::context_usage::reset(&self.cfg.team, &spec.member);
        let ctx_window =
            crate::context_usage::window_for(spec.model.as_deref(), spec.context_limit);
        let lines: Ring = Arc::default();
        let events = Arc::new(AtomicUsize::new(0));
        let rec = Arc::new(Mutex::new(LaunchRecord::new(
            unix_ms() as i64,
            events.clone(),
        )));
        diagnostics::publish(&self.cfg.team, &spec.member, rec.clone());
        let limited: Arc<Mutex<Option<i64>>> = Arc::default();
        let last_error: Arc<Mutex<Option<String>>> = Arc::default();
        let mut readers = Vec::new();
        if let Some(out) = child.stdout.take() {
            let ring = lines.clone();
            let events = events.clone();
            let sessions = self.sessions.clone();
            let spath = Self::sessions_path(&self.cfg);
            let rt = rt.clone();
            let limited = limited.clone();
            let last_error = last_error.clone();
            let mut tee = self.open_run_file(&spec.member);
            let (team, member) = (self.cfg.team.clone(), spec.member.clone());
            let rt_name = rt_id.clone();
            readers.push(thread::spawn(move || {
                // Only Claude's window is known from the model; others show tokens alone.
                let window = (rt_name == "claude").then_some(ctx_window);
                for l in BufReader::new(out).lines().map_while(Result::ok) {
                    task_logs::ingest(&team, &member, unix_ms() as i64, &l);
                    crate::context_usage::ingest(&team, &member, &rt_name, window, &l);
                    if let Some(f) = tee.as_mut() {
                        let _ = writeln!(f, "{l}");
                    }
                    if !l.trim().is_empty() {
                        events.fetch_add(1, Ordering::SeqCst);
                    }
                    let id = match rt.as_deref() {
                        Some(r) => r.session_id(&l),
                        None => claude_session_id(&l),
                    };
                    if let Some(id) = id.filter(|s| !s.is_empty()) {
                        let mut m = sessions.lock().unwrap_or_else(|e| e.into_inner());
                        if m.get(&key) != Some(&id) {
                            m.insert(key.clone(), id);
                            save_sessions(&spath, &m);
                        }
                    }
                    if !is_conversation_line(&l) {
                        note_limit(&limited, &l);
                    }
                    let shown = match rt.as_deref() {
                        Some(r) => r.summarize_line(&l),
                        None => summarize_line(&l),
                    };
                    for s in shown {
                        if let Some(e) = s.strip_prefix("error:") {
                            set_last(&last_error, e);
                        }
                        push(&ring, s);
                    }
                }
            }));
        }
        if let Some(err) = child.stderr.take() {
            let ring = lines.clone();
            let limited = limited.clone();
            let last_error = last_error.clone();
            readers.push(thread::spawn(move || {
                for l in BufReader::new(err).lines().map_while(Result::ok) {
                    note_limit(&limited, &l);
                    set_last(&last_error, &l);
                    push(&ring, format!("stderr: {l}"));
                }
            }));
        }
        let status = AgentStatus::Running { pid: child.id() };
        self.agents.insert(
            spec.member.clone(),
            Entry {
                child: Some(child),
                status,
                lines,
                readers,
                spec,
                resumed,
                events,
                runtime: rt_id,
                limited,
                last_error,
                rec,
                stopped: false,
            },
        );
        Ok(())
    }

    /// Record a launch that never got a process: the member shows as failed with a diagnosis.
    /// Returns the message to hand back as the launch error.
    fn reject(&mut self, spec: AgentSpec, msg: String, cause: Cause) -> String {
        let line = format!("error: {msg}");
        let lines: Ring = Arc::default();
        push(&lines, line.clone());
        let mut rec = LaunchRecord::new(unix_ms() as i64, Arc::default());
        rec.ended_ms = rec.started_ms.into();
        rec.rejected = true;
        rec.diag = Some(LaunchDiag {
            cause,
            code: None,
            detail: Some(line.clone()),
            tail: vec![line],
        });
        let rec = Arc::new(Mutex::new(rec));
        diagnostics::publish(&self.cfg.team, &spec.member, rec.clone());
        let rt = spec.runtime.clone().unwrap_or_else(|| "claude".into());
        self.agents.insert(
            spec.member.clone(),
            Entry {
                child: None,
                status: AgentStatus::Failed(msg.clone()),
                lines,
                readers: Vec::new(),
                spec,
                resumed: false,
                events: Arc::default(),
                runtime: rt,
                limited: Arc::default(),
                last_error: Arc::new(Mutex::new(Some(msg.clone()))),
                rec,
                stopped: false,
            },
        );
        msg
    }

    /// Launch record of the member's latest launch (this supervisor's).
    #[cfg(test)]
    pub fn launch_record(&self, member: &str) -> Option<LaunchRecord> {
        let e = self.agents.get(member)?;
        let g = e.rec.lock().unwrap_or_else(|p| p.into_inner());
        Some(g.clone())
    }

    /// Terminate the member's whole process group (SIGTERM, then SIGKILL after ~2s if anything
    /// is still alive) without blocking. No-op if not running. Status settles on `poll()`.
    pub fn stop(&mut self, member: &str) {
        let Some(e) = self.agents.get_mut(member) else {
            return;
        };
        let Some(child) = e.child.as_mut() else {
            return;
        };
        e.stopped = true;
        let pgid = child.id();
        if !signal_group("TERM", pgid) {
            let _ = child.kill();
        }
        thread::spawn(move || {
            for _ in 0..40 {
                thread::sleep(Duration::from_millis(50));
                if !signal_group("0", pgid) {
                    return;
                }
            }
            signal_group("KILL", pgid);
        });
    }

    /// Stop the member (SIGKILL to its group, briefly blocking to reap) and launch it again
    /// with its last spec. Errors if the member was never launched.
    pub fn restart(&mut self, member: &str) -> Result<(), String> {
        let Some(e) = self.agents.get_mut(member) else {
            return Err(format!("{member} was never launched"));
        };
        if let Some(mut c) = e.child.take() {
            if !signal_group("KILL", c.id()) {
                let _ = c.kill();
            }
            let _ = c.wait();
            e.status = AgentStatus::Exited { code: None };
            for h in e.readers.drain(..) {
                let _ = h.join();
            }
        }
        let spec = e.spec.clone();
        self.launch(spec)
    }

    /// Reap exited children without blocking.
    pub fn poll(&mut self) {
        let mut stale = Vec::new();
        for (member, e) in self.agents.iter_mut() {
            let Some(child) = e.child.as_mut() else {
                continue;
            };
            let pgid = child.id();
            match child.try_wait() {
                Ok(None) => {}
                Ok(Some(st)) => {
                    e.status = AgentStatus::Exited { code: st.code() };
                    e.child = None;
                    track_group(pgid, false);
                }
                Err(err) => {
                    e.status = AgentStatus::Failed(err.to_string());
                    e.child = None;
                    track_group(pgid, false);
                }
            }
            if e.child.is_none() {
                // Pipes hit EOF once the child is gone; let readers drain what is buffered.
                for h in e.readers.drain(..) {
                    let _ = h.join();
                }
                if !e.stopped {
                    let now = unix_ms() as i64;
                    let lines: Vec<String> = e
                        .lines
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .iter()
                        .cloned()
                        .collect();
                    let code = match e.status {
                        AgentStatus::Exited { code } => code,
                        _ => None,
                    };
                    let mut rec = e.rec.lock().unwrap_or_else(|p| p.into_inner());
                    if rec.ended_ms.is_none() {
                        rec.ended_ms = Some(now);
                        let env = DiagEnv {
                            cli_found: true,
                            workdir_ok: e.spec.workdir.is_dir(),
                        };
                        rec.diag = diagnostics::diagnose_exit(
                            code,
                            now - rec.started_ms,
                            e.events.load(Ordering::SeqCst),
                            &lines,
                            env,
                        );
                    }
                } else if let Ok(mut rec) = e.rec.lock() {
                    rec.ended_ms.get_or_insert(unix_ms() as i64);
                }
                // A resumed run that failed straight away: the session is unusable.
                let failed = matches!(e.status, AgentStatus::Exited { code: Some(c) } if c != 0);
                if e.resumed && failed && e.events.load(Ordering::SeqCst) <= 1 {
                    stale.push(session_key(member, &e.runtime));
                }
            }
        }
        for m in stale {
            self.clear_key(&m);
        }
    }

    /// Snapshot of all agents, sorted by member.
    pub fn agents(&self) -> Vec<AgentView> {
        self.agents
            .iter()
            .map(|(member, e)| AgentView {
                member: member.clone(),
                status: e.status.clone(),
                last_lines: e
                    .lines
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .iter()
                    .cloned()
                    .collect(),
                rate_limited_until: *e.limited.lock().unwrap_or_else(|p| p.into_inner()),
                last_error: e
                    .last_error
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone(),
                runtime: e.runtime.clone(),
            })
            .collect()
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        for e in self.agents.values_mut() {
            if let Some(c) = e.child.as_mut() {
                if !signal_group("KILL", c.id()) {
                    let _ = c.kill();
                }
                let _ = c.wait();
                track_group(c.id(), false);
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rteams-launcher-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn spec(dir: &Path, member: &str) -> AgentSpec {
        AgentSpec {
            member: member.into(),
            role: "dev".into(),
            workdir: dir.to_path_buf(),
            prompt: "go".into(),
            ..Default::default()
        }
    }

    fn fake_claude(dir: &Path, body: &str) -> LaunchConfig {
        let p = dir.join("fake-claude");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut cfg = LaunchConfig::new(dir.to_path_buf(), "t");
        cfg.claude_bin = p.to_string_lossy().into_owned();
        cfg
    }

    fn wait_until(sup: &mut Supervisor, f: impl Fn(&AgentView) -> bool) -> AgentView {
        for _ in 0..200 {
            sup.poll();
            let v = sup.agents().remove(0);
            if f(&v) {
                return v;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("timed out: {:?}", sup.agents());
    }

    fn args_of(cfg: &LaunchConfig, spec: &AgentSpec) -> Vec<String> {
        build_command(cfg, spec)
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn cfg_plain() -> LaunchConfig {
        let mut cfg = LaunchConfig::new(PathBuf::from("/h"), "tm");
        cfg.rteams_bin = "/bin/rteams".into();
        cfg
    }

    #[test]
    fn mode_and_permission_flags() {
        let mut cfg = cfg_plain();
        let w = Path::new("/w");
        let mut s = spec(w, "a");
        let a = args_of(&cfg, &s);
        assert!(a.contains(&"--allowedTools=mcp__rteams".into()));
        assert!(!a.iter().any(|x| x.starts_with("--disallowedTools")));
        assert!(a
            .windows(2)
            .any(|p| p == ["--permission-mode", "acceptEdits"]));
        assert!(!a.contains(&"--model".into()) && !a.contains(&"--resume".into()));

        s.mode = Mode::Ask;
        let a = args_of(&cfg, &s);
        assert!(a.contains(&"--allowedTools=mcp__rteams,Read,Grep,Glob".into()));
        assert!(a.contains(&format!("--disallowedTools={NO_EDIT}")));
        assert!(a.windows(2).any(|p| p == ["--permission-mode", "default"]));
        assert!(a.last().unwrap().contains("answer only, change nothing"));

        s.mode = Mode::Delegate;
        let a = args_of(&cfg, &s);
        assert!(a.contains(&"--allowedTools=mcp__rteams".into()));
        assert!(a.contains(&format!("--disallowedTools={NO_EDIT}")));
        assert!(a.last().unwrap().contains("do not do the work yourself"));

        s.mode = Mode::Do;
        s.model = Some("sonnet".into());
        s.instructions = Some("Always run clippy.".into());
        s.session = Some("sid-9".into());
        let a = args_of(&cfg, &s);
        assert!(a.contains(&"--allowedTools=mcp__rteams".into()));
        assert!(a.windows(2).any(|p| p == ["--model", "sonnet"]));
        assert!(a.windows(2).any(|p| p == ["--resume", "sid-9"]));
        assert!(a.last().unwrap().ends_with("Always run clippy."));
        s.mode = Mode::Ask;
        assert!(args_of(&cfg, &s).contains(&"--allowedTools=mcp__rteams,Read,Grep,Glob".into()));

        s.resume = false;
        assert!(!args_of(&cfg, &s).contains(&"--resume".into()));
        s.mode = Mode::Do;
        cfg.permissions = Permissions::SkipAll;
        let a = args_of(&cfg, &s);
        assert!(a.contains(&"--dangerously-skip-permissions".into()));
        assert!(!a.contains(&"--permission-mode".into()));
    }

    #[test]
    fn discovery_picks_existing_candidate() {
        let dir = tmp("disc");
        let missing = dir.join("none/claude");
        let present = dir.join("bin/claude");
        std::fs::create_dir_all(present.parent().unwrap()).unwrap();
        std::fs::write(&present, "").unwrap();
        let c = vec![missing, present.clone()];
        assert_eq!(
            find_claude(Some(OsStr::new("/nonexistent")), &c),
            Some(present)
        );
        let on_path = std::env::join_paths([dir.join("bin")]).unwrap();
        assert_eq!(
            find_claude(Some(&on_path), &[]),
            Some(dir.join("bin/claude"))
        );
        assert_eq!(find_claude(None, &[dir.join("zzz")]), None);
        assert!(claude_candidates(Some(Path::new("/u")))
            .contains(&PathBuf::from("/u/.local/bin/claude")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_captured_persisted_and_resumed() {
        let dir = tmp("sess");
        let body = format!(
            "printf '%s\\n' \"$@\" > {d}/args.$$\n\
             echo '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-1\"}}'\n\
             echo '{{\"type\":\"result\",\"result\":\"ok\"}}'",
            d = dir.display()
        );
        let mut sup = Supervisor::new(fake_claude(&dir, &body));
        sup.launch(spec(&dir, "bob")).unwrap();
        wait_until(&mut sup, |v| matches!(v.status, AgentStatus::Exited { .. }));
        assert_eq!(sup.session("bob").as_deref(), Some("sess-1"));
        let saved = std::fs::read_to_string(dir.join("t/sessions.json")).unwrap();
        assert!(saved.contains("sess-1"));
        // A fresh supervisor reads it back and passes --resume.
        let mut sup2 = Supervisor::new(fake_claude(&dir, &body));
        assert_eq!(sup2.session("bob").as_deref(), Some("sess-1"));
        sup2.launch(spec(&dir, "bob")).unwrap();
        wait_until(&mut sup2, |v| {
            matches!(v.status, AgentStatus::Exited { .. })
        });
        let found = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("args."))
            .map(|e| std::fs::read_to_string(e.path()).unwrap())
            .any(|s| s.contains("--resume\nsess-1\n"));
        assert!(found, "second launch should resume");
        // Two runs recorded, newest first, containing raw lines.
        let runs = sup2.runs("bob");
        assert_eq!(runs.len(), 2);
        assert!(std::fs::read_to_string(&runs[0])
            .unwrap()
            .contains("\"session_id\":\"sess-1\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn context_usage_tracked_from_stream() {
        let dir = tmp("ctxusage");
        let body = r#"echo '{"type":"assistant","message":{"content":[],"usage":{"input_tokens":10,"cache_creation_input_tokens":20,"cache_read_input_tokens":70}}}'"#;
        let mut sup = Supervisor::new(fake_claude(&dir, body));
        let mut s = spec(&dir, "ctxbob");
        s.model = Some("opus[1m]".into());
        sup.launch(s).unwrap();
        wait_until(&mut sup, |v| matches!(v.status, AgentStatus::Exited { .. }));
        let u = crate::context_usage::get("t", "ctxbob").unwrap();
        assert_eq!((u.tokens, u.window), (100, Some(1_000_000)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_resume_clears_session() {
        let dir = tmp("badresume");
        std::fs::create_dir_all(dir.join("t")).unwrap();
        std::fs::write(dir.join("t/sessions.json"), r#"{"bob":"gone"}"#).unwrap();
        let mut sup = Supervisor::new(fake_claude(&dir, "echo 'no such session' >&2\nexit 1"));
        assert_eq!(sup.session("bob").as_deref(), Some("gone"));
        sup.launch(spec(&dir, "bob")).unwrap();
        wait_until(&mut sup, |v| {
            v.status == AgentStatus::Exited { code: Some(1) }
        });
        assert_eq!(sup.session("bob"), None);
        assert!(!std::fs::read_to_string(dir.join("t/sessions.json"))
            .unwrap()
            .contains("gone"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stop_kills_process_group_and_restart_relaunches() {
        let dir = tmp("group");
        let pidfile = dir.join("sleep.pid");
        let body = format!("sleep 30 &\necho $! > {}\nwait", pidfile.display());
        let mut sup = Supervisor::new(fake_claude(&dir, &body));
        sup.launch(spec(&dir, "g")).unwrap();
        assert_eq!(sup.running_count(), 1);
        let mut pid = String::new();
        for _ in 0..200 {
            pid = std::fs::read_to_string(&pidfile).unwrap_or_default();
            if pid.trim().len() > 1 {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        let pid = pid.trim().to_string();
        let alive = |p: &str| {
            Command::new("kill")
                .args(["-0", p])
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        assert!(alive(&pid));
        sup.stop("g");
        wait_until(&mut sup, |v| matches!(v.status, AgentStatus::Exited { .. }));
        assert_eq!(sup.running_count(), 0);
        for _ in 0..100 {
            if !alive(&pid) {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        assert!(!alive(&pid), "grandchild should die with the group");
        // restart relaunches with the last spec.
        sup.restart("g").unwrap();
        assert_eq!(sup.running_count(), 1);
        assert!(sup.restart("nobody").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extra_args_validation() {
        assert_eq!(
            split_args(r#"--add-dir "/a b" -x 'q r'"#).unwrap(),
            ["--add-dir", "/a b", "-x", "q r"]
        );
        assert!(split_args("--x \"open").is_err());
        let help = "  --add-dir <dirs>  Extra dirs\n  --model <m>\n  --fallback-model <m>";
        assert_eq!(
            validate_extra_args("--add-dir /tmp", Some(help)).unwrap(),
            ["--add-dir", "/tmp"]
        );
        assert!(validate_extra_args("--model=x", Some(help)).is_ok());
        assert!(
            validate_extra_args("--fallback", Some(help)).is_err(),
            "prefix is not a flag"
        );
        assert!(validate_extra_args("--resume abc", None)
            .unwrap_err()
            .contains("--resume"));
        assert!(validate_extra_args("--dangerously-skip-permissions", Some(help)).is_err());
        assert!(validate_extra_args(&"a".repeat(3000), None).is_err());
        let spec = AgentSpec {
            extra_args: vec!["--add-dir".into(), "/tmp".into()],
            ..Default::default()
        };
        let cfg = LaunchConfig::new(PathBuf::from("/tmp/h"), "t");
        let args: Vec<String> = build_command(&cfg, &spec)
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let i = args.iter().position(|a| a == "--add-dir").unwrap();
        assert_eq!(args[i + 1], "/tmp");
        assert_eq!(args[i + 2], "--append-system-prompt");
    }

    #[test]
    fn command_args() {
        let cfg = cfg_plain();
        let cmd = build_command(&cfg, &spec(Path::new("/w"), "alice"));
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let mcp = args
            .iter()
            .find_map(|a| a.strip_prefix("--mcp-config="))
            .unwrap();
        let v: Value = serde_json::from_str(mcp).unwrap();
        assert_eq!(v["mcpServers"]["rteams"]["command"], "/bin/rteams");
        assert_eq!(
            v["mcpServers"]["rteams"]["args"],
            json!(["serve", "--home", "/h", "--team", "tm"])
        );
        assert!(args.contains(&"--allowedTools=mcp__rteams".to_string()));
        assert!(args.contains(&"--output-format=stream-json".to_string()));
        assert_eq!(args[..2], ["-p", "go"]);
        let sp = args.last().unwrap();
        assert!(sp.contains("alice") && sp.contains("member_briefing") && sp.contains("/w"));
        assert!(sp.contains("Do NOT send messages"));
        assert!(!sp.contains("inbox_get") && !sp.contains("Poll"));
        assert!(sp.lines().count() <= 12);
        assert!(sp.contains("at most 3 tasks"));
        assert!(sp.contains("ONLY bullet points"));
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/w")));
    }

    #[test]
    fn launch_options_map_to_flags() {
        let cfg = cfg_plain();
        let mut s = spec(Path::new("/w"), "dev");
        s.effort = Some("high".into());
        s.fast = true;
        s.model = Some("sonnet[1m]".into());
        s.context_limit = Some(200_000);
        s.permissions = Some(Permissions::SkipAll);
        let a = args_of(&cfg, &s);
        assert!(a.windows(2).any(|w| w == ["--effort", "high"]));
        assert!(a.windows(2).any(|w| w == ["--model", "sonnet"]));
        assert!(a.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(a.iter().any(|x| x.contains("fastMode")));
        s.effort = Some("bogus".into());
        assert!(!args_of(&cfg, &s).contains(&"--effort".to_string()));
    }

    #[test]
    fn mcp_allow_ignores_unknown_servers() {
        assert!(
            resolve_mcp_allow(&["no-such-server-xyz".into(), "rteams".into()], None).is_empty()
        );
        let mut s = spec(Path::new("/w"), "dev");
        s.mcp_allow = vec!["no-such-server-xyz".into()];
        let a = args_of(&cfg_plain(), &s);
        assert!(a.contains(&"--allowedTools=mcp__rteams".to_string()));
    }

    #[test]
    fn prompt_tells_agents_to_heartbeat() {
        let mut s = spec(Path::new("/w"), "dev");
        s.role = "dev".into();
        assert!(system_prompt(&s).contains("runtime_heartbeat"));
    }

    #[test]
    fn summarize() {
        let a = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi\nthere"},{"type":"tool_use","name":"mcp__rteams__inbox_get","input":{"member":"a"}}]}}"#;
        assert_eq!(
            summarize_line(a),
            vec![
                "hi there".to_string(),
                r#"tool: inbox_get {"member":"a"}"#.to_string()
            ]
        );
        let r = r#"{"type":"result","subtype":"success","is_error":false,"result":"done"}"#;
        assert_eq!(summarize_line(r), vec!["result: done".to_string()]);
        assert_eq!(summarize_line("not json"), vec!["not json".to_string()]);
        assert!(summarize_line("").is_empty());
    }

    #[test]
    fn runs_and_exits() {
        let dir = tmp("run");
        let body = r#"echo '{"type":"assistant","message":{"content":[{"type":"text","text":"hello"}]}}'
echo '{"type":"result","result":"fin"}'
echo oops >&2"#;
        let mut sup = Supervisor::new(fake_claude(&dir, body));
        sup.launch(spec(&dir, "bob")).unwrap();
        let first = sup.agents().remove(0);
        assert!(
            matches!(first.status, AgentStatus::Running { .. })
                || first.status == AgentStatus::Exited { code: Some(0) }
        );
        let v = wait_until(&mut sup, |v| {
            v.status == AgentStatus::Exited { code: Some(0) }
        });
        assert!(v.last_lines.contains(&"hello".to_string()));
        assert!(v.last_lines.contains(&"result: fin".to_string()));
        assert!(v.last_lines.contains(&"stderr: oops".to_string()));
        // Relaunching an exited member is allowed.
        sup.launch(spec(&dir, "bob")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stop_kills_and_refuses_duplicates() {
        let dir = tmp("stop");
        let mut sup = Supervisor::new(fake_claude(&dir, "exec sleep 30"));
        sup.launch(spec(&dir, "c")).unwrap();
        assert!(sup.launch(spec(&dir, "c")).is_err());
        sup.stop("c");
        let v = wait_until(&mut sup, |v| matches!(v.status, AgentStatus::Exited { .. }));
        assert_eq!(v.status, AgentStatus::Exited { code: None });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_binary_is_error() {
        let dir = tmp("missing");
        let mut cfg = LaunchConfig::new(dir.clone(), "t");
        cfg.claude_bin = dir.join("nope").to_string_lossy().into_owned();
        let mut sup = Supervisor::new(cfg);
        assert!(sup.launch(spec(&dir, "d")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn mcp_of(a: &[String]) -> Value {
        let m = a
            .iter()
            .find_map(|x| x.strip_prefix("--mcp-config="))
            .unwrap();
        serde_json::from_str(m).unwrap()
    }

    #[test]
    fn mcp_env_has_member() {
        let a = args_of(&cfg_plain(), &spec(Path::new("/w"), "alice"));
        assert_eq!(
            mcp_of(&a)["mcpServers"]["rteams"]["env"]["RTEAMS_MEMBER"],
            "alice"
        );
    }

    #[test]
    fn manual_permissions_use_prompt_tool() {
        let mut cfg = cfg_plain();
        cfg.permissions = Permissions::Manual;
        let a = args_of(&cfg, &spec(Path::new("/w"), "a"));
        assert!(a.windows(2).any(|p| p
            == [
                "--permission-prompt-tool",
                "mcp__rteams__permission_request"
            ]));
        assert!(!a.contains(&"--dangerously-skip-permissions".into()));
    }

    #[test]
    fn solo_prompt() {
        let mut s = spec(Path::new("/w"), "a");
        assert!(!system_prompt(&s).contains("only agent on this team"));
        s.solo = true;
        assert!(system_prompt(&s).contains(
            "You are the only agent on this team: own every task on the board yourself; \
             create tasks for multi-step work."
        ));
    }

    #[test]
    fn lead_agent_prompt() {
        let mut s = spec(Path::new("/w"), "a");
        assert!(!system_prompt(&s).contains("YOU LEAD THIS TEAM"));
        s.role = "lead".into();
        let p = system_prompt(&s);
        assert!(p.contains("YOU LEAD THIS TEAM") && p.contains("assign each"));
        // A solo agent does everything itself, lead or not.
        s.solo = true;
        assert!(!system_prompt(&s).contains("YOU LEAD THIS TEAM"));
    }

    #[test]
    fn rate_limit_detection() {
        let now = 1_700_000_000_000i64;
        let b = RATE_BUFFER_MS;
        assert_eq!(rate_limit_reset("all good", now), None);
        assert_eq!(rate_limit_reset("I talked about nothing", now), None);
        // Default one hour.
        assert_eq!(
            rate_limit_reset("Claude usage limit reached", now),
            Some(now + RATE_DEFAULT_MS + b)
        );
        // Relative.
        assert_eq!(
            rate_limit_reset("Rate limit hit. Try again in 2 hours 30 minutes.", now),
            Some(now + 9_000_000 + b)
        );
        assert_eq!(
            rate_limit_reset("rate limit: try again in 45 seconds", now),
            Some(now + 45_000 + b)
        );
        // Capped at 12h.
        assert_eq!(
            rate_limit_reset("usage limit, try again in 5 days", now),
            Some(now + RATE_CAP_MS)
        );
        // JSON timestamps (seconds and ms).
        let j = format!(
            r#"{{"type":"rate_limit_event","rate_limit_info":{{"status":"rejected","resetsAt":{}}}}}"#,
            now / 1000 + 600
        );
        assert_eq!(rate_limit_reset(&j, now), Some(now + 600_000 + b));
        let j = format!(r#"{{"error":"rate limit","reset_at":{}}}"#, now + 120_000);
        assert_eq!(rate_limit_reset(&j, now), Some(now + 120_000 + b));
        // Allowed status is not a limit.
        let ok = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1700000500}}"#;
        assert_eq!(rate_limit_reset(ok, now), None);
        // Legacy pipe timestamp.
        assert_eq!(
            rate_limit_reset("Claude AI usage limit reached|1700003000", now),
            Some(1_700_003_000_000 + b)
        );
        // Clock time, UTC offset 0: now is 22:13:20 UTC, so 3pm is next day.
        assert_eq!(
            rate_limit_at("You've hit your limit - resets 3pm (UTC)", now, 0),
            Some(now + RATE_CAP_MS) // 16.7h away, capped
        );
        let t = rate_limit_at("usage limit reached, resets at 11:30pm", now, 0).unwrap();
        assert_eq!(t, (1_700_000_000 - 80_000 + 84_600) * 1000 + b);
        // Offset shifts the instant.
        let t2 = rate_limit_at("usage limit reached, resets at 11:30pm", now, 3600).unwrap();
        assert_eq!(t2, t - 3_600_000);
    }

    #[test]
    fn last_error_and_runtime_in_view() {
        let dir = tmp("lasterr");
        let body = "echo 'first' >&2\necho 'rate limit reached, try again in 10 minutes' >&2";
        let mut sup = Supervisor::new(fake_claude(&dir, body));
        sup.launch(spec(&dir, "e")).unwrap();
        let v = wait_until(&mut sup, |v| matches!(v.status, AgentStatus::Exited { .. }));
        assert_eq!(v.runtime, "claude");
        assert!(v.last_error.as_deref().unwrap().contains("try again in"));
        let until = v.rate_limited_until.unwrap();
        let now = unix_ms() as i64;
        assert!(until > now + 9 * 60_000 && until < now + 11 * 60_000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn last_error_from_result_line() {
        let dir = tmp("resulterr");
        let body = r#"echo '{"type":"result","is_error":true,"result":"it broke"}'"#;
        let mut sup = Supervisor::new(fake_claude(&dir, body));
        sup.launch(spec(&dir, "e")).unwrap();
        let v = wait_until(&mut sup, |v| matches!(v.status, AgentStatus::Exited { .. }));
        assert_eq!(v.last_error.as_deref(), Some("it broke"));
        assert_eq!(v.rate_limited_until, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_registry_falls_back_to_claude_builder() {
        let cfg = cfg_plain();
        let mut s = spec(Path::new("/w"), "a");
        s.runtime = Some("codex".into());
        // No runtime resolved (empty registry): Claude's builder, whatever the spec asks for.
        let cmd = command_for(&cfg, &s, None);
        assert_eq!(cmd.get_program(), OsStr::new(&cfg.claude_bin));
        assert_eq!(cmd.get_args().next().unwrap(), "-p");
        let rt = runtimes::get(s.runtime.as_deref());
        let cmd = command_for(&cfg, &s, rt.as_deref());
        if runtimes::all().is_empty() {
            assert!(rt.is_none());
            assert_eq!(cmd.get_program(), OsStr::new(&cfg.claude_bin));
            assert_eq!(cmd.get_args().next().unwrap(), "-p");
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let st = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?}");
    }

    #[test]
    fn worktree_is_the_cwd() {
        let dir = tmp("wt");
        let project = dir.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        git(&project, &["init", "-q"]);
        std::fs::write(project.join("f"), "x").unwrap();
        git(&project, &["add", "."]);
        git(&project, &["commit", "-q", "-m", "init"]);
        let out = dir.join("pwd.txt");
        let body = format!("pwd > {}", out.display());
        let mut sup = Supervisor::new(fake_claude(&dir, &body));
        let mut s = spec(&project, "wt-a");
        s.worktree = true;
        s.project = Some(project.clone());
        sup.launch(s).unwrap();
        wait_until(&mut sup, |v| matches!(v.status, AgentStatus::Exited { .. }));
        let cwd = std::fs::read_to_string(&out).unwrap();
        let cwd = std::fs::canonicalize(cwd.trim()).unwrap();
        let want = std::fs::canonicalize(dir.join("t/wt/wt-a")).unwrap();
        assert_eq!(cwd, want);
        // A non-repo project fails the launch with git's message.
        let mut bad = spec(&dir, "wt-b");
        bad.worktree = true;
        bad.project = Some(dir.join("nope"));
        assert!(sup.launch(bad).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_exit_is_diagnosed_from_stderr() {
        let dir = tmp("diag-exit");
        let cfg = fake_claude(&dir, "echo \"error: unknown option '--bogus'\" >&2\nexit 2");
        let mut sup = Supervisor::new(cfg);
        sup.launch(spec(&dir, "ada")).unwrap();
        wait_until(&mut sup, |a| {
            !matches!(a.status, AgentStatus::Running { .. })
        });
        let rec = sup.launch_record("ada").unwrap();
        let d = rec.diag.unwrap();
        assert_eq!(d.cause, Cause::UnknownFlag);
        assert_eq!(d.code, Some(2));
        assert!(d.tail.iter().any(|l| l.contains("--bogus")));
        assert!(rec.ended_ms.is_some() && !rec.rejected);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clean_run_and_stopped_run_are_not_failures() {
        let dir = tmp("diag-ok");
        let cfg = fake_claude(
            &dir,
            "for i in 1 2 3; do echo '{\"type\":\"result\",\"result\":\"ok\"}'; done\nsleep 6",
        );
        let mut sup = Supervisor::new(cfg);
        sup.launch(spec(&dir, "ada")).unwrap();
        wait_until(&mut sup, |a| a.last_lines.len() >= 3);
        assert_eq!(
            SpawnState::of(true, &sup.launch_record("ada").unwrap(), None),
            SpawnState::Running
        );
        sup.stop("ada");
        wait_until(&mut sup, |a| {
            !matches!(a.status, AgentStatus::Running { .. })
        });
        assert!(sup.launch_record("ada").unwrap().diag.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_cli_and_workdir_are_recorded_as_failed_members() {
        let dir = tmp("diag-reject");
        let mut cfg = fake_claude(&dir, "exit 0");
        cfg.claude_bin = dir.join("nope").to_string_lossy().into_owned();
        let mut sup = Supervisor::new(cfg);
        assert!(sup.launch(spec(&dir, "ada")).is_err());
        let d = sup.launch_record("ada").unwrap().diag.unwrap();
        assert_eq!(d.cause, Cause::CliNotFound);
        assert!(matches!(sup.agents()[0].status, AgentStatus::Failed(_)));
        let missing = dir.join("gone");
        assert!(sup.launch(spec(&missing, "bob")).is_err());
        assert_eq!(
            sup.launch_record("bob").unwrap().diag.unwrap().cause,
            Cause::WorkdirMissing
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn worktree_members_are_blocked_until_the_repo_is_ready() {
        let dir = tmp("diag-wt");
        let project = dir.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let cfg = fake_claude(&dir, "exit 0");
        let mut sup = Supervisor::new(cfg);
        let mut s = spec(&project, "ada");
        s.worktree = true;
        s.project = Some(project.clone());
        let err = sup.launch(s.clone()).unwrap_err();
        assert!(err.contains("not a git repository"), "{err}");
        assert_eq!(
            sup.launch_record("ada").unwrap().diag.unwrap().cause,
            Cause::WorktreeNotReady
        );
        // A member without a worktree launches normally.
        let mut plain = spec(&project, "bob");
        plain.worktree = false;
        sup.launch(plain).unwrap();
        diagnostics::init_repo(&project).unwrap();
        assert!(sup.launch(s.clone()).unwrap_err().contains("no commit"));
        std::fs::write(project.join("f"), "x").unwrap();
        diagnostics::initial_commit(&project).unwrap();
        sup.launch(s).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
}
