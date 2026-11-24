//! Built-in tools: the filesystem and shell primitives a coding agent needs.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use rsagent_ai::ToolSpec;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc::UnboundedSender;

use crate::approval::Permission;
use crate::glob::glob_match;
use crate::html::html_to_text;
use crate::memory::DEFAULT_RECALL_LIMIT;
use crate::platform;
use crate::sources::http::read_body;
use crate::sources::subprocess::{drain_within, MAX_OUTPUT_BYTES};
use crate::tool::{Tool, ToolContext, ToolResult};

/// Directory names that recursive tools never descend into.
///
/// These hold build output or vendored dependencies; searching them buries the real
/// matches under noise (a single `target/` can dwarf an entire source tree). The
/// directory the user explicitly points at is still searched.
const SKIPPED_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
];

fn is_skipped_dir(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| SKIPPED_DIRS.contains(&name))
}

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

/// A string argument, or `default` when absent.
fn opt_str(args: &Value, key: &str, default: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

/// A non-negative count argument (e.g. `max_results`), or `default` when absent.
fn opt_count(args: &Value, key: &str, default: usize) -> usize {
    args.get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(default)
}

/// One directory's immediate children, split into subdirectories and files.
#[derive(Debug)]
struct Children {
    dirs: Vec<PathBuf>,
    files: Vec<PathBuf>,
}

/// Read a directory's immediate children, skipping build/vendor subdirectories.
///
/// Shared by `search` and `glob` so the skip policy and entry classification live in
/// one place. The directory passed in is always read; only its children are filtered.
async fn read_children(dir: &Path) -> Result<Children, String> {
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .map_err(|error| format!("failed to read {}: {error}", dir.display()))?;

    let mut children = Children {
        dirs: Vec::new(),
        files: Vec::new(),
    };
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| format!("failed to read entry: {error}"))?
    {
        let file_type = entry.file_type().await.ok();
        let path = entry.path();
        if matches!(file_type.as_ref(), Some(t) if t.is_dir()) {
            if !is_skipped_dir(&entry.file_name()) {
                children.dirs.push(path);
            }
        } else if matches!(file_type.as_ref(), Some(t) if t.is_file()) {
            children.files.push(path);
        }
    }
    Ok(children)
}

/// Read a file's contents as UTF-8 text, with line numbers.
///
/// Each line is prefixed with its 1-based line number, so the model can refer to exact
/// locations and match `edit_file` snippets reliably. `offset` and `limit` read a range
/// of a large file instead of pulling the whole thing into context.
#[derive(Debug)]
pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "read_file",
            "Read a file as UTF-8 text; every line is prefixed with its line number. Use `offset` and `limit` to read a range of a large file. The path may be absolute or relative to the working directory.",
            object_schema(
                &json!({
                    "path": { "type": "string", "description": "File path" },
                    "offset": { "type": "integer", "description": "First line to read, 1-based (default 1)" },
                    "limit": { "type": "integer", "description": "Maximum number of lines to read (default: to end of file)" },
                }),
                &["path"],
            ),
        )
    }

    fn label(&self) -> String {
        "Read File".to_string()
    }

    fn permission(&self) -> Permission {
        Permission::Read
    }

    async fn execute(
        &self,
        _call_id: &str,
        args: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, String> {
        let path = required_str(&args, "path")?;
        let resolved = context.resolve_within(&path).await?;
        let data = tokio::fs::read_to_string(&resolved)
            .await
            .map_err(|error| format!("failed to read {}: {error}", resolved.display()))?;

        // Split on `\n` but keep any `\r`, so the returned text is byte-for-byte the
        // file's lines. `str::lines()` would strip the `\r`, and then a model that reads a
        // CRLF file and pastes a line back into `edit_file` (an exact-match tool) would
        // always miss (ACTIONS.md A3).
        let lines: Vec<&str> = data
            .split_inclusive('\n')
            .map(|line| line.strip_suffix('\n').unwrap_or(line))
            .collect();
        let total = lines.len();
        if total == 0 {
            return Ok(ToolResult::text(format!(
                "(empty file: {})",
                resolved.display()
            )));
        }

        let offset = args
            .get("offset")
            .and_then(Value::as_u64)
            .map_or(1, |value| usize::try_from(value).unwrap_or(1).max(1));
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok());
        let start = offset.saturating_sub(1).min(total);
        if start >= total {
            return Ok(ToolResult::text(format!(
                "(offset {offset} is past the end of {}: {total} line(s))",
                resolved.display()
            )));
        }
        let end = match limit {
            Some(limit) => start.saturating_add(limit).min(total),
            None => total,
        };

        // Right-align the numbers to the width of the largest one in the slice.
        let width = end.to_string().len();
        let mut out = String::new();
        for (index, line) in lines[start..end].iter().enumerate() {
            let number = start + index + 1;
            let _ = writeln!(out, "{number:>width$}\t{line}");
        }
        if start > 0 || end < total {
            let first = start + 1;
            let _ = writeln!(out, "\n… (showing lines {first}-{end} of {total})");
        }
        Ok(ToolResult::text(out))
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

    fn permission(&self) -> Permission {
        Permission::Write
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
            "Replace an exact string in a file with a new string. Fails if the old string is not found, or if it appears more than once and `replace_all` is not set — in that case include more surrounding context to make it unique.",
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

    fn permission(&self) -> Permission {
        Permission::Write
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

        let occurrences = original.matches(&old_string).count();
        if occurrences == 0 {
            return Err(format!("old_string not found in {}", resolved.display()));
        }
        // An ambiguous match almost always means the model meant a different site, so
        // refuse rather than silently editing the first one. `replace_all` opts in.
        if occurrences > 1 && !replace_all {
            return Err(format!(
                "old_string occurs {occurrences} times in {}; include more surrounding context to make it unique, or pass replace_all to replace every occurrence",
                resolved.display()
            ));
        }

        let (updated, count) = if replace_all {
            (original.replace(&old_string, &new_string), occurrences)
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

    fn permission(&self) -> Permission {
        Permission::Read
    }

    async fn execute(
        &self,
        _call_id: &str,
        args: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, String> {
        let path = opt_str(&args, "path", ".");
        let resolved = context.resolve_within(&path).await?;

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
/// are reported as `path:line: text`, mirroring `grep -rn`. Build-output and vendor
/// directories (`SKIPPED_DIRS`) are not descended into.
#[derive(Debug)]
pub struct SearchTool;

/// Files larger than this are skipped by [`SearchTool`] rather than read into memory.
///
/// The scan needs a whole file in memory to walk its lines, and the 32K *output* cap
/// truncates only *after* that read — so without a read-side limit a single huge file sets
/// the process's memory peak. 8 MiB is far above any real source file, so ordinary searches
/// are unaffected.
const MAX_SEARCH_FILE_BYTES: u64 = 8 * 1024 * 1024;

#[async_trait]
impl Tool for SearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "search",
            "Search for a literal substring in files under a directory, reporting matching lines as `path:line: text`. Skips build and vendor directories (target, node_modules, .git, ...). Read-only.",
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

    fn permission(&self) -> Permission {
        Permission::Read
    }

    async fn execute(
        &self,
        _call_id: &str,
        args: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, String> {
        let pattern = required_str(&args, "pattern")?;
        let path = opt_str(&args, "path", ".");
        let max_results = opt_count(&args, "max_results", 100);
        let root = context.resolve_within(&path).await?;

        // An explicit stack avoids async recursion (which would need boxing) and lets us
        // stop the moment `max_results` is reached.
        let mut stack = vec![root.clone()];
        let mut matches: Vec<String> = Vec::new();
        let mut skipped: Vec<PathBuf> = Vec::new();

        'walk: while let Some(dir) = stack.pop() {
            let children = read_children(&dir).await?;
            stack.extend(children.dirs);
            for entry_path in children.files {
                // Bound the read: skip a file too large to hold in memory rather than
                // buffering it whole. `metadata` is cheap next to the read it avoids.
                if tokio::fs::metadata(&entry_path)
                    .await
                    .is_ok_and(|meta| meta.len() > MAX_SEARCH_FILE_BYTES)
                {
                    skipped.push(entry_path);
                    continue;
                }
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

        if !skipped.is_empty() {
            let names = skipped
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            matches.push(format!(
                "(skipped {} file(s) larger than {} MiB: {names})",
                skipped.len(),
                MAX_SEARCH_FILE_BYTES / (1024 * 1024)
            ));
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

/// Find files whose path matches a glob pattern.
///
/// Read-only, so it needs no approval. The syntax is small on purpose: `*` (within a
/// segment), `?` (one character), and `**` (any number of segments, e.g. `**/*.rs`).
/// A pattern without a `/` matches file names anywhere in the tree. Build-output and
/// vendor directories (`SKIPPED_DIRS`) are not descended into.
#[derive(Debug)]
pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "glob",
            "Find files whose path matches a glob pattern, one path per line. Supports `*` (within a segment), `?` (one character), and `**` (any number of segments, e.g. `**/*.rs`). A pattern without a `/` matches file names. Read-only.",
            object_schema(
                &json!({
                    "pattern": { "type": "string", "description": "Glob pattern, e.g. `**/*.rs`" },
                    "path": { "type": "string", "description": "Directory to search (default \".\")" },
                    "max_results": { "type": "integer", "description": "Stop after this many matches (default 100)" },
                }),
                &["pattern"],
            ),
        )
    }

    fn label(&self) -> String {
        "Glob".to_string()
    }

    fn permission(&self) -> Permission {
        Permission::Read
    }

    async fn execute(
        &self,
        _call_id: &str,
        args: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, String> {
        let pattern = required_str(&args, "pattern")?;
        let path = opt_str(&args, "path", ".");
        let max_results = opt_count(&args, "max_results", 100);
        let root = context.resolve_within(&path).await?;

        // Same explicit-stack walk as `search`: no async recursion, and it stops the
        // moment enough matches are found.
        let mut stack = vec![root.clone()];
        let mut matches: Vec<String> = Vec::new();

        'walk: while let Some(dir) = stack.pop() {
            let children = read_children(&dir).await?;
            stack.extend(children.dirs);
            for entry_path in children.files {
                let relative = entry_path.strip_prefix(&root).unwrap_or(&entry_path);
                let candidate = relative.to_string_lossy().replace('\\', "/");
                if glob_match(&pattern, &candidate) {
                    matches.push(entry_path.display().to_string());
                    if matches.len() >= max_results {
                        break 'walk;
                    }
                }
            }
        }

        if matches.is_empty() {
            return Ok(ToolResult::text(format!(
                "No files match `{pattern}` under {}",
                root.display()
            )));
        }
        matches.sort();
        Ok(ToolResult::text(matches.join("\n")))
    }
}

/// Fetch a URL over HTTP(S) and return its content as text.
///
/// HTML bodies are reduced to plain text; anything else is returned as-is. This is the
/// only tool that sends a request to an arbitrary external host — a network egress path
/// that would otherwise bypass the `bash` approval gate — so it requires approval, and
/// the prompt shows the exact URL.
#[derive(Debug)]
pub struct WebFetchTool {
    client: reqwest::Client,
    max_bytes: usize,
}

impl Default for WebFetchTool {
    fn default() -> Self {
        let client = reqwest::Client::builder()
            .user_agent(concat!("rsagent/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        WebFetchTool {
            client,
            max_bytes: 1_048_576,
        }
    }
}

impl WebFetchTool {
    /// Create a fetch tool with the default client and size cap.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "webfetch",
            "Fetch a URL over HTTP(S) and return its content as text. HTML is converted to plain text. Requires approval.",
            object_schema(
                &json!({
                    "url": { "type": "string", "description": "Absolute http(s) URL to fetch" },
                }),
                &["url"],
            ),
        )
    }

    fn label(&self) -> String {
        "Fetch".to_string()
    }

    fn permission(&self) -> Permission {
        Permission::Network
    }

    fn requires_approval(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _call_id: &str,
        args: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, String> {
        let raw = required_str(&args, "url")?;
        let url =
            reqwest::Url::parse(&raw).map_err(|error| format!("invalid URL `{raw}`: {error}"))?;
        match url.scheme() {
            "http" | "https" => {}
            other => {
                return Err(format!(
                    "unsupported URL scheme `{other}` (only http and https are allowed)"
                ))
            }
        }

        let response = self
            .client
            .get(url.clone())
            .send()
            .await
            .map_err(|error| format!("request to {url} failed: {error}"))?;

        let status = response.status();
        let is_html = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("html"));

        let (body, _truncated) = read_body(response, self.max_bytes).await?;
        if !status.is_success() {
            let preview: String = body.chars().take(2_000).collect();
            return Err(format!("HTTP {status} from {url}\n{preview}"));
        }

        let text = if is_html { html_to_text(&body) } else { body };
        if text.trim().is_empty() {
            return Ok(ToolResult::text(format!("(empty body from {url})")));
        }
        Ok(ToolResult::text(text))
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

    fn permission(&self) -> Permission {
        Permission::Exec
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

        let mut child = platform::shell_command(&command)
            .current_dir(&context.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("failed to spawn command: {error}"))?;

        // Drain each pipe on its own task so a full buffer cannot deadlock the wait,
        // and so lines can be streamed as progress while the command runs. Output also
        // lands in a shared sink, so a bounded drain below still yields what arrived.
        let stdout_sink = Arc::new(StdMutex::new(TextBuffer::default()));
        let stderr_sink = Arc::new(StdMutex::new(TextBuffer::default()));
        let mut stdout_task = tokio::spawn(stream_pipe(
            child.stdout.take(),
            context.progress().cloned(),
            Arc::clone(&stdout_sink),
        ));
        let mut stderr_task = tokio::spawn(stream_pipe(
            child.stderr.take(),
            context.progress().cloned(),
            Arc::clone(&stderr_sink),
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
                platform::kill_tree(&mut child).await;
                return Err("command cancelled".to_string());
            }
            BashExit::TimedOut => {
                platform::kill_tree(&mut child).await;
                return Err(format!(
                    "command timed out after {}s and was killed",
                    timeout.as_secs()
                ));
            }
        };

        // A grandchild that inherited the pipes can hold them open forever; stop waiting
        // after a short grace so a backgrounded daemon cannot hang the turn.
        drain_within(&mut stdout_task).await;
        drain_within(&mut stderr_task).await;
        let (stdout, stdout_capped) = take_sink(&stdout_sink);
        let (stderr, stderr_capped) = take_sink(&stderr_sink);
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
        // The model sees the output truncated to its context budget anyway; say so when the
        // capture itself was cut, rather than implying the command printed no more.
        if stdout_capped || stderr_capped {
            if !combined.is_empty() && !combined.ends_with('\n') {
                combined.push('\n');
            }
            let _ = write!(
                combined,
                "… (output truncated at {} MiB)",
                MAX_OUTPUT_BYTES / (1024 * 1024)
            );
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

/// A shared text buffer that stops growing at [`MAX_OUTPUT_BYTES`].
///
/// The stream keeps reading past the cap — so a chatty command never blocks on a full pipe —
/// but stores nothing more, and records that it did so.
#[derive(Default)]
struct TextBuffer {
    text: String,
    truncated: bool,
}

/// Read a pipe to the end, streaming each line to `progress` and appending it to `sink`.
///
/// Lines are decoded only once a full line (ending in `\n`, or the final partial at EOF)
/// has been read, so a multi-byte character is never split. Storage stops at
/// [`MAX_OUTPUT_BYTES`], but the pipe is read to the end regardless.
async fn stream_pipe<R>(
    pipe: Option<R>,
    progress: Option<UnboundedSender<String>>,
    sink: Arc<StdMutex<TextBuffer>>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let Some(pipe) = pipe else {
        return;
    };
    let mut reader = BufReader::new(pipe);
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
                if let Ok(mut buffer) = sink.lock() {
                    let room = MAX_OUTPUT_BYTES.saturating_sub(buffer.text.len());
                    if line.len() > room {
                        // Never cut mid-character: back the boundary up to the previous one.
                        let mut cut = room;
                        while cut > 0 && !line.is_char_boundary(cut) {
                            cut -= 1;
                        }
                        buffer.text.push_str(&line[..cut]);
                        buffer.truncated = true;
                    } else {
                        buffer.text.push_str(&line);
                    }
                }
            }
        }
    }
}

/// Snapshot a shared output sink, tolerating a poisoned lock. The bool is whether the
/// buffer hit [`MAX_OUTPUT_BYTES`].
fn take_sink(sink: &Arc<StdMutex<TextBuffer>>) -> (String, bool) {
    let buffer = sink
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    (buffer.text.clone(), buffer.truncated)
}

/// The error the memory tools return when the agent has no memory attached.
fn memory_disabled() -> String {
    "memory is not enabled; start rsagent with `--memory <FILE>` to use it".to_string()
}

/// Record a durable note in long-term memory.
///
/// The note survives the session: the newest notes are folded into the system prompt of
/// every later request, so a fact learned once need not be re-derived. This is the only
/// tool that writes memory; it needs no approval because it touches only the memory file,
/// never the working tree.
#[derive(Debug)]
pub struct RememberTool;

#[async_trait]
impl Tool for RememberTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "remember",
            "Record a durable note in long-term memory — a fact, a preference, or a lesson worth keeping across sessions. The newest notes are shown to you at the start of each request. Requires memory to be enabled (`--memory`).",
            object_schema(
                &json!({ "text": { "type": "string", "description": "The note to remember" } }),
                &["text"],
            ),
        )
    }

    fn label(&self) -> String {
        "Remember".to_string()
    }

    fn permission(&self) -> Permission {
        Permission::Write
    }

    fn is_sequential(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _call_id: &str,
        args: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, String> {
        let text = required_str(&args, "text")?;
        let memory = context.memory().ok_or_else(memory_disabled)?;
        memory.append(&text)?;
        Ok(ToolResult::text(
            "Remembered. It will be shown at the start of future requests.",
        ))
    }
}

/// Read back the most recent notes from long-term memory.
///
/// Read-only, so it needs no approval. The output mirrors what is folded into the system
/// prompt, so the model sees the same view whether it recalls or the prompt injects.
#[derive(Debug)]
pub struct RecallTool;

#[async_trait]
impl Tool for RecallTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "recall",
            "Read back the most recent notes from long-term memory, oldest first. Requires memory to be enabled (`--memory`).",
            object_schema(
                &json!({ "limit": { "type": "integer", "description": "How many notes to return (default 20)" } }),
                &[],
            ),
        )
    }

    fn label(&self) -> String {
        "Recall".to_string()
    }

    fn permission(&self) -> Permission {
        Permission::Read
    }

    async fn execute(
        &self,
        _call_id: &str,
        args: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, String> {
        let limit = opt_count(&args, "limit", DEFAULT_RECALL_LIMIT);
        let memory = context.memory().ok_or_else(memory_disabled)?;
        let rendered = memory.render(limit);
        if rendered.is_empty() {
            return Ok(ToolResult::text("(no notes in memory)"));
        }
        Ok(ToolResult::text(rendered))
    }
}

/// The long-term-memory tools, offered only when memory is enabled.
///
/// Kept out of [`builtin_tools`] so a run without `--memory` never advertises tools that
/// would only fail.
pub fn memory_tools() -> Vec<Box<dyn Tool>> {
    vec![Box::new(RememberTool), Box::new(RecallTool)]
}

/// The default coding-agent tool set.
pub fn builtin_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(ReadFileTool),
        Box::new(WriteFileTool),
        Box::new(EditFileTool),
        Box::new(ListDirTool),
        Box::new(SearchTool),
        Box::new(GlobTool),
        Box::new(WebFetchTool::new()),
        Box::new(BashTool::new()),
    ]
}

/// Select built-in tools by name.
///
/// When `only` is `Some`, just the listed tools are kept; names in `exclude` are always
/// dropped afterwards. Returns the selected tools and the requested names (from `only`)
/// that matched no built-in tool, so the caller can warn about typos.
pub fn select_builtin_tools(
    only: Option<&[String]>,
    exclude: &[&str],
) -> (Vec<Box<dyn Tool>>, Vec<String>) {
    let mut tools = builtin_tools();
    let mut unknown = Vec::new();
    if let Some(only) = only {
        for name in only {
            if !tools.iter().any(|tool| tool.spec().name == *name) {
                unknown.push(name.clone());
            }
        }
        tools.retain(|tool| only.iter().any(|name| *name == tool.spec().name));
    }
    if !exclude.is_empty() {
        tools.retain(|tool| !exclude.contains(&tool.spec().name.as_str()));
    }
    (tools, unknown)
}

#[cfg(test)]
mod tests {
    use super::{builtin_tools, select_builtin_tools};

    fn names(tools: &[Box<dyn super::Tool>]) -> Vec<String> {
        tools.iter().map(|tool| tool.spec().name).collect()
    }

    #[tokio::test]
    async fn command_output_beyond_the_cap_is_dropped_and_flagged() {
        use super::{stream_pipe, take_sink, TextBuffer, MAX_OUTPUT_BYTES};
        use std::sync::{Arc, Mutex as StdMutex};

        // A reader with more than the cap and no newline: `stream_pipe` reads it as one line
        // at EOF, so the cap must cut it and flag the cut (the read still runs to the end).
        let data = vec![b'a'; MAX_OUTPUT_BYTES + 4096];
        let sink = Arc::new(StdMutex::new(TextBuffer::default()));

        stream_pipe(Some(&data[..]), None, Arc::clone(&sink)).await;

        let (text, truncated) = take_sink(&sink);
        assert!(truncated, "a buffer that hit the cap must be flagged");
        assert_eq!(text.len(), MAX_OUTPUT_BYTES, "storage must stop at the cap");
    }

    #[test]
    fn builtin_tools_are_registered_in_a_stable_order() {
        assert_eq!(
            names(&builtin_tools()),
            vec![
                "read_file",
                "write_file",
                "edit_file",
                "list_dir",
                "search",
                "glob",
                "webfetch",
                "bash",
            ]
        );
    }

    #[test]
    fn builtin_tools_declare_their_permission_category() {
        use super::Permission;

        let pairs: Vec<(String, Permission)> = builtin_tools()
            .iter()
            .map(|tool| (tool.spec().name, tool.permission()))
            .collect();
        let expected = [
            ("read_file", Permission::Read),
            ("write_file", Permission::Write),
            ("edit_file", Permission::Write),
            ("list_dir", Permission::Read),
            ("search", Permission::Read),
            ("glob", Permission::Read),
            ("webfetch", Permission::Network),
            ("bash", Permission::Exec),
        ];
        for (name, permission) in expected {
            let found = pairs
                .iter()
                .find(|(candidate, _)| candidate == name)
                .map(|(_, permission)| *permission);
            assert_eq!(found, Some(permission), "category for {name}");
        }
    }

    #[test]
    fn memory_tools_are_separate_from_the_builtin_set() {
        assert_eq!(names(&super::memory_tools()), vec!["remember", "recall"]);
        assert!(!names(&builtin_tools()).contains(&"remember".to_string()));
    }

    #[test]
    fn memory_tools_declare_their_permission_category() {
        use super::Permission;

        let pairs: Vec<(String, Permission)> = super::memory_tools()
            .iter()
            .map(|tool| (tool.spec().name, tool.permission()))
            .collect();
        assert!(pairs.contains(&("remember".to_string(), Permission::Write)));
        assert!(pairs.contains(&("recall".to_string(), Permission::Read)));
    }

    #[tokio::test]
    async fn memory_tools_error_without_a_memory() {
        use super::{Tool, ToolContext};
        use serde_json::json;

        let context = ToolContext::new(".");
        let error = super::RememberTool
            .execute("c1", json!({ "text": "x" }), &context)
            .await
            .expect_err("no memory is attached");
        assert!(error.contains("--memory"), "error names the flag: {error}");
    }

    #[test]
    fn only_keeps_the_listed_tools() {
        let only = vec!["read_file".to_string(), "glob".to_string()];
        let (tools, unknown) = select_builtin_tools(Some(&only), &[]);
        assert_eq!(names(&tools), vec!["read_file", "glob"]);
        assert_eq!(unknown, Vec::<String>::new());
    }

    #[test]
    fn exclude_drops_a_tool_from_the_full_set() {
        let (tools, unknown) = select_builtin_tools(None, &["bash"]);
        assert_eq!(unknown, Vec::<String>::new());
        assert!(
            !names(&tools).contains(&"bash".to_string()),
            "bash must be dropped"
        );
    }

    #[test]
    fn exclude_wins_over_only() {
        let only = vec!["read_file".to_string(), "bash".to_string()];
        let (tools, _unknown) = select_builtin_tools(Some(&only), &["bash"]);
        assert_eq!(names(&tools), vec!["read_file"]);
    }

    #[test]
    fn unknown_names_are_reported() {
        let only = vec!["read_file".to_string(), "nope".to_string()];
        let (tools, unknown) = select_builtin_tools(Some(&only), &[]);
        assert_eq!(names(&tools), vec!["read_file"]);
        assert_eq!(unknown, vec!["nope".to_string()]);
    }
}
