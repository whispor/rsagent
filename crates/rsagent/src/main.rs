//! `rsagent` — a minimal coding-agent CLI, in the spirit of pi's `coding-agent` package.

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "this binary is the CLI's output layer; printing to the terminal is its purpose"
)]

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::Parser;
use futures::future::FutureExt;
use rsagent_agent::{builtin_tools, Agent, AgentEvent, Approver};
use rsagent_ai::{
    AnthropicProvider, Context, ErrorReason, FauxProvider, FauxResponse, Message, Model, Models,
    OpenAiProvider, StreamEvent, StreamOptions,
};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

/// A single buffered reader over stdin, shared by the REPL and the approval prompt.
///
/// Both read through this one instance so input buffered for one is never lost to the
/// other (two independent readers over the same terminal can swallow each other's bytes).
type SharedInput = Arc<Mutex<BufReader<tokio::io::Stdin>>>;

fn shared_stdin() -> SharedInput {
    Arc::new(Mutex::new(BufReader::new(tokio::io::stdin())))
}

/// Read one line from the shared reader, returning `None` at end of input.
async fn read_line(input: &SharedInput) -> std::io::Result<Option<String>> {
    let mut guard = input.lock().await;
    let mut line = String::new();
    if guard.read_line(&mut line).await? == 0 {
        return Ok(None);
    }
    Ok(Some(line))
}

const DEFAULT_SYSTEM_PROMPT: &str = "\
You are rsagent, a concise coding assistant running in a terminal.
Use the provided tools to read and modify files and to run commands.
Prefer small, focused changes. Explain briefly what you did.";

#[derive(Parser, Debug)]
#[command(name = "rsagent", about = "A minimal Rust coding agent")]
struct Cli {
    /// Model id to use (default: gpt-4o-mini).
    #[arg(short, long, env = "RSAGENT_MODEL")]
    model: Option<String>,

    /// Run a single prompt and exit.
    #[arg(short, long)]
    print: Option<String>,

    /// Base URL of an OpenAI-compatible endpoint.
    #[arg(long, env = "RSAGENT_BASE_URL")]
    base_url: Option<String>,

    /// API key; overrides the provider's environment resolution.
    #[arg(long, env = "RSAGENT_API_KEY")]
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

    /// Auto-approve every tool call (skip the confirmation prompt).
    #[arg(short = 'y', long)]
    yes: bool,

    /// Persist the conversation to a JSON file, resuming it if it already exists.
    #[arg(long)]
    session: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load `.env` before parsing so clap's `env = ...` attributes see it.
    let _ = dotenvy::dotenv();

    let cli = Cli::parse();

    let input = shared_stdin();
    let (mut agent, faux_handle, model) = build_agent(&cli, Arc::clone(&input));

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
    agent.subscribe(tx);

    // Resume a persisted transcript, if one exists.
    if let Some(path) = &cli.session {
        match load_session(path) {
            Ok(Some(context)) => {
                let count = context.messages.len();
                agent.load_messages(context.messages);
                eprintln!(
                    "\x1b[2m· resumed {} message(s) from {}\x1b[0m",
                    count,
                    path.display()
                );
            }
            Ok(None) => {}
            Err(err) => eprintln!("\x1b[33m· could not read {}: {err}\x1b[0m", path.display()),
        }
    }

    let mut renderer = Renderer::new(cli.cwd.clone());

    if let Some(prompt) = cli.print.clone() {
        if let Some(faux) = &faux_handle {
            faux.set_responses(vec![FauxResponse::text(format!(
                "(faux) you said: {prompt}"
            ))]);
        }
        run_prompt(&mut agent, &mut rx, &mut renderer, prompt).await?;
        save_session(cli.session.as_deref(), &agent);
        return Ok(());
    }

    println!(
        "rsagent — model: {} ({}), cwd: {}",
        model.id,
        model.provider,
        cli.cwd.display()
    );
    println!("Type a prompt, or 'exit' to quit.\n");

    run_repl(
        &mut agent,
        &mut rx,
        &mut renderer,
        faux_handle.as_ref(),
        &cli,
        &input,
    )
    .await
}

/// Build the model registry, the agent, and (in `--faux` mode) the scripted provider.
fn build_agent(cli: &Cli, input: SharedInput) -> (Agent, Option<FauxProvider>, Model) {
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
        ..StreamOptions::default()
    };

    let agent = Agent::new(models, model.clone(), system_prompt)
        .with_tools(builtin_tools())
        .with_cwd(&cli.cwd)
        .with_session_id(format!("rsagent-{}", std::process::id()))
        .with_approver(CliApprover {
            mode: approval,
            input,
            allow_all: Arc::new(AtomicBool::new(false)),
        })
        .with_options(options);

    (agent, faux_handle, model)
}

/// Read prompts from stdin and run the agent until the user quits.
async fn run_repl(
    agent: &mut Agent,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    renderer: &mut Renderer,
    faux: Option<&FauxProvider>,
    cli: &Cli,
    input: &SharedInput,
) -> anyhow::Result<()> {
    loop {
        print!("> ");
        std::io::stdout().flush().ok();

        let Some(line) = read_line(input).await? else {
            break; // EOF
        };
        let input_text = line.trim();
        if input_text.is_empty() {
            continue;
        }
        if input_text == "exit" || input_text == "quit" {
            break;
        }

        if let Some(faux) = faux {
            faux.set_responses(vec![FauxResponse::text(format!(
                "(faux) you said: {input_text}"
            ))]);
        }

        run_prompt(agent, rx, renderer, input_text.to_string()).await?;
        save_session(cli.session.as_deref(), agent);
    }

    Ok(())
}

/// Read a persisted transcript, if the file exists.
fn load_session(path: &std::path::Path) -> anyhow::Result<Option<Context>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)?;
    let context: Context = serde_json::from_str(&text)?;
    Ok(Some(context))
}

/// Write the transcript to `path`, if a session file was requested.
fn save_session(path: Option<&std::path::Path>, agent: &Agent) {
    let Some(path) = path else {
        return;
    };
    let context = agent.transcript();
    match serde_json::to_string_pretty(&context) {
        Ok(json) => {
            if let Err(err) = std::fs::write(path, json) {
                eprintln!(
                    "\x1b[33m· could not save session to {}: {err}\x1b[0m",
                    path.display()
                );
            }
        }
        Err(err) => eprintln!("\x1b[33m· could not serialize session: {err}\x1b[0m"),
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
                eprintln!("\n\x1b[33m· aborting…\x1b[0m");
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
    /// Shared with the REPL so both read the terminal through one buffer.
    input: SharedInput,
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
                    "\x1b[33m· skipped {tool_name}: approval required (pass --yes to allow)\x1b[0m"
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
                match read_line(&self.input).await {
                    Ok(Some(line)) => match line.trim().to_ascii_lowercase().as_str() {
                        "y" | "yes" => true,
                        "a" | "all" => {
                            self.allow_all.store(true, Ordering::Relaxed);
                            println!("\x1b[33m  · 好的，本次会话后续操作将不再询问\x1b[0m");
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
}

impl Renderer {
    fn new(cwd: PathBuf) -> Self {
        Renderer {
            line_open: false,
            thinking_open: false,
            cwd,
        }
    }

    fn render(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::MessageUpdate { event } => match event {
                StreamEvent::ThinkingDelta { delta, .. } => {
                    // Switching from an answer back to reasoning starts a new line.
                    if self.line_open && !self.thinking_open {
                        self.flush_line();
                    }
                    print!("\x1b[2m{delta}\x1b[0m");
                    std::io::stdout().flush().ok();
                    self.line_open = true;
                    self.thinking_open = true;
                }
                StreamEvent::TextDelta { delta, .. } => {
                    // The answer must not run into the preceding reasoning line.
                    if self.thinking_open {
                        self.flush_line();
                    }
                    print!("{delta}");
                    std::io::stdout().flush().ok();
                    self.line_open = true;
                }
                StreamEvent::Error { error, reason } => {
                    self.flush_line();
                    let label = match reason {
                        ErrorReason::Aborted => "aborted",
                        _ => "error",
                    };
                    let detail = error.error_message.as_deref().unwrap_or("unknown error");
                    eprintln!("\x1b[31m✗ model {label}:\x1b[0m {detail}");
                }
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
                        "\x1b[2m· tokens: {} in / {} out (total {})\x1b[0m",
                        usage.input,
                        usage.output,
                        usage.total_tokens()
                    );
                }
            }
            AgentEvent::ToolApprovalRequest {
                tool_name, args, ..
            } => {
                self.flush_line();
                let mut lines = self.describe_approval(tool_name, args).into_iter();
                let title = lines
                    .next()
                    .unwrap_or_else(|| format!("调用工具「{tool_name}」"));
                println!("\x1b[33m? 需要你确认\x1b[0m");
                println!("\x1b[33m  操作：{title}\x1b[0m");
                for line in lines {
                    println!("\x1b[33m  {line}\x1b[0m");
                }
                println!("\x1b[33m  → 输入 y 再回车 = 只允许这一次\x1b[0m");
                println!("\x1b[33m  → 输入 a 再回车 = 允许，且本次会话后续操作都不再询问\x1b[0m");
                println!("\x1b[33m  → 直接回车 = 拒绝（我会换个方式继续）\x1b[0m");
            }
            AgentEvent::ToolExecutionStart {
                tool_name, args, ..
            } => {
                self.flush_line();
                println!("\x1b[36m⚙ {}\x1b[0m", describe_action(tool_name, args));
            }
            AgentEvent::ToolExecutionUpdate { content, .. } => {
                self.flush_line();
                println!("\x1b[2m  │ {}\x1b[0m", truncate(content, 200));
            }
            AgentEvent::ToolExecutionEnd {
                tool_name,
                is_error,
                content,
                ..
            } => {
                self.flush_line();
                let marker = if *is_error { "✗" } else { "✓" };
                let color = if *is_error { "31" } else { "32" };
                println!(
                    "\x1b[{color}m{marker} {}：\x1b[0m{}",
                    tool_display_name(tool_name),
                    truncate(content, 200)
                );
            }
            _ => {}
        }
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
