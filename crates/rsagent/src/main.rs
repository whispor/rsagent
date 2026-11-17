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
use rsagent_agent::{select_builtin_tools, Agent, AgentEvent, Approver, PathFence, Tool};
use rsagent_ai::{
    AnthropicProvider, Context, FauxProvider, FauxResponse, Model, Models, OpenAiProvider,
    StreamOptions, ThinkingLevel,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use input::InputHandle;
use render::{ColorChoice, Palette, Renderer};

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

    /// Let read tools access paths outside the working directory.
    #[arg(long)]
    allow_outside_cwd: bool,

    /// Enable only these tools (comma-separated); default is all built-ins.
    #[arg(long, value_delimiter = ',')]
    tools: Option<Vec<String>>,

    /// Disable the `bash` tool.
    #[arg(long)]
    no_bash: bool,

    /// Extended-reasoning budget: off, minimal, low, medium, high, xhigh, or max.
    #[arg(long, value_parser = parse_thinking, default_value = "off")]
    thinking: ThinkingLevel,
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
        thinking: cli.thinking,
        ..StreamOptions::default()
    };

    // Read tools are confined to the working directory unless the user opts out.
    let fence = if cli.allow_outside_cwd {
        PathFence::Unrestricted
    } else {
        PathFence::WorkingDirectory
    };

    let agent = Agent::new(models, model.clone(), system_prompt)
        .with_tools(choose_tools(cli, palette))
        .with_cwd(&cli.cwd)
        .with_path_fence(fence)
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

/// Choose the tools the agent may call, honoring `--tools` and `--no-bash`.
///
/// `--tools` narrows the set to the listed names; `--no-bash` always drops `bash`
/// afterwards. Unknown names and an empty result are reported on stderr.
fn choose_tools(cli: &Cli, palette: Palette) -> Vec<Box<dyn Tool>> {
    let exclude: &[&str] = if cli.no_bash { &["bash"] } else { &[] };
    let (tools, unknown) = select_builtin_tools(cli.tools.as_deref(), exclude);
    for name in &unknown {
        eprintln!(
            "{}",
            palette.yellow(&format!("· unknown tool in --tools: {name}"))
        );
    }
    if tools.is_empty() {
        eprintln!(
            "{}",
            palette.yellow("· warning: no tools are enabled; the agent cannot act")
        );
    }
    tools
}

/// Parse a `--thinking` level into a [`ThinkingLevel`].
fn parse_thinking(value: &str) -> Result<ThinkingLevel, String> {
    match value.to_ascii_lowercase().as_str() {
        "off" => Ok(ThinkingLevel::Off),
        "minimal" => Ok(ThinkingLevel::Minimal),
        "low" => Ok(ThinkingLevel::Low),
        "medium" => Ok(ThinkingLevel::Medium),
        "high" => Ok(ThinkingLevel::High),
        "xhigh" => Ok(ThinkingLevel::XHigh),
        "max" => Ok(ThinkingLevel::Max),
        other => Err(format!(
            "unknown thinking level `{other}` (expected off, minimal, low, medium, high, xhigh, or max)"
        )),
    }
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

// The event renderer and its presentation helpers live in `render.rs`.
