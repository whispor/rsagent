//! Terminal styling and event rendering for the CLI's output paths.

use std::io::IsTerminal;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use rsagent_agent::AgentEvent;
use rsagent_ai::{AssistantMessage, ErrorReason, Message, StreamEvent};
use serde_json::Value;

/// The user's color preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorChoice {
    /// Colorize when stdout is a terminal and `NO_COLOR` is unset.
    Auto,
    /// Never colorize.
    Never,
}

/// How a run's output is split between the answer and its progress chatter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputMode {
    /// Interactive REPL: progress interleaves with the answer on stdout, and a blank
    /// line separates one turn's output from the next prompt.
    Interactive,
    /// One-shot `--print`: the answer owns stdout while progress (tool activity,
    /// approval prompts) goes to stderr, so redirected output is exactly the answer.
    OneShot,
    /// `--json`: every event is written to stdout as one JSON object per line, and the
    /// human-facing progress chatter is suppressed. For CI and scripts.
    Json,
}

/// ANSI styling that degrades to plain text.
///
/// Color is disabled by `--no-color`, by a non-empty `NO_COLOR` environment variable,
/// or when stdout is not a terminal, so redirected output and log files stay clean.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Palette {
    enabled: bool,
}

impl Palette {
    /// Decide whether to colorize output.
    pub(crate) fn detect(choice: ColorChoice) -> Self {
        let no_color_env = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
        let want_color = choice == ColorChoice::Auto && !no_color_env;
        Palette {
            enabled: want_color && std::io::stdout().is_terminal(),
        }
    }

    /// Wrap `text` in the SGR `code`, or return it unchanged when color is disabled.
    pub(crate) fn paint(self, code: &str, text: &str) -> String {
        if self.enabled {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    /// Dim (faint) text, used for secondary status lines.
    pub(crate) fn dim(self, text: &str) -> String {
        self.paint("2", text)
    }

    /// Yellow text, used for prompts and warnings.
    pub(crate) fn yellow(self, text: &str) -> String {
        self.paint("33", text)
    }

    /// Red text, used for errors.
    pub(crate) fn red(self, text: &str) -> String {
        self.paint("31", text)
    }

    /// Green text, used for successful tool results.
    pub(crate) fn green(self, text: &str) -> String {
        self.paint("32", text)
    }

    /// Cyan text, used for tool-call headers.
    pub(crate) fn cyan(self, text: &str) -> String {
        self.paint("36", text)
    }
}

/// Prints agent events to stdout as they arrive.
pub(crate) struct Renderer {
    /// True while a streamed line is open (no trailing newline yet).
    line_open: bool,
    /// True while the open line holds reasoning text, which is rendered dim.
    thinking_open: bool,
    /// Working directory used to explain file operations in plain language.
    cwd: PathBuf,
    /// Styling that honors `--no-color` / `NO_COLOR`.
    pub(crate) palette: Palette,
    /// Where progress goes and whether a trailing blank line is wanted.
    mode: OutputMode,
    /// When set, emit extra diagnostics on stderr (`--verbose`).
    verbose: bool,
    /// The current turn number, for verbose timing.
    turn: usize,
    /// When the current turn started, for verbose timing.
    turn_started: Option<Instant>,
}

impl Renderer {
    pub(crate) fn new(cwd: PathBuf, palette: Palette) -> Self {
        Renderer {
            line_open: false,
            thinking_open: false,
            cwd,
            palette,
            mode: OutputMode::Interactive,
            verbose: false,
            turn: 0,
            turn_started: None,
        }
    }

    /// Choose how progress is separated from the answer (see [`OutputMode`]).
    #[must_use]
    pub(crate) fn with_mode(mut self, mode: OutputMode) -> Self {
        self.mode = mode;
        self
    }

    /// Emit extra diagnostics on stderr (`--verbose`).
    #[must_use]
    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "the verbosity flag is naturally boolean; a two-variant enum would add ceremony without clarity"
    )]
    pub(crate) fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    pub(crate) fn render(&mut self, event: &AgentEvent) {
        if self.verbose {
            self.render_verbose(event);
        }
        if self.mode == OutputMode::Json {
            self.render_json(event);
            return;
        }
        match event {
            AgentEvent::MessageUpdate { event } => match event {
                StreamEvent::ThinkingDelta { delta, .. } => self.render_thinking_delta(delta),
                StreamEvent::TextDelta { delta, .. } => self.render_text_delta(delta),
                StreamEvent::Error { error, reason } => self.render_stream_error(error, *reason),
                _ => {}
            },
            AgentEvent::TurnEnd {
                message: Message::Assistant(assistant),
                ..
            } => {
                let usage = &assistant.usage;
                if usage.input > 0 || usage.output > 0 {
                    self.flush_line();
                    eprintln!(
                        "{}",
                        self.palette.dim(&format!(
                            "· tokens: {} in / {} out (total {})",
                            usage.input,
                            usage.output,
                            usage.total_tokens()
                        ))
                    );
                }
            }
            AgentEvent::ToolApprovalRequest {
                tool_name, args, ..
            } => {
                self.flush_line();
                self.print_approval_prompt(tool_name, args);
            }
            AgentEvent::ToolExecutionStart {
                tool_name, args, ..
            } => {
                self.flush_line();
                self.progress_line(
                    &self
                        .palette
                        .cyan(&format!("⚙ {}", describe_action(tool_name, args))),
                );
            }
            AgentEvent::ToolExecutionUpdate { content, .. } => {
                self.flush_line();
                self.progress_line(&self.palette.dim(&format!("  │ {}", truncate(content, 200))));
            }
            AgentEvent::ToolExecutionEnd {
                tool_name,
                is_error,
                content,
                ..
            } => {
                self.flush_line();
                let marker = if *is_error { "✗" } else { "✓" };
                let label = format!("{marker} {}：", tool_display_name(tool_name));
                let painted = if *is_error {
                    self.palette.red(&label)
                } else {
                    self.palette.green(&label)
                };
                self.progress_line(&format!("{painted}{}", truncate(content, 200)));
            }
            AgentEvent::ContextCompacted { dropped_messages } => {
                self.flush_line();
                eprintln!(
                    "{}",
                    self.palette.dim(&format!(
                        "· 上下文接近上限，已裁剪较早的 {dropped_messages} 条消息"
                    ))
                );
            }
            AgentEvent::SourceUnhealthy {
                source_id,
                kind,
                reason,
            } => {
                self.flush_line();
                eprintln!(
                    "{}",
                    self.palette.yellow(&format!(
                        "· 工具来源 `{source_id}`（{kind}）不可用：{reason}"
                    ))
                );
            }
            _ => {}
        }
    }

    /// Emit extra diagnostics on stderr when `--verbose` is set.
    ///
    /// Diagnostics always go to stderr, so they never contaminate the answer on stdout
    /// or the JSON stream in `--json` mode.
    fn render_verbose(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::TurnStart => {
                self.turn += 1;
                self.turn_started = Some(Instant::now());
                eprintln!(
                    "{}",
                    self.palette
                        .dim(&format!("· [verbose] turn {} start", self.turn))
                );
            }
            AgentEvent::TurnEnd { .. } => {
                if let Some(started) = self.turn_started.take() {
                    eprintln!(
                        "{}",
                        self.palette.dim(&format!(
                            "· [verbose] turn {} done in {} ms",
                            self.turn,
                            started.elapsed().as_millis()
                        ))
                    );
                }
            }
            AgentEvent::ToolExecutionStart {
                tool_name, args, ..
            } => {
                eprintln!(
                    "{}",
                    self.palette.dim(&format!(
                        "· [verbose] call {tool_name} {}",
                        truncate(&args.to_string(), 200)
                    ))
                );
            }
            _ => {}
        }
    }

    /// Emit one event as a single JSON line on stdout.
    ///
    /// The event types contain only strings, numbers, and already-serializable messages,
    /// so serialization does not fail in practice; if it ever did, the problem is
    /// reported on stderr rather than panicking, so a consumer always sees well-formed
    /// JSON lines and can still detect the failure from the exit code.
    fn render_json(&self, event: &AgentEvent) {
        match serde_json::to_string(event) {
            Ok(line) => println!("{line}"),
            Err(error) => eprintln!(
                "{}",
                self.palette
                    .red(&format!("· could not serialize event: {error}"))
            ),
        }
    }

    /// Append a reasoning delta, starting a fresh line if an answer was in progress.
    ///
    /// Reasoning is progress, not the answer, so in a one-shot `--print` run it goes to
    /// stderr — `--print --thinking high "…" > answer.md` leaves `answer.md` holding the
    /// answer alone. The REPL keeps it on stdout, interleaved with the answer.
    fn render_thinking_delta(&mut self, delta: &str) {
        if self.line_open && !self.thinking_open {
            self.flush_line();
        }
        let text = self.palette.dim(delta);
        self.write_thinking(&text);
        self.line_open = true;
        self.thinking_open = true;
    }

    /// Write reasoning output to the stream that matches the output mode: stderr for a
    /// one-shot `--print` run (keeping stdout to the answer), stdout otherwise.
    fn write_thinking(&self, text: &str) {
        if self.mode == OutputMode::OneShot {
            eprint!("{text}");
            std::io::Write::flush(&mut std::io::stderr()).ok();
        } else {
            print!("{text}");
            std::io::Write::flush(&mut std::io::stdout()).ok();
        }
    }

    /// Append an answer delta, starting a fresh line if reasoning was in progress.
    fn render_text_delta(&mut self, delta: &str) {
        if self.thinking_open {
            self.flush_line();
        }
        print!("{delta}");
        std::io::Write::flush(&mut std::io::stdout()).ok();
        self.line_open = true;
    }

    /// Report a terminal stream error (or an abort) on stderr.
    fn render_stream_error(&mut self, error: &AssistantMessage, reason: ErrorReason) {
        self.flush_line();
        let label = match reason {
            ErrorReason::Aborted => "aborted",
            _ => "error",
        };
        let detail = error.error_message.as_deref().unwrap_or("unknown error");
        eprintln!(
            "{} {detail}",
            self.palette.red(&format!("✗ model {label}:"))
        );
    }

    /// Print the plain-language approval prompt for a tool call.
    fn print_approval_prompt(&self, tool_name: &str, args: &Value) {
        let mut lines = self.describe_approval(tool_name, args).into_iter();
        let title = lines
            .next()
            .unwrap_or_else(|| format!("调用工具「{tool_name}」"));
        self.progress_line(&self.palette.yellow("? 需要你确认"));
        self.progress_line(&self.palette.yellow(&format!("  操作：{title}")));
        for line in lines {
            self.progress_line(&self.palette.yellow(&format!("  {line}")));
        }
        self.progress_line(&self.palette.yellow("  → 输入 y 再回车 = 只允许这一次"));
        self.progress_line(
            &self
                .palette
                .yellow("  → 输入 a 再回车 = 允许，且本次会话后续操作都不再询问"),
        );
        self.progress_line(
            &self
                .palette
                .yellow("  → 直接回车 = 拒绝（我会换个方式继续）"),
        );
    }

    /// A plain-language explanation of what an approval request will do.
    ///
    /// The first line is the operation title; the rest are detail lines. Written for
    /// users who do not read shell commands or raw JSON.
    fn describe_approval(&self, tool_name: &str, args: &Value) -> Vec<String> {
        match tool_name {
            "bash" => vec![
                format!("执行一条系统命令：{}", arg_str(args, "command")),
                "说明：这会在你的电脑上运行这条命令，可能读取、修改或删除文件。请确认它安全。"
                    .to_string(),
            ],
            "write_file" => {
                let path = arg_str(args, "path");
                let bytes = args
                    .get("content")
                    .and_then(Value::as_str)
                    .map_or(0, str::len);
                let target = if self.cwd.join(&path).exists() {
                    format!("覆盖已有文件：{path}（原有内容会被替换）")
                } else {
                    format!("新建文件：{path}")
                };
                let mut lines = vec![target, format!("说明：将写入约 {bytes} 字节内容。")];
                if looks_outside_cwd(&self.cwd, &path) {
                    lines.push(outside_warning(&self.cwd));
                }
                lines
            }
            "edit_file" => {
                let path = arg_str(args, "path");
                let warning = if self.cwd.join(&path).exists() {
                    String::new()
                } else {
                    "（注意：该文件当前不存在，可能失败）".to_string()
                };
                let mut lines = vec![
                    format!("修改文件：{path}{warning}"),
                    format!(
                        "改动：把 {} 替换为 {}",
                        quote(&arg_str(args, "old_string")),
                        quote(&arg_str(args, "new_string"))
                    ),
                    "说明：只替换这一处文字，文件其它内容保持不变。".to_string(),
                ];
                if looks_outside_cwd(&self.cwd, &path) {
                    lines.push(outside_warning(&self.cwd));
                }
                lines
            }
            "webfetch" => vec![
                format!("抓取网页：{}", arg_str(args, "url")),
                "说明：这会向该网址发起网络请求，对方会看到这次访问。请确认网址可信。".to_string(),
            ],
            other => vec![
                format!("调用工具「{other}」"),
                format!("参数：{}", compact(args)),
            ],
        }
    }

    pub(crate) fn flush_line(&mut self) {
        if self.line_open {
            // Close the line on the stream it was opened on: a reasoning line lives on
            // stderr in `--print` mode, an answer line always on stdout.
            if self.thinking_open {
                self.write_thinking("\n");
            } else {
                println!();
            }
            self.line_open = false;
        }
        self.thinking_open = false;
    }

    /// Write one line of progress (tool activity, approval prompt) to the stream that
    /// matches the output mode: stdout when interactive, stderr for one-shot runs.
    fn progress_line(&self, text: &str) {
        match self.mode {
            OutputMode::Interactive => println!("{text}"),
            OutputMode::OneShot => eprintln!("{text}"),
            // The JSON stream already carries every event; a human progress line would
            // only corrupt it. (Unreachable today — `render` returns before this — but
            // kept explicit so the mode stays exhaustive.)
            OutputMode::Json => {}
        }
    }

    /// Close out one prompt's output.
    ///
    /// The REPL leaves a blank line before the next prompt; a one-shot `--print` run
    /// keeps stdout to the answer alone, so it only ends the current line.
    pub(crate) fn finish_prompt(&self) {
        if self.mode == OutputMode::Interactive {
            println!();
        }
    }
}

/// A one-line, plain-language summary of a tool call for the progress log.
fn describe_action(tool_name: &str, args: &Value) -> String {
    match tool_name {
        "read_file" => format!("读取文件：{}", arg_str(args, "path")),
        "list_dir" => format!(
            "查看目录：{}",
            args.get("path").and_then(Value::as_str).unwrap_or(".")
        ),
        "write_file" => format!("写入文件：{}", arg_str(args, "path")),
        "edit_file" => format!("修改文件：{}", arg_str(args, "path")),
        "bash" => format!("执行命令：{}", arg_str(args, "command")),
        "webfetch" => format!("抓取网页：{}", arg_str(args, "url")),
        other => format!("{other} {}", compact(args)),
    }
}

/// A Chinese display name for a tool, used in result lines.
fn tool_display_name(tool_name: &str) -> &str {
    match tool_name {
        "read_file" => "读取文件",
        "write_file" => "写入文件",
        "edit_file" => "修改文件",
        "list_dir" => "查看目录",
        "bash" => "执行命令",
        "webfetch" => "抓取网页",
        other => other,
    }
}

/// Read a string argument, falling back to a placeholder when it is missing.
fn arg_str(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .unwrap_or("（未提供）")
        .to_string()
}

/// Wrap a snippet in Chinese quotes, flattened to one line and truncated.
fn quote(text: &str) -> String {
    format!("「{}」", truncate(text, 60))
}

fn compact(value: &Value) -> String {
    let rendered = serde_json::to_string(value).unwrap_or_default();
    truncate(&rendered, 120)
}

pub(crate) fn truncate(text: &str, max: usize) -> String {
    let single_line = text.replace('\n', " ");
    if single_line.chars().count() <= max {
        single_line
    } else {
        let truncated: String = single_line.chars().take(max).collect();
        format!("{truncated}…")
    }
}

/// The warning line shown when a mutating tool targets a path outside the working directory.
fn outside_warning(cwd: &Path) -> String {
    format!(
        "⚠ 该路径在工作目录之外：{}（请确认你确实要改动这里）",
        cwd.display()
    )
}

/// Whether `path` lexically points outside `cwd`, without touching the filesystem.
///
/// This is only a best-effort hint for the approval prompt; the read-tool fence is what
/// enforces the boundary, by canonicalizing. A relative path counts as outside when it
/// climbs above the working directory with `..`; an absolute path counts as outside when
/// it is not already under `cwd`.
fn looks_outside_cwd(cwd: &Path, path: &str) -> bool {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return !candidate.starts_with(cwd);
    }
    let mut depth: i32 = 0;
    for component in candidate.components() {
        match component {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            Component::Normal(_) => depth += 1,
            _ => {}
        }
    }
    false
}
