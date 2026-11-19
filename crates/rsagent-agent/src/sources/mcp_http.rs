//! A source backed by a [Model Context Protocol] server, spoken over HTTP.
//!
//! This implements the MCP *Streamable HTTP* transport: the client POSTs JSON-RPC
//! messages to a single endpoint URL, and the server answers either with one JSON object
//! (`Content-Type: application/json`) or with a Server-Sent Events stream
//! (`text/event-stream`) whose `data:` payloads are the messages. Either encoding is
//! accepted. The subset implemented is what rsagent needs — `initialize`,
//! `notifications/initialized`, `tools/list`, and `tools/call` — against protocol revision
//! [`super::mcp::MCP_PROTOCOL_VERSION`].
//!
//! [Model Context Protocol]: https://modelcontextprotocol.io
//!
//! # Session
//!
//! The server may answer `initialize` with an `Mcp-Session-Id` response header. When it
//! does, every later request echoes it back, together with an `MCP-Protocol-Version`
//! header. When it does not, no session id is sent and each request stands alone.
//!
//! # Failure classes
//!
//! The same split as [`mcp-stdio`](super::mcp_stdio): a **transport** failure — the
//! endpoint is unreachable, a request times out, the status is not a success and the body
//! carries no JSON-RPC error, or the body does not parse — is connection-class and trips
//! the circuit breaker. A well-formed JSON-RPC `error`, or a `tools/call` result with
//! `"isError": true`, is a protocol/business failure that leaves the breaker closed.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rsagent_ai::ToolSpec;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::approval::Permission;
use crate::breaker::{BreakerPolicy, BreakerReport, BreakerState, CircuitBreaker};
use crate::source::{SourceHealth, SourceKind, ToolSource};
use crate::tool::{Tool, ToolContext, ToolResult};

use super::mcp::{
    initialize_params, parse_call_result, parse_tools, CallError, McpToolSpec, MCP_PROTOCOL_VERSION,
};

/// Per-request timeout when the configuration does not set one.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// The `Accept` value MCP requires: the client understands both encodings.
const ACCEPT: &str = "application/json, text/event-stream";

/// How to reach the server.
#[derive(Debug, Clone)]
struct Endpoint {
    url: String,
    /// Extra request headers (an `Authorization` token, a tenant id, ...).
    headers: Vec<(String, String)>,
    timeout: Duration,
}

/// The session the server handed us, if any.
#[derive(Debug, Clone)]
struct SessionState {
    /// The `Mcp-Session-Id` the server issued on `initialize`.
    session_id: Option<String>,
}

/// The shared connection: one HTTP client, one session, one id counter.
#[derive(Debug)]
struct Connection {
    id: String,
    endpoint: Endpoint,
    client: reqwest::Client,
    /// The live session, or `None` when the endpoint must be (re)initialized.
    session: Mutex<Option<SessionState>>,
    /// The next JSON-RPC request id.
    next_id: AtomicI64,
}

/// One parsed response: the JSON-RPC result plus any session id the server issued.
struct Exchange {
    result: Value,
    session_id: Option<String>,
}

impl Connection {
    /// Send a request, initializing the session first when there is none.
    async fn call(&self, method: &str, params: Value) -> Result<Value, CallError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let session_id = self.ensure_session().await?;
        match self.exchange(&message, session_id.as_deref()).await {
            Ok(exchange) => {
                if let Some(issued) = exchange.session_id {
                    self.set_session_id(issued).await;
                }
                Ok(exchange.result)
            }
            Err(CallError::Transport(message)) => {
                // The endpoint is unreachable; re-initialize on the next call.
                *self.session.lock().await = None;
                Err(CallError::Transport(message))
            }
            Err(error) => Err(error),
        }
    }

    /// The current session id, initializing the session on first use.
    async fn ensure_session(&self) -> Result<Option<String>, CallError> {
        let mut session = self.session.lock().await;
        if session.is_none() {
            *session = Some(self.initialize().await?);
        }
        Ok(session.as_ref().and_then(|state| state.session_id.clone()))
    }

    /// Run the `initialize` handshake and remember the session it establishes.
    async fn initialize(&self) -> Result<SessionState, CallError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": initialize_params(),
        });
        let exchange = self.exchange(&message, None).await?;
        let state = SessionState {
            session_id: exchange.session_id,
        };
        // The client must confirm initialization before making other requests.
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {},
        });
        self.exchange(&notification, state.session_id.as_deref())
            .await?;
        Ok(state)
    }

    /// Ask for the server's tools, following pagination cursors.
    async fn list_tools(&self) -> Result<Vec<McpToolSpec>, CallError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = cursor
                .as_deref()
                .map_or_else(|| json!({}), |cursor| json!({ "cursor": cursor }));
            let result = self.call("tools/list", params).await?;
            tools.extend(parse_tools(&result).map_err(CallError::Transport)?);
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

    /// Replace the remembered session id.
    async fn set_session_id(&self, session_id: String) {
        if let Some(state) = self.session.lock().await.as_mut() {
            state.session_id = Some(session_id);
        }
    }

    /// POST one JSON-RPC message and read its response.
    async fn exchange(
        &self,
        message: &Value,
        session_id: Option<&str>,
    ) -> Result<Exchange, CallError> {
        let timeout = self.endpoint.timeout;
        let expected_id = message.get("id").and_then(Value::as_i64);

        let mut request = self
            .client
            .post(&self.endpoint.url)
            .header("Accept", ACCEPT)
            .header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION)
            .json(message);
        if let Some(session_id) = session_id {
            request = request.header("Mcp-Session-Id", session_id);
        }
        for (name, value) in &self.endpoint.headers {
            request = request.header(name, value);
        }

        let response = tokio::time::timeout(timeout, request.send())
            .await
            .map_err(|_| {
                CallError::Transport(format!(
                    "the request timed out after {}s",
                    timeout.as_secs()
                ))
            })?
            .map_err(|error| {
                CallError::Transport(format!("cannot reach the MCP endpoint: {error}"))
            })?;

        let status = response.status();
        let session_id = response
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let content_type = response
            .headers()
            .get("Content-Type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();

        let body = tokio::time::timeout(timeout, response.text())
            .await
            .map_err(|_| {
                CallError::Transport(format!(
                    "reading the response timed out after {}s",
                    timeout.as_secs()
                ))
            })?
            .map_err(|error| CallError::Transport(format!("cannot read the response: {error}")))?;

        // An empty body is a notification acknowledgement (typically `202 Accepted`).
        if body.trim().is_empty() {
            if status.is_success() {
                return Ok(Exchange {
                    result: Value::Null,
                    session_id,
                });
            }
            return Err(CallError::Transport(format!(
                "the MCP endpoint returned HTTP {status}"
            )));
        }

        let message = if content_type.contains("text/event-stream") {
            find_event_response(&body, expected_id)?
        } else {
            serde_json::from_str::<Value>(&body).map_err(|error| {
                CallError::Transport(format!("the MCP endpoint sent invalid JSON: {error}"))
            })?
        };

        Ok(Exchange {
            result: read_result(&message, expected_id)?,
            session_id,
        })
    }
}

/// Pull the JSON-RPC result out of a response, mapping a JSON-RPC error to
/// [`CallError::Protocol`].
fn read_result(message: &Value, expected_id: Option<i64>) -> Result<Value, CallError> {
    if let Some(error) = message.get("error") {
        let code = error
            .get("code")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let text = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(CallError::Protocol(format!("JSON-RPC {code}: {text}")));
    }
    if let Some(expected) = expected_id {
        if message.get("id").and_then(Value::as_i64) != Some(expected) {
            return Err(CallError::Transport(format!(
                "the MCP endpoint answered a different request (expected id {expected})"
            )));
        }
    }
    Ok(message.get("result").cloned().unwrap_or(Value::Null))
}

/// Find the JSON-RPC message with `id` among an SSE body's `data:` payloads.
fn find_event_response(body: &str, id: Option<i64>) -> Result<Value, CallError> {
    for payload in event_payloads(body) {
        let Ok(message) = serde_json::from_str::<Value>(&payload) else {
            continue;
        };
        if let Some(expected) = id {
            if message.get("id").and_then(Value::as_i64) != Some(expected) {
                continue; // a notification or a response to another request
            }
        }
        return Ok(message);
    }
    Err(CallError::Transport(
        "the MCP endpoint's event stream carried no matching response".to_string(),
    ))
}

/// The `data:` payloads of an SSE body, in order, one per event.
fn event_payloads(body: &str) -> Vec<String> {
    let mut payloads = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in body.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            if !current.is_empty() {
                payloads.push(current.join("\n"));
                current.clear();
            }
            continue;
        }
        // Only `data:` carries the message; `event:`, `id:`, and comments are ignored.
        if let Some(data) = line.strip_prefix("data:") {
            current.push(data.strip_prefix(' ').unwrap_or(data));
        }
    }
    if !current.is_empty() {
        payloads.push(current.join("\n"));
    }
    payloads
}

/// A tool source backed by an MCP server reached over HTTP.
///
/// Construct one with [`new`](McpHttpSource::new), which runs the handshake and reads the
/// server's tool list. The endpoint is re-initialized transparently after a transport
/// failure.
#[derive(Debug)]
pub struct McpHttpSource {
    connection: Arc<Connection>,
    breaker: Arc<CircuitBreaker>,
    tools: Vec<McpToolSpec>,
}

impl McpHttpSource {
    /// Connect to `url` and read its tool list.
    ///
    /// `headers` are added to every request (for example `("Authorization", "Bearer ...")`).
    ///
    /// # Errors
    ///
    /// Returns a message when the HTTP client cannot be built, the endpoint is unreachable,
    /// the handshake fails, or the server does not answer `tools/list` in time.
    pub async fn new(
        id: impl Into<String>,
        url: impl Into<String>,
        headers: Vec<(String, String)>,
        timeout: Option<Duration>,
    ) -> Result<Self, String> {
        let endpoint = Endpoint {
            url: url.into(),
            headers,
            timeout: timeout.unwrap_or(DEFAULT_TIMEOUT),
        };
        let client = reqwest::Client::builder()
            .build()
            .map_err(|error| format!("cannot build the HTTP client: {error}"))?;
        let connection = Arc::new(Connection {
            id: id.into(),
            endpoint,
            client,
            session: Mutex::new(None),
            next_id: AtomicI64::new(1),
        });

        // Fail at load time on a bad endpoint, then read the tool list.
        let state = connection.initialize().await.map_err(call_message)?;
        *connection.session.lock().await = Some(state);
        let tools = connection.list_tools().await.map_err(call_message)?;

        Ok(McpHttpSource {
            connection,
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

/// Flatten a [`CallError`] into its message.
fn call_message(error: CallError) -> String {
    match error {
        CallError::Transport(message) | CallError::Protocol(message) => message,
    }
}

impl ToolSource for McpHttpSource {
    fn kind(&self) -> SourceKind {
        SourceKind::McpHttp
    }

    fn id(&self) -> &str {
        &self.connection.id
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        self.tools
            .iter()
            .map(|spec| -> Box<dyn Tool> {
                Box::new(McpHttpTool {
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

    fn breaker_report(&self) -> Option<BreakerReport> {
        Some(self.breaker.report())
    }

    fn restore_breaker(&self, report: &BreakerReport) {
        self.breaker.restore(report);
    }
}

/// A tool offered by an [`McpHttpSource`].
///
/// Unlike `mcp-stdio`, HTTP requests are independent, so these tools are *not*
/// sequential: the agent may run several at once.
#[derive(Debug)]
struct McpHttpTool {
    source_id: String,
    spec: McpToolSpec,
    connection: Arc<Connection>,
    breaker: Arc<CircuitBreaker>,
}

#[async_trait]
impl Tool for McpHttpTool {
    fn spec(&self) -> ToolSpec {
        self.spec.tool_spec()
    }

    fn requires_approval(&self) -> bool {
        self.spec.requires_approval
    }

    /// An MCP HTTP tool makes a network request, so it is a network call.
    fn permission(&self) -> Permission {
        Permission::Network
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

#[cfg(test)]
mod tests {
    use super::{event_payloads, McpHttpSource};
    use crate::breaker::BreakerPolicy;
    use crate::source::{SourceHealth, SourceKind, ToolSource};
    use crate::tool::ToolContext;
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};

    /// One HTTP request, as the mock server sees it.
    struct Request {
        headers: Vec<(String, String)>,
        body: String,
    }

    impl Request {
        /// The value of a header, case-insensitively.
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        }

        /// The JSON-RPC `method` of the request body.
        fn method(&self) -> Option<String> {
            let message: Value = serde_json::from_str(&self.body).ok()?;
            message
                .get("method")
                .and_then(Value::as_str)
                .map(str::to_string)
        }

        /// A JSON-RPC response echoing the request's id.
        fn reply(&self, result: Value) -> Reply {
            let message: Value = serde_json::from_str(&self.body).expect("request body");
            let id = message.get("id").cloned().unwrap_or(Value::Null);
            let mut object = serde_json::Map::new();
            object.insert("jsonrpc".to_string(), Value::from("2.0"));
            object.insert("id".to_string(), id);
            object.insert("result".to_string(), result);
            Reply::json(Value::Object(object).to_string())
        }

        /// A JSON-RPC error echoing the request's id.
        fn reply_error(&self, code: i64, text: &str) -> Reply {
            let message: Value = serde_json::from_str(&self.body).expect("request body");
            let id = message.get("id").cloned().unwrap_or(Value::Null);
            Reply::json(
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": text } })
                    .to_string(),
            )
        }
    }

    /// A canned HTTP response.
    struct Reply {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: String,
    }

    impl Reply {
        fn json(body: String) -> Self {
            Reply {
                status: 200,
                headers: vec![("Content-Type", "application/json".to_string())],
                body,
            }
        }

        fn sse(body: String) -> Self {
            Reply {
                status: 200,
                headers: vec![("Content-Type", "text/event-stream".to_string())],
                body,
            }
        }

        fn status(mut self, status: u16) -> Self {
            self.status = status;
            self
        }

        fn with_header(mut self, name: &'static str, value: &str) -> Self {
            self.headers.push((name, value.to_string()));
            self
        }
    }

    /// A notification acknowledgement: `202 Accepted`, no body.
    fn accepted() -> Reply {
        Reply {
            status: 202,
            headers: Vec::new(),
            body: String::new(),
        }
    }

    fn tool_json(name: &str) -> Value {
        json!({
            "name": name,
            "description": "A test tool.",
            "inputSchema": { "type": "object", "properties": {} },
        })
    }

    /// Start a one-request-per-connection HTTP server; return its endpoint URL.
    async fn serve(handler: impl Fn(&Request) -> Reply + Send + 'static) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                if let Some(request) = read_request(&mut stream).await {
                    let reply = handler(&request);
                    let _ = write_reply(&mut stream, &reply).await;
                }
            }
        });
        format!("http://{addr}/mcp")
    }

    /// Read one HTTP/1.1 request (headers plus a `Content-Length` body).
    async fn read_request(stream: &mut TcpStream) -> Option<Request> {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        if reader.read_line(&mut line).await.ok()? == 0 {
            return None;
        }
        let mut headers = Vec::new();
        let mut content_length = 0_usize;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).await.ok()? == 0 {
                break;
            }
            let header = header.trim_end_matches(['\r', '\n']);
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':') {
                let name = name.trim();
                let value = value.trim();
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = value.parse().unwrap_or(0);
                }
                headers.push((name.to_string(), value.to_string()));
            }
        }
        let mut body = vec![0_u8; content_length];
        if content_length > 0 {
            reader.read_exact(&mut body).await.ok()?;
        }
        Some(Request {
            headers,
            body: String::from_utf8_lossy(&body).to_string(),
        })
    }

    /// Write a response and close the connection.
    async fn write_reply(stream: &mut TcpStream, reply: &Reply) -> std::io::Result<()> {
        let mut head = format!(
            "HTTP/1.1 {} OK\r\nContent-Length: {}\r\nConnection: close\r\n",
            reply.status,
            reply.body.len()
        );
        for (name, value) in &reply.headers {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(value);
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(reply.body.as_bytes()).await?;
        stream.flush().await
    }

    /// A source over a server that answers the standard handshake and one tool.
    async fn source_over(handler: impl Fn(&Request) -> Reply + Send + 'static) -> McpHttpSource {
        let url = serve(handler).await;
        McpHttpSource::new("test", url, Vec::new(), Some(Duration::from_secs(5)))
            .await
            .expect("the endpoint must handshake and list its tools")
    }

    #[tokio::test]
    async fn initializes_lists_and_calls_a_tool() {
        let source = source_over(|request| match request.method().as_deref() {
            Some("initialize") => {
                request.reply(json!({ "protocolVersion": "2025-06-18", "capabilities": {} }))
            }
            Some("notifications/initialized") => accepted(),
            Some("tools/list") => request.reply(json!({ "tools": [tool_json("echo")] })),
            Some("tools/call") => {
                request.reply(json!({ "content": [{ "type": "text", "text": "pong" }] }))
            }
            _ => Reply::json("{}".to_string()).status(404),
        })
        .await;

        assert_eq!(source.kind(), SourceKind::McpHttp);
        assert_eq!(source.id(), "test");
        assert_eq!(source.health(), SourceHealth::Healthy);

        let tools = source.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].spec().name, "echo");
        assert!(tools[0].requires_approval(), "no read-only hint opts out");
        assert!(!tools[0].is_sequential(), "HTTP calls may run concurrently");

        let context = ToolContext::new(".");
        let result = tools[0]
            .execute("call-1", json!({}), &context)
            .await
            .expect("the call must succeed");
        assert_eq!(result.content[0].as_text(), Some("pong"));
        assert_eq!(source.health(), SourceHealth::Healthy);
    }

    #[tokio::test]
    async fn an_sse_response_is_understood() {
        let source = source_over(|request| match request.method().as_deref() {
            Some("initialize") => request.reply(json!({ "protocolVersion": "2025-06-18" })),
            Some("notifications/initialized") => accepted(),
            Some("tools/list") => {
                // The server streams its reply as SSE, as the spec permits.
                let message: Value = serde_json::from_str(&request.body).expect("body");
                let id = message.get("id").cloned().unwrap_or(Value::Null);
                let payload = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "tools": [tool_json("echo")] },
                });
                Reply::sse(format!("event: message\ndata: {payload}\n\n"))
            }
            _ => Reply::json("{}".to_string()).status(404),
        })
        .await;

        assert_eq!(source.tools().len(), 1);
        assert_eq!(source.tools()[0].spec().name, "echo");
    }

    #[tokio::test]
    async fn the_session_id_is_echoed_back() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        let source = source_over(move |request| {
            let session = request.header("Mcp-Session-Id").map(str::to_string);
            recorder
                .lock()
                .expect("lock")
                .push((request.method(), session));
            match request.method().as_deref() {
                Some("initialize") => request
                    .reply(json!({ "protocolVersion": "2025-06-18" }))
                    .with_header("Mcp-Session-Id", "session-42"),
                Some("notifications/initialized") => accepted(),
                Some("tools/list") => request.reply(json!({ "tools": [tool_json("echo")] })),
                _ => Reply::json("{}".to_string()).status(404),
            }
        })
        .await;
        assert_eq!(source.tools().len(), 1);

        let calls = seen.lock().expect("lock").clone();
        // `initialize` carries no session; everything after it echoes the issued id.
        let initialize = calls
            .iter()
            .find(|(method, _)| method.as_deref() == Some("initialize"))
            .expect("initialize was sent");
        assert_eq!(initialize.1, None);
        let listed = calls
            .iter()
            .find(|(method, _)| method.as_deref() == Some("tools/list"))
            .expect("tools/list was sent");
        assert_eq!(listed.1.as_deref(), Some("session-42"));
    }

    #[tokio::test]
    async fn extra_headers_are_sent_on_every_request() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        let url = serve(move |request| {
            recorder
                .lock()
                .expect("lock")
                .push(request.header("Authorization").map(str::to_string));
            match request.method().as_deref() {
                Some("initialize") => request.reply(json!({ "protocolVersion": "2025-06-18" })),
                Some("notifications/initialized") => accepted(),
                Some("tools/list") => request.reply(json!({ "tools": [] })),
                _ => Reply::json("{}".to_string()).status(404),
            }
        })
        .await;
        McpHttpSource::new(
            "auth",
            url,
            vec![("Authorization".to_string(), "Bearer token".to_string())],
            Some(Duration::from_secs(5)),
        )
        .await
        .expect("handshake");

        for seen in seen.lock().expect("lock").iter() {
            assert_eq!(seen.as_deref(), Some("Bearer token"));
        }
    }

    #[tokio::test]
    async fn follows_pagination_cursors() {
        let source = source_over(|request| match request.method().as_deref() {
            Some("initialize") => request.reply(json!({ "protocolVersion": "2025-06-18" })),
            Some("notifications/initialized") => accepted(),
            Some("tools/list") => {
                let message: Value = serde_json::from_str(&request.body).expect("body");
                let cursor = message
                    .get("params")
                    .and_then(|params| params.get("cursor"))
                    .and_then(Value::as_str);
                if cursor.is_none() {
                    request.reply(json!({ "tools": [tool_json("one")], "nextCursor": "page-2" }))
                } else {
                    request.reply(json!({ "tools": [tool_json("two")] }))
                }
            }
            _ => Reply::json("{}".to_string()).status(404),
        })
        .await;

        let names: Vec<String> = source.tools().iter().map(|t| t.spec().name).collect();
        assert_eq!(names, vec!["one", "two"]);
    }

    #[tokio::test]
    async fn a_tool_is_error_result_is_a_business_failure() {
        let source = source_over(|request| match request.method().as_deref() {
            Some("initialize") => request.reply(json!({ "protocolVersion": "2025-06-18" })),
            Some("notifications/initialized") => accepted(),
            Some("tools/list") => request.reply(json!({ "tools": [tool_json("echo")] })),
            Some("tools/call") => request
                .reply(json!({ "content": [{ "type": "text", "text": "boom" }], "isError": true })),
            _ => Reply::json("{}".to_string()).status(404),
        })
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
        let source = source_over(|request| match request.method().as_deref() {
            Some("initialize") => request.reply(json!({ "protocolVersion": "2025-06-18" })),
            Some("notifications/initialized") => accepted(),
            Some("tools/list") => request.reply(json!({ "tools": [tool_json("echo")] })),
            Some("tools/call") => request.reply_error(-32602, "invalid params"),
            _ => Reply::json("{}".to_string()).status(404),
        })
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
    async fn an_unreachable_endpoint_opens_the_breaker() {
        // Port 1 on loopback is never listening: the connection is refused at once.
        let source = McpHttpSource::new(
            "dead",
            "http://127.0.0.1:1/mcp",
            Vec::new(),
            Some(Duration::from_secs(5)),
        )
        .await
        .expect_err("an unreachable endpoint must fail to load");
        assert!(source.contains("cannot reach the MCP endpoint"), "{source}");
    }

    #[tokio::test]
    async fn a_transport_failure_after_loading_opens_the_breaker() {
        // Answer the handshake and the tool list, then refuse everything else by
        // returning a non-success status with no JSON-RPC error.
        let source = source_over(|request| match request.method().as_deref() {
            Some("initialize") => request.reply(json!({ "protocolVersion": "2025-06-18" })),
            Some("notifications/initialized") => accepted(),
            Some("tools/list") => request.reply(json!({ "tools": [tool_json("echo")] })),
            _ => Reply::json(String::new()).status(500),
        })
        .await
        .with_breaker_policy(BreakerPolicy {
            threshold: 1,
            base_cooldown: Duration::from_secs(60),
            max_cooldown: Duration::from_secs(600),
        });

        let tools = source.tools();
        let context = ToolContext::new(".");
        let error = tools[0]
            .execute("call", json!({}), &context)
            .await
            .expect_err("a 500 with no JSON-RPC error is a transport failure");
        assert!(error.contains("HTTP 500"), "{error}");

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
    fn event_payloads_split_events_and_strip_the_data_prefix() {
        let payloads = event_payloads("event: message\ndata: {\"a\":1}\n\ndata: two\n\n");
        assert_eq!(payloads, vec!["{\"a\":1}".to_string(), "two".to_string()]);
    }

    #[tokio::test]
    async fn an_mcp_http_tool_is_a_network_call() {
        // An MCP HTTP tool makes a network request, so it is `network`, never `exec`.
        use crate::approval::Permission;

        let source = source_over(|request| match request.method().as_deref() {
            Some("initialize") => request.reply(json!({ "protocolVersion": "2025-06-18" })),
            Some("tools/list") => request.reply(json!({ "tools": [tool_json("echo")] })),
            _ => accepted(),
        })
        .await;

        assert_eq!(source.tools()[0].permission(), Permission::Network);
    }
}
