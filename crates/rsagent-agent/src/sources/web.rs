//! A source backed by a single HTTP endpoint.
//!
//! Where `mcp-http` speaks a protocol, `web` wraps *one* plain endpoint as *one* tool: the
//! model's arguments become the request, and the response body becomes the result. It is
//! the lightest way to reach an existing HTTP API — no server changes, just a URL.
//!
//! # The request
//!
//! - `GET` (the default) and `HEAD` send the arguments as **query parameters**: each
//!   key becomes a parameter, a string is used verbatim, anything else is rendered as
//!   JSON, and a `null` is dropped (so an omitted optional field stays omitted).
//! - Every other method sends the arguments object as the **JSON request body**.
//!
//! `headers` are added to every request, which is how you supply an `Authorization`
//! token.
//!
//! # Failure classes
//!
//! A **transport** failure — the endpoint is unreachable, a request times out — is
//! connection-class and trips the circuit breaker, exactly as for the MCP sources. An
//! HTTP response is *not*: once the server has answered, it is reachable, so a `4xx` or
//! `5xx` is a **business** failure that leaves the breaker closed and is reported to the
//! model as an error. This is deliberate — a `400` caused by bad arguments must not take
//! the whole source out of the tool list.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rsagent_ai::ToolSpec;
use serde_json::{json, Value};

use crate::approval::Permission;
use crate::breaker::{BreakerPolicy, BreakerReport, BreakerState, CircuitBreaker};
use crate::source::{SourceHealth, SourceKind, ToolSource};
use crate::tool::{Tool, ToolContext, ToolResult};

use super::http::{read_body, MAX_BODY_BYTES};

/// Per-request timeout when the configuration does not set one.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// The method used when the configuration does not set one.
const DEFAULT_METHOD: &str = "GET";

/// How to turn one HTTP endpoint into a tool.
///
/// Built by the CLI from a `[[tools.sources]]` entry. Start from [`new`](WebSourceConfig::new)
/// and override the fields you need.
#[derive(Debug, Clone)]
pub struct WebSourceConfig {
    /// The endpoint URL.
    pub url: String,
    /// The tool name the model sees (before any source prefix). Defaults to the source id.
    pub name: String,
    /// A one-line description for the model.
    pub description: String,
    /// The JSON Schema the model must satisfy. Defaults to an empty object.
    pub parameters: Value,
    /// The HTTP method. `GET`/`HEAD` use query parameters; others use a JSON body.
    pub method: String,
    /// Extra request headers (an `Authorization` token, ...).
    pub headers: Vec<(String, String)>,
    /// Whether a call needs the user's approval first. Defaults to `true`.
    pub requires_approval: bool,
}

impl WebSourceConfig {
    /// A config for `url`: `GET`, no headers, approval required, empty parameter schema.
    ///
    /// `name` is left empty; [`WebSource::new`] fills it with the source id.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        WebSourceConfig {
            url: url.into(),
            name: String::new(),
            description: String::new(),
            parameters: empty_schema(),
            method: DEFAULT_METHOD.to_string(),
            headers: Vec::new(),
            requires_approval: true,
        }
    }
}

/// The parameter schema used when none is given.
fn empty_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

/// One endpoint, with its command line resolved.
#[derive(Debug, Clone)]
struct WebToolSpec {
    name: String,
    description: String,
    parameters: Value,
    requires_approval: bool,
    method: reqwest::Method,
    url: String,
    headers: Vec<(String, String)>,
}

/// A tool source backed by a single HTTP endpoint.
///
/// Construct one with [`new`](WebSource::new). Loading is synchronous — nothing is sent
/// until a tool is called.
#[derive(Debug)]
pub struct WebSource {
    id: String,
    client: reqwest::Client,
    timeout: Duration,
    tool: WebToolSpec,
    breaker: Arc<CircuitBreaker>,
}

impl WebSource {
    /// Build a source that wraps `config`'s endpoint as one tool.
    ///
    /// # Errors
    ///
    /// Returns a message when the HTTP client cannot be built or the method is not a valid
    /// HTTP method.
    pub fn new(
        id: impl Into<String>,
        config: WebSourceConfig,
        timeout: Option<Duration>,
    ) -> Result<Self, String> {
        let id = id.into();
        let method = reqwest::Method::from_bytes(config.method.as_bytes())
            .map_err(|error| format!("`{}` is not a valid HTTP method: {error}", config.method))?;
        let client = reqwest::Client::builder()
            .build()
            .map_err(|error| format!("cannot build the HTTP client: {error}"))?;

        let name = if config.name.is_empty() {
            id.clone()
        } else {
            config.name
        };
        let tool = WebToolSpec {
            name,
            description: config.description,
            parameters: config.parameters,
            requires_approval: config.requires_approval,
            method,
            url: config.url,
            headers: config.headers,
        };

        Ok(WebSource {
            id,
            client,
            timeout: timeout.unwrap_or(DEFAULT_TIMEOUT),
            tool,
            breaker: Arc::new(CircuitBreaker::new(BreakerPolicy::default())),
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

impl ToolSource for WebSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Web
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![Box::new(WebTool {
            source_id: self.id.clone(),
            client: self.client.clone(),
            timeout: self.timeout,
            spec: self.tool.clone(),
            breaker: Arc::clone(&self.breaker),
        })]
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

/// A tool offered by a [`WebSource`].
///
/// HTTP requests are independent, so this tool is *not* sequential: the agent may run it
/// alongside others.
#[derive(Debug)]
struct WebTool {
    source_id: String,
    client: reqwest::Client,
    timeout: Duration,
    spec: WebToolSpec,
    breaker: Arc<CircuitBreaker>,
}

impl WebTool {
    /// Build the request: arguments as query parameters for `GET`/`HEAD`, else as a body.
    fn request(&self, args: &Value) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .request(self.spec.method.clone(), &self.spec.url);
        if self.spec.method == reqwest::Method::GET || self.spec.method == reqwest::Method::HEAD {
            request = request.query(&query_pairs(args));
        } else {
            request = request.json(args);
        }
        for (name, value) in &self.spec.headers {
            request = request.header(name, value);
        }
        request
    }
}

#[async_trait]
impl Tool for WebTool {
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

    /// A web tool reaches an HTTP endpoint, so it is a network call — never `exec`.
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

        let timeout = self.timeout;
        let response = tokio::time::timeout(timeout, self.request(&args).send())
            .await
            .map_err(|_| {
                self.breaker.record_failure(Instant::now());
                format!("the request timed out after {}s", timeout.as_secs())
            })?
            .map_err(|error| {
                self.breaker.record_failure(Instant::now());
                format!("cannot reach the endpoint: {error}")
            })?;

        let status = response.status();
        let (body, truncated) = tokio::time::timeout(timeout, read_body(response, MAX_BODY_BYTES))
            .await
            .map_err(|_| {
                self.breaker.record_failure(Instant::now());
                format!(
                    "reading the response timed out after {}s",
                    timeout.as_secs()
                )
            })?
            .inspect_err(|_| {
                self.breaker.record_failure(Instant::now());
            })?;

        // The endpoint answered, so it is reachable — whatever the answer says.
        self.breaker.record_success();
        if !status.is_success() {
            return Err(format!(
                "the endpoint returned HTTP {status}: {}",
                snippet(&body)
            ));
        }

        // A body past the cap is data the model would never see whole anyway; keep the head
        // and say it was cut, rather than failing a fetch that mostly succeeded.
        let body = if truncated {
            format!(
                "{body}\n… (truncated at {} MiB)",
                MAX_BODY_BYTES / (1024 * 1024)
            )
        } else {
            body
        };

        let mut result = ToolResult::text(body.clone());
        if let Ok(value) = serde_json::from_str::<Value>(&body) {
            result.details = Some(value);
        }
        Ok(result)
    }
}

/// The query parameters for a `GET`/`HEAD` request: one per argument, `null`s dropped.
fn query_pairs(args: &Value) -> Vec<(String, String)> {
    let Some(object) = args.as_object() else {
        return Vec::new();
    };
    object
        .iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(key, value)| (key.clone(), scalar(value)))
        .collect()
}

/// A value as a query-parameter string: a string verbatim, anything else as JSON.
fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// A short, single-line preview of a response body for an error message.
fn snippet(body: &str) -> String {
    const LIMIT: usize = 200;
    let trimmed = body.trim();
    let mut snippet: String = trimmed.chars().take(LIMIT).collect();
    if trimmed.chars().count() > LIMIT {
        snippet.push('…');
    }
    snippet
}

#[cfg(test)]
mod tests {
    use super::{scalar, snippet, WebSource, WebSourceConfig};
    use crate::approval::Permission;
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
        /// The request line, e.g. `GET /path?a=1 HTTP/1.1`.
        line: String,
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

        /// The HTTP method.
        fn method(&self) -> &str {
            self.line.split_whitespace().next().unwrap_or("")
        }

        /// The request target (path plus query), as sent.
        fn target(&self) -> &str {
            self.line.split_whitespace().nth(1).unwrap_or("")
        }
    }

    /// A canned HTTP response.
    struct Reply {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: String,
    }

    impl Reply {
        fn text(body: impl Into<String>) -> Self {
            Reply {
                status: 200,
                headers: vec![("Content-Type", "text/plain".to_string())],
                body: body.into(),
            }
        }

        fn json(body: impl Into<String>) -> Self {
            Reply {
                status: 200,
                headers: vec![("Content-Type", "application/json".to_string())],
                body: body.into(),
            }
        }

        fn status(mut self, status: u16) -> Self {
            self.status = status;
            self
        }
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
        format!("http://{addr}/tool")
    }

    /// Read one HTTP/1.1 request (headers plus a `Content-Length` body).
    async fn read_request(stream: &mut TcpStream) -> Option<Request> {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        if reader.read_line(&mut line).await.ok()? == 0 {
            return None;
        }
        let line = line.trim_end_matches(['\r', '\n']).to_string();
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
            line,
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

    /// A `GET` source over a server that echoes its request target.
    async fn get_source(handler: impl Fn(&Request) -> Reply + Send + 'static) -> WebSource {
        let url = serve(handler).await;
        WebSource::new(
            "api",
            WebSourceConfig::new(url),
            Some(Duration::from_secs(5)),
        )
        .expect("build the source")
    }

    #[tokio::test]
    async fn a_get_source_sends_arguments_as_query_parameters() {
        let seen = Arc::new(Mutex::new(String::new()));
        let recorder = Arc::clone(&seen);
        let source = get_source(move |request| {
            *recorder.lock().expect("lock") = request.target().to_string();
            Reply::text("pong")
        })
        .await;

        assert_eq!(source.kind(), SourceKind::Web);
        assert_eq!(source.id(), "api");
        assert_eq!(source.health(), SourceHealth::Healthy);

        let tools = source.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].spec().name,
            "api",
            "the tool defaults to the source id"
        );
        assert!(tools[0].requires_approval(), "approval is opt-out");
        assert!(!tools[0].is_sequential(), "HTTP calls may run concurrently");

        let context = ToolContext::new(".");
        let result = tools[0]
            .execute("call", json!({ "city": "Oslo", "days": 3 }), &context)
            .await
            .expect("the call must succeed");
        assert_eq!(result.content[0].as_text(), Some("pong"));

        let target = seen.lock().expect("lock").clone();
        assert!(target.starts_with("/tool?"), "{target}");
        assert!(target.contains("city=Oslo"), "{target}");
        assert!(target.contains("days=3"), "{target}");
    }

    #[tokio::test]
    async fn a_post_source_sends_arguments_as_a_json_body() {
        let seen = Arc::new(Mutex::new((String::new(), String::new())));
        let recorder = Arc::clone(&seen);
        let url = serve(move |request| {
            *recorder.lock().expect("lock") = (request.method().to_string(), request.body.clone());
            Reply::json(r#"{"ok":true}"#)
        })
        .await;

        let mut config = WebSourceConfig::new(url);
        config.method = "POST".to_string();
        config.name = "create".to_string();
        let source =
            WebSource::new("api", config, Some(Duration::from_secs(5))).expect("build the source");

        let tools = source.tools();
        assert_eq!(tools[0].spec().name, "create");
        let context = ToolContext::new(".");
        let result = tools[0]
            .execute("call", json!({ "title": "hi" }), &context)
            .await
            .expect("the call must succeed");
        assert_eq!(result.content[0].as_text(), Some(r#"{"ok":true}"#));
        assert_eq!(result.details, Some(json!({ "ok": true })), "JSON is kept");

        let (method, body) = seen.lock().expect("lock").clone();
        assert_eq!(method, "POST");
        assert_eq!(
            serde_json::from_str::<Value>(&body).expect("a JSON body"),
            json!({ "title": "hi" })
        );
    }

    #[tokio::test]
    async fn null_arguments_are_dropped_from_the_query() {
        let seen = Arc::new(Mutex::new(String::new()));
        let recorder = Arc::clone(&seen);
        let source = get_source(move |request| {
            *recorder.lock().expect("lock") = request.target().to_string();
            Reply::text("ok")
        })
        .await;

        let context = ToolContext::new(".");
        source.tools()[0]
            .execute("call", json!({ "keep": "1", "drop": null }), &context)
            .await
            .expect("the call must succeed");

        let target = seen.lock().expect("lock").clone();
        assert!(target.contains("keep=1"), "{target}");
        assert!(!target.contains("drop"), "{target}");
    }

    #[tokio::test]
    async fn extra_headers_are_sent() {
        let seen = Arc::new(Mutex::new(None));
        let recorder = Arc::clone(&seen);
        let url = serve(move |request| {
            *recorder.lock().expect("lock") = request.header("Authorization").map(str::to_string);
            Reply::text("ok")
        })
        .await;

        let mut config = WebSourceConfig::new(url);
        config.headers = vec![("Authorization".to_string(), "Bearer token".to_string())];
        let source =
            WebSource::new("api", config, Some(Duration::from_secs(5))).expect("build the source");

        let context = ToolContext::new(".");
        source.tools()[0]
            .execute("call", json!({}), &context)
            .await
            .expect("the call must succeed");

        assert_eq!(seen.lock().expect("lock").as_deref(), Some("Bearer token"));
    }

    #[tokio::test]
    async fn an_http_error_status_is_a_business_failure() {
        let source = get_source(|_| Reply::text("no such city").status(404)).await;

        let tools = source.tools();
        let context = ToolContext::new(".");
        for _ in 0..3 {
            let error = tools[0]
                .execute("call", json!({}), &context)
                .await
                .expect_err("the endpoint returned an error");
            assert!(error.contains("HTTP 404"), "{error}");
            assert!(error.contains("no such city"), "{error}");
        }
        assert_eq!(
            source.health(),
            SourceHealth::Healthy,
            "a response means the endpoint is reachable"
        );
    }

    #[tokio::test]
    async fn an_unreachable_endpoint_opens_the_breaker() {
        let source = WebSource::new(
            "dead",
            WebSourceConfig::new("http://127.0.0.1:1/tool"),
            Some(Duration::from_secs(5)),
        )
        .expect("build the source")
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
            .expect_err("the endpoint is unreachable");
        assert!(error.contains("cannot reach the endpoint"), "{error}");

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
    fn an_invalid_method_is_refused() {
        // A space is not allowed in an HTTP method token.
        let mut config = WebSourceConfig::new("http://example.com");
        config.method = "GET EXTRA".to_string();
        let error = WebSource::new("api", config, None).expect_err("not an HTTP method");
        assert!(error.contains("not a valid HTTP method"), "{error}");
    }

    #[test]
    fn scalar_renders_strings_verbatim_and_the_rest_as_json() {
        assert_eq!(scalar(&json!("a b")), "a b");
        assert_eq!(scalar(&json!(3)), "3");
        assert_eq!(scalar(&json!(true)), "true");
        assert_eq!(scalar(&json!({ "n": 1 })), r#"{"n":1}"#);
    }

    #[test]
    fn snippet_truncates_a_long_body() {
        assert_eq!(snippet("  hello  "), "hello");
        let long = "x".repeat(300);
        let short = snippet(&long);
        assert_eq!(
            short.chars().count(),
            201,
            "200 characters plus the ellipsis"
        );
        assert!(short.ends_with('…'));
    }

    #[test]
    fn a_web_tool_is_a_network_call() {
        // A web tool reaches an HTTP endpoint; `--allow network` must be what lets it
        // through, not `--allow exec` (see ACTIONS.md A1).
        let source = WebSource::new("api", WebSourceConfig::new("http://127.0.0.1:1/tool"), None)
            .expect("build the source");
        assert_eq!(source.tools()[0].permission(), Permission::Network);
    }
}
