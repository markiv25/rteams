//! Local JSON + SSE API for external tools and dashboards. OFF unless `general.server_enabled`;
//! binds 127.0.0.1 only; every route needs `Authorization: Bearer <server_token>`; no CORS.
//! Reads go through `rteams-store` read-only; the few POST routes go through
//! `rteams_server::Server::call_tool`, so they obey the same rules as agent tool calls.
//! Routes are documented in `packaging/HTTP_API.md`.

use std::collections::HashMap;
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, StatusCode};

use crate::config::AppConfig;

pub mod auth;
pub mod routes_config;
pub mod routes_misc;
pub mod routes_sessions;
pub mod routes_teams;
pub mod sse;

pub const MAX_BODY: usize = 1024 * 1024;
const MAX_CONNECTIONS: usize = 64;

pub struct Ctx {
    pub home: PathBuf,
}

pub struct Req {
    pub method: String,
    pub segs: Vec<String>,
    pub query: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Req {
    pub fn segs(&self) -> Vec<&str> {
        self.segs.iter().map(String::as_str).collect()
    }
    pub fn q(&self, k: &str) -> Option<&str> {
        self.query.get(k).map(String::as_str)
    }
}

pub struct Resp {
    pub status: u16,
    pub body: Value,
}

impl Resp {
    pub fn ok(body: Value) -> Resp {
        Resp { status: 200, body }
    }
    pub fn err(status: u16, msg: &str) -> Resp {
        Resp {
            status,
            body: json!({ "error": msg }),
        }
    }
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_url(url: &str) -> (Vec<String>, HashMap<String, String>) {
    let (path, q) = url.split_once('?').unwrap_or((url, ""));
    let segs = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(pct_decode)
        .collect();
    let query = q
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (pct_decode(k), pct_decode(v))
        })
        .collect();
    (segs, query)
}

/// Route a request (after auth). Pure: no sockets, so it is directly testable.
pub fn route(ctx: &Ctx, r: &Req) -> Resp {
    routes_misc::handle(ctx, r)
        .or_else(|| routes_teams::handle(ctx, r))
        .or_else(|| routes_sessions::handle(ctx, r))
        .or_else(|| routes_config::handle(ctx, r))
        .unwrap_or_else(|| Resp::err(404, "not found"))
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header")
}

fn send(req: Request, resp: Resp) {
    let body = resp.body.to_string();
    let r = Response::from_string(body)
        .with_status_code(StatusCode(resp.status))
        .with_header(header("Content-Type", "application/json"))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("X-Content-Type-Options", "nosniff"));
    let _ = req.respond(r);
}

fn header_of(req: &Request, name: &str) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
}

struct Shared {
    ctx: Ctx,
    token: String,
    stop: AtomicBool,
    active: AtomicUsize,
}

fn handle_request(sh: &Shared, mut req: Request) {
    if !auth::host_ok(header_of(&req, "Host").as_deref()) {
        return send(req, Resp::err(403, "forbidden host"));
    }
    if !auth::bearer_ok(header_of(&req, "Authorization").as_deref(), &sh.token) {
        return send(req, Resp::err(401, "missing or invalid bearer token"));
    }
    let method = match req.method() {
        Method::Get => "GET",
        Method::Post => "POST",
        _ => return send(req, Resp::err(405, "method not allowed")),
    };
    let (segs, query) = parse_url(req.url());
    if method == "GET" && segs.iter().map(String::as_str).eq(["api", "events"]) {
        let only = query.get("team").filter(|t| routes_teams::valid_team(t));
        let tailer = sse::Tailer::new(&sh.ctx.home, only.map(String::as_str));
        let _ = sse::serve(req.into_writer(), tailer, &sh.stop);
        return;
    }
    let mut body = Vec::new();
    if method == "POST" {
        if req.body_length().is_some_and(|n| n > MAX_BODY) {
            return send(req, Resp::err(413, "body too large"));
        }
        let _ = req
            .as_reader()
            .take(MAX_BODY as u64 + 1)
            .read_to_end(&mut body);
        if body.len() > MAX_BODY {
            return send(req, Resp::err(413, "body too large"));
        }
    }
    let r = Req {
        method: method.into(),
        segs,
        query,
        body,
    };
    let resp = route(&sh.ctx, &r);
    send(req, resp);
}

pub struct HttpServer;

/// A running server. Dropping it stops it.
pub struct Handle {
    addr: SocketAddr,
    token: String,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
    pub fn port(&self) -> u16 {
        self.addr.port()
    }
    pub fn token(&self) -> &str {
        &self.token
    }
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
    pub fn stop(mut self) {
        self.shutdown();
    }
    fn shutdown(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl HttpServer {
    /// Fill an empty `server_token` with a fresh random one. Returns true if it changed (the
    /// caller should persist the config).
    pub fn ensure_token(cfg: &mut AppConfig) -> bool {
        if cfg.general.server_token.trim().is_empty() {
            cfg.general.server_token = auth::generate_token();
            true
        } else {
            false
        }
    }

    /// Start serving on `127.0.0.1:<server_port>` (0 = any free port). Refuses unless
    /// `server_enabled`. An empty token is replaced by a random one, readable via
    /// `Handle::token`.
    pub fn start(home: &Path, cfg: &AppConfig) -> Result<Handle, String> {
        if !cfg.general.server_enabled {
            return Err("HTTP server is disabled in settings".into());
        }
        let mut cfg = cfg.clone();
        Self::ensure_token(&mut cfg);
        let token = cfg.general.server_token.clone();
        let server = tiny_http::Server::http(("127.0.0.1", cfg.general.server_port))
            .map_err(|e| format!("cannot bind 127.0.0.1:{}: {e}", cfg.general.server_port))?;
        let addr = server
            .server_addr()
            .to_ip()
            .ok_or("server has no IP address")?;
        let shared = Arc::new(Shared {
            ctx: Ctx {
                home: home.to_path_buf(),
            },
            token: token.clone(),
            stop: AtomicBool::new(false),
            active: AtomicUsize::new(0),
        });
        let sh = shared.clone();
        let thread = std::thread::spawn(move || {
            while !sh.stop.load(Ordering::Relaxed) {
                let Ok(Some(req)) = server.recv_timeout(Duration::from_millis(200)) else {
                    continue;
                };
                if sh.active.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                    send(req, Resp::err(503, "too many connections"));
                    continue;
                }
                sh.active.fetch_add(1, Ordering::Relaxed);
                let sh2 = sh.clone();
                std::thread::spawn(move || {
                    handle_request(&sh2, req);
                    sh2.active.fetch_sub(1, Ordering::Relaxed);
                });
            }
        });
        Ok(Handle {
            addr,
            token,
            shared,
            thread: Some(thread),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpStream;

    fn http(h: &Handle, method: &str, path: &str, token: Option<&str>, body: &str) -> (u16, Value) {
        let mut s = TcpStream::connect(h.addr()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let auth = token.map_or(String::new(), |t| format!("Authorization: Bearer {t}\r\n"));
        write!(
            s,
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut raw = String::new();
        s.read_to_string(&mut raw).unwrap();
        let status = raw.split(' ').nth(1).unwrap().parse().unwrap();
        let body = raw.split("\r\n\r\n").nth(1).unwrap_or("");
        (status, serde_json::from_str(body).unwrap_or(Value::Null))
    }

    fn cfg(token: &str) -> AppConfig {
        let mut c = AppConfig::default();
        c.general.server_enabled = true;
        c.general.server_port = 0;
        c.general.server_token = token.into();
        c
    }

    fn make_team(home: &Path, name: &str) {
        use rteams_core::command::Command;
        use rteams_store::Store;
        let mut st = rteams_store::FileStore::open(home, name).unwrap();
        st.execute_guarded(
            "lead",
            1,
            None,
            Command::CreateTeam { name: name.into() },
            &|_| Ok(()),
        )
        .unwrap();
        st.execute_guarded(
            "lead",
            2,
            None,
            Command::AddMember {
                name: "lead".into(),
                role: "lead".into(),
                runtime: None,
            },
            &|_| Ok(()),
        )
        .unwrap();
    }

    #[test]
    fn disabled_by_default() {
        let d = tempfile::tempdir().unwrap();
        assert!(HttpServer::start(d.path(), &AppConfig::default()).is_err());
    }

    #[test]
    fn auth_and_routes() {
        let d = tempfile::tempdir().unwrap();
        make_team(d.path(), "alpha");
        let h = HttpServer::start(d.path(), &cfg("")).unwrap();
        assert!(h.addr().ip().is_loopback());
        assert_eq!(h.token().len(), 64, "random token generated");
        let tok = h.token().to_string();

        assert_eq!(http(&h, "GET", "/api/health", None, "").0, 401);
        assert_eq!(http(&h, "GET", "/api/health", Some("nope"), "").0, 401);
        assert_eq!(http(&h, "GET", "/nothing", None, "").0, 401);
        let (st, v) = http(&h, "GET", "/api/health", Some(&tok), "");
        assert_eq!((st, v["ok"].clone()), (200, json!(true)));
        let (_, v) = http(&h, "GET", "/api/teams", Some(&tok), "");
        assert_eq!(v["teams"][0]["name"], "alpha");
        assert_eq!(
            http(&h, "GET", "/api/teams/alpha/kanban", Some(&tok), "").0,
            200
        );
        assert_eq!(
            http(&h, "GET", "/api/teams/nope/kanban", Some(&tok), "").0,
            404
        );
        assert_eq!(http(&h, "GET", "/api/teams/..%2Fx", Some(&tok), "").0, 400);
        assert_eq!(http(&h, "GET", "/api/usage", Some(&tok), "").0, 200);
        assert_eq!(http(&h, "GET", "/api/zzz", Some(&tok), "").0, 404);
        h.stop();
    }

    #[test]
    fn config_redacts_token() {
        let d = tempfile::tempdir().unwrap();
        let c = cfg("s3cret-token-value");
        c.save(d.path()).unwrap();
        let h = HttpServer::start(d.path(), &c).unwrap();
        let (st, v) = http(&h, "GET", "/api/config", Some("s3cret-token-value"), "");
        assert_eq!(st, 200);
        assert!(!v.to_string().contains("s3cret-token-value"));
        h.stop();
    }

    #[test]
    fn post_requires_command_id_and_goes_through_tools() {
        let d = tempfile::tempdir().unwrap();
        make_team(d.path(), "alpha");
        let h = HttpServer::start(d.path(), &cfg("t0k")).unwrap();
        let (st, _) = http(
            &h,
            "POST",
            "/api/teams/alpha/tasks",
            Some("t0k"),
            r#"{"actor":"lead","subject":"x"}"#,
        );
        assert_eq!(st, 400);
        let (st, _) = http(
            &h,
            "POST",
            "/api/teams/alpha/messages",
            Some("t0k"),
            r#"{"actor":"ghost","text":"x","command_id":"c1"}"#,
        );
        assert_eq!(st, 422, "non-member actor refused like an agent");
        let (st, v) = http(
            &h,
            "POST",
            "/api/teams/alpha/tasks",
            Some("t0k"),
            r#"{"actor":"lead","subject":"x","command_id":"c2"}"#,
        );
        assert_eq!(st, 200, "{v}");
        let (_, v) = http(&h, "GET", "/api/teams/alpha/tasks", Some("t0k"), "");
        assert_eq!(v["tasks"].as_array().unwrap().len(), 1);
        let (st, _) = http(&h, "POST", "/api/teams/alpha/members", Some("t0k"), "{}");
        assert_eq!(st, 404, "non-whitelisted tool");
        h.stop();
    }

    #[test]
    fn sse_receives_appended_event() {
        let d = tempfile::tempdir().unwrap();
        make_team(d.path(), "alpha");
        let h = HttpServer::start(d.path(), &cfg("t0k")).unwrap();
        let mut s = TcpStream::connect(h.addr()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        write!(
            s,
            "GET /api/events HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer t0k\r\n\r\n"
        )
        .unwrap();
        let mut rd = BufReader::new(s);
        let mut line = String::new();
        loop {
            line.clear();
            rd.read_line(&mut line).unwrap();
            if line.starts_with(": connected") {
                break;
            }
        }
        // Append after the stream is established.
        let mut st = rteams_store::FileStore::open(d.path(), "alpha").unwrap();
        use rteams_store::Store;
        st.execute_guarded(
            "lead",
            2,
            None,
            rteams_core::command::Command::SendMessage {
                to: None,
                text: "hi".into(),
                mode: None,
                from_team: None,
                attachments: vec![],
                chat: None,
            },
            &|_| Ok(()),
        )
        .ok();
        let want = st.loaded_events().len() as u64;
        let mut got = None;
        for _ in 0..50 {
            line.clear();
            if rd.read_line(&mut line).unwrap() == 0 {
                break;
            }
            if let Some(j) = line.strip_prefix("data: ") {
                got = Some(serde_json::from_str::<Value>(j).unwrap());
                break;
            }
        }
        let v = got.expect("sse data frame");
        assert_eq!(v["team"], "alpha");
        assert_eq!(v["seq"], want);
        h.stop();
    }

    #[test]
    fn url_parsing() {
        let (s, q) = parse_url("/api/teams/a%20b?limit=5&x=%41");
        assert_eq!(s, ["api", "teams", "a b"]);
        assert_eq!(q["limit"], "5");
        assert_eq!(q["x"], "A");
    }
}
