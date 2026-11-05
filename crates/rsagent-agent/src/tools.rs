//! Built-in tools: the filesystem and shell primitives a coding agent needs.

use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use rsagent_ai::ToolSpec;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;

use crate::tool::{Tool, ToolContext, ToolResult};

fn object_schema(properties: &Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

fn required_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing required string argument `{key}`"))
}

/// Read a file's contents as UTF-8 text.
#[derive(Debug)]
pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "read_file",
            "Read a file's contents as UTF-8 text. The path may be absolute or relative to the working directory.",
            object_schema(
                &json!({ "path": { "type": "string", "description": "File path" } }),
                &["path"],
            ),
        )
    }

    fn label(&self) -> String {
        "Read File".to_string()
    }

    async fn execute(
        &self,
        _call_id: &str,
        args: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, String> {
        let path = required_str(&args, "path")?;
        let resolved = context.resolve(&path);
        match tokio::fs::read_to_string(&resolved).await {
            Ok(data) => Ok(ToolResult::text(data)),
            Err(error) => Err(format!("failed to read {}: {error}", resolved.display())),
        }
    }
}

/// Write content to a file, creating parent directories as needed.
#[derive(Debug)]
pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "write_file",
            "Write content to a file, creating parent directories as needed. Overwrites any existing file.",
            object_schema(
                &json!({
                    "path": { "type": "string", "description": "File path" },
                    "content": { "type": "string", "description": "Content to write" },
                }),
                &["path", "content"],
            ),
        )
    }

    fn label(&self) -> String {
        "Write File".to_string()
    }

    fn is_sequential(&self) -> bool {
        true
    }

    fn requires_approval(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _call_id: &str,
        args: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, String> {
        let path = required_str(&args, "path")?;
        let body = required_str(&args, "content")?;
        let resolved = context.resolve(&path);

        if let Some(parent) = resolved.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        }
        tokio::fs::write(&resolved, body.as_bytes())
            .await
            .map_err(|error| format!("failed to write {}: {error}", resolved.display()))?;
        Ok(ToolResult::text(format!(
            "Wrote {} bytes to {}",
            body.len(),
            resolved.display()
        )))
    }
}

/// Replace an exact string in a file.
#[derive(Debug)]
pub struct EditFileTool;

#[async_trait]
impl Tool for EditFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "edit_file",
            "Replace an exact string in a file with a new string. Fails if the old string is not found.",
            object_schema(
                &json!({
                    "path": { "type": "string", "description": "File path" },
                    "old_string": { "type": "string", "description": "Exact text to replace" },
                    "new_string": { "type": "string", "description": "Replacement text" },
                    "replace_all": { "type": "boolean", "description": "Replace every occurrence (default false)" },
                }),
                &["path", "old_string", "new_string"],
            ),
        )
    }

    fn label(&self) -> String {
        "Edit File".to_string()
    }

    fn is_sequential(&self) -> bool {
        true
    }

    fn requires_approval(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _call_id: &str,
        args: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, String> {
        let path = required_str(&args, "path")?;
        let old_string = required_str(&args, "old_string")?;
        let new_string = required_str(&args, "new_string")?;
        let replace_all = args
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let resolved = context.resolve(&path);

        let original = tokio::fs::read_to_string(&resolved)
            .await
            .map_err(|error| format!("failed to read {}: {error}", resolved.display()))?;

        if !original.contains(&old_string) {
            return Err(format!("old_string not found in {}", resolved.display()));
        }

        let (updated, count) = if replace_all {
            let count = original.matches(&old_string).count();
            (original.replace(&old_string, &new_string), count)
        } else {
            (original.replacen(&old_string, &new_string, 1), 1)
        };

        tokio::fs::write(&resolved, updated.as_bytes())
            .await
            .map_err(|error| format!("failed to write {}: {error}", resolved.display()))?;
        Ok(ToolResult::text(format!(
            "Replaced {count} occurrence(s) in {}",
            resolved.display()
        )))
    }
}

/// List directory entries.
#[derive(Debug)]
pub struct ListDirTool;

#[async_trait]
impl Tool for ListDirTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "list_dir",
            "List the entries of a directory. Defaults to the working directory.",
            object_schema(
                &json!({ "path": { "type": "string", "description": "Directory path (default \".\")" } }),
                &[],
            ),
        )
    }

    fn label(&self) -> String {
        "List Directory".to_string()
    }

    async fn execute(
        &self,
        _call_id: &str,
        args: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, String> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or(".")
            .to_string();
        let resolved = context.resolve(&path);

        let mut entries = tokio::fs::read_dir(&resolved)
            .await
            .map_err(|error| format!("failed to list {}: {error}", resolved.display()))?;

        let mut lines = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| format!("failed to read entry: {error}"))?
        {
            let file_type = entry.file_type().await.ok();
            let kind = match file_type {
                Some(t) if t.is_dir() => "dir ",
                Some(t) if t.is_symlink() => "link",
                _ => "file",
            };
            lines.push(format!("{kind} {}", entry.file_name().to_string_lossy()));
        }
        lines.sort();
        Ok(ToolResult::text(lines.join("\n")))
    }
}

/// Search for a literal substring across files under a directory.
///
/// Read-only: it never mutates the filesystem, so it needs no approval. Matching lines
/// are reported as `path:line: text`, mirroring `grep -rn`.
#[derive(Debug)]
pub struct SearchTool;

#[async_trait]
impl Tool for SearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "search",
            "Search for a literal substring in files under a directory, reporting matching lines as `path:line: text`. Read-only.",
            object_schema(
                &json!({
                    "pattern": { "type": "string", "description": "Literal text to search for" },
                    "path": { "type": "string", "description": "Directory to search (default \".\")" },
                    "max_results": { "type": "integer", "description": "Stop after this many matches (default 100)" },
                }),
                &["pattern"],
            ),
        )
    }

    fn label(&self) -> String {
        "Search".to_string()
    }

    async fn execute(
        &self,
        _call_id: &str,
        args: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, String> {
        let pattern = required_str(&args, "pattern")?;
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or(".")
            .to_string();
        let max_results = usize::try_from(
            args.get("max_results")
                .and_then(Value::as_u64)
                .unwrap_or(100),
        )
        .unwrap_or(100);
        let root = context.resolve(&path);

        // An explicit stack avoids async recursion (which would need boxing) and lets us
        // stop the moment `max_results` is reached.
        let mut stack = vec![root.clone()];
        let mut matches: Vec<String> = Vec::new();

        'walk: while let Some(dir) = stack.pop() {
            let mut entries = tokio::fs::read_dir(&dir)
                .await
                .map_err(|error| format!("failed to read {}: {error}", dir.display()))?;
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|error| format!("failed to read entry: {error}"))?
            {
                let file_type = entry.file_type().await.ok();
                let entry_path = entry.path();
                if matches!(file_type.as_ref(), Some(t) if t.is_dir()) {
                    stack.push(entry_path);
                } else if matches!(file_type.as_ref(), Some(t) if t.is_file()) {
                    // Skip unreadable or non-UTF-8 files rather than failing the search.
                    let Ok(text) = tokio::fs::read_to_string(&entry_path).await else {
                        continue;
                    };
                    for (index, line) in text.lines().enumerate() {
                        if line.contains(&pattern) {
                            matches.push(format!(
                                "{}:{}: {}",
                                entry_path.display(),
                                index + 1,
                                line.trim()
                            ));
                            if matches.len() >= max_results {
                                break 'walk;
                            }
                        }
                    }
                }
            }
        }

        if matches.is_empty() {
            return Ok(ToolResult::text(format!(
                "No matches for `{pattern}` under {}",
                root.display()
            )));
        }
        Ok(ToolResult::text(matches.join("\n")))
    }
}

/// Run a shell command in the working directory and return its combined output.
///
/// The command is killed if it exceeds its timeout or the run is cancelled, so a
/// hung process (a build waiting on input, `yes`, an interactive prompt, ...) cannot
/// stall the agent forever. Output is streamed line-by-line as progress when a
/// progress sink is attached.
#[derive(Debug)]
pub struct BashTool {
    timeout: Duration,
}

impl Default for BashTool {
    fn default() -> Self {
        BashTool {
            timeout: Duration::from_secs(120),
        }
    }
}

impl BashTool {
    /// Create a tool with the default 120-second timeout.
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the default per-command timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Why a command stopped.
enum BashExit {
    Exited(std::io::Result<std::process::ExitStatus>),
    Cancelled,
    TimedOut,
}

#[async_trait]
impl Tool for BashTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "bash",
            "Run a shell command in the working directory and return its combined output. Commands are killed after a timeout.",
            object_schema(
                &json!({
                    "command": { "type": "string", "description": "Shell command to run" },
                    "timeout_secs": { "type": "integer", "description": "Kill the command after this many seconds (default 120)" },
                }),
                &["command"],
            ),
        )
    }

    fn label(&self) -> String {
        "Shell".to_string()
    }

    fn is_sequential(&self) -> bool {
        true
    }

    fn requires_approval(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _call_id: &str,
        args: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, String> {
        let command = required_str(&args, "command")?;
        let timeout = args
            .get("timeout_secs")
            .and_then(Value::as_u64)
            .map_or(self.timeout, Duration::from_secs);

        #[cfg(windows)]
        let mut process = {
            let mut process = Command::new("cmd");
            process.arg("/C");
            process
        };
        #[cfg(not(windows))]
        let mut process = {
            let mut process = Command::new("sh");
            process.arg("-c");
            process
        };

        let mut child = process
            .arg(&command)
            .current_dir(&context.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("failed to spawn command: {error}"))?;

        // Drain each pipe on its own task so a full buffer cannot deadlock the wait,
        // and so lines can be streamed as progress while the command runs.
        let stdout_task = tokio::spawn(stream_pipe(
            child.stdout.take(),
            context.progress().cloned(),
        ));
        let stderr_task = tokio::spawn(stream_pipe(
            child.stderr.take(),
            context.progress().cloned(),
        ));
        let cancel = context.cancel().clone();

        let outcome = tokio::select! {
            () = cancel.cancelled() => BashExit::Cancelled,
            () = tokio::time::sleep(timeout) => BashExit::TimedOut,
            status = child.wait() => BashExit::Exited(status),
        };

        let status = match outcome {
            BashExit::Exited(Ok(status)) => status,
            BashExit::Exited(Err(error)) => {
                return Err(format!("failed to wait for command: {error}"))
            }
            BashExit::Cancelled => {
                let _ = child.kill().await;
                return Err("command cancelled".to_string());
            }
            BashExit::TimedOut => {
                let _ = child.kill().await;
                return Err(format!(
                    "command timed out after {}s and was killed",
                    timeout.as_secs()
                ));
            }
        };

        let stdout = stdout_task.await.unwrap_or_default();
        let stderr = stderr_task.await.unwrap_or_default();
        let mut combined = String::new();
        if !stdout.is_empty() {
            combined.push_str(&stdout);
        }
        if !stderr.is_empty() {
            if !combined.is_empty() && !combined.ends_with('\n') {
                combined.push('\n');
            }
            combined.push_str(&stderr);
        }

        match status.code() {
            Some(0) => Ok(ToolResult::text(if combined.is_empty() {
                "(no output)".to_string()
            } else {
                combined
            })),
            Some(code) => Err(format!("command exited with code {code}\n{combined}")),
            None => Err(format!("command terminated by signal\n{combined}")),
        }
    }
}

/// Read a pipe to the end, streaming each line to `progress` and returning the full text.
async fn stream_pipe<R>(pipe: Option<R>, progress: Option<UnboundedSender<String>>) -> String
where
    R: tokio::io::AsyncRead + Unpin,
{
    let Some(pipe) = pipe else {
        return String::new();
    };
    let mut reader = BufReader::new(pipe);
    let mut out = String::new();
    let mut buf = Vec::new();

    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            // EOF (0 bytes) or a read error both end the stream.
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf);
                if let Some(sender) = &progress {
                    let _ = sender.send(line.trim_end_matches(['\n', '\r']).to_string());
                }
                out.push_str(&line);
            }
        }
    }
    out
}

/// The default coding-agent tool set.
pub fn builtin_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(ReadFileTool),
        Box::new(WriteFileTool),
        Box::new(EditFileTool),
        Box::new(ListDirTool),
        Box::new(SearchTool),
        Box::new(BashTool::new()),
    ]
}
