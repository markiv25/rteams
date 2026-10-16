//! rteams-server: MCP over stdio. One dispatch path: tool call -> core `Command` ->
//! event appended to the store -> updated projection returned. Other transports
//! (TUI, later HTTP) call `Server::call_tool` / `Server::handle`, never the store directly.
//!
//! Transport is a hand-rolled JSON-RPC 2.0 line loop (initialize, ping, tools/list,
//! tools/call); the tool layer in `tools` is transport-independent, so swapping in `rmcp`
//! later only replaces this file's protocol handling.

mod tools;

use std::io;
use std::sync::Mutex;

use rteams_store::{MemoryStore, Store};
use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

pub use tools::ToolError;

const SUPPORTED_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

pub struct Server {
    store: Mutex<Box<dyn Store + Send>>,
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    /// Ephemeral, in-process state (tests, throwaway servers).
    pub fn new() -> Self {
        Self::with_store(MemoryStore::new())
    }

    /// Serve any store. Give several servers (processes) a `FileStore` over the same root
    /// and team and they share state: every tool call locks, catches up, then decides.
    pub fn with_store(store: impl Store + Send + 'static) -> Self {
        Self {
            store: Mutex::new(Box::new(store)),
        }
    }

    /// Run a tool directly (no JSON-RPC framing). `Ok` is the tool's JSON result.
    pub fn call_tool(&self, name: &str, args: Value) -> Result<Value, ToolError> {
        let mut store = self.store.lock().expect("store lock poisoned");
        tools::call(&mut **store, name, args)
    }

    /// Handle one raw JSON-RPC message line. `None` for notifications and client responses.
    pub fn handle_line(&self, line: &str) -> Option<String> {
        let reply = match serde_json::from_str::<Value>(line) {
            Ok(msg) => self.handle(msg)?,
            Err(e) => rpc_error(Value::Null, PARSE_ERROR, format!("parse error: {e}")),
        };
        Some(reply.to_string())
    }

    /// Handle one decoded JSON-RPC message.
    pub fn handle(&self, msg: Value) -> Option<Value> {
        let Some(obj) = msg.as_object() else {
            return Some(rpc_error(
                Value::Null,
                INVALID_REQUEST,
                "request must be a JSON object".into(),
            ));
        };
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            return None; // a response from the client; we send no requests, so ignore
        };
        let Some(id) = obj.get("id").cloned() else {
            return None; // notification (e.g. notifications/initialized)
        };
        let params = obj.get("params").cloned().unwrap_or(Value::Null);

        Some(match method {
            "initialize" => rpc_result(id, initialize(&params)),
            "ping" => rpc_result(id, json!({})),
            "tools/list" => rpc_result(id, json!({ "tools": tools::definitions() })),
            "tools/call" => self.tools_call(id, &params),
            other => rpc_error(id, METHOD_NOT_FOUND, format!("method not found: {other}")),
        })
    }

    fn tools_call(&self, id: Value, params: &Value) -> Value {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return rpc_error(
                id,
                INVALID_PARAMS,
                "tools/call requires a string `name`".into(),
            );
        };
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        match self.call_tool(name, args) {
            Ok(value) => rpc_result(id, tool_result(&value, false)),
            // Domain failures are tool errors the model can read and react to.
            Err(ToolError::Core(e)) => {
                let body = json!({ "code": tools::error_code(&e), "message": e.to_string() });
                rpc_result(id, tool_result(&body, true))
            }
            // Storage failures (I/O, corrupt log) are also readable by the model, not protocol errors.
            Err(ToolError::Store(message)) => {
                let body = json!({ "code": "store_error", "message": message });
                rpc_result(id, tool_result(&body, true))
            }
            Err(ToolError::Forbidden(m)) => {
                let body = json!({ "code": "forbidden", "message": m });
                rpc_result(id, tool_result(&body, true))
            }
            Err(e @ ToolError::UnknownTool(_)) | Err(e @ ToolError::InvalidArguments(_)) => {
                rpc_error(id, INVALID_PARAMS, e.to_string())
            }
        }
    }

    /// Serve newline-delimited JSON-RPC until the reader hits EOF.
    pub async fn serve<R, W>(&self, reader: R, mut writer: W) -> io::Result<()>
    where
        R: AsyncBufRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut lines = reader.lines();
        while let Some(line) = lines.next_line().await? {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(reply) = self.handle_line(line) {
                writer.write_all(reply.as_bytes()).await?;
                writer.write_all(b"\n").await?;
                writer.flush().await?;
            }
        }
        Ok(())
    }

    pub async fn serve_stdio(&self) -> io::Result<()> {
        self.serve(BufReader::new(tokio::io::stdin()), tokio::io::stdout())
            .await
    }
}

fn initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|v| SUPPORTED_VERSIONS.contains(v))
        .unwrap_or(SUPPORTED_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "rteams", "version": env!("CARGO_PKG_VERSION") },
    })
}

fn tool_result(value: &Value, is_error: bool) -> Value {
    let mut r = json!({
        "content": [{ "type": "text", "text": value.to_string() }],
        "isError": is_error,
    });
    if !is_error {
        r["structuredContent"] = value.clone();
    }
    r
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i64, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}
