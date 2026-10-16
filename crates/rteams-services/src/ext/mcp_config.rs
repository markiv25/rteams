//! Reading and writing the MCP server entries of the agent CLIs' own config files:
//!
//! * Claude Code: `~/.claude.json` (`mcpServers` = user scope, `projects[<path>].mcpServers` =
//!   local scope) and `<project>/.mcp.json` (project scope).
//! * Codex: `~/.codex/config.toml` / `<project>/.codex/config.toml`, table `[mcp_servers.<name>]`,
//!   edited with `toml_edit` so comments and unrelated settings survive.
//! * OpenCode: `~/.config/opencode/opencode.json` / `<project>/opencode.json`, key `mcp`.
//! * Gemini: `~/.gemini/settings.json` / `<project>/.gemini/settings.json`, key `mcpServers`.
//!
//! Only the touched entry is rewritten; every unknown key (in the file and inside an entry) is
//! kept, files are replaced atomically and keep their permission bits. A file that exists but
//! does not parse is never overwritten: the call fails instead.
//!
//! Disabling: Codex and OpenCode have a native `enabled` flag. Claude and Gemini do not, so a
//! disabled entry is parked unchanged under `rteamsDisabledMcpServers` next to `mcpServers`
//! (the CLI ignores that key) and moved back on enable.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use thiserror::Error;

/// Key next to `mcpServers` where disabled Claude / Gemini entries are parked.
pub const PARKED_KEY: &str = "rteamsDisabledMcpServers";

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{0}")]
pub struct ConfigError(pub String);

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        ConfigError(e.to_string())
    }
}

type Result<T> = std::result::Result<T, ConfigError>;

fn err<T>(m: impl Into<String>) -> Result<T> {
    Err(ConfigError(m.into()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Runtime {
    Claude,
    Codex,
    OpenCode,
    Gemini,
}

impl Runtime {
    pub const ALL: [Runtime; 4] = [
        Runtime::Claude,
        Runtime::Codex,
        Runtime::OpenCode,
        Runtime::Gemini,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Runtime::Claude => "claude",
            Runtime::Codex => "codex",
            Runtime::OpenCode => "opencode",
            Runtime::Gemini => "gemini",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Runtime::Claude => "Claude Code",
            Runtime::Codex => "Codex",
            Runtime::OpenCode => "OpenCode",
            Runtime::Gemini => "Gemini CLI",
        }
    }

    pub fn from_id(id: &str) -> Option<Runtime> {
        Runtime::ALL.into_iter().find(|r| r.id() == id)
    }

    /// Scopes this runtime's config has a place for.
    pub fn scopes(self) -> &'static [Scope] {
        match self {
            Runtime::Claude => &[Scope::Local, Scope::Project, Scope::User],
            _ => &[Scope::Project, Scope::User],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    /// Only for you in this project (Claude: `projects[path]` in `~/.claude.json`).
    Local,
    /// Shared with the project (checked-in config file).
    Project,
    /// Every project.
    User,
}

impl Scope {
    pub fn id(self) -> &'static str {
        match self {
            Scope::Local => "local",
            Scope::Project => "project",
            Scope::User => "user",
        }
    }

    pub fn from_id(s: &str) -> Option<Scope> {
        match s {
            "local" => Some(Scope::Local),
            "project" => Some(Scope::Project),
            "user" | "global" => Some(Scope::User),
            _ => None,
        }
    }
}

/// Where the config files live: injected so tests never touch the real home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientPaths {
    pub user_home: PathBuf,
    /// Project root for project / local scope.
    pub project: Option<PathBuf>,
}

impl ClientPaths {
    pub fn new(user_home: impl Into<PathBuf>, project: Option<PathBuf>) -> Self {
        ClientPaths {
            user_home: user_home.into(),
            project,
        }
    }

    /// The real home directory.
    pub fn real(project: Option<PathBuf>) -> Self {
        ClientPaths::new(
            dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
            project,
        )
    }

    fn project_root(&self) -> Result<&Path> {
        self.project
            .as_deref()
            .ok_or_else(|| ConfigError("no project is open for project / local scope".into()))
    }

    /// The file `rt` keeps MCP servers of `scope` in.
    pub fn config_file(&self, rt: Runtime, scope: Scope) -> Result<PathBuf> {
        Ok(match (rt, scope) {
            (Runtime::Claude, Scope::User | Scope::Local) => self.user_home.join(".claude.json"),
            (Runtime::Claude, Scope::Project) => self.project_root()?.join(".mcp.json"),
            (Runtime::Codex, Scope::User) => self.user_home.join(".codex/config.toml"),
            (Runtime::Codex, Scope::Project) => self.project_root()?.join(".codex/config.toml"),
            (Runtime::OpenCode, Scope::User) => {
                self.user_home.join(".config/opencode/opencode.json")
            }
            (Runtime::OpenCode, Scope::Project) => self.project_root()?.join("opencode.json"),
            (Runtime::Gemini, Scope::User) => self.user_home.join(".gemini/settings.json"),
            (Runtime::Gemini, Scope::Project) => self.project_root()?.join(".gemini/settings.json"),
            (rt, Scope::Local) => {
                return err(format!("{} has no local scope", rt.label()));
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transport {
    #[default]
    Stdio,
    Http,
    Sse,
}

impl Transport {
    pub fn id(self) -> &'static str {
        match self {
            Transport::Stdio => "stdio",
            Transport::Http => "http",
            Transport::Sse => "sse",
        }
    }
}

/// A transport-neutral MCP server definition.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerSpec {
    pub transport: Transport,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub url: String,
    pub headers: BTreeMap<String, String>,
}

impl ServerSpec {
    pub fn stdio(command: &str, args: &[&str]) -> Self {
        ServerSpec {
            transport: Transport::Stdio,
            command: command.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    pub fn http(url: &str) -> Self {
        ServerSpec {
            transport: Transport::Http,
            url: url.into(),
            ..Default::default()
        }
    }

    /// `npx -y pkg --flag` style one-liner, or the URL, for display.
    pub fn launch_target(&self) -> String {
        match self.transport {
            Transport::Stdio => {
                let mut s = self.command.clone();
                for a in &self.args {
                    s.push(' ');
                    s.push_str(a);
                }
                s
            }
            _ => self.url.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledServer {
    pub name: String,
    pub runtime: Runtime,
    pub scope: Scope,
    pub spec: ServerSpec,
    pub enabled: bool,
    pub file: PathBuf,
}

// ---------------------------------------------------------------------------------------------
// File IO
// ---------------------------------------------------------------------------------------------

/// Atomic replace that keeps the old file's permission bits (new files are owner-only, since
/// MCP env blocks often hold tokens).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let tmp = dir.join(format!(".{name}.{}.rteams-tmp", std::process::id()));
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o600)
    };
    let res = (|| {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(mode);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
        }
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

fn read_text(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => err(format!("cannot read {}: {e}", path.display())),
    }
}

fn read_json(path: &Path) -> Result<Value> {
    let text = read_text(path)?;
    if text.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| ConfigError(format!("{} is not valid JSON: {e}", path.display())))?;
    if !v.is_object() {
        return err(format!("{} is not a JSON object", path.display()));
    }
    Ok(v)
}

fn write_json(path: &Path, v: &Value) -> Result<()> {
    let mut s = serde_json::to_string_pretty(v).map_err(|e| ConfigError(e.to_string()))?;
    s.push('\n');
    write_atomic(path, s.as_bytes()).map_err(Into::into)
}

// ---------------------------------------------------------------------------------------------
// JSON runtimes (Claude, OpenCode, Gemini)
// ---------------------------------------------------------------------------------------------

struct JsonLoc {
    file: PathBuf,
    /// Path of the servers map inside the document.
    servers: Vec<String>,
    /// Path of the parked (disabled) map, when the runtime has no `enabled` flag.
    parked: Option<Vec<String>>,
}

fn json_loc(paths: &ClientPaths, rt: Runtime, scope: Scope) -> Result<JsonLoc> {
    let file = paths.config_file(rt, scope)?;
    let s = |p: &[&str]| p.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    Ok(match rt {
        Runtime::Claude if scope == Scope::Local => {
            let proj = paths.project_root()?.to_string_lossy().into_owned();
            JsonLoc {
                file,
                servers: vec!["projects".into(), proj.clone(), "mcpServers".into()],
                parked: Some(vec!["projects".into(), proj, PARKED_KEY.into()]),
            }
        }
        Runtime::Claude | Runtime::Gemini => JsonLoc {
            file,
            servers: s(&["mcpServers"]),
            parked: Some(s(&[PARKED_KEY])),
        },
        Runtime::OpenCode => JsonLoc {
            file,
            servers: s(&["mcp"]),
            parked: None,
        },
        Runtime::Codex => return err("codex uses TOML"),
    })
}

fn get_path<'a>(v: &'a Value, path: &[String]) -> Option<&'a Map<String, Value>> {
    let mut cur = v;
    for p in path {
        cur = cur.get(p)?;
    }
    cur.as_object()
}

fn obj_mut<'a>(v: &'a mut Value, path: &[String]) -> Result<&'a mut Map<String, Value>> {
    let mut cur = v;
    for p in path {
        let Some(map) = cur.as_object_mut() else {
            return err(format!("config key `{p}` is not inside an object"));
        };
        cur = map
            .entry(p.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    cur.as_object_mut()
        .ok_or_else(|| ConfigError("config key is not an object".into()))
}

fn str_map(v: Option<&Value>) -> BTreeMap<String, String> {
    v.and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn string_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn to_json_map(m: &BTreeMap<String, String>) -> Value {
    Value::Object(
        m.iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect(),
    )
}

/// Entry JSON -> spec, per runtime dialect. `None` if it is not recognisable as a server.
fn decode_json(rt: Runtime, e: &Value) -> Option<(ServerSpec, bool)> {
    let o = e.as_object()?;
    match rt {
        Runtime::Claude => {
            let ty = o.get("type").and_then(Value::as_str);
            let has_url = o.get("url").is_some();
            let transport = match ty {
                Some("http") | Some("streamable-http") => Transport::Http,
                Some("sse") => Transport::Sse,
                Some("stdio") => Transport::Stdio,
                _ if has_url => Transport::Http,
                _ => Transport::Stdio,
            };
            Some((
                ServerSpec {
                    transport,
                    command: o
                        .get("command")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into(),
                    args: string_list(o.get("args")),
                    env: str_map(o.get("env")),
                    url: o
                        .get("url")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into(),
                    headers: str_map(o.get("headers")),
                },
                true,
            ))
        }
        Runtime::Gemini => {
            let (transport, url) = if let Some(u) = o.get("httpUrl").and_then(Value::as_str) {
                (Transport::Http, u)
            } else if let Some(u) = o.get("url").and_then(Value::as_str) {
                (Transport::Sse, u)
            } else {
                (Transport::Stdio, "")
            };
            Some((
                ServerSpec {
                    transport,
                    command: o
                        .get("command")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into(),
                    args: string_list(o.get("args")),
                    env: str_map(o.get("env")),
                    url: url.into(),
                    headers: str_map(o.get("headers")),
                },
                true,
            ))
        }
        Runtime::OpenCode => {
            let enabled = o.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            let spec = if o.get("type").and_then(Value::as_str) == Some("remote") {
                ServerSpec {
                    transport: Transport::Http,
                    url: o
                        .get("url")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into(),
                    headers: str_map(o.get("headers")),
                    ..Default::default()
                }
            } else {
                let mut cmd = string_list(o.get("command"));
                let command = if cmd.is_empty() {
                    String::new()
                } else {
                    cmd.remove(0)
                };
                ServerSpec {
                    transport: Transport::Stdio,
                    command,
                    args: cmd,
                    env: str_map(o.get("environment")),
                    ..Default::default()
                }
            };
            Some((spec, enabled))
        }
        Runtime::Codex => None,
    }
}

fn set_or_remove(o: &mut Map<String, Value>, key: &str, v: Option<Value>) {
    match v {
        Some(v) => {
            o.insert(key.into(), v);
        }
        None => {
            o.remove(key);
        }
    }
}

/// Apply `spec` onto `base` (the existing entry, to keep its unknown fields).
fn encode_json(rt: Runtime, base: Option<&Value>, spec: &ServerSpec, enabled: bool) -> Value {
    let mut o = base.and_then(Value::as_object).cloned().unwrap_or_default();
    let nonempty = |m: &BTreeMap<String, String>| (!m.is_empty()).then(|| to_json_map(m));
    match rt {
        Runtime::Claude => {
            o.insert("type".into(), spec.transport.id().into());
            if spec.transport == Transport::Stdio {
                o.insert("command".into(), spec.command.clone().into());
                o.insert(
                    "args".into(),
                    Value::Array(spec.args.iter().cloned().map(Value::String).collect()),
                );
                o.insert("env".into(), to_json_map(&spec.env));
                o.remove("url");
                o.remove("headers");
            } else {
                o.insert("url".into(), spec.url.clone().into());
                set_or_remove(&mut o, "headers", nonempty(&spec.headers));
                for k in ["command", "args", "env"] {
                    o.remove(k);
                }
            }
        }
        Runtime::Gemini => {
            if spec.transport == Transport::Stdio {
                o.insert("command".into(), spec.command.clone().into());
                set_or_remove(
                    &mut o,
                    "args",
                    (!spec.args.is_empty()).then(|| {
                        Value::Array(spec.args.iter().cloned().map(Value::String).collect())
                    }),
                );
                set_or_remove(&mut o, "env", nonempty(&spec.env));
                for k in ["url", "httpUrl", "headers"] {
                    o.remove(k);
                }
            } else {
                let key = if spec.transport == Transport::Http {
                    "httpUrl"
                } else {
                    "url"
                };
                let other = if key == "url" { "httpUrl" } else { "url" };
                o.insert(key.into(), spec.url.clone().into());
                o.remove(other);
                set_or_remove(&mut o, "headers", nonempty(&spec.headers));
                for k in ["command", "args", "env"] {
                    o.remove(k);
                }
            }
        }
        Runtime::OpenCode => {
            if spec.transport == Transport::Stdio {
                o.insert("type".into(), "local".into());
                let mut cmd = vec![spec.command.clone()];
                cmd.extend(spec.args.iter().cloned());
                o.insert(
                    "command".into(),
                    Value::Array(cmd.into_iter().map(Value::String).collect()),
                );
                set_or_remove(&mut o, "environment", nonempty(&spec.env));
                for k in ["url", "headers"] {
                    o.remove(k);
                }
            } else {
                o.insert("type".into(), "remote".into());
                o.insert("url".into(), spec.url.clone().into());
                set_or_remove(&mut o, "headers", nonempty(&spec.headers));
                for k in ["command", "environment"] {
                    o.remove(k);
                }
            }
            if enabled {
                o.remove("enabled");
            } else {
                o.insert("enabled".into(), false.into());
            }
        }
        Runtime::Codex => {}
    }
    Value::Object(o)
}

fn list_json(paths: &ClientPaths, rt: Runtime, scope: Scope) -> Result<Vec<InstalledServer>> {
    let loc = json_loc(paths, rt, scope)?;
    let doc = read_json(&loc.file)?;
    let mut out = Vec::new();
    let mut push = |m: &Map<String, Value>, parked: bool| {
        for (name, e) in m {
            if let Some((spec, enabled)) = decode_json(rt, e) {
                out.push(InstalledServer {
                    name: name.clone(),
                    runtime: rt,
                    scope,
                    spec,
                    enabled: enabled && !parked,
                    file: loc.file.clone(),
                });
            }
        }
    };
    if let Some(m) = get_path(&doc, &loc.servers) {
        push(m, false);
    }
    if let Some(p) = &loc.parked {
        if let Some(m) = get_path(&doc, p) {
            push(m, true);
        }
    }
    Ok(out)
}

fn upsert_json(
    paths: &ClientPaths,
    rt: Runtime,
    scope: Scope,
    name: &str,
    spec: &ServerSpec,
) -> Result<()> {
    let loc = json_loc(paths, rt, scope)?;
    let mut doc = read_json(&loc.file)?;
    // Keep the entry's unknown fields and its enabled state, wherever it currently lives.
    let mut parked_entry = None;
    if let Some(p) = &loc.parked {
        if get_path(&doc, p).is_some_and(|m| m.contains_key(name)) {
            parked_entry = obj_mut(&mut doc, p)?.remove(name);
        }
    }
    let live = get_path(&doc, &loc.servers)
        .and_then(|m| m.get(name))
        .cloned();
    let base = live.or(parked_entry.clone());
    let enabled = parked_entry.is_none()
        && base
            .as_ref()
            .and_then(|b| decode_json(rt, b))
            .is_none_or(|(_, en)| en);
    let entry = encode_json(rt, base.as_ref(), spec, enabled);
    if let (false, Some(p)) = (enabled, &loc.parked) {
        obj_mut(&mut doc, p)?.insert(name.into(), entry);
    } else {
        obj_mut(&mut doc, &loc.servers)?.insert(name.into(), entry);
    }
    write_json(&loc.file, &doc)
}

fn remove_json(paths: &ClientPaths, rt: Runtime, scope: Scope, name: &str) -> Result<bool> {
    let loc = json_loc(paths, rt, scope)?;
    let mut doc = read_json(&loc.file)?;
    let mut found = false;
    for p in std::iter::once(&loc.servers).chain(loc.parked.iter()) {
        if get_path(&doc, p).is_some_and(|m| m.contains_key(name)) {
            found |= obj_mut(&mut doc, p)?.remove(name).is_some();
        }
    }
    if found {
        write_json(&loc.file, &doc)?;
    }
    Ok(found)
}

fn set_enabled_json(
    paths: &ClientPaths,
    rt: Runtime,
    scope: Scope,
    name: &str,
    enabled: bool,
) -> Result<bool> {
    let loc = json_loc(paths, rt, scope)?;
    let mut doc = read_json(&loc.file)?;
    match &loc.parked {
        Some(parked) => {
            let (from, to) = if enabled {
                (parked, &loc.servers)
            } else {
                (&loc.servers, parked)
            };
            if !get_path(&doc, from).is_some_and(|m| m.contains_key(name)) {
                return Ok(get_path(&doc, to).is_some_and(|m| m.contains_key(name)));
            }
            let entry = obj_mut(&mut doc, from)?.remove(name).unwrap_or(Value::Null);
            obj_mut(&mut doc, to)?.insert(name.into(), entry);
        }
        None => {
            let Some(e) = get_path(&doc, &loc.servers)
                .and_then(|m| m.get(name))
                .cloned()
            else {
                return Ok(false);
            };
            let Some((spec, _)) = decode_json(rt, &e) else {
                return Ok(false);
            };
            let entry = encode_json(rt, Some(&e), &spec, enabled);
            obj_mut(&mut doc, &loc.servers)?.insert(name.into(), entry);
        }
    }
    write_json(&loc.file, &doc)?;
    Ok(true)
}

// ---------------------------------------------------------------------------------------------
// Codex (TOML)
// ---------------------------------------------------------------------------------------------

use toml_edit::{value as tv, Array, DocumentMut, Item, Table};

fn read_toml(path: &Path) -> Result<DocumentMut> {
    read_text(path)?
        .parse::<DocumentMut>()
        .map_err(|e| ConfigError(format!("{} is not valid TOML: {e}", path.display())))
}

fn write_toml(path: &Path, doc: &DocumentMut) -> Result<()> {
    write_atomic(path, doc.to_string().as_bytes()).map_err(Into::into)
}

fn toml_str_map(item: Option<&Item>) -> BTreeMap<String, String> {
    item.and_then(|i| i.as_table_like())
        .map(|t| {
            t.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.to_string(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn decode_toml(t: &dyn toml_edit::TableLike) -> (ServerSpec, bool) {
    let get_s = |k: &str| {
        t.get(k)
            .and_then(Item::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let enabled = t.get("enabled").and_then(Item::as_bool).unwrap_or(true);
    let args = t
        .get("args")
        .and_then(Item::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if t.contains_key("url") {
        (
            ServerSpec {
                transport: Transport::Http,
                url: get_s("url"),
                headers: toml_str_map(t.get("http_headers")),
                ..Default::default()
            },
            enabled,
        )
    } else {
        (
            ServerSpec {
                transport: Transport::Stdio,
                command: get_s("command"),
                args,
                env: toml_str_map(t.get("env")),
                ..Default::default()
            },
            enabled,
        )
    }
}

fn string_table(m: &BTreeMap<String, String>) -> Item {
    let mut t = Table::new();
    for (k, v) in m {
        t.insert(k, tv(v.as_str()));
    }
    Item::Table(t)
}

/// Set `key` keeping the key's own formatting and any comment above it.
fn set_item(entry: &mut Table, key: &str, item: Item) {
    match entry.get_mut(key) {
        Some(slot) => *slot = item,
        None => {
            entry.insert(key, item);
        }
    }
}

fn encode_toml(entry: &mut Table, spec: &ServerSpec) {
    if spec.transport == Transport::Stdio {
        set_item(entry, "command", tv(spec.command.as_str()));
        let mut a = Array::new();
        for x in &spec.args {
            a.push(x.as_str());
        }
        set_item(entry, "args", tv(a));
        if spec.env.is_empty() {
            entry.remove("env");
        } else {
            set_item(entry, "env", string_table(&spec.env));
        }
        entry.remove("url");
        entry.remove("http_headers");
    } else {
        set_item(entry, "url", tv(spec.url.as_str()));
        if spec.headers.is_empty() {
            entry.remove("http_headers");
        } else {
            set_item(entry, "http_headers", string_table(&spec.headers));
        }
        for k in ["command", "args", "env"] {
            entry.remove(k);
        }
    }
}

fn list_toml(paths: &ClientPaths, scope: Scope) -> Result<Vec<InstalledServer>> {
    let file = paths.config_file(Runtime::Codex, scope)?;
    let doc = read_toml(&file)?;
    let mut out = Vec::new();
    if let Some(t) = doc.get("mcp_servers").and_then(Item::as_table_like) {
        for (name, item) in t.iter() {
            if let Some(e) = item.as_table_like() {
                let (spec, enabled) = decode_toml(e);
                out.push(InstalledServer {
                    name: name.to_string(),
                    runtime: Runtime::Codex,
                    scope,
                    spec,
                    enabled,
                    file: file.clone(),
                });
            }
        }
    }
    Ok(out)
}

fn servers_table(doc: &mut DocumentMut) -> Result<&mut Table> {
    if !doc.contains_key("mcp_servers") {
        let mut t = Table::new();
        t.set_implicit(true);
        doc.insert("mcp_servers", Item::Table(t));
    }
    doc["mcp_servers"]
        .as_table_mut()
        .ok_or_else(|| ConfigError("`mcp_servers` is not a table".into()))
}

fn upsert_toml(paths: &ClientPaths, scope: Scope, name: &str, spec: &ServerSpec) -> Result<()> {
    let file = paths.config_file(Runtime::Codex, scope)?;
    let mut doc = read_toml(&file)?;
    let servers = servers_table(&mut doc)?;
    if !servers.contains_key(name) {
        servers.insert(name, Item::Table(Table::new()));
    }
    let Some(entry) = servers[name].as_table_mut() else {
        return err(format!("`mcp_servers.{name}` is not a table"));
    };
    encode_toml(entry, spec);
    write_toml(&file, &doc)
}

fn remove_toml(paths: &ClientPaths, scope: Scope, name: &str) -> Result<bool> {
    let file = paths.config_file(Runtime::Codex, scope)?;
    let mut doc = read_toml(&file)?;
    let found = doc
        .get_mut("mcp_servers")
        .and_then(Item::as_table_like_mut)
        .is_some_and(|t| t.remove(name).is_some());
    if found {
        write_toml(&file, &doc)?;
    }
    Ok(found)
}

fn set_enabled_toml(paths: &ClientPaths, scope: Scope, name: &str, enabled: bool) -> Result<bool> {
    let file = paths.config_file(Runtime::Codex, scope)?;
    let mut doc = read_toml(&file)?;
    let Some(entry) = doc
        .get_mut("mcp_servers")
        .and_then(Item::as_table_like_mut)
        .and_then(|t| t.get_mut(name))
        .and_then(Item::as_table_like_mut)
    else {
        return Ok(false);
    };
    if enabled {
        entry.remove("enabled");
    } else {
        entry.insert("enabled", tv(false));
    }
    write_toml(&file, &doc)?;
    Ok(true)
}

// ---------------------------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------------------------

fn check_scope(rt: Runtime, scope: Scope) -> Result<()> {
    if rt.scopes().contains(&scope) {
        Ok(())
    } else {
        err(format!("{} has no {} scope", rt.label(), scope.id()))
    }
}

/// Servers of one scope.
pub fn list_scope(paths: &ClientPaths, rt: Runtime, scope: Scope) -> Result<Vec<InstalledServer>> {
    check_scope(rt, scope)?;
    if rt == Runtime::Codex {
        list_toml(paths, scope)
    } else {
        list_json(paths, rt, scope)
    }
}

/// Every installed server of `rt` in all scopes that can be read (project / local are skipped
/// when no project is open; an unreadable file is skipped and reported in the second value).
pub fn list(paths: &ClientPaths, rt: Runtime) -> (Vec<InstalledServer>, Vec<String>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for &scope in rt.scopes() {
        if scope != Scope::User && paths.project.is_none() {
            continue;
        }
        match list_scope(paths, rt, scope) {
            Ok(v) => out.extend(v),
            Err(e) => errors.push(e.0),
        }
    }
    (out, errors)
}

/// Add or replace the server `name`; unknown fields of an existing entry are kept.
pub fn upsert(
    paths: &ClientPaths,
    rt: Runtime,
    scope: Scope,
    name: &str,
    spec: &ServerSpec,
) -> Result<()> {
    check_scope(rt, scope)?;
    if name.is_empty() {
        return err("server name is required");
    }
    if rt == Runtime::Codex {
        upsert_toml(paths, scope, name, spec)
    } else {
        upsert_json(paths, rt, scope, name, spec)
    }
}

/// Remove `name`; `Ok(false)` if it was not there.
pub fn remove(paths: &ClientPaths, rt: Runtime, scope: Scope, name: &str) -> Result<bool> {
    check_scope(rt, scope)?;
    if rt == Runtime::Codex {
        remove_toml(paths, scope, name)
    } else {
        remove_json(paths, rt, scope, name)
    }
}

/// Enable or disable `name`; `Ok(false)` if there is no such server.
pub fn set_enabled(
    paths: &ClientPaths,
    rt: Runtime,
    scope: Scope,
    name: &str,
    enabled: bool,
) -> Result<bool> {
    check_scope(rt, scope)?;
    if rt == Runtime::Codex {
        set_enabled_toml(paths, scope, name, enabled)
    } else {
        set_enabled_json(paths, rt, scope, name, enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, ClientPaths) {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("home");
        let proj = d.path().join("proj");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&proj).unwrap();
        let p = ClientPaths::new(home, Some(proj));
        (d, p)
    }

    #[test]
    fn claude_user_scope_round_trip_keeps_unknown_fields() {
        let (_d, p) = setup();
        let f = p.config_file(Runtime::Claude, Scope::User).unwrap();
        std::fs::write(
            &f,
            r#"{"numStartups":7,"oauthAccount":{"x":1},"mcpServers":{"old":{"type":"stdio","command":"a","args":[],"env":{},"extra":true}}}"#,
        )
        .unwrap();
        let mut spec = ServerSpec::stdio("npx", &["-y", "pkg"]);
        spec.env.insert("K".into(), "v".into());
        upsert(&p, Runtime::Claude, Scope::User, "new", &spec).unwrap();
        // Re-upserting `old` keeps its unknown `extra` field.
        upsert(
            &p,
            Runtime::Claude,
            Scope::User,
            "old",
            &ServerSpec::stdio("b", &[]),
        )
        .unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["numStartups"], 7);
        assert_eq!(v["oauthAccount"]["x"], 1);
        assert_eq!(v["mcpServers"]["old"]["extra"], true);
        assert_eq!(v["mcpServers"]["old"]["command"], "b");
        assert_eq!(v["mcpServers"]["new"]["args"][1], "pkg");
        assert_eq!(v["mcpServers"]["new"]["env"]["K"], "v");
        let (l, errs) = list(&p, Runtime::Claude);
        assert!(errs.is_empty());
        assert_eq!(l.len(), 2);
        assert!(remove(&p, Runtime::Claude, Scope::User, "new").unwrap());
        assert!(!remove(&p, Runtime::Claude, Scope::User, "new").unwrap());
    }

    #[test]
    fn claude_scopes_use_their_own_files() {
        let (_d, p) = setup();
        let s = ServerSpec::http("https://x.test/mcp");
        upsert(&p, Runtime::Claude, Scope::Project, "web", &s).unwrap();
        upsert(
            &p,
            Runtime::Claude,
            Scope::Local,
            "loc",
            &ServerSpec::stdio("c", &[]),
        )
        .unwrap();
        let mcp = p.project.as_ref().unwrap().join(".mcp.json");
        let v: Value = serde_json::from_str(&std::fs::read_to_string(mcp).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["web"]["type"], "http");
        assert_eq!(v["mcpServers"]["web"]["url"], "https://x.test/mcp");
        let user: Value = serde_json::from_str(
            &std::fs::read_to_string(p.user_home.join(".claude.json")).unwrap(),
        )
        .unwrap();
        let key = p.project.as_ref().unwrap().to_string_lossy().into_owned();
        assert_eq!(user["projects"][&key]["mcpServers"]["loc"]["command"], "c");
        let (l, _) = list(&p, Runtime::Claude);
        let scopes: Vec<_> = l.iter().map(|s| (s.name.as_str(), s.scope)).collect();
        assert!(scopes.contains(&("web", Scope::Project)));
        assert!(scopes.contains(&("loc", Scope::Local)));
    }

    #[test]
    fn disable_parks_claude_entry_and_enable_restores() {
        let (_d, p) = setup();
        upsert(
            &p,
            Runtime::Claude,
            Scope::User,
            "a",
            &ServerSpec::stdio("x", &[]),
        )
        .unwrap();
        assert!(set_enabled(&p, Runtime::Claude, Scope::User, "a", false).unwrap());
        let f = p.config_file(Runtime::Claude, Scope::User).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert!(v["mcpServers"].get("a").is_none());
        assert_eq!(v[PARKED_KEY]["a"]["command"], "x");
        let (l, _) = list(&p, Runtime::Claude);
        assert_eq!(l.len(), 1);
        assert!(!l[0].enabled);
        // Updating a disabled server keeps it disabled.
        upsert(
            &p,
            Runtime::Claude,
            Scope::User,
            "a",
            &ServerSpec::stdio("y", &[]),
        )
        .unwrap();
        let (l, _) = list(&p, Runtime::Claude);
        assert!(!l[0].enabled && l[0].spec.command == "y");
        assert!(set_enabled(&p, Runtime::Claude, Scope::User, "a", true).unwrap());
        let (l, _) = list(&p, Runtime::Claude);
        assert!(l[0].enabled);
        assert!(!set_enabled(&p, Runtime::Claude, Scope::User, "zzz", true).unwrap());
    }

    #[test]
    fn invalid_json_is_never_overwritten() {
        let (_d, p) = setup();
        let f = p.config_file(Runtime::Claude, Scope::User).unwrap();
        std::fs::write(&f, "{ not json").unwrap();
        assert!(upsert(
            &p,
            Runtime::Claude,
            Scope::User,
            "a",
            &ServerSpec::stdio("x", &[])
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "{ not json");
    }

    #[cfg(unix)]
    #[test]
    fn permissions_are_preserved() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, p) = setup();
        let f = p.config_file(Runtime::Claude, Scope::User).unwrap();
        std::fs::write(&f, "{}").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        upsert(
            &p,
            Runtime::Claude,
            Scope::User,
            "a",
            &ServerSpec::stdio("x", &[]),
        )
        .unwrap();
        assert_eq!(
            std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    const CODEX: &str = "# my codex config\nmodel = \"gpt-5\"  # keep me\n\n[mcp_servers.docs]\n# the docs server\ncommand = \"npx\"\nargs = [\"-y\", \"docs-mcp\"]\ncustom = 42\n\n[mcp_servers.docs.env]\nTOKEN = \"abc\"\n\n[profiles.fast]\nmodel = \"mini\"\n";

    #[test]
    fn codex_toml_preserves_comments_and_unknown() {
        let (_d, p) = setup();
        let f = p.config_file(Runtime::Codex, Scope::User).unwrap();
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, CODEX).unwrap();
        let (l, _) = list(&p, Runtime::Codex);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].spec.command, "npx");
        assert_eq!(l[0].spec.env["TOKEN"], "abc");
        upsert(
            &p,
            Runtime::Codex,
            Scope::User,
            "web",
            &ServerSpec::http("https://h.test/mcp"),
        )
        .unwrap();
        let mut s = ServerSpec::stdio("uvx", &["thing"]);
        s.env.insert("A".into(), "b".into());
        upsert(&p, Runtime::Codex, Scope::User, "docs", &s).unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.contains("# my codex config"));
        assert!(text.contains("# keep me"));
        assert!(text.contains("# the docs server"));
        assert!(text.contains("custom = 42"));
        assert!(text.contains("[profiles.fast]"));
        assert!(text.contains("url = \"https://h.test/mcp\""));
        let (l, _) = list(&p, Runtime::Codex);
        let docs = l.iter().find(|s| s.name == "docs").unwrap();
        assert_eq!(docs.spec.command, "uvx");
        assert_eq!(docs.spec.env["A"], "b");
        assert!(!docs.spec.env.contains_key("TOKEN"));
        let web = l.iter().find(|s| s.name == "web").unwrap();
        assert_eq!(web.spec.transport, Transport::Http);
    }

    #[test]
    fn codex_enable_disable_and_remove() {
        let (_d, p) = setup();
        let f = p.config_file(Runtime::Codex, Scope::User).unwrap();
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, CODEX).unwrap();
        assert!(set_enabled(&p, Runtime::Codex, Scope::User, "docs", false).unwrap());
        assert!(std::fs::read_to_string(&f)
            .unwrap()
            .contains("enabled = false"));
        assert!(!list(&p, Runtime::Codex).0[0].enabled);
        assert!(set_enabled(&p, Runtime::Codex, Scope::User, "docs", true).unwrap());
        assert!(list(&p, Runtime::Codex).0[0].enabled);
        assert!(remove(&p, Runtime::Codex, Scope::User, "docs").unwrap());
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(!text.contains("docs-mcp") && text.contains("# my codex config"));
    }

    #[test]
    fn codex_creates_missing_file() {
        let (_d, p) = setup();
        upsert(
            &p,
            Runtime::Codex,
            Scope::User,
            "a",
            &ServerSpec::stdio("x", &["1"]),
        )
        .unwrap();
        let f = p.config_file(Runtime::Codex, Scope::User).unwrap();
        assert!(std::fs::read_to_string(f)
            .unwrap()
            .contains("[mcp_servers.a]"));
    }

    #[test]
    fn opencode_and_gemini_dialects() {
        let (_d, p) = setup();
        let mut s = ServerSpec::stdio("npx", &["-y", "p"]);
        s.env.insert("E".into(), "1".into());
        upsert(&p, Runtime::OpenCode, Scope::User, "oc", &s).unwrap();
        upsert(
            &p,
            Runtime::OpenCode,
            Scope::User,
            "rem",
            &ServerSpec::http("https://r.test"),
        )
        .unwrap();
        let f = p.config_file(Runtime::OpenCode, Scope::User).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["mcp"]["oc"]["type"], "local");
        assert_eq!(v["mcp"]["oc"]["command"][2], "p");
        assert_eq!(v["mcp"]["oc"]["environment"]["E"], "1");
        assert_eq!(v["mcp"]["rem"]["type"], "remote");
        assert!(set_enabled(&p, Runtime::OpenCode, Scope::User, "oc", false).unwrap());
        let (l, _) = list(&p, Runtime::OpenCode);
        let oc = l.iter().find(|x| x.name == "oc").unwrap();
        assert!(!oc.enabled);
        assert_eq!(oc.spec.args, vec!["-y", "p"]);

        upsert(
            &p,
            Runtime::Gemini,
            Scope::User,
            "g",
            &ServerSpec::http("https://g.test"),
        )
        .unwrap();
        upsert(&p, Runtime::Gemini, Scope::Project, "gs", &s).unwrap();
        let (l, _) = list(&p, Runtime::Gemini);
        assert_eq!(l.len(), 2);
        let g = l.iter().find(|x| x.name == "g").unwrap();
        assert_eq!(g.spec.transport, Transport::Http);
        let gf = p.config_file(Runtime::Gemini, Scope::User).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(gf).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["g"]["httpUrl"], "https://g.test");
    }

    #[test]
    fn local_scope_only_for_claude() {
        let (_d, p) = setup();
        assert!(upsert(
            &p,
            Runtime::Codex,
            Scope::Local,
            "a",
            &ServerSpec::stdio("x", &[])
        )
        .is_err());
        let no_proj = ClientPaths::new(p.user_home.clone(), None);
        assert!(upsert(
            &no_proj,
            Runtime::Claude,
            Scope::Project,
            "a",
            &ServerSpec::stdio("x", &[])
        )
        .is_err());
        assert!(list(&no_proj, Runtime::Claude).1.is_empty());
    }
}
