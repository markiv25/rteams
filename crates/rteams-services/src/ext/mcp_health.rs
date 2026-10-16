//! MCP health check: start the server the way an agent would, run the `initialize` handshake,
//! ask for `tools/list` and report what came back (server name / version, tool count) or why it
//! failed. Stdio servers are spawned; remote servers get a JSON-RPC POST through `curl`.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::mcp_config::{ServerSpec, Transport};

pub const PROTOCOL_VERSION: &str = "2025-03-26";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    Ok,
    /// Handshake worked but the server wants credentials (HTTP 401 / 403).
    NeedsAuth,
    Error,
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    pub status: HealthStatus,
    pub server_name: Option<String>,
    pub server_version: Option<String>,
    pub protocol: Option<String>,
    pub tool_count: Option<usize>,
    pub tool_names: Vec<String>,
    pub error: Option<String>,
    pub elapsed_ms: u64,
}

impl Health {
    fn new(status: HealthStatus, started: Instant) -> Self {
        Health {
            status,
            server_name: None,
            server_version: None,
            protocol: None,
            tool_count: None,
            tool_names: Vec::new(),
            error: None,
            elapsed_ms: started.elapsed().as_millis() as u64,
        }
    }

    fn fail(status: HealthStatus, started: Instant, msg: impl Into<String>) -> Self {
        let mut h = Health::new(status, started);
        h.error = Some(msg.into());
        h
    }

    pub fn is_ok(&self) -> bool {
        self.status == HealthStatus::Ok
    }
}

fn initialize_request() -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {"name":"rteams","version": env!("CARGO_PKG_VERSION")}
    }})
}

fn fill_from_initialize(h: &mut Health, resp: &Value) {
    let r = &resp["result"];
    h.server_name = r["serverInfo"]["name"].as_str().map(str::to_string);
    h.server_version = r["serverInfo"]["version"].as_str().map(str::to_string);
    h.protocol = r["protocolVersion"].as_str().map(str::to_string);
}

fn fill_from_tools(h: &mut Health, resp: &Value) {
    if let Some(tools) = resp["result"]["tools"].as_array() {
        h.tool_names = tools
            .iter()
            .filter_map(|t| t["name"].as_str().map(str::to_string))
            .collect();
        h.tool_count = Some(tools.len());
    }
}

fn rpc_error(resp: &Value) -> Option<String> {
    resp.get("error").map(|e| {
        e["message"]
            .as_str()
            .map_or_else(|| e.to_string(), str::to_string)
    })
}

/// Check a stdio server: spawn `command args` with `env`, handshake, list tools, kill.
pub fn check_stdio(
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&Path>,
    timeout: Duration,
) -> Health {
    let started = Instant::now();
    let deadline = started + timeout;
    let mut cmd = Command::new(command);
    cmd.args(args)
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(c) = cwd {
        cmd.current_dir(c);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Health::fail(
                HealthStatus::Error,
                started,
                format!("cannot start `{command}`: {e}"),
            )
        }
    };
    let mut stdin = child.stdin.take();
    let (tx, rx) = mpsc::channel::<String>();
    if let Some(out) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
    }
    let (etx, erx) = mpsc::channel::<String>();
    if let Some(err) = child.stderr.take() {
        std::thread::spawn(move || {
            let mut tail = String::new();
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                tail.push_str(&line);
                tail.push('\n');
                if tail.len() > 4000 {
                    tail = tail[tail.len() - 2000..].to_string();
                }
                let _ = etx.send(tail.clone());
            }
        });
    }

    let send = |stdin: &mut Option<std::process::ChildStdin>, v: &Value| -> bool {
        match stdin {
            Some(w) => writeln!(w, "{v}").and_then(|_| w.flush()).is_ok(),
            None => false,
        }
    };
    // Wait for the response with `id`, skipping logs and notifications.
    let recv = |id: i64| -> Result<Value, HealthStatus> {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(HealthStatus::Timeout);
            }
            match rx.recv_timeout(left) {
                Ok(line) => {
                    if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
                        if v["id"].as_i64() == Some(id) {
                            return Ok(v);
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => return Err(HealthStatus::Timeout),
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(HealthStatus::Error),
            }
        }
    };

    let stderr_tail = || {
        let mut last = String::new();
        while let Ok(t) = erx.try_recv() {
            last = t;
        }
        last.trim().to_string()
    };

    let result = (|| {
        if !send(&mut stdin, &initialize_request()) {
            return Err((HealthStatus::Error, "server closed its input".to_string()));
        }
        let init = recv(1).map_err(|s| (s, String::new()))?;
        if let Some(e) = rpc_error(&init) {
            return Err((HealthStatus::Error, format!("initialize failed: {e}")));
        }
        let mut h = Health::new(HealthStatus::Ok, started);
        fill_from_initialize(&mut h, &init);
        send(
            &mut stdin,
            &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        );
        send(
            &mut stdin,
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        );
        match recv(2) {
            Ok(r) => {
                if let Some(e) = rpc_error(&r) {
                    // Servers without tools answer "method not found": still healthy.
                    if r["error"]["code"].as_i64() != Some(-32601) {
                        h.error = Some(format!("tools/list failed: {e}"));
                    } else {
                        h.tool_count = Some(0);
                    }
                } else {
                    fill_from_tools(&mut h, &r);
                }
            }
            Err(HealthStatus::Timeout) => h.error = Some("tools/list timed out".into()),
            Err(_) => h.error = Some("server exited before listing tools".into()),
        }
        Ok(h)
    })();

    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();

    match result {
        Ok(mut h) => {
            h.elapsed_ms = started.elapsed().as_millis() as u64;
            h
        }
        Err((status, msg)) => {
            let mut text = msg;
            let tail = stderr_tail();
            if status == HealthStatus::Timeout {
                text = "no response to initialize before the timeout".into();
            } else if text.is_empty() {
                text = "server exited before answering".into();
            }
            if !tail.is_empty() {
                text.push_str(": ");
                text.push_str(tail.lines().last().unwrap_or(""));
            }
            Health::fail(status, started, text)
        }
    }
}

fn parse_http_body(body: &str) -> Option<Value> {
    let t = body.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return Some(v);
    }
    // text/event-stream: take the last JSON `data:` payload.
    t.lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
        .next_back()
}

struct HttpReply {
    status: u16,
    session: Option<String>,
    body: String,
}

fn http_post(
    url: &str,
    headers: &BTreeMap<String, String>,
    session: Option<&str>,
    body: &Value,
    timeout: Duration,
) -> Result<HttpReply, String> {
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "-i", "-m", &timeout.as_secs().max(1).to_string()])
        .args(["-X", "POST", "-H", "Content-Type: application/json"])
        .args(["-H", "Accept: application/json, text/event-stream"]);
    for (k, v) in headers {
        cmd.args(["-H", &format!("{k}: {v}")]);
    }
    if let Some(s) = session {
        cmd.args(["-H", &format!("Mcp-Session-Id: {s}")]);
    }
    cmd.args(["--data-binary", "@-", "--", url])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("curl unavailable: {e}"))?;
    if let Some(mut w) = child.stdin.take() {
        let _ = w.write_all(body.to_string().as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let e = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if e.is_empty() {
            format!("curl exit {}", out.status)
        } else {
            e
        });
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    // May start with several header blocks (100 Continue, redirects): use the last one.
    let mut rest = text.as_str();
    let mut status = 0u16;
    let mut session_id = None;
    while rest.starts_with("HTTP/") {
        let (head, tail) = rest
            .split_once("\r\n\r\n")
            .or_else(|| rest.split_once("\n\n"))
            .unwrap_or((rest, ""));
        let mut lines = head.lines();
        status = lines
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        for l in lines {
            if let Some((k, v)) = l.split_once(':') {
                if k.eq_ignore_ascii_case("mcp-session-id") {
                    session_id = Some(v.trim().to_string());
                }
            }
        }
        rest = tail;
    }
    Ok(HttpReply {
        status,
        session: session_id,
        body: rest.to_string(),
    })
}

/// Check a streamable-HTTP server: POST `initialize`, then `tools/list` with the session id.
pub fn check_http(url: &str, headers: &BTreeMap<String, String>, timeout: Duration) -> Health {
    let started = Instant::now();
    let reply = match http_post(url, headers, None, &initialize_request(), timeout) {
        Ok(r) => r,
        Err(e) => return Health::fail(HealthStatus::Error, started, e),
    };
    if matches!(reply.status, 401 | 403) {
        return Health::fail(
            HealthStatus::NeedsAuth,
            started,
            format!("HTTP {}: authentication required", reply.status),
        );
    }
    if !(200..300).contains(&reply.status) {
        return Health::fail(
            HealthStatus::Error,
            started,
            format!("HTTP {}", reply.status),
        );
    }
    let Some(init) = parse_http_body(&reply.body) else {
        return Health::fail(HealthStatus::Error, started, "response is not MCP JSON-RPC");
    };
    if let Some(e) = rpc_error(&init) {
        return Health::fail(
            HealthStatus::Error,
            started,
            format!("initialize failed: {e}"),
        );
    }
    let mut h = Health::new(HealthStatus::Ok, started);
    fill_from_initialize(&mut h, &init);
    let session = reply.session;
    let _ = http_post(
        url,
        headers,
        session.as_deref(),
        &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        timeout,
    );
    match http_post(
        url,
        headers,
        session.as_deref(),
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        timeout,
    ) {
        Ok(r) => match parse_http_body(&r.body) {
            Some(v) if rpc_error(&v).is_none() => fill_from_tools(&mut h, &v),
            Some(v) => h.error = rpc_error(&v).map(|e| format!("tools/list failed: {e}")),
            None => {}
        },
        Err(e) => h.error = Some(e),
    }
    h.elapsed_ms = started.elapsed().as_millis() as u64;
    h
}

/// Legacy SSE endpoints cannot be handshaken with a plain POST: just confirm the stream opens.
pub fn check_sse(url: &str, headers: &BTreeMap<String, String>, timeout: Duration) -> Health {
    let started = Instant::now();
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "-N", "-o", "/dev/null", "-w", "%{http_code}", "-m"])
        .arg(timeout.as_secs().clamp(1, 5).to_string())
        .args(["-H", "Accept: text/event-stream"]);
    for (k, v) in headers {
        cmd.args(["-H", &format!("{k}: {v}")]);
    }
    cmd.args(["--", url]);
    let out = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            return Health::fail(
                HealthStatus::Error,
                started,
                format!("curl unavailable: {e}"),
            )
        }
    };
    // curl exits 28 when the open stream hits `-m`: that is success for SSE.
    let code: u16 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    match code {
        200..=299 => Health::new(HealthStatus::Ok, started),
        401 | 403 => Health::fail(
            HealthStatus::NeedsAuth,
            started,
            format!("HTTP {code}: authentication required"),
        ),
        0 => Health::fail(
            HealthStatus::Error,
            started,
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ),
        c => Health::fail(HealthStatus::Error, started, format!("HTTP {c}")),
    }
}

/// Check any server spec.
pub fn check(spec: &ServerSpec, cwd: Option<&Path>, timeout: Duration) -> Health {
    match spec.transport {
        Transport::Stdio => check_stdio(&spec.command, &spec.args, &spec.env, cwd, timeout),
        Transport::Http => check_http(&spec.url, &spec.headers, timeout),
        Transport::Sse => check_sse(&spec.url, &spec.headers, timeout),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const FAKE: &str = r#"#!/bin/sh
echo "starting up" >&2
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      echo 'noise that is not json'
      echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"9.9"}}}' ;;
    *'"method":"tools/list"'*)
      echo '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"alpha"},{"name":"beta"},{"name":"gamma"}]}}' ;;
  esac
done
"#;

    fn script(dir: &Path, name: &str, body: &str) -> String {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn handshake_with_fake_server_counts_tools() {
        let d = tempfile::tempdir().unwrap();
        let s = script(d.path(), "fake.sh", FAKE);
        let h = check_stdio(&s, &[], &BTreeMap::new(), None, Duration::from_secs(10));
        assert_eq!(h.status, HealthStatus::Ok, "{:?}", h.error);
        assert_eq!(h.server_name.as_deref(), Some("fake"));
        assert_eq!(h.server_version.as_deref(), Some("9.9"));
        assert_eq!(h.protocol.as_deref(), Some("2025-03-26"));
        assert_eq!(h.tool_count, Some(3));
        assert_eq!(h.tool_names, ["alpha", "beta", "gamma"]);
    }

    #[test]
    fn silent_server_times_out() {
        let d = tempfile::tempdir().unwrap();
        let s = script(d.path(), "silent.sh", "#!/bin/sh\ncat >/dev/null\n");
        let t = Instant::now();
        let h = check_stdio(&s, &[], &BTreeMap::new(), None, Duration::from_millis(400));
        assert_eq!(h.status, HealthStatus::Timeout);
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn crashing_server_reports_stderr() {
        let d = tempfile::tempdir().unwrap();
        let s = script(
            d.path(),
            "boom.sh",
            "#!/bin/sh\necho 'missing token' >&2\nexit 1\n",
        );
        let h = check_stdio(&s, &[], &BTreeMap::new(), None, Duration::from_secs(5));
        assert_eq!(h.status, HealthStatus::Error);
        assert!(h.error.unwrap().contains("missing token"));
    }

    #[test]
    fn missing_binary_is_an_error() {
        let h = check_stdio(
            "/no/such/binary-xyz",
            &[],
            &BTreeMap::new(),
            None,
            Duration::from_secs(2),
        );
        assert_eq!(h.status, HealthStatus::Error);
        assert!(h.error.unwrap().contains("cannot start"));
    }

    #[test]
    fn env_reaches_the_server() {
        let d = tempfile::tempdir().unwrap();
        let s = script(
            d.path(),
            "envsrv.sh",
            "#!/bin/sh\nIFS= read -r line\nprintf '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"serverInfo\":{\"name\":\"%s\"}}}\\n' \"$NAME_X\"\nIFS= read -r a\nIFS= read -r b\nprintf '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[]}}\\n'\n",
        );
        let mut env = BTreeMap::new();
        env.insert("NAME_X".to_string(), "from-env".to_string());
        let h = check_stdio(&s, &[], &env, None, Duration::from_secs(5));
        assert_eq!(h.server_name.as_deref(), Some("from-env"), "{:?}", h.error);
        assert_eq!(h.tool_count, Some(0));
    }

    #[test]
    fn parses_sse_bodies() {
        let v = parse_http_body("event: message\ndata: {\"id\":1,\"result\":{}}\n\n").unwrap();
        assert_eq!(v["id"], 1);
        assert!(parse_http_body("garbage").is_none());
    }
}
