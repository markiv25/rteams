//! Agent runtimes: the CLIs that can run a team member (Claude Code, Codex, OpenCode, ...),
//! as in agent-teams' runtime-provider-management. Each runtime knows how to find its binary,
//! report version and sign-in state, list models, build a one-shot non-interactive command
//! wired to the rteams MCP server, and turn its JSON output into display lines.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub mod claude;
pub mod codex;
pub mod compat;
pub mod copilot;
pub mod cursor;
pub mod gemini;
pub mod grok;
pub mod kiro;
pub mod opencode;
pub mod qwen;

/// Where and what version of a runtime's binary was found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detection {
    pub path: Option<PathBuf>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthStatus {
    SignedIn(Option<String>),
    SignedOut,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// Value passed to the CLI's model flag.
    pub id: String,
    pub label: String,
}

/// Everything a runtime needs to build one agent run.
#[derive(Debug, Clone, Default)]
pub struct LaunchCtx {
    pub bin: PathBuf,
    pub prompt: String,
    /// Role, rules and mode text (the launcher builds it).
    pub system_prompt: String,
    /// The rteams MCP server: `rteams serve --home .. --team ..` plus env (RTEAMS_MEMBER).
    pub mcp_command: String,
    pub mcp_args: Vec<String>,
    pub mcp_env: Vec<(String, String)>,
    pub model: Option<String>,
    /// `do` | `ask` | `delegate`.
    pub mode: String,
    /// `edits` | `skip` | `manual`.
    pub permissions: String,
    /// Session to resume, when the runtime supports it.
    pub resume: Option<String>,
    pub workdir: PathBuf,
    /// `low|medium|high|xhigh|max`, where the runtime has a reasoning-effort knob.
    pub effort: Option<String>,
    /// Fast mode, where the runtime has one.
    pub fast: bool,
    /// Context window cap in tokens, where the runtime can limit it.
    pub context_limit: Option<u32>,
    /// Extra MCP servers (name -> `{command,args,env}` / `{url}` JSON) beside `rteams`.
    pub extra_mcp: Vec<(String, serde_json::Value)>,
}

pub trait AgentRuntime: Send + Sync {
    /// Stable id stored on the member (`claude`, `codex`, ...).
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn detect(&self) -> Detection;
    fn auth_status(&self) -> AuthStatus;
    fn models(&self) -> Vec<ModelInfo>;
    /// Shell hint shown when the binary is missing.
    fn install_hint(&self) -> &'static str;
    fn build_command(&self, ctx: &LaunchCtx) -> Command;
    /// One output line -> zero or more short display lines (`tool: ..`, `result: ..`, text).
    fn summarize_line(&self, line: &str) -> Vec<String>;
    /// Session id announced on this output line, if any.
    fn session_id(&self, line: &str) -> Option<String>;
    fn supports_resume(&self) -> bool;
    /// Command that signs the user in (run in a terminal), when the CLI has one.
    fn login_command(&self) -> Option<&'static str> {
        None
    }
    /// Env var holding the provider API key, for runtimes that take one (shows "Connect").
    fn api_key_env(&self) -> Option<&'static str> {
        None
    }
}

// ---------------------------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------------------------

pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Directories searched after `PATH`: where CLI installers usually put binaries.
pub fn default_search_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = home_dir() {
        for d in [
            ".local/bin",
            ".npm-global/bin",
            ".opencode/bin",
            ".cursor/bin",
            ".bun/bin",
            ".cargo/bin",
            ".claude/local",
        ] {
            v.push(h.join(d));
        }
    }
    v.push(PathBuf::from("/opt/homebrew/bin"));
    v.push(PathBuf::from("/usr/local/bin"));
    v
}

/// First executable named one of `names` in `dirs` (pure: no PATH lookup).
pub fn which_in(names: &[&str], dirs: &[PathBuf]) -> Option<PathBuf> {
    for dir in dirs {
        for name in names {
            let p = dir.join(name);
            if is_executable(&p) {
                return Some(p);
            }
        }
    }
    None
}

fn is_executable(p: &Path) -> bool {
    let Ok(md) = std::fs::metadata(p) else {
        return false;
    };
    if !md.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        md.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Find a binary: `PATH` first, then `extra_candidates`, then the common install dirs.
pub fn which(names: &[&str], extra_candidates: &[PathBuf]) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(extra_candidates.iter().cloned());
    dirs.extend(default_search_dirs());
    which_in(names, &dirs)
}

/// Run `bin args` and return stdout if it exits successfully within `timeout`.
pub fn run_timeout(bin: &Path, args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = std::io::Read::read_to_end(&mut out, &mut s);
        let _ = tx.send(s);
    });
    let deadline = Instant::now() + timeout;
    let bytes = match rx.recv_timeout(timeout) {
        Ok(b) => b,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    let left = deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_millis(200));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if start.elapsed() < left => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    status
        .success()
        .then(|| String::from_utf8_lossy(&bytes).into_owned())
}

/// First non-empty line of `bin args` (3s timeout), e.g. `claude --version`.
pub fn version_of(bin: &Path, args: &[&str]) -> Option<String> {
    let out = run_timeout(bin, args, Duration::from_secs(3))?;
    out.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// Detect a runtime binary in `dirs` and read its version.
#[cfg(test)]
pub fn detect_in(names: &[&str], dirs: &[PathBuf], version_args: &[&str]) -> Detection {
    match which_in(names, dirs) {
        Some(p) => Detection {
            version: version_of(&p, version_args),
            path: Some(p),
        },
        None => Detection::default(),
    }
}

/// Detect via `PATH` + common dirs + `extra`.
pub fn detect(names: &[&str], extra: &[PathBuf], version_args: &[&str]) -> Detection {
    match which(names, extra) {
        Some(p) => Detection {
            version: version_of(&p, version_args),
            path: Some(p),
        },
        None => Detection::default(),
    }
}

pub(crate) fn detected_bin(names: &[&str], extra: &[PathBuf]) -> Option<PathBuf> {
    which(names, extra)
}

pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            if it.peek() == Some(&'[') {
                it.next();
                for n in it.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        one
    } else {
        let t: String = one.chars().take(max).collect();
        format!("{t}...")
    }
}

/// Drop the `mcp__rteams__` prefix so tool lines read `tool: task_create ..`.
pub fn short_tool(name: &str) -> &str {
    name.strip_prefix("mcp__rteams__").unwrap_or(name)
}

/// Fallback for unknown JSON-line formats: plain text passes through, JSON objects are
/// searched for a human-readable `text` / `message` / `content` / `result` string.
pub fn generic_summarize(line: &str) -> Vec<String> {
    if line.trim().is_empty() {
        return vec![];
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return vec![clip(&strip_ansi(line), 300)];
    };
    for key in ["text", "message", "content", "result", "output"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            let t = clip(s, 300);
            if !t.is_empty() {
                return vec![t];
            }
        }
    }
    vec![]
}

/// Session id from any of the common field spellings.
pub fn session_field(v: &serde_json::Value) -> Option<String> {
    for k in ["session_id", "sessionID", "sessionId", "thread_id"] {
        if let Some(s) = v.get(k).and_then(|x| x.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

/// `{"mcpServers": {"rteams": {command, args, env}}}` fragment shared by Claude/Cursor/Gemini.
pub fn mcp_server_entry(ctx: &LaunchCtx, trust: bool) -> serde_json::Value {
    let env: serde_json::Map<String, serde_json::Value> = ctx
        .mcp_env
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();
    let mut e = serde_json::json!({
        "command": ctx.mcp_command,
        "args": ctx.mcp_args,
        "env": env,
    });
    if trust {
        e["trust"] = serde_json::Value::Bool(true);
    }
    e
}

/// Merge the rteams server into `<workdir>/<rel>` under `mcpServers`, keeping other entries.
/// Returns the file written.
#[cfg(test)]
pub fn write_mcp_servers_file(
    workdir: &Path,
    rel: &str,
    entry: serde_json::Value,
) -> std::io::Result<PathBuf> {
    write_mcp_servers_file_with(workdir, rel, entry, &[])
}

/// Like [`write_mcp_servers_file`], also adding `extras` (never replacing `rteams`).
pub fn write_mcp_servers_file_with(
    workdir: &Path,
    rel: &str,
    entry: serde_json::Value,
    extras: &[(String, serde_json::Value)],
) -> std::io::Result<PathBuf> {
    let path = workdir.join(rel);
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut doc: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    if !doc["mcpServers"].is_object() {
        doc["mcpServers"] = serde_json::json!({});
    }
    for (k, v) in extras.iter().filter(|(k, _)| k != "rteams") {
        doc["mcpServers"][k] = v.clone();
    }
    doc["mcpServers"]["rteams"] = entry;
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&doc).unwrap_or_default(),
    )?;
    Ok(path)
}

/// `{"mcpServers": {rteams, ..extras}}` for runtimes that take the whole document inline.
pub fn mcp_doc(ctx: &LaunchCtx, trust: bool) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    for (k, v) in ctx.extra_mcp.iter().filter(|(k, _)| k != "rteams") {
        m.insert(k.clone(), v.clone());
    }
    m.insert("rteams".into(), mcp_server_entry(ctx, trust));
    serde_json::json!({ "mcpServers": m })
}

/// System prompt + task for CLIs without a system-prompt flag.
pub fn prompt_with_system(ctx: &LaunchCtx) -> String {
    if ctx.system_prompt.trim().is_empty() {
        ctx.prompt.clone()
    } else {
        format!("{}\n\n---\n\n{}", ctx.system_prompt.trim(), ctx.prompt)
    }
}

/// Mode restricts the agent to read-only (`ask`) or to coordination only (`delegate`).
pub fn read_only_mode(ctx: &LaunchCtx) -> bool {
    matches!(ctx.mode.as_str(), "ask" | "delegate")
}

pub(crate) fn base_command(ctx: &LaunchCtx) -> Command {
    let mut c = Command::new(&ctx.bin);
    c.current_dir(&ctx.workdir);
    for (k, v) in &ctx.mcp_env {
        c.env(k, v);
    }
    c
}

/// Value of an API key: process env first, then the rteams key store.
pub(crate) fn find_key(names: &[&str]) -> Option<String> {
    for n in names {
        if let Ok(v) = std::env::var(n) {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    let stored = rteams_services::ext::apikeys::env_for("opencode");
    names.iter().find_map(|n| {
        stored
            .iter()
            .find(|(k, v)| k == n && !v.is_empty())
            .map(|(_, v)| v.clone())
    })
}

#[cfg(test)]
pub(crate) fn args_of(c: &Command) -> Vec<String> {
    c.get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

/// TOML basic string literal.
pub(crate) fn toml_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;
    pub fn ctx(mode: &str, perms: &str) -> LaunchCtx {
        LaunchCtx {
            bin: PathBuf::from("/bin/agent"),
            prompt: "do the thing".into(),
            system_prompt: "you are dev".into(),
            mcp_command: "/usr/bin/rteams".into(),
            mcp_args: vec![
                "serve".into(),
                "--home".into(),
                "/h".into(),
                "--team".into(),
                "t".into(),
            ],
            mcp_env: vec![("RTEAMS_MEMBER".into(), "dev".into())],
            model: None,
            mode: mode.into(),
            permissions: perms.into(),
            resume: None,
            workdir: std::env::temp_dir(),
            ..Default::default()
        }
    }
    pub fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rteams-rt-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_in_finds_executable_only() {
        let d = testutil::tmpdir("which");
        let f = d.join("mybin");
        std::fs::write(&f, "#!/bin/sh\necho v1.2\n").unwrap();
        assert!(which_in(&["mybin"], std::slice::from_ref(&d)).is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(
                which_in(&["nope", "mybin"], std::slice::from_ref(&d)),
                Some(f.clone())
            );
            let det = detect_in(&["mybin"], std::slice::from_ref(&d), &[]);
            assert_eq!(det.path, Some(f));
            assert_eq!(det.version.as_deref(), Some("v1.2"));
        }
        assert_eq!(detect_in(&["zzz"], &[d], &[]), Detection::default());
    }

    #[test]
    fn generic_and_helpers() {
        assert!(generic_summarize("  ").is_empty());
        assert_eq!(generic_summarize("plain"), vec!["plain"]);
        assert_eq!(generic_summarize(r#"{"text":"hi"}"#), vec!["hi"]);
        assert_eq!(strip_ansi("\u{1b}[90mx\u{1b}[0m"), "x");
        assert_eq!(clip("a   b", 10), "a b");
        assert_eq!(toml_str("a\"b\\"), "\"a\\\"b\\\\\"");
        assert_eq!(short_tool("mcp__rteams__x"), "x");
    }

    #[test]
    fn mcp_file_merges() {
        let d = testutil::tmpdir("mcpfile");
        std::fs::create_dir_all(d.join(".cursor")).unwrap();
        std::fs::write(
            d.join(".cursor/mcp.json"),
            r#"{"mcpServers":{"other":{"command":"x"}}}"#,
        )
        .unwrap();
        let ctx = testutil::ctx("do", "skip");
        write_mcp_servers_file(&d, ".cursor/mcp.json", mcp_server_entry(&ctx, false)).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(d.join(".cursor/mcp.json")).unwrap())
                .unwrap();
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        assert_eq!(v["mcpServers"]["rteams"]["env"]["RTEAMS_MEMBER"], "dev");
    }

    #[test]
    fn registry() {
        let ids: Vec<_> = all().iter().map(|r| r.id()).collect();
        assert_eq!(
            ids,
            [
                "claude",
                "codex",
                "opencode",
                "cursor",
                "gemini",
                "qwen",
                "copilot",
                "grok",
                "kiro",
                "zai",
                "minimax",
                "openrouter",
                "ollama",
                "lmstudio"
            ]
        );
        assert_eq!(get(None).unwrap().id(), "claude");
        assert_eq!(get(Some("codex")).unwrap().id(), "codex");
        assert_eq!(get(Some("bogus")).unwrap().id(), "claude");
    }
}

/// Every known runtime, Claude Code first (the default).
pub fn all() -> Vec<Box<dyn AgentRuntime>> {
    vec![
        Box::new(claude::Claude::default()),
        Box::new(codex::Codex::default()),
        Box::new(opencode::OpenCode::default()),
        Box::new(cursor::Cursor::default()),
        Box::new(gemini::Gemini::default()),
        Box::new(qwen::Qwen::default()),
        Box::new(copilot::Copilot::default()),
        Box::new(grok::Grok::default()),
        Box::new(kiro::Kiro::default()),
        Box::new(compat::Compat::zai()),
        Box::new(compat::Compat::minimax()),
        Box::new(compat::Compat::openrouter()),
        Box::new(compat::Compat::ollama()),
        Box::new(compat::Compat::lmstudio()),
    ]
}

/// Runtime by id; `None` or unknown -> Claude Code.
pub fn get(id: Option<&str>) -> Option<Box<dyn AgentRuntime>> {
    let mut rs = all();
    let i = rs.iter().position(|r| Some(r.id()) == id).unwrap_or(0);
    (!rs.is_empty()).then(|| rs.swap_remove(i))
}
