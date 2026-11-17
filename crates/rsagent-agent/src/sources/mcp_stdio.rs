//! A source backed by a [Model Context Protocol] server, spoken over stdio.
//!
//! An MCP server is a long-running process that speaks JSON-RPC 2.0 over its stdin and
//! stdout: the client sends requests, the server answers, and (unlike the [`local`] source)
//! the process stays alive between calls. This module implements the small subset rsagent
//! needs — `initialize`, `tools/list`, and `tools/call` — against protocol revision
//! [`MCP_PROTOCOL_VERSION`].
//!
//! [Model Context Protocol]: https://modelcontextprotocol.io
//! [`local`]: super::local
//!
//! # Wire format
//!
//! Every message is one line of JSON, terminated by a newline. A request carries an `id`;
//! the server's response echoes it. Notifications (like `notifications/initialized`) carry
//! no `id` and get no response. Anything the server writes that is not a response to our
//! request is ignored, so logging-style notifications are harmless.
//!
//! # Failure classes
//!
//! - A **transport** failure — cannot start, cannot read or write, a timeout, non-JSON
//!   output, or the server closing its stdout — is connection-class: it trips the circuit
//!   breaker and the session is torn down so the next call reconnects.
//! - A **protocol** failure — a well-formed JSON-RPC `error` response — means the server is
//!   reachable and speaking the protocol correctly; it rejected the request, but it is
//!   working. Like the `local` source's `{"error": ...}` reply, it does not trip the breaker.
//! - A `tools/call` result with `"isError": true` is a **business** failure for the same
//!   reason: the tool ran and reported an error.

use std::fmt;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rsagent_ai::ToolSpec;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, Command};
use tokio::sync::Mutex;

use crate::breaker::{BreakerPolicy, BreakerState, CircuitBreaker};
use crate::source::{SourceHealth, SourceKind, ToolSource};
use crate::tool::{Tool, ToolContext, ToolResult};

/// The MCP revision this client asks for.
///
/// The server may answer with a different revision it supports; the subset used here
/// (`initialize` / `tools/list` / `tools/call`) is stable across revisions, so any answer
/// is accepted rather than failing the connection.
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

/// Per-request timeout when the configuration does not set one.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// How much of the server's stderr to keep for diagnostics.
const STDERR_LIMIT: usize = 4096;

/// How to (re)start the server process.
#[derive(Debug, Clone)]
struct SpawnSpec {
    command: String,
    args: Vec<String>,
}

/// A live connection to the server: its process and the two pipes.
struct Session {
    /// The child process. Kept only so it is killed when the session is dropped
    /// (`kill_on_drop`); the pipes below are what we actually talk through.
    child: Option<Child>,
    stdin: Box<dyn AsyncWrite + Unpin + Send>,
    stdout: BufReader<Box<dyn AsyncRead + Unpin + Send>>,
    /// The tail of the server's stderr, for error messages.
    stderr: Arc<StdMutex<String>>,
    /// The next JSON-RPC request id.
    next_id: i64,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("child", &self.child.as_ref().map(Child::id))
            .field("next_id", &self.next_id)
            .finish_non_exhaustive()
    }
}

/// Why a single request did not produce a result.
enum RequestError {
    /// The transport is broken; the session must be re-established.
    Transport(String),
    /// The server answered, but with a JSON-RPC error.
    Protocol(String),
}

impl RequestError {
    /// The human-readable message, whichever class it is.
    fn into_message(self) -> String {
        match self {
            RequestError::Transport(message) | RequestError::Protocol(message) => message,
        }
    }
}

/// Why a call to the server failed, from the caller's point of view.
enum CallError {
    /// Connection-class: the transport failed (or could not be re-established).
    Transport(String),
    /// The server is reachable but rejected the request.
    Protocol(String),
}

impl Session {
    /// Start the server and complete the MCP handshake.
    ///
    /// # Errors
    ///
    /// Returns a message when the process cannot be started, when the handshake fails, or
    /// when the server does not answer in time. The server's stderr tail is appended when
    /// there is one, since a failing server usually explains itself there.
    async fn connect(spec: &SpawnSpec, timeout: Duration) -> Result<Self, String> {
        let mut command = Command::new(&spec.command);
        command
            .args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| format!("cannot start MCP server `{}`: {error}", spec.command))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "the MCP server's stdin was not piped".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "the MCP server's stdout was not piped".to_string())?;

        let stderr = Arc::new(StdMutex::new(String::new()));
        if let Some(pipe) = child.stderr.take() {
            let sink = Arc::clone(&stderr);
            tokio::spawn(drain_stderr(pipe, sink));
        }

        let mut session = Session {
            child: Some(child),
            stdin: Box::new(stdin),
            stdout: BufReader::new(Box::new(stdout)),
            stderr,
            next_id: 1,
        };
        session.handshake(timeout).await.map_err(|error| {
            let tail = session.stderr_tail();
            if tail.is_empty() {
                error
            } else {
                format!("{error} (server stderr: {tail})")
            }
        })?;
        Ok(session)
    }

    /// A session over arbitrary streams, for tests.
    #[cfg(test)]
    fn over(
        stdin: Box<dyn AsyncWrite + Unpin + Send>,
        stdout: Box<dyn AsyncRead + Unpin + Send>,
    ) -> Self {
        Session {
            child: None,
            stdin,
            stdout: BufReader::new(stdout),
            stderr: Arc::new(StdMutex::new(String::new())),
            next_id: 1,
        }
    }

    /// Send `initialize`, then the `initialized` notification.
    async fn handshake(&mut self, timeout: Duration) -> Result<(), String> {
        let params = json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "rsagent", "version": env!("CARGO_PKG_VERSION") },
        });
        self.request("initialize", params, timeout)
            .await
            .map_err(RequestError::into_message)?;
        self.notify("notifications/initialized", json!({})).await
    }

    /// Ask for the server's tools, following pagination cursors.
    async fn list_tools(&mut self, timeout: Duration) -> Result<Vec<McpToolSpec>, String> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = cursor
                .as_deref()
                .map_or_else(|| json!({}), |cursor| json!({ "cursor": cursor }));
            let result = self
                .request("tools/list", params, timeout)
                .await
                .map_err(RequestError::into_message)?;
            tools.extend(parse_tools(&result)?);
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }

    /// Send a request and wait for the response with the matching id.
    async fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, RequestError> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.send(&message).await.map_err(RequestError::Transport)?;

        loop {
            let incoming = tokio::time::timeout(timeout, self.read_message())
                .await
                .map_err(|_| {
                    RequestError::Transport(format!(
                        "`{method}` timed out after {}s",
                        timeout.as_secs()
                    ))
                })??;
            // Skip notifications and responses to other requests.
            if incoming.get("id").and_then(Value::as_i64) != Some(id) {
                continue;
            }
            if let Some(error) = incoming.get("error") {
                let code = error
                    .get("code")
                    .and_then(Value::as_i64)
                    .unwrap_or_default();
                let text = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error");
                return Err(RequestError::Protocol(format!(
                    "`{method}` failed: JSON-RPC {code}: {text}"
                )));
            }
            return Ok(incoming.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// Send a notification (no id, no response expected).
    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.send(&message).await
    }

    /// Write one newline-terminated JSON message.
    async fn send(&mut self, message: &Value) -> Result<(), String> {
        let mut line = serde_json::to_string(message).map_err(|error| error.to_string())?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|error| format!("cannot write to the MCP server: {error}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|error| format!("cannot flush to the MCP server: {error}"))
    }

    /// Read one JSON message.
    async fn read_message(&mut self) -> Result<Value, RequestError> {
        let mut line = String::new();
        let read = self.stdout.read_line(&mut line).await.map_err(|error| {
            RequestError::Transport(format!("cannot read from the MCP server: {error}"))
        })?;
        if read == 0 {
            return Err(RequestError::Transport(
                "the MCP server closed the connection".to_string(),
            ));
        }
        serde_json::from_str(line.trim()).map_err(|error| {
            RequestError::Transport(format!("the MCP server sent invalid JSON: {error}"))
        })
    }

    /// The tail of the server's stderr, trimmed.
    fn stderr_tail(&self) -> String {
        self.stderr
            .lock()
            .map(|text| text.trim().to_string())
            .unwrap_or_default()
    }
}

/// Read the server's stderr to the end, keeping only the most recent [`STDERR_LIMIT`] bytes.
async fn drain_stderr(mut pipe: ChildStderr, sink: Arc<StdMutex<String>>) {
    let mut buffer = [0_u8; 1024];
    loop {
        match pipe.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if let Ok(mut text) = sink.lock() {
                    text.push_str(&String::from_utf8_lossy(&buffer[..read]));
                    if text.len() > STDERR_LIMIT {
                        let cut = text.len() - STDERR_LIMIT;
                        text.drain(..cut);
                    }
                }
            }
        }
    }
}

/// The shared connection to the server, so every tool of the source talks through one pipe.
#[derive(Debug)]
struct Connection {
    id: String,
    spec: SpawnSpec,
    timeout: Duration,
    /// The live session, or `None` when it must be (re)established on the next call.
    session: Mutex<Option<Session>>,
}

impl Connection {
    /// Send a request, (re)connecting first when there is no live session.
    async fn call(&self, method: &str, params: Value) -> Result<Value, CallError> {
        let mut session = self.session.lock().await;
        if session.is_none() {
            let connected = Session::connect(&self.spec, self.timeout)
                .await
                .map_err(CallError::Transport)?;
            *session = Some(connected);
        }
        let active = session.as_mut().expect("connected above");
        match active.request(method, params, self.timeout).await {
            Ok(value) => Ok(value),
            Err(RequestError::Transport(message)) => {
                // The pipe is broken; drop the session so the next call reconnects.
                *session = None;
                Err(CallError::Transport(message))
            }
            Err(RequestError::Protocol(message)) => Err(CallError::Protocol(message)),
        }
    }
}

/// One tool as declared by the server.
#[derive(Debug, Clone)]
struct McpToolSpec {
    name: String,
    description: String,
    parameters: Value,
    requires_approval: bool,
}

/// A tool source backed by an MCP server run as a subprocess.
///
/// Construct one with [`new`](McpStdioSource::new), which starts the server and reads its
/// tool list. The process stays alive until the source is dropped.
#[derive(Debug)]
pub struct McpStdioSource {
    connection: Arc<Connection>,
    breaker: Arc<CircuitBreaker>,
    tools: Vec<McpToolSpec>,
}

impl McpStdioSource {
    /// Start `command` and read its tool list.
    ///
    /// # Errors
    ///
    /// Returns a message when the server cannot be started, the handshake fails, or the
    /// server does not answer `tools/list` in time.
    pub async fn new(
        id: impl Into<String>,
        command: impl Into<String>,
        args: Vec<String>,
        timeout: Option<Duration>,
    ) -> Result<Self, String> {
        let spec = SpawnSpec {
            command: command.into(),
            args,
        };
        let timeout = timeout.unwrap_or(DEFAULT_TIMEOUT);
        let mut session = Session::connect(&spec, timeout).await?;
        let tools = session.list_tools(timeout).await?;

        let connection = Connection {
            id: id.into(),
            spec,
            timeout,
            session: Mutex::new(Some(session)),
        };
        Ok(McpStdioSource {
            connection: Arc::new(connection),
            breaker: Arc::new(CircuitBreaker::new(BreakerPolicy::default())),
            tools,
        })
    }

    /// Replace the default breaker policy (used by tests and by a future `doctor`).
    #[must_use]
    pub fn with_breaker_policy(mut self, policy: BreakerPolicy) -> Self {
        self.breaker = Arc::new(CircuitBreaker::new(policy));
        self
    }

    /// Build a source over arbitrary streams, for tests.
    #[cfg(test)]
    async fn over(
        id: &str,
        stdin: Box<dyn AsyncWrite + Unpin + Send>,
        stdout: Box<dyn AsyncRead + Unpin + Send>,
        timeout: Duration,
    ) -> Result<Self, String> {
        let mut session = Session::over(stdin, stdout);
        session.handshake(timeout).await?;
        let tools = session.list_tools(timeout).await?;
        let connection = Connection {
            id: id.to_string(),
            // Any reconnect attempt must fail: the tests never give a real program.
            spec: SpawnSpec {
                command: "rsagent-test-no-such-program".to_string(),
                args: Vec::new(),
            },
            timeout,
            session: Mutex::new(Some(session)),
        };
        Ok(McpStdioSource {
            connection: Arc::new(connection),
            breaker: Arc::new(CircuitBreaker::new(BreakerPolicy::default())),
            tools,
        })
    }

    /// The current health, derived from the circuit breaker.
    fn health(&self) -> SourceHealth {
        let now = Instant::now();
        match self.breaker.state(now) {
            BreakerState::Closed => SourceHealth::Healthy,
            BreakerState::HalfOpen => {
                SourceHealth::Unhealthy("circuit half-open; the next call probes it".to_string())
            }
            BreakerState::Open => {
                let wait = self.breaker.retry_after(now).map_or(0, |d| d.as_secs());
                SourceHealth::Unhealthy(format!("circuit open; retry in {wait}s"))
            }
        }
    }
}

impl ToolSource for McpStdioSource {
    fn kind(&self) -> SourceKind {
        SourceKind::McpStdio
    }

    fn id(&self) -> &str {
        &self.connection.id
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        self.tools
            .iter()
            .map(|spec| -> Box<dyn Tool> {
                Box::new(McpTool {
                    source_id: self.connection.id.clone(),
                    spec: spec.clone(),
                    connection: Arc::clone(&self.connection),
                    breaker: Arc::clone(&self.breaker),
                })
            })
            .collect()
    }

    fn health(&self) -> SourceHealth {
        self.health()
    }
}

/// A tool offered by an [`McpStdioSource`].
#[derive(Debug)]
struct McpTool {
    source_id: String,
    spec: McpToolSpec,
    connection: Arc<Connection>,
    breaker: Arc<CircuitBreaker>,
}

#[async_trait]
impl Tool for McpTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            self.spec.name.clone(),
            self.spec.description.clone(),
            self.spec.parameters.clone(),
        )
    }

    fn requires_approval(&self) -> bool {
        self.spec.requires_approval
    }

    /// MCP tools share one stdio pipe, so they never run concurrently with others.
    fn is_sequential(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _call_id: &str,
        args: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, String> {
        let now = Instant::now();
        if !self.breaker.allows(now) {
            let wait = self.breaker.retry_after(now).map_or(0, |d| d.as_secs());
            return Err(format!(
                "tool source `{}` is unavailable (circuit open); retry in {wait}s",
                self.source_id
            ));
        }

        let params = json!({ "name": self.spec.name, "arguments": args });
        match self.connection.call("tools/call", params).await {
            Ok(value) => {
                self.breaker.record_success();
                parse_call_result(&value)
            }
            Err(CallError::Transport(message)) => {
                self.breaker.record_failure(Instant::now());
                Err(message)
            }
            Err(CallError::Protocol(message)) => {
                // Reachable and speaking the protocol; it just refused this request.
                self.breaker.record_success();
                Err(message)
            }
        }
    }
}

/// Read the `tools` array of a `tools/list` result.
fn parse_tools(result: &Value) -> Result<Vec<McpToolSpec>, String> {
    let tools = result
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| "`tools/list` returned no `tools` array".to_string())?;
    tools.iter().map(parse_tool).collect()
}

/// Read one entry of a `tools/list` result.
fn parse_tool(value: &Value) -> Result<McpToolSpec, String> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "a listed tool is missing `name`".to_string())?
        .to_string();
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let parameters = value
        .get("inputSchema")
        .cloned()
        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    // Approval is the default: an arbitrary server may do anything. A tool that declares
    // itself read-only (`annotations.readOnlyHint`) opts out.
    let requires_approval = !value
        .get("annotations")
        .and_then(|annotations| annotations.get("readOnlyHint"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    Ok(McpToolSpec {
        name,
        description,
        parameters,
        requires_approval,
    })
}

/// Turn a `tools/call` result into a tool result.
fn parse_call_result(value: &Value) -> Result<ToolResult, String> {
    let text = render_content(value.get("content"));
    let is_error = value
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if is_error {
        return Err(if text.is_empty() {
            "the tool reported an error".to_string()
        } else {
            text
        });
    }
    let mut result = ToolResult::text(text);
    if let Some(details) = value.get("structuredContent") {
        result.details = Some(details.clone());
    }
    Ok(result)
}

/// Render an MCP `content` array as plain text.
fn render_content(content: Option<&Value>) -> String {
    let Some(blocks) = content.and_then(Value::as_array) else {
        return String::new();
    };
    blocks
        .iter()
        .map(render_block)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render one content block. Non-text blocks get a short placeholder, since a terminal
/// tool result is text.
fn render_block(block: &Value) -> String {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => block
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        Some(kind @ ("image" | "audio")) => {
            let mime = block
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("[{kind}: {mime}]")
        }
        Some("resource" | "resource_link") => {
            let uri = block
                .get("resource")
                .and_then(|resource| resource.get("uri"))
                .and_then(Value::as_str)
                .or_else(|| block.get("uri").and_then(Value::as_str))
                .unwrap_or("");
            format!("[resource: {uri}]")
        }
        Some(other) => format!("[{other}]"),
        None => block.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        parse_call_result, parse_tool, parse_tools, render_content, McpStdioSource, Session,
    };
    use crate::breaker::BreakerPolicy;
    use crate::source::{SourceHealth, SourceKind, ToolSource};
    use crate::tool::ToolContext;
    use serde_json::{json, Value};
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

    /// A scripted server: read request lines, answer each with `handler`.
    async fn serve(
        input: DuplexStream,
        mut output: DuplexStream,
        handler: impl Fn(&Value) -> Option<Value> + Send + 'static,
    ) {
        let mut reader = BufReader::new(input);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            let Some(response) = handler(&message) else {
                continue;
            };
            let mut text = serde_json::to_string(&response).expect("serialize");
            text.push('\n');
            if output.write_all(text.as_bytes()).await.is_err() {
                break;
            }
            let _ = output.flush().await;
        }
    }

    /// A JSON-RPC response echoing the request's id.
    fn respond(message: &Value, result: Value) -> Option<Value> {
        let id = message.get("id")?.clone();
        let mut object = serde_json::Map::new();
        object.insert("jsonrpc".to_string(), json!("2.0"));
        object.insert("id".to_string(), id);
        object.insert("result".to_string(), result);
        Some(Value::Object(object))
    }

    /// A JSON-RPC error echoing the request's id.
    fn respond_error(message: &Value, code: i64, text: &str) -> Option<Value> {
        message.get("id").map(
            |id| json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": text } }),
        )
    }

    fn tool_json(name: &str) -> Value {
        json!({
            "name": name,
            "description": "A test tool.",
            "inputSchema": { "type": "object", "properties": {} },
        })
    }

    /// Connect a source to a scripted server.
    async fn source_over(
        handler: impl Fn(&Value) -> Option<Value> + Send + 'static,
    ) -> McpStdioSource {
        let (client_write, server_read) = tokio::io::duplex(4096);
        let (server_write, client_read) = tokio::io::duplex(4096);
        tokio::spawn(serve(server_read, server_write, handler));
        McpStdioSource::over(
            "test",
            Box::new(client_write),
            Box::new(client_read),
            Duration::from_secs(5),
        )
        .await
        .expect("the scripted server must handshake")
    }

    #[tokio::test]
    async fn initializes_lists_and_calls_a_tool() {
        let source = source_over(
            |message| match message.get("method").and_then(Value::as_str) {
                Some("initialize") => respond(
                    message,
                    json!({ "protocolVersion": "2025-06-18", "capabilities": {} }),
                ),
                Some("tools/list") => respond(message, json!({ "tools": [tool_json("echo")] })),
                Some("tools/call") => respond(
                    message,
                    json!({ "content": [{ "type": "text", "text": "pong" }] }),
                ),
                _ => None,
            },
        )
        .await;

        assert_eq!(source.kind(), SourceKind::McpStdio);
        assert_eq!(source.id(), "test");
        assert_eq!(source.health(), SourceHealth::Healthy);

        let tools = source.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].spec().name, "echo");
        assert!(tools[0].requires_approval(), "no read-only hint opts out");
        assert!(tools[0].is_sequential(), "MCP tools share one pipe");

        let context = ToolContext::new(".");
        let result = tools[0]
            .execute("call-1", json!({}), &context)
            .await
            .expect("the call must succeed");
        assert_eq!(result.content[0].as_text(), Some("pong"));
        assert_eq!(source.health(), SourceHealth::Healthy);
    }

    #[tokio::test]
    async fn a_read_only_annotation_opts_out_of_approval() {
        let source = source_over(
            |message| match message.get("method").and_then(Value::as_str) {
                Some("initialize") => respond(message, json!({ "protocolVersion": "2025-06-18" })),
                Some("tools/list") => respond(
                    message,
                    json!({ "tools": [{
                    "name": "peek",
                    "inputSchema": { "type": "object", "properties": {} },
                    "annotations": { "readOnlyHint": true },
                }] }),
                ),
                _ => None,
            },
        )
        .await;

        assert!(!source.tools()[0].requires_approval());
    }

    #[tokio::test]
    async fn follows_pagination_cursors() {
        let source = source_over(
            |message| match message.get("method").and_then(Value::as_str) {
                Some("initialize") => respond(message, json!({ "protocolVersion": "2025-06-18" })),
                Some("tools/list") => {
                    let cursor = message
                        .get("params")
                        .and_then(|params| params.get("cursor"))
                        .and_then(Value::as_str);
                    if cursor.is_none() {
                        respond(
                            message,
                            json!({ "tools": [tool_json("one")], "nextCursor": "page-2" }),
                        )
                    } else {
                        respond(message, json!({ "tools": [tool_json("two")] }))
                    }
                }
                _ => None,
            },
        )
        .await;

        let names: Vec<String> = source.tools().iter().map(|t| t.spec().name).collect();
        assert_eq!(names, vec!["one", "two"]);
    }

    #[tokio::test]
    async fn a_tool_is_error_result_is_a_business_failure() {
        // `isError: true` means the tool ran and failed: the source is working, so the
        // breaker must stay closed (decision D4).
        let source = source_over(
            |message| match message.get("method").and_then(Value::as_str) {
                Some("initialize") => respond(message, json!({ "protocolVersion": "2025-06-18" })),
                Some("tools/list") => respond(message, json!({ "tools": [tool_json("echo")] })),
                Some("tools/call") => respond(
                    message,
                    json!({ "content": [{ "type": "text", "text": "boom" }], "isError": true }),
                ),
                _ => None,
            },
        )
        .await;

        let tools = source.tools();
        let context = ToolContext::new(".");
        for _ in 0..3 {
            let error = tools[0]
                .execute("call", json!({}), &context)
                .await
                .expect_err("the tool reported an error");
            assert_eq!(error, "boom");
        }
        assert_eq!(source.health(), SourceHealth::Healthy);
    }

    #[tokio::test]
    async fn a_json_rpc_error_does_not_trip_the_breaker() {
        // A well-formed JSON-RPC error means the server is reachable; it refused the call.
        let source = source_over(
            |message| match message.get("method").and_then(Value::as_str) {
                Some("initialize") => respond(message, json!({ "protocolVersion": "2025-06-18" })),
                Some("tools/list") => respond(message, json!({ "tools": [tool_json("echo")] })),
                Some("tools/call") => respond_error(message, -32602, "invalid params"),
                _ => None,
            },
        )
        .await;

        let tools = source.tools();
        let context = ToolContext::new(".");
        let error = tools[0]
            .execute("call", json!({}), &context)
            .await
            .expect_err("the server refused the call");
        assert!(error.contains("invalid params"), "{error}");
        assert_eq!(source.health(), SourceHealth::Healthy);
    }

    #[tokio::test]
    async fn a_broken_transport_opens_the_breaker() {
        // A server that answers the handshake and tool list, then closes its stdout.
        async fn closing_server(mut input: DuplexStream, mut output: DuplexStream) {
            let mut reader = BufReader::new(&mut input);
            let mut line = String::new();
            let mut answered = 0;
            while answered < 2 {
                line.clear();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                    continue;
                };
                let Some(method) = message.get("method").and_then(Value::as_str) else {
                    continue; // the `initialized` notification
                };
                let result = match method {
                    "initialize" => json!({ "protocolVersion": "2025-06-18" }),
                    "tools/list" => json!({ "tools": [tool_json("echo")] }),
                    _ => continue,
                };
                let response = json!({ "jsonrpc": "2.0", "id": message["id"], "result": result });
                let mut text = serde_json::to_string(&response).expect("serialize");
                text.push('\n');
                if output.write_all(text.as_bytes()).await.is_err() {
                    return;
                }
                let _ = output.flush().await;
                answered += 1;
            }
            // Returning drops both pipes: the connection is now closed.
        }

        let (client_write, server_read) = tokio::io::duplex(4096);
        let (server_write, client_read) = tokio::io::duplex(4096);
        tokio::spawn(closing_server(server_read, server_write));

        let source = McpStdioSource::over(
            "flaky",
            Box::new(client_write),
            Box::new(client_read),
            Duration::from_secs(5),
        )
        .await
        .expect("the handshake must succeed")
        .with_breaker_policy(BreakerPolicy {
            threshold: 1,
            base_cooldown: Duration::from_secs(60),
            max_cooldown: Duration::from_secs(600),
        });

        let tools = source.tools();
        let context = ToolContext::new(".");
        // The first call finds the closed pipe: a transport failure trips the breaker.
        // Depending on timing the write or the read fails first, so accept either.
        let error = tools[0]
            .execute("call", json!({}), &context)
            .await
            .expect_err("the pipe is closed");
        assert!(
            error.contains("closed the connection") || error.contains("broken pipe"),
            "expected a transport failure, got: {error}"
        );

        // The next call is refused before touching the transport at all.
        let error = tools[0]
            .execute("call", json!({}), &context)
            .await
            .expect_err("an open breaker refuses the call");
        assert!(error.contains("circuit open"), "{error}");

        match source.health() {
            SourceHealth::Unhealthy(reason) => assert!(reason.contains("circuit open"), "{reason}"),
            other => panic!("expected an unhealthy source, got {other:?}"),
        }
    }

    #[test]
    fn a_listed_tool_defaults_its_schema_and_approval() {
        let spec = parse_tool(&json!({ "name": "bare" })).expect("parse");
        assert_eq!(spec.name, "bare");
        assert_eq!(
            spec.parameters,
            json!({ "type": "object", "properties": {} })
        );
        assert!(spec.requires_approval, "approval is the default");
    }

    #[test]
    fn a_listed_tool_without_a_name_is_refused() {
        assert!(parse_tool(&json!({ "description": "anonymous" })).is_err());
        assert!(parse_tools(&json!({ "noTools": [] })).is_err());
    }

    #[test]
    fn a_call_error_result_becomes_err() {
        let error = parse_call_result(&json!({
            "content": [{ "type": "text", "text": "nope" }],
            "isError": true
        }))
        .expect_err("isError becomes Err");
        assert_eq!(error, "nope");
    }

    #[test]
    fn a_call_success_renders_text_and_keeps_details() {
        let result = parse_call_result(&json!({
            "content": [{ "type": "text", "text": "ok" }],
            "structuredContent": { "n": 1 }
        }))
        .expect("parse");
        assert_eq!(result.content[0].as_text(), Some("ok"));
        assert_eq!(result.details, Some(json!({ "n": 1 })));
    }

    #[test]
    fn non_text_blocks_get_a_placeholder() {
        let text = render_content(Some(&json!([
            { "type": "text", "text": "look:" },
            { "type": "image", "mimeType": "image/png" },
            { "type": "resource", "resource": { "uri": "file:///x" } }
        ])));
        assert_eq!(text, "look:\n[image: image/png]\n[resource: file:///x]");
    }

    #[test]
    fn a_session_with_no_stderr_reports_none() {
        let (client_write, _server_read) = tokio::io::duplex(64);
        let (_server_write, client_read) = tokio::io::duplex(64);
        let session = Session::over(Box::new(client_write), Box::new(client_read));
        assert_eq!(session.stderr_tail(), "");
    }
}
