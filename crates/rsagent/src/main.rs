//! `rsagent` — a minimal coding-agent CLI, in the spirit of pi's `coding-agent` package.

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "this binary is the CLI's output layer; printing to the terminal is its purpose"
)]

mod input;
mod render;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::Parser;
use futures::future::FutureExt;
use rsagent_agent::{builtin_tools, Agent, AgentEvent, Approver};
use rsagent_ai::{
    AnthropicProvider, AssistantMessage, Context, ErrorReason, FauxProvider, FauxResponse, Message,
    Model, Models, OpenAiProvider, StreamEvent, StreamOptions,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use input::InputHandle;
use render::{ColorChoice, Palette};

const DEFAULT_SYSTEM_PROMPT: &str = "\
You are rsagent, a concise coding assistant running in a terminal.
Use the provided tools to read and modify files and to run commands.
Prefer small, focused changes. Explain briefly what you did.";

/// Session files are wrapped in a versioned envelope, so a future format change is
/// detected instead of silently misread.
const SESSION_VERSION: u32 = 1;

/// The session file resumed by `--continue`.
const DEFAULT_SESSION_FILE: &str = ".rsagent-session.json";

#[derive(Parser, Debug)]
#[command(name = "rsagent", version, about = "A minimal Rust coding agent")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a CLI's flags are naturally boolean; grouping them would obscure the interface"
)]
struct Cli {
    /// Model id to use (default: gpt-4o-mini).
    #[arg(short, long, env = "RSAGENT_MODEL", hide_env_values = true)]
    model: Option<String>,

    /// Run a single prompt and exit.
    #[arg(short, long)]
    print: Option<String>,

    /// Base URL of an OpenAI-compatible endpoint.
    #[arg(long, env = "RSAGENT_BASE_URL", hide_env_values = true)]
    base_url: Option<String>,

    /// API key; overrides the provider's environment resolution.
    #[arg(long, env = "RSAGENT_API_KEY", hide_env_values = true)]
    api_key: Option<String>,

    /// Use an offline, scripted provider (no network, no API key).
    #[arg(long)]
    faux: bool,

    /// Talk to an Anthropic Messages API endpoint instead of an OpenAI-compatible one.
    #[arg(long)]
    anthropic: bool,

    /// Working directory the agent operates in.
    #[arg(long, default_value = ".")]
    cwd: PathBuf,

    /// Override the system prompt.
    #[arg(long)]
    system: Option<String>,

    /// Maximum number of model turns in a single run.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..), default_value_t = 50)]
    max_turns: u32,

    /// Sampling temperature (provider default when unset).
    #[arg(long)]
    temperature: Option<f32>,

    /// Maximum tokens to generate per response (provider default when unset).
    #[arg(long)]
    max_tokens: Option<u32>,

    /// Auto-approve every tool call (skip the confirmation prompt).
    #[arg(short = 'y', long)]
    yes: bool,

    /// Persist the conversation to a JSON file, resuming it if it already exists.
    #[arg(long)]
    session: Option<PathBuf>,

    /// Resume the default session file (`.rsagent-session.json`).
    #[arg(long = "continue")]
    continue_: bool,

    /// Disable ANSI colors (also disabled by the `NO_COLOR` environment variable).
    #[arg(long)]
    no_color: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load `.env` before parsing so clap's `env = ...` attributes see it.
    let _ = dotenvy::dotenv();

    let cli = Cli::parse();
    let color = if cli.no_color {
        ColorChoice::Never
    } else {
        ColorChoice::Auto
    };
    let palette = Palette::detect(color);

    let input = InputHandle::spawn();
    let (mut agent, faux_handle, model) = build_agent(&cli, input.clone(), palette);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
    agent.subscribe(tx);

    // `--session` names a file explicitly; `--continue` falls back to the default one.
    let session_path = cli
        .session
        .clone()
        .or_else(|| cli.continue_.then(|| PathBuf::from(DEFAULT_SESSION_FILE)));

    // Resume a persisted transcript, if one exists.
    if let Some(path) = &session_path {
        match load_session(path) {
            Ok(Some(context)) => {
                let count = context.messages.len();
                agent.load_messages(context.messages);
                // The saved system prompt wins unless the user passed `--system`.
                if cli.system.is_none() {
                    if let Some(prompt) = context.system_prompt {
                        agent.set_system_prompt(prompt);
                    }
                }
                eprintln!(
                    "{}",
                    palette.dim(&format!(
                        "· resumed {count} message(s) from {}",
                        path.display()
                    ))
                );
            }
            Ok(None) => {}
            Err(err) => eprintln!(
                "{}",
                palette.yellow(&format!("· could not read {}: {err}", path.display()))
            ),
        }
    }

    let mut renderer = Renderer::new(cli.cwd.clone(), palette);

    if let Some(prompt) = cli.print.clone() {
        if let Some(faux) = &faux_handle {
            faux.set_responses(vec![FauxResponse::text(format!(
                "(faux) you said: {prompt}"
            ))]);
        }
        run_prompt(&mut agent, &mut rx, &mut renderer, prompt).await?;
        save_session(session_path.as_deref(), &agent, palette);
        return Ok(());
    }

    println!(
        "rsagent — model: {} ({}), cwd: {}",
        model.id,
        model.provider,
        cli.cwd.display()
    );
    println!("Type a prompt, or /help for commands ('exit' to quit).\n");

    run_repl(
        &mut agent,
        &mut rx,
        &mut renderer,
        faux_handle.as_ref(),
        &input,
        session_path.as_deref(),
    )
    .await
}

/// Build the model registry, the agent, and (in `--faux` mode) the scripted provider.
fn build_agent(
    cli: &Cli,
    input: InputHandle,
    palette: Palette,
) -> (Agent, Option<FauxProvider>, Model) {
    let default_model = if cli.anthropic {
        "claude-3-5-sonnet-latest"
    } else {
        "gpt-4o-mini"
    };
    let model_id = cli
        .model
        .clone()
        .unwrap_or_else(|| default_model.to_string());
    let system_prompt = cli
        .system
        .clone()
        .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string());

    let mut models = Models::new();
    let faux_handle: Option<FauxProvider> = if cli.faux {
        let faux = FauxProvider::new();
        models.register(faux.clone());
        Some(faux)
    } else if cli.anthropic {
        let base_url = cli
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.anthropic.com".to_string());
        models.register(AnthropicProvider::custom(
            "anthropic",
            base_url,
            model_id.clone(),
            vec!["ANTHROPIC_API_KEY".to_string()],
        ));
        None
    } else {
        let base_url = cli
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        models.register(OpenAiProvider::custom(
            "openai",
            base_url,
            model_id.clone(),
            vec!["OPENAI_API_KEY".to_string()],
        ));
        None
    };

    let provider_id = if cli.faux {
        "faux"
    } else if cli.anthropic {
        "anthropic"
    } else {
        "openai"
    };
    let model_key = if cli.faux {
        "faux-1"
    } else {
        model_id.as_str()
    };
    let model = models
        .get_model(provider_id, model_key)
        .expect("requested model is registered");

    // Interactive sessions ask before running mutating tools; one-shot `--print`
    // denies them unless `--yes` is given, so automation is never surprised.
    let approval = if cli.yes {
        ApprovalMode::Auto
    } else if cli.print.is_some() {
        ApprovalMode::Deny
    } else {
        ApprovalMode::Ask
    };

    let options = StreamOptions {
        api_key: cli.api_key.clone(),
        temperature: cli.temperature,
        max_tokens: cli.max_tokens,
        ..StreamOptions::default()
    };

    let agent = Agent::new(models, model.clone(), system_prompt)
        .with_tools(builtin_tools())
        .with_cwd(&cli.cwd)
        .with_max_turns(usize::try_from(cli.max_turns).unwrap_or(50))
        .with_session_id(format!("rsagent-{}", std::process::id()))
        .with_approver(CliApprover {
            mode: approval,
            input,
            palette,
            allow_all: Arc::new(AtomicBool::new(false)),
        })
        .with_options(options);

    (agent, faux_handle, model)
}

/// Print the REPL's slash-command help.
fn print_repl_help() {
    println!("可用命令：");
    println!("  /help    显示本帮助");
    println!("  /clear   清空当前会话记录（用 --session 时同步写回文件）");
    println!("  /exit    退出（也可用 exit、quit）");
    println!();
    println!("其它输入会作为提示词发给模型。");
    println!("工具调用需确认：y=允许一次，a=本会话后续全部允许，回车=拒绝。");
}

/// Read prompts from stdin and run the agent until the user quits.
async fn run_repl(
    agent: &mut Agent,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    renderer: &mut Renderer,
    faux: Option<&FauxProvider>,
    input: &InputHandle,
    session_path: Option<&Path>,
) -> anyhow::Result<()> {
    loop {
        let Some(line) = input.read_prompt("> ").await? else {
            break; // EOF
        };
        let input_text = line.trim();
        if input_text.is_empty() {
            continue;
        }
        if matches!(input_text, "exit" | "quit" | "/exit" | "/quit") {
            break;
        }

        if let Some(command) = input_text.strip_prefix('/') {
            match command.trim() {
                "help" => print_repl_help(),
                "clear" => {
                    let dropped = agent.messages().len();
                    agent.load_messages(Vec::new());
                    save_session(session_path, agent, renderer.palette);
                    println!(
                        "{}",
                        renderer
                            .palette
                            .dim(&format!("· 已清空会话记录（丢弃 {dropped} 条消息）"))
                    );
                }
                "" => {}
                other => println!(
                    "{}",
                    renderer
                        .palette
                        .yellow(&format!("· 未知命令 /{other}，输入 /help 查看可用命令"))
                ),
            }
            continue;
        }

        if let Some(faux) = faux {
            faux.set_responses(vec![FauxResponse::text(format!(
                "(faux) you said: {input_text}"
            ))]);
        }

        run_prompt(agent, rx, renderer, input_text.to_string()).await?;
        save_session(session_path, agent, renderer.palette);
    }

    Ok(())
}

/// A session file: a versioned envelope around the persisted [`Context`].
#[derive(Debug, Serialize, Deserialize)]
struct SessionFile {
    /// Format version, so a newer file can be recognized (and rejected) by an older build.
    version: u32,
    /// The persisted transcript and system prompt.
    context: Context,
}

/// Read a persisted transcript, if the file exists.
///
/// Both the current versioned envelope and the older bare-`Context` format are
/// accepted; a file written by a newer format version is rejected rather than misread.
fn load_session(path: &Path) -> anyhow::Result<Option<Context>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&text)?;
    if value.get("version").is_some() {
        let file: SessionFile = serde_json::from_value(value)?;
        if file.version > SESSION_VERSION {
            anyhow::bail!(
                "written by a newer rsagent (format v{}); this build understands up to v{}",
                file.version,
                SESSION_VERSION
            );
        }
        Ok(Some(file.context))
    } else {
        // Legacy format: a bare `Context` with no envelope.
        Ok(Some(serde_json::from_value(value)?))
    }
}

/// Write the transcript to `path`, if a session file was requested.
fn save_session(path: Option<&Path>, agent: &Agent, palette: Palette) {
    let Some(path) = path else {
        return;
    };
    let file = SessionFile {
        version: SESSION_VERSION,
        context: agent.transcript(),
    };
    match serde_json::to_string_pretty(&file) {
        Ok(json) => {
            if let Err(err) = std::fs::write(path, json) {
                eprintln!(
                    "{}",
                    palette.yellow(&format!(
                        "· could not save session to {}: {err}",
                        path.display()
                    ))
                );
            }
        }
        Err(err) => eprintln!(
            "{}",
            palette.yellow(&format!("· could not serialize session: {err}"))
        ),
    }
}

/// Drive one prompt while concurrently rendering the agent's events.
async fn run_prompt(
    agent: &mut Agent,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    renderer: &mut Renderer,
    input: String,
) -> anyhow::Result<()> {
    let cancel = agent.cancel_token();
    let prompt = agent.prompt(input).fuse();
    tokio::pin!(prompt);

    loop {
        tokio::select! {
            result = &mut prompt => {
                result?;
                break;
            }
            Some(event) = rx.recv() => {
                renderer.render(&event);
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("{}", renderer.palette.yellow("\n· aborting…"));
                cancel.cancel();
            }
        }
    }

    // Drain any events that were queued right before completion.
    while let Ok(event) = rx.try_recv() {
        renderer.render(&event);
    }
    renderer.flush_line();
    println!();
    Ok(())
}

/// How the CLI answers approval requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalMode {
    /// Allow everything.
    Auto,
    /// Ask on the terminal.
    Ask,
    /// Deny everything (non-interactive without `--yes`).
    Deny,
}

/// An [`Approver`] driven by [`ApprovalMode`].
#[derive(Debug, Clone)]
struct CliApprover {
    mode: ApprovalMode,
    /// Shared with the REPL so both read the terminal through one reader.
    input: InputHandle,
    /// Styling for the CLI's own messages.
    palette: Palette,
    /// Set once the user answers `a`: every later call is allowed without asking.
    allow_all: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl Approver for CliApprover {
    async fn approve(&self, tool_name: &str, _args: &Value) -> bool {
        match self.mode {
            ApprovalMode::Auto => true,
            ApprovalMode::Deny => {
                eprintln!(
                    "{}",
                    self.palette.yellow(&format!(
                        "· skipped {tool_name}: approval required (pass --yes to allow)"
                    ))
                );
                false
            }
            ApprovalMode::Ask => {
                // The prompt itself is rendered from the `ToolApprovalRequest` event
                // (see `Renderer`) so it lines up with the rest of the output; here we
                // only read the decision from the shared input reader.
                if self.allow_all.load(Ordering::Relaxed) {
                    return true;
                }
                match self.input.read_answer("> ").await {
                    Ok(Some(line)) => match line.trim().to_ascii_lowercase().as_str() {
                        "y" | "yes" => true,
                        "a" | "all" => {
                            self.allow_all.store(true, Ordering::Relaxed);
                            println!(
                                "{}",
                                self.palette.yellow("  · 好的，本次会话后续操作将不再询问")
                            );
                            true
                        }
                        _ => false,
                    },
                    Ok(None) | Err(_) => false,
                }
            }
        }
    }

    fn needs_prompt(&self, _tool_name: &str) -> bool {
        self.mode == ApprovalMode::Ask && !self.allow_all.load(Ordering::Relaxed)
    }
}

/// Prints agent events to stdout as they arrive.
struct Renderer {
    /// True while a streamed line is open (no trailing newline yet).
    line_open: bool,
    /// True while the open line holds reasoning text, which is rendered dim.
    thinking_open: bool,
    /// Working directory used to explain file operations in plain language.
    cwd: PathBuf,
    /// Styling that honors `--no-color` / `NO_COLOR`.
    palette: Palette,
}

impl Renderer {
    fn new(cwd: PathBuf, palette: Palette) -> Self {
        Renderer {
            line_open: false,
            thinking_open: false,
            cwd,
            palette,
        }
    }

    fn render(&mut self, event: &AgentEvent) {
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
                println!(
                    "{}",
                    self.palette
                        .cyan(&format!("⚙ {}", describe_action(tool_name, args)))
                );
            }
            AgentEvent::ToolExecutionUpdate { content, .. } => {
                self.flush_line();
                println!(
                    "{}",
                    self.palette.dim(&format!("  │ {}", truncate(content, 200)))
                );
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
                println!("{painted}{}", truncate(content, 200));
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
            _ => {}
        }
    }

    /// Append a reasoning delta, starting a fresh line if an answer was in progress.
    fn render_thinking_delta(&mut self, delta: &str) {
        if self.line_open && !self.thinking_open {
            self.flush_line();
        }
        print!("{}", self.palette.dim(delta));
        std::io::Write::flush(&mut std::io::stdout()).ok();
        self.line_open = true;
        self.thinking_open = true;
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
        println!("{}", self.palette.yellow("? 需要你确认"));
        println!("{}", self.palette.yellow(&format!("  操作：{title}")));
        for line in lines {
            println!("{}", self.palette.yellow(&format!("  {line}")));
        }
        println!(
            "{}",
            self.palette.yellow("  → 输入 y 再回车 = 只允许这一次")
        );
        println!(
            "{}",
            self.palette
                .yellow("  → 输入 a 再回车 = 允许，且本次会话后续操作都不再询问")
        );
        println!(
            "{}",
            self.palette
                .yellow("  → 直接回车 = 拒绝（我会换个方式继续）")
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
                vec![target, format!("说明：将写入约 {bytes} 字节内容。")]
            }
            "edit_file" => {
                let path = arg_str(args, "path");
                let warning = if self.cwd.join(&path).exists() {
                    String::new()
                } else {
                    "（注意：该文件当前不存在，可能失败）".to_string()
                };
                vec![
                    format!("修改文件：{path}{warning}"),
                    format!(
                        "改动：把 {} 替换为 {}",
                        quote(&arg_str(args, "old_string")),
                        quote(&arg_str(args, "new_string"))
                    ),
                    "说明：只替换这一处文字，文件其它内容保持不变。".to_string(),
                ]
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

    fn flush_line(&mut self) {
        if self.line_open {
            println!();
            self.line_open = false;
        }
        self.thinking_open = false;
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

fn compact(value: &serde_json::Value) -> String {
    let rendered = serde_json::to_string(value).unwrap_or_default();
    truncate(&rendered, 120)
}

fn truncate(text: &str, max: usize) -> String {
    let single_line = text.replace('\n', " ");
    if single_line.chars().count() <= max {
        single_line
    } else {
        let truncated: String = single_line.chars().take(max).collect();
        format!("{truncated}…")
    }
}
