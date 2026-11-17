//! A source backed by a program, speaking a small JSON protocol.
//!
//! This is the "活样板" for tool extensibility: the same shape a future `mcp-stdio` source
//! will take, but with no external protocol to implement. See `docs/EXTENSIBILITY.md` §3.2.
//!
//! # Protocol, version 1
//!
//! The program is invoked twice, with a phase flag the source appends to `args`:
//!
//! - `program <args...> --describe` prints one JSON object on stdout:
//!   `{"protocol": 1, "tools": [{"name", "description", "parameters", "requires_approval"}]}`
//! - `program <args...> --call` reads one JSON object from stdin
//!   (`{"tool": "<name>", "arguments": {...}}`) and prints one JSON object on stdout:
//!   `{"content": "..."}` or `{"error": "..."}`. `content` may be any JSON value; a
//!   string is used verbatim, anything else is rendered as compact JSON.
//!
//! `requires_approval` defaults to `true`: an arbitrary program may do anything, so it
//! must opt *out* of the approval prompt, never accidentally into it.
//!
//! # Failure classes
//!
//! A run that cannot start, times out, exits non-zero, or prints something that is not
//! JSON is a **connection-class** failure and feeds the circuit breaker. A well-formed
//! `{"error": ...}` response is a **business** failure: the program is working, so it must
//! not trip the breaker (decision D4).

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rsagent_ai::ToolSpec;
use serde_json::{json, Value};

use crate::breaker::{BreakerPolicy, BreakerState, CircuitBreaker};
use crate::source::{SourceHealth, SourceKind, ToolSource};
use crate::tool::{Tool, ToolContext, ToolResult};

use super::subprocess::{parse_response, run};

/// The protocol version this client speaks.
pub const PROTOCOL_VERSION: u32 = 1;

/// Per-call timeout when the configuration does not set one.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// A source backed by a program run as a subprocess.
#[derive(Debug)]
pub struct LocalSource {
    id: String,
    program: String,
    args: Vec<String>,
    timeout: Duration,
    tools: Vec<LocalToolSpec>,
    breaker: Arc<CircuitBreaker>,
}

impl LocalSource {
    /// Ask `program` for its tool list.
    ///
    /// # Errors
    ///
    /// Returns a message when the program cannot be run, times out, or prints a tool list
    /// this build cannot understand.
    pub async fn new(
        id: impl Into<String>,
        program: impl Into<String>,
        args: Vec<String>,
        timeout: Option<Duration>,
    ) -> Result<Self, String> {
        let id = id.into();
        let program = program.into();
        let timeout = timeout.unwrap_or(DEFAULT_TIMEOUT);

        let stdout = run(&command_argv(&program, &args, "--describe"), None, timeout).await?;
        let value: Value = serde_json::from_str(&stdout)
            .map_err(|error| format!("`{program} --describe` did not print JSON: {error}"))?;
        let tools = parse_describe(&value)?;

        Ok(LocalSource {
            id,
            program,
            args,
            timeout,
            tools,
            breaker: Arc::new(CircuitBreaker::new(BreakerPolicy::default())),
        })
    }

    /// Replace the default breaker policy (used by tests and by a future `doctor`).
    #[must_use]
    pub fn with_breaker_policy(mut self, policy: BreakerPolicy) -> Self {
        self.breaker = Arc::new(CircuitBreaker::new(policy));
        self
    }

    /// The program this source runs.
    pub fn program(&self) -> &str {
        &self.program
    }
}

impl ToolSource for LocalSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Local
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        self.tools
            .iter()
            .map(|spec| -> Box<dyn Tool> {
                Box::new(LocalTool {
                    source_id: self.id.clone(),
                    program: self.program.clone(),
                    args: self.args.clone(),
                    timeout: self.timeout,
                    spec: spec.clone(),
                    breaker: Arc::clone(&self.breaker),
                })
            })
            .collect()
    }

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

/// One tool as declared by the program.
#[derive(Debug, Clone)]
struct LocalToolSpec {
    name: String,
    description: String,
    parameters: Value,
    requires_approval: bool,
}

/// A tool offered by a [`LocalSource`].
#[derive(Debug)]
struct LocalTool {
    source_id: String,
    program: String,
    args: Vec<String>,
    timeout: Duration,
    spec: LocalToolSpec,
    breaker: Arc<CircuitBreaker>,
}

#[async_trait]
impl Tool for LocalTool {
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

    /// Local tools are opaque subprocesses, so they never run concurrently with others.
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

        let request = json!({ "tool": self.spec.name, "arguments": args });
        let stdout = match run(
            &command_argv(&self.program, &self.args, "--call"),
            Some(&request),
            self.timeout,
        )
        .await
        {
            Ok(stdout) => stdout,
            Err(error) => {
                self.breaker.record_failure(Instant::now());
                return Err(error);
            }
        };

        let value: Value = match serde_json::from_str(&stdout) {
            Ok(value) => value,
            Err(error) => {
                self.breaker.record_failure(Instant::now());
                return Err(format!(
                    "`{} --call` printed invalid JSON: {error}",
                    self.program
                ));
            }
        };

        // The program answered: it is reachable, whatever the answer says.
        self.breaker.record_success();
        parse_response(&value)
    }
}

/// The full command line for one phase: the program, its fixed args, then the phase flag.
fn command_argv(program: &str, args: &[String], phase: &str) -> Vec<String> {
    let mut command = Vec::with_capacity(args.len() + 2);
    command.push(program.to_string());
    command.extend(args.iter().cloned());
    command.push(phase.to_string());
    command
}

/// Read a `--describe` response.
fn parse_describe(value: &Value) -> Result<Vec<LocalToolSpec>, String> {
    let protocol = value.get("protocol").and_then(Value::as_u64);
    if protocol != Some(u64::from(PROTOCOL_VERSION)) {
        return Err(format!(
            "unsupported protocol {protocol:?}; this build speaks {PROTOCOL_VERSION}"
        ));
    }
    let tools = value
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| "the describe response has no `tools` array".to_string())?;
    tools.iter().map(parse_tool_spec).collect()
}

/// Read one entry of the `tools` array.
fn parse_tool_spec(value: &Value) -> Result<LocalToolSpec, String> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "a described tool is missing `name`".to_string())?
        .to_string();
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let parameters = value
        .get("parameters")
        .cloned()
        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    // An arbitrary program may do anything, so approval is the default.
    let requires_approval = value
        .get("requires_approval")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    Ok(LocalToolSpec {
        name,
        description,
        parameters,
        requires_approval,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_describe;
    use serde_json::json;

    #[test]
    fn describe_requires_the_protocol_version() {
        let missing = json!({ "tools": [] });
        assert!(parse_describe(&missing).is_err());

        let wrong = json!({ "protocol": 99, "tools": [] });
        let error = parse_describe(&wrong).expect_err("a wrong version is refused");
        assert!(error.contains("unsupported protocol"), "{error}");
    }

    #[test]
    fn describe_defaults_approval_to_true() {
        let value = json!({
            "protocol": 1,
            "tools": [{ "name": "danger", "description": "Does things." }]
        });
        let tools = parse_describe(&value).expect("parse");
        assert!(tools[0].requires_approval, "approval must be opt-out");
    }

    #[test]
    fn describe_reads_an_explicit_approval_flag() {
        let value = json!({
            "protocol": 1,
            "tools": [{ "name": "peek", "description": "Reads.", "requires_approval": false }]
        });
        let tools = parse_describe(&value).expect("parse");
        assert!(!tools[0].requires_approval);
    }

    #[test]
    fn describe_rejects_a_tool_without_a_name() {
        let value = json!({ "protocol": 1, "tools": [{ "description": "anonymous" }] });
        assert!(parse_describe(&value).is_err());
    }
}
