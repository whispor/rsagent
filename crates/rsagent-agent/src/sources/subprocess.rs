//! Running a tool as a subprocess and reading its JSON reply.
//!
//! Two sources hand work to a program: `local` runs one program that serves many tools,
//! and `skill` runs one script per tool. The *contract* is the same either way — the
//! caller writes one JSON value to the program's stdin and reads one JSON object
//! (`{"content": ...}` or `{"error": ...}`) from its stdout. The spawn, the timeout, and
//! the reply parsing are identical, so they live here.
//!
//! # Failure classes
//!
//! A run that cannot start, times out, exits non-zero, or prints something that is not
//! JSON is a **connection-class** failure and feeds the circuit breaker. A well-formed
//! `{"error": ...}` response is a **business** failure: the program is working, so it must
//! not trip the breaker (decision D4).

use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use crate::tool::ToolResult;

/// Run `argv` once, optionally sending `request` on stdin, and return its trimmed stdout.
///
/// `argv[0]` is the program; the rest are its arguments.
///
/// # Errors
///
/// Returns a message when the program cannot be started, times out, or exits non-zero.
/// Every error here is connection-class: it means the program could not be talked to.
pub(crate) async fn run(
    argv: &[String],
    request: Option<&Value>,
    timeout: Duration,
) -> Result<String, String> {
    let (program, rest) = argv
        .split_first()
        .ok_or_else(|| "a tool command must not be empty".to_string())?;
    let label = argv.join(" ");

    let mut child = tokio::process::Command::new(program)
        .args(rest)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run `{label}`: {error}"))?;

    // Send the request (if any) and close stdin, so the program sees a clean EOF.
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "stdin was not piped".to_string())?;
        if let Some(request) = request {
            let payload = format!("{request}\n");
            stdin
                .write_all(payload.as_bytes())
                .await
                .map_err(|error| format!("cannot send the request to `{label}`: {error}"))?;
            let _ = stdin.flush().await;
        }
    }

    // Drain both pipes on their own tasks so a full buffer cannot deadlock the wait.
    let stdout_task = tokio::spawn(read_all(child.stdout.take()));
    let stderr_task = tokio::spawn(read_all(child.stderr.take()));

    let outcome = tokio::select! {
        status = child.wait() => Outcome::Exited(status),
        () = tokio::time::sleep(timeout) => Outcome::TimedOut,
    };

    let status = match outcome {
        Outcome::Exited(Ok(status)) => status,
        Outcome::Exited(Err(error)) => return Err(format!("`{label}` failed: {error}")),
        Outcome::TimedOut => {
            let _ = child.kill().await;
            return Err(format!(
                "`{label}` timed out after {}s and was killed",
                timeout.as_secs()
            ));
        }
    };

    let stdout = stdout_task.await.unwrap_or_default();
    let stderr = stderr_task.await.unwrap_or_default();
    if !status.success() {
        let code = status
            .code()
            .map_or_else(|| "a signal".to_string(), |code| code.to_string());
        return Err(format!("`{label}` exited with {code}: {}", stderr.trim()));
    }
    Ok(stdout.trim().to_string())
}

/// Why a run stopped.
enum Outcome {
    Exited(std::io::Result<std::process::ExitStatus>),
    TimedOut,
}

/// Read a pipe to the end, ignoring errors (a broken pipe is reported by the exit status).
async fn read_all<R: AsyncRead + Unpin>(reader: Option<R>) -> String {
    let Some(mut reader) = reader else {
        return String::new();
    };
    let mut buffer = String::new();
    let _ = reader.read_to_string(&mut buffer).await;
    buffer
}

/// Read a `{"content"|"error"}` reply into a tool result.
///
/// # Errors
///
/// Returns the message when the reply carries `error`, or when it has neither key.
pub(crate) fn parse_response(value: &Value) -> Result<ToolResult, String> {
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(error.to_string());
    }
    let content = value
        .get("content")
        .ok_or_else(|| "the call response has neither `content` nor `error`".to_string())?;
    let text = match content {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let mut result = ToolResult::text(text);
    if let Some(details) = value.get("details") {
        result.details = Some(details.clone());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::parse_response;
    use serde_json::json;

    #[test]
    fn an_error_response_is_a_business_failure() {
        let error = parse_response(&json!({ "error": "no such thing" }))
            .expect_err("an error response becomes Err");
        assert_eq!(error, "no such thing");
    }

    #[test]
    fn a_content_response_is_the_result() {
        let result = parse_response(&json!({ "content": "pong" })).expect("parse");
        assert_eq!(result.content[0].as_text(), Some("pong"));
    }

    #[test]
    fn non_string_content_is_rendered_as_json() {
        let result = parse_response(&json!({ "content": { "n": 1 } })).expect("parse");
        assert_eq!(result.content[0].as_text(), Some("{\"n\":1}"));
    }

    #[test]
    fn details_are_passed_through() {
        let result =
            parse_response(&json!({ "content": "ok", "details": { "bytes": 2 } })).expect("parse");
        assert_eq!(result.details, Some(json!({ "bytes": 2 })));
    }

    #[test]
    fn a_response_with_neither_key_is_a_protocol_error() {
        assert!(parse_response(&json!({ "whatever": 1 })).is_err());
    }
}
