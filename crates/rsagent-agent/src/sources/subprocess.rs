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
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;

use crate::platform::kill_tree;
use crate::tool::ToolResult;

/// How long to wait for the stdout/stderr pipes to reach EOF after the process exits (or
/// is killed). A grandchild that inherited a pipe can hold it open forever; this bounds
/// that wait so a turn can never hang on a backgrounded daemon.
pub(crate) const DRAIN_GRACE: Duration = Duration::from_secs(1);

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
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("cannot run `{label}`: {error}"))?;

    // Drain both pipes on their own tasks so a full buffer cannot deadlock the wait. The
    // output accumulates in a shared buffer, so an aborted drain still yields what arrived.
    let stdout_sink = Arc::new(StdMutex::new(Vec::new()));
    let stderr_sink = Arc::new(StdMutex::new(Vec::new()));
    let mut stdout_task = tokio::spawn(drain_into(child.stdout.take(), Arc::clone(&stdout_sink)));
    let mut stderr_task = tokio::spawn(drain_into(child.stderr.take(), Arc::clone(&stderr_sink)));

    // One deadline covers sending the request *and* waiting for exit, so a program that
    // never reads its stdin (a large request blocks on a full pipe) or never exits cannot
    // hold the turn open past `timeout`.
    let outcome = tokio::time::timeout(timeout, async {
        if let Some(mut stdin) = child.stdin.take() {
            if let Some(request) = request {
                let payload = format!("{request}\n");
                stdin
                    .write_all(payload.as_bytes())
                    .await
                    .map_err(|error| format!("cannot send the request to `{label}`: {error}"))?;
                let _ = stdin.flush().await;
            }
            // Dropping `stdin` here closes it, so the program sees a clean EOF.
        }
        child
            .wait()
            .await
            .map_err(|error| format!("`{label}` failed: {error}"))
    })
    .await;

    let status = match outcome {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => return Err(error),
        Err(_) => {
            kill_tree(&mut child).await;
            return Err(format!(
                "`{label}` timed out after {}s and was killed",
                timeout.as_secs()
            ));
        }
    };

    // The process has exited; give the pipes a moment to flush, then stop waiting.
    drain_within(&mut stdout_task).await;
    drain_within(&mut stderr_task).await;
    let stdout = take_text(&stdout_sink);
    let stderr = take_text(&stderr_sink);

    if !status.success() {
        let code = status
            .code()
            .map_or_else(|| "a signal".to_string(), |code| code.to_string());
        return Err(format!("`{label}` exited with {code}: {}", stderr.trim()));
    }
    Ok(stdout.trim().to_string())
}

/// Read a pipe to the end into `sink`, ignoring errors (a broken pipe shows in the exit
/// status). Bytes are accumulated, not decoded, so a multi-byte character split across
/// reads is never corrupted.
async fn drain_into<R: AsyncRead + Unpin>(reader: Option<R>, sink: Arc<StdMutex<Vec<u8>>>) {
    let Some(mut reader) = reader else {
        return;
    };
    let mut chunk = [0_u8; 4096];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if let Ok(mut bytes) = sink.lock() {
                    bytes.extend_from_slice(&chunk[..read]);
                }
            }
        }
    }
}

/// Await a drain task, but give up after [`DRAIN_GRACE`] and abort it. The partial output
/// stays in the sink, so nothing already read is lost.
pub(crate) async fn drain_within(task: &mut JoinHandle<()>) {
    if tokio::time::timeout(DRAIN_GRACE, &mut *task).await.is_err() {
        task.abort();
    }
}

/// Decode a shared byte sink into a string (lossily, in case the drain was aborted mid-character).
fn take_text(sink: &Arc<StdMutex<Vec<u8>>>) -> String {
    let bytes = sink
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    String::from_utf8_lossy(&bytes).into_owned()
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
    use super::{parse_response, run};
    use serde_json::json;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn a_pipe_held_open_by_a_grandchild_does_not_hang_the_run() {
        // The shell exits at once, but a background child inherits its stdout, so the read
        // side never sees EOF. Without a bounded drain this awaited the grandchild forever.
        #[cfg(windows)]
        let argv = vec![
            "cmd".to_string(),
            "/C".to_string(),
            "start /B ping -n 30 127.0.0.1 & echo done".to_string(),
        ];
        #[cfg(not(windows))]
        let argv = vec![
            "sh".to_string(),
            "-c".to_string(),
            "sleep 30 & echo done".to_string(),
        ];

        let start = Instant::now();
        let output = run(&argv, None, Duration::from_secs(5))
            .await
            .expect("the shell itself exits cleanly");
        assert!(output.contains("done"), "stdout: {output:?}");
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the drain must be bounded, took {:?}",
            start.elapsed()
        );
    }

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
