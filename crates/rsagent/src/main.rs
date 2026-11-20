//! `rsagent` — a minimal coding-agent CLI, in the spirit of pi's `coding-agent` package.

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "this binary is the CLI's output layer; printing to the terminal is its purpose"
)]

mod breakers;
mod config;
mod fs;
mod input;
mod log;
mod render;
mod session;
mod tools;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use futures::future::FutureExt;
use rsagent_agent::{
    Agent, AgentError, AgentEvent, Approver, BuiltinSource, HealthProbe, LocalSource,
    McpHttpSource, McpStdioSource, Memory, PathFence, Permission, SkillSource, SourceEntry,
    SourceHealth, SourceKind, Tool, ToolCatalog, UnhealthySource, WebSource, WebSourceConfig,
    DEFAULT_MEMORY_CAP,
};
use rsagent_ai::{
    AnthropicProvider, FauxBlock, FauxProvider, FauxResponse, Model, Models, OpenAiProvider,
    StopReason, StreamOptions, ThinkingLevel,
};
use serde_json::Value;

use config::{find_upward, load_layered, SourceConfig};
use input::InputHandle;
use render::{truncate, ColorChoice, OutputMode, Palette, Renderer};

const DEFAULT_SYSTEM_PROMPT: &str = "\
You are rsagent, a concise coding assistant running in a terminal.
Use the provided tools to read and modify files and to run commands.
Prefer small, focused changes. Explain briefly what you did.";

/// Project instruction file, read from the working directory upward. The name is the
/// cross-tool convention (`AGENTS.md`), so the same file also serves other agents.
const PROJECT_INSTRUCTIONS: &str = "AGENTS.md";

/// Environment variable the OpenAI-compatible provider reads its key from. Named once here
/// so the preflight message and the provider registration can never disagree.
const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";

/// Environment variable the Anthropic provider reads its key from. See above.
const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";

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

    /// Emit the agent event stream as JSON lines on stdout, for CI and scripts.
    ///
    /// Requires `--print`: the JSON stream replaces the interactive rendering.
    #[arg(long)]
    json: bool,

    /// Append the agent event stream to a file as JSON lines, for post-mortem diagnosis.
    ///
    /// Works in every mode (interactive or `--print`); the file is appended to, so a
    /// trace accumulates across runs.
    #[arg(long, value_name = "FILE")]
    log: Option<PathBuf>,

    /// Print extra diagnostics on stderr (resolved config, sources, per-turn timing).
    #[arg(short, long)]
    verbose: bool,

    /// Base URL of an OpenAI-compatible endpoint.
    #[arg(long, env = "RSAGENT_BASE_URL", hide_env_values = true)]
    base_url: Option<String>,

    /// API key; overrides the provider's environment resolution.
    #[arg(long, env = "RSAGENT_API_KEY", hide_env_values = true)]
    api_key: Option<String>,

    /// Use an offline, scripted provider (no network, no API key).
    #[arg(long)]
    faux: bool,

    /// Script a `--faux` tool call (repeatable); `SPEC` is `name` or `name:JSON_ARGS`.
    ///
    /// Each becomes one scripted tool-call turn, followed by a final text reply — enough to
    /// exercise the approval path offline.
    #[arg(long, value_name = "SPEC", requires = "faux")]
    faux_tool: Vec<String>,

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

    /// Auto-approve tools in these categories without prompting (comma-separated).
    ///
    /// Categories: read, write, exec, network. `--yes` is shorthand for all of them.
    #[arg(long, value_delimiter = ',', value_name = "CATEGORY")]
    allow: Vec<String>,

    /// Persist the conversation to a JSON file, resuming it if it already exists.
    #[arg(long)]
    session: Option<PathBuf>,

    /// Resume the default session file (`.rsagent-session.json`).
    #[arg(long = "continue")]
    continue_: bool,

    /// Persist long-term memory to a JSONL file and enable the `remember`/`recall` tools.
    ///
    /// The newest notes are folded into the system prompt on every request, so knowledge
    /// recorded in one session is available in the next.
    #[arg(long, value_name = "FILE")]
    memory: Option<PathBuf>,

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

    /// List the built-in tools and whether each needs approval, then exit.
    ///
    /// Deprecated in favour of `rsagent tools list`; kept working for existing scripts.
    #[arg(long)]
    list_tools: bool,

    /// Extended-reasoning budget: off, minimal, low, medium, high, xhigh, or max.
    #[arg(long, value_parser = parse_thinking, default_value = "off")]
    thinking: ThinkingLevel,

    /// Manage tool sources: list, doctor, add, remove.
    #[command(subcommand)]
    command: Option<tools::Command>,
}

/// Environment variables an auto-discovered `.env` must never set.
///
/// `RSAGENT_BASE_URL` redirects where the model request — and the user's real API key — is
/// sent; `RSAGENT_USER_CONFIG` points the user-level config at a file the repo controls.
/// A `.env` that arrived with a `git clone` is untrusted input, so these two are refused:
/// they are only honored from the real environment, never from a project file.
const PROTECTED_ENV: &[&str] = &["RSAGENT_BASE_URL", "RSAGENT_USER_CONFIG"];

/// The protected variables to drop: names absent before loading `.env` but present after.
///
/// A variable already set in the real environment is left alone — `dotenvy` never overrides
/// it — so only values a `.env` *introduced* are dropped.
fn dropped_protected_vars(
    was_set: &dyn Fn(&str) -> bool,
    is_set: &dyn Fn(&str) -> bool,
) -> Vec<&'static str> {
    PROTECTED_ENV
        .iter()
        .copied()
        .filter(|name| !was_set(name) && is_set(name))
        .collect()
}

/// Load `.env` (from the working directory upward) without letting it set the
/// security-sensitive variables in [`PROTECTED_ENV`].
fn load_dotenv() {
    let was_set = |name: &str| std::env::var_os(name).is_some();
    let before: Vec<&str> = PROTECTED_ENV
        .iter()
        .copied()
        .filter(|n| was_set(n))
        .collect();

    let _ = dotenvy::dotenv();

    let is_set = |name: &str| std::env::var_os(name).is_some();
    for name in dropped_protected_vars(&|n| before.contains(&n), &is_set) {
        std::env::remove_var(name);
        eprintln!(
            "· ignoring `{name}` from .env: it can redirect where your credentials are sent, \
             so it is read only from the environment, never a project file"
        );
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load `.env` before parsing so clap's `env = ...` attributes see it — but never let a
    // project `.env` set a credential-redirecting variable (see `load_dotenv`).
    load_dotenv();

    let cli = Cli::parse();
    let color = if cli.no_color {
        ColorChoice::Never
    } else {
        ColorChoice::Auto
    };
    let palette = Palette::detect(color);

    // A subcommand runs first: it never needs a model or a provider.
    if let Some(command) = &cli.command {
        if let Err(error) = tools::run(command, &cli, palette).await {
            eprintln!("{}", palette.red(&format!("· {error}")));
            std::process::exit(2);
        }
        return Ok(());
    }

    // `--json` streams events for machines; it only makes sense in one-shot `--print`
    // mode, so reject the combination early rather than emitting a half-interactive
    // stream. Fail fast, before any source is spawned.
    if cli.json && cli.print.is_none() {
        eprintln!(
            "{}",
            palette.red("· --json requires --print (it streams events for CI and scripts)")
        );
        std::process::exit(2);
    }

    // Open the event-log sink (no-op without `--log`). Done before any source is spawned
    // so a bad path fails fast rather than after a run has started.
    let mut log = open_event_log(&cli, palette);

    // Assemble the tool set once, from `rsagent.toml` plus CLI flags, so `--list-tools`
    // and the agent see exactly the same catalog.
    let catalog = match build_catalog(&cli, palette).await {
        Ok(catalog) => catalog,
        Err(error) => {
            eprintln!("{}", palette.red(&format!("· {error}")));
            std::process::exit(2);
        }
    };

    // Seed persisted circuit-breaker state before any source does work, so a source that
    // was cooling down in the last run stays out of rotation (§3.5). `--list-tools` and
    // the run itself both see the same seeded state.
    let mut breakers = breakers::BreakerStore::load(&cli.cwd);
    breakers.seed(catalog.entries());

    if cli.list_tools {
        print_catalog(&catalog, palette);
        return Ok(());
    }

    // Keep the sources (with their breakers) after handing the tools to the agent, so the
    // run's trips can be snapshotted afterwards. `Arc` lets the agent's health probe share
    // them too, without the loop ever naming a `ToolSource` (decision D2).
    let (tools, sources) = catalog.into_parts();
    let sources: Arc<[SourceEntry]> = sources.into();
    let probe = health_probe(Arc::clone(&sources));
    let input = InputHandle::spawn();
    let (mut agent, faux_handle, model) = build_agent(&cli, input.clone(), palette, tools, probe);
    log_startup_summary(&cli, &model, &sources, palette);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
    agent.subscribe(tx);

    let mut session = open_session(&cli, &mut agent, palette);

    // A one-shot `--print` run keeps stdout to the answer alone; the REPL interleaves.
    // `--json` replaces both with one JSON object per event.
    let mode = if cli.json {
        OutputMode::Json
    } else if cli.print.is_some() {
        OutputMode::OneShot
    } else {
        OutputMode::Interactive
    };
    let mut renderer = Renderer::new(cli.cwd.clone(), palette)
        .with_mode(mode)
        .with_verbose(cli.verbose);

    if let Some(prompt) = cli.print.clone() {
        if let Some(faux) = &faux_handle {
            faux.set_responses(faux_responses(&cli, &prompt));
        }
        let result = run_prompt(&mut agent, &mut rx, &mut renderer, &mut log, prompt).await;
        // Persist before acting on the result, so a failed run is still recorded.
        if let Some(session) = &mut session {
            session.save(&agent, palette);
        }
        save_breakers(&mut breakers, &sources, palette);
        // A model-level failure (bad key, network error, provider refusal) ends the run
        // "successfully" as far as the loop is concerned, and the renderer has already
        // reported the reason on stderr. In one-shot `--print` mode that must still be
        // visible to scripts and CI, so exit non-zero; any other error propagates.
        if let Err(error) = result {
            if !is_model_failure(&error) {
                return Err(error);
            }
            std::process::exit(1);
        }
        return Ok(());
    }

    println!(
        "rsagent — model: {} ({}), cwd: {}",
        model.id,
        model.provider,
        cli.cwd.display()
    );
    println!("Type a prompt, or /help for commands ('exit' to quit).\n");

    // A `--faux-tool` script is queued once here; the REPL leaves a pending queue alone,
    // so the calls replay on the first prompt and later prompts fall back to a text reply.
    if let Some(faux) = &faux_handle {
        if !cli.faux_tool.is_empty() {
            faux.set_responses(faux_responses(&cli, "(scripted)"));
        }
    }

    let result = run_repl(
        &mut agent,
        &mut rx,
        &mut renderer,
        &mut log,
        faux_handle.as_ref(),
        &input,
        session.as_mut(),
    )
    .await;
    save_breakers(&mut breakers, &sources, palette);
    result
}

/// Whether an error is a model-level failure — one the renderer has already reported as an
/// event — as opposed to a structural failure the caller must surface.
fn is_model_failure(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<AgentError>(),
        Some(AgentError::ModelFailed { .. })
    )
}

/// The scripted `--faux` reply for a one-shot `--print` run.
///
/// When a thinking level is requested the reply carries a reasoning block too, so the
/// offline demo actually exercises the reasoning path — and a test can assert that the
/// answer alone reaches stdout while the reasoning goes to stderr (`ACTIONS.md` A4).
fn faux_reply(prompt: &str, thinking: ThinkingLevel) -> FauxResponse {
    let mut blocks = Vec::new();
    if thinking != ThinkingLevel::Off {
        blocks.push(FauxBlock::Thinking("(faux) reasoning".to_string()));
    }
    blocks.push(FauxBlock::Text(format!("(faux) you said: {prompt}")));
    FauxResponse {
        blocks,
        stop_reason: StopReason::Stop,
    }
}

/// The scripted `--faux` responses for one prompt.
///
/// Each `--faux-tool` becomes a tool-call turn, then a final text reply closes the run.
/// With no `--faux-tool` this is just the plain text reply, as before.
fn faux_responses(cli: &Cli, prompt: &str) -> Vec<FauxResponse> {
    let mut responses: Vec<FauxResponse> = cli
        .faux_tool
        .iter()
        .map(|spec| {
            let (name, arguments) = faux_tool_call(spec);
            FauxResponse::tool_calls(vec![(name, arguments)])
        })
        .collect();
    responses.push(faux_reply(prompt, cli.thinking));
    responses
}

/// Parse one `--faux-tool` spec (`name` or `name:JSON`) into a tool name and arguments.
///
/// A malformed JSON argument is a usage error (`exit 2`), matching `--allow`/`--tools`.
fn faux_tool_call(spec: &str) -> (String, Value) {
    let Some((name, json)) = spec.split_once(':') else {
        return (spec.to_string(), Value::Object(serde_json::Map::new()));
    };
    match serde_json::from_str(json) {
        Ok(arguments) => (name.to_string(), arguments),
        Err(error) => {
            eprintln!("error: bad JSON in --faux-tool `{spec}`: {error}");
            std::process::exit(2);
        }
    }
}

/// Open the `--log` sink, or a no-op sink when `--log` was not passed.
///
/// A bad path is fatal (`exit 2`): `--log` is an explicit request for a trace, so silently
/// dropping it would defeat its purpose. Doing this before any source is spawned means the
/// failure surfaces immediately, not after a run has already done work.
fn open_event_log(cli: &Cli, palette: Palette) -> log::EventLog {
    let Some(path) = &cli.log else {
        return log::EventLog::disabled();
    };
    match log::EventLog::open(path) {
        Ok(log) => {
            eprintln!(
                "{}",
                palette.dim(&format!("· logging events to {}", path.display()))
            );
            log
        }
        Err(error) => {
            eprintln!(
                "{}",
                palette.red(&format!("· could not open log {}: {error}", path.display()))
            );
            std::process::exit(2);
        }
    }
}

/// Print a one-line startup summary when `--verbose` is set: the resolved model, the working
/// directory, and how many sources and tools were assembled. The per-source breakdown is
/// printed separately by [`build_catalog`]; this line is the at-a-glance header.
fn log_startup_summary(cli: &Cli, model: &Model, sources: &[SourceEntry], palette: Palette) {
    // Memory is a behaviour change worth naming even without `--verbose`: it enables two
    // tools and folds past notes into the prompt.
    if let Some(path) = &cli.memory {
        eprintln!("{}", palette.dim(&format!("· memory: {}", path.display())));
    }
    if !cli.verbose {
        return;
    }
    let tools: usize = sources
        .iter()
        .map(|entry| entry.source().tools().len())
        .sum();
    eprintln!(
        "{}",
        palette.dim(&format!(
            "· [verbose] model {} ({}), cwd {} — {} source(s), {tools} tool(s)",
            model.id,
            model.provider,
            cli.cwd.display(),
            sources.len()
        ))
    );
}

/// Open the session file named by `--session`/`--continue` and resume it if present.
///
/// Returns `None` when neither flag was given, so the caller simply runs without
/// persistence. `--session` names a file explicitly; `--continue` falls back to the
/// default file.
fn open_session(cli: &Cli, agent: &mut Agent, palette: Palette) -> Option<session::Session> {
    let path = cli.session.clone().or_else(|| {
        cli.continue_
            .then(|| PathBuf::from(session::DEFAULT_SESSION_FILE))
    })?;
    let mut session = session::Session::open(&path);
    resume_session(&mut session, agent, cli, palette);
    Some(session)
}

/// Snapshot every source's breaker and persist it, reporting (but never failing on) an
/// error — losing breaker state must not fail an otherwise-successful run.
fn save_breakers(store: &mut breakers::BreakerStore, sources: &[SourceEntry], palette: Palette) {
    store.capture(sources);
    if let Err(error) = store.save() {
        eprintln!(
            "{}",
            palette.yellow(&format!("· could not save breaker state: {error}"))
        );
    }
}

/// Build the health probe the agent polls each turn.
///
/// It reads the *live* sources' health (which reflects their circuit breakers) and hands
/// the unhealthy ones to the loop as plain data, so the loop never sees a `ToolSource`
/// (decision D2). The `Arc` keeps the sources alive and shared with the post-run snapshot.
fn health_probe(sources: Arc<[SourceEntry]>) -> HealthProbe {
    Arc::new(move || {
        sources
            .iter()
            .filter_map(|entry| {
                let source = entry.source();
                match source.health() {
                    SourceHealth::Unhealthy(reason) => Some(UnhealthySource {
                        id: source.id().to_string(),
                        kind: source.kind().as_str().to_string(),
                        reason,
                    }),
                    // `SourceHealth` is `#[non_exhaustive]`; anything but unhealthy is
                    // reported as nothing here.
                    _ => None,
                }
            })
            .collect()
    })
}

/// Load a persisted transcript into the agent, reporting what happened.
fn resume_session(session: &mut session::Session, agent: &mut Agent, cli: &Cli, palette: Palette) {
    match session.load() {
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
                    session.path().display()
                ))
            );
        }
        Ok(None) => {}
        Err(err) => eprintln!(
            "{}",
            palette.yellow(&format!(
                "· could not read {}: {err}",
                session.path().display()
            ))
        ),
    }
    // A crash can leave a half-written final line; the loader drops it. Say so, rather than
    // silently losing the tail of the transcript.
    if session.dropped_truncated_tail() {
        eprintln!(
            "{}",
            palette.yellow(&format!(
                "· {} ended with a truncated line (likely a crash); the partial record was dropped",
                session.path().display()
            ))
        );
    }
}

/// Build the model registry, the agent, and (in `--faux` mode) the scripted provider.
fn build_agent(
    cli: &Cli,
    input: InputHandle,
    palette: Palette,
    tools: Vec<Box<dyn Tool>>,
    probe: HealthProbe,
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
    let system_prompt = compose_system_prompt(cli);

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
            vec![ANTHROPIC_API_KEY_ENV.to_string()],
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
            vec![OPENAI_API_KEY_ENV.to_string()],
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

    // Fail fast (or warn) on a missing key, before the user types a prompt. The check
    // borrows `models`, so it must run before the registry moves into the agent.
    preflight_credentials(cli, &models, &model, palette);

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

    let mut agent = Agent::new(models, model.clone(), system_prompt)
        .with_tools(tools)
        .with_cwd(&cli.cwd)
        .with_path_fence(fence)
        .with_max_turns(usize::try_from(cli.max_turns).unwrap_or(50))
        .with_session_id(format!("rsagent-{}", std::process::id()))
        .with_health_probe(probe)
        .with_approver(CliApprover {
            mode: approval,
            allowed: allowed_categories(cli),
            input,
            palette,
            allow_all: Arc::new(AtomicBool::new(false)),
        })
        .with_options(options);
    if let Some(path) = &cli.memory {
        agent = agent.with_memory(Arc::new(Memory::new(path.clone(), DEFAULT_MEMORY_CAP)));
    }

    (agent, faux_handle, model)
}

/// Fail fast — or warn — when the selected provider has no usable API key.
///
/// Without this, a first-run user with no key types a prompt and only learns the problem
/// after a round-trip, as a terse `✗ model error: missing API key`. The message here names
/// the exact environment variable and the flags that fix it.
///
/// It does not run when the key is not needed or already known:
/// - `--faux` is offline and `--api-key` always wins, so both are skipped;
/// - `--json` keeps its machine contract (stdout stays JSON lines), so the failure is left
///   to surface as an error event the consumer can parse;
/// - a custom `--base-url` often points at a local endpoint that needs no key, so that case
///   only warns, while the built-in cloud endpoints will certainly fail and stop here.
fn preflight_credentials(cli: &Cli, models: &Models, model: &Model, palette: Palette) {
    if cli.faux || cli.json || cli.api_key.is_some() {
        return;
    }
    // Ask the provider itself, so the key-resolution rule lives in exactly one place.
    let resolved = models
        .get_provider(&model.provider)
        .and_then(|provider| provider.resolve_auth(model).api_key)
        .is_some();
    if resolved {
        return;
    }

    let env_var = if cli.anthropic {
        ANTHROPIC_API_KEY_ENV
    } else {
        OPENAI_API_KEY_ENV
    };
    if cli.base_url.is_some() {
        eprintln!(
            "{}",
            palette.yellow(&format!(
                "· no {env_var} found; continuing because --base-url was set (a local endpoint may not need a key)"
            ))
        );
        return;
    }
    eprintln!(
        "{}",
        palette.red(&format!("· missing API key: no {env_var} is set"))
    );
    eprintln!("  → export {env_var}=<KEY>, or pass --api-key <KEY>");
    eprintln!("  → for a local endpoint that needs no key, pass --base-url <URL>");
    eprintln!("  → to try it offline with no model at all, pass --faux");
    std::process::exit(2);
}

/// Compose the system prompt.
///
/// Precedence: `--system` wins outright. Otherwise the built-in prompt is used, and any
/// `AGENTS.md` found at or above the working directory is appended under a heading. The
/// project file is additive on purpose: the built-in rules about *how to behave* should
/// survive, while the project file contributes *what this project wants*.
fn compose_system_prompt(cli: &Cli) -> String {
    if let Some(system) = &cli.system {
        return system.clone();
    }
    let mut prompt = DEFAULT_SYSTEM_PROMPT.to_string();
    if let Some(instructions) = read_project_instructions(&cli.cwd) {
        let instructions = instructions.trim();
        if !instructions.is_empty() {
            prompt.push_str("\n\n# Project instructions (AGENTS.md)\n\n");
            prompt.push_str(instructions);
            prompt.push('\n');
        }
    }
    prompt
}

/// The nearest `AGENTS.md` at or above `start`, or `None` when there is none.
///
/// Walking upward lets a repo-root `AGENTS.md` apply from any subdirectory. A missing or
/// unreadable file is simply "no instructions" — never a hard error, since running the
/// agent somewhere without project instructions is normal.
fn read_project_instructions(start: &Path) -> Option<String> {
    let path = find_upward(start, PROJECT_INSTRUCTIONS)?;
    std::fs::read_to_string(path).ok()
}

/// Build the tool sources from `rsagent.toml` and the CLI flags, then assemble them.
///
/// A source that cannot be prepared is reported and skipped: one broken source must not
/// take the whole agent down (`docs/EXTENSIBILITY.md` §3.5).
async fn build_catalog(cli: &Cli, palette: Palette) -> anyhow::Result<ToolCatalog> {
    // Layer the user-level file under the project file (project wins by source id).
    let layered = load_layered(&cli.cwd);
    for warning in &layered.warnings {
        eprintln!(
            "{}",
            palette.yellow(&format!("· ignoring config: {warning}"))
        );
    }
    for path in &layered.paths {
        eprintln!("{}", palette.dim(&format!("· config: {}", path.display())));
    }
    let config = layered.config;

    let mut entries = Vec::new();
    if config.tools.sources.is_empty() {
        // No configuration: one built-in source, narrowed by `--tools` / `--no-bash`.
        entries.push(SourceEntry::new(Box::new(builtin_source(
            cli, None, palette,
        ))));
    } else {
        for spec in &config.tools.sources {
            match build_source(cli, spec, palette).await {
                Ok(source) => {
                    entries.push(match &spec.prefix {
                        Some(prefix) => SourceEntry::with_prefix(source, prefix.clone()),
                        None => SourceEntry::new(source),
                    });
                }
                Err(error) => eprintln!(
                    "{}",
                    palette.yellow(&format!("· skipping tool source `{}`: {error}", spec.id))
                ),
            }
        }
    }

    let catalog = ToolCatalog::build(entries).map_err(|error| anyhow::anyhow!("{error}"))?;
    if cli.verbose {
        for entry in catalog.entries() {
            let source = entry.source();
            let prefix = entry
                .prefix()
                .map_or_else(String::new, |prefix| format!(", prefix {prefix}"));
            eprintln!(
                "{}",
                palette.dim(&format!(
                    "· [verbose] source {} [{}]: {} tool(s){prefix}",
                    source.id(),
                    source.kind().as_str(),
                    source.tools().len()
                ))
            );
        }
    }
    if catalog.is_empty() {
        eprintln!(
            "{}",
            palette.yellow("· warning: no tools are enabled; the agent cannot act")
        );
    }
    Ok(catalog)
}

/// Build one source from its configuration entry.
async fn build_source(
    cli: &Cli,
    spec: &SourceConfig,
    palette: Palette,
) -> Result<Box<dyn rsagent_agent::ToolSource>, String> {
    // Resolve the kind through the enum, so the accepted names live in exactly one place
    // (`SourceKind::ALL`) instead of being repeated as string literals here.
    let Some(kind) = SourceKind::parse(&spec.kind) else {
        return Err(unknown_kind(&spec.kind));
    };

    match kind {
        SourceKind::Builtin => Ok(Box::new(builtin_source(cli, Some(spec), palette))),
        SourceKind::Local => {
            let command = spec
                .command
                .as_deref()
                .ok_or_else(|| "a `local` source needs `command`".to_string())?;
            let timeout = spec.timeout_secs.map(Duration::from_secs);
            let source =
                LocalSource::new(spec.id.clone(), command, spec.args.clone(), timeout).await?;
            Ok(Box::new(source))
        }
        SourceKind::McpStdio => {
            let command = spec
                .command
                .as_deref()
                .ok_or_else(|| "an `mcp-stdio` source needs `command`".to_string())?;
            let timeout = spec.timeout_secs.map(Duration::from_secs);
            let source =
                McpStdioSource::new(spec.id.clone(), command, spec.args.clone(), timeout).await?;
            Ok(Box::new(source))
        }
        SourceKind::McpHttp => {
            let url = spec
                .url
                .as_deref()
                .ok_or_else(|| "an `mcp-http` source needs `url`".to_string())?;
            let headers = spec
                .headers
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect::<Vec<_>>();
            let timeout = spec.timeout_secs.map(Duration::from_secs);
            let source = McpHttpSource::new(spec.id.clone(), url, headers, timeout).await?;
            Ok(Box::new(source))
        }
        SourceKind::Skill => {
            let path = spec
                .path
                .as_deref()
                .ok_or_else(|| "a `skill` source needs `path`".to_string())?;
            let timeout = spec.timeout_secs.map(Duration::from_secs);
            let source = SkillSource::new(spec.id.clone(), path, timeout)?;
            Ok(Box::new(source))
        }
        SourceKind::Web => {
            let url = spec
                .url
                .as_deref()
                .ok_or_else(|| "a `web` source needs `url`".to_string())?;
            let mut config = WebSourceConfig::new(url);
            if let Some(name) = spec.name.clone() {
                config.name = name;
            }
            if let Some(description) = spec.description.clone() {
                config.description = description;
            }
            if let Some(parameters) = spec.parameters.clone() {
                config.parameters = parameters;
            }
            if let Some(method) = spec.method.clone() {
                config.method = method;
            }
            if let Some(headers) = spec.headers.clone() {
                config.headers = headers.into_iter().collect();
            }
            if let Some(requires_approval) = spec.requires_approval {
                config.requires_approval = requires_approval;
            }
            let timeout = spec.timeout_secs.map(Duration::from_secs);
            let source = WebSource::new(spec.id.clone(), config, timeout)?;
            Ok(Box::new(source))
        }
        // `SourceKind` is `#[non_exhaustive]`, so a crate outside `rsagent-agent` needs a
        // catch-all; an unknown kind never reaches here (see `parse` above).
        _ => Err(unknown_kind(&spec.kind)),
    }
}

/// The error for a config `kind` that names no known source, listing the known kinds.
fn unknown_kind(kind: &str) -> String {
    let known = SourceKind::ALL.map(SourceKind::as_str).join(", ");
    format!("unknown source kind `{kind}` (expected one of: {known})")
}

/// The built-in source, with CLI flags layered over the config entry.
///
/// `--tools` replaces the config's `only`; `--no-bash` adds to the config's `exclude`.
/// This is the "CLI beats config" half of the precedence rule (§3.3).
fn builtin_source(cli: &Cli, spec: Option<&SourceConfig>, palette: Palette) -> BuiltinSource {
    let only = cli
        .tools
        .clone()
        .or_else(|| spec.and_then(|spec| spec.only.clone()));
    let mut exclude = spec.map_or_else(Vec::new, |spec| spec.exclude.clone());
    if cli.no_bash && !exclude.iter().any(|name| name == "bash") {
        exclude.push("bash".to_string());
    }

    let mut source = BuiltinSource::new()
        .with_id(spec.map_or_else(|| "core".to_string(), |spec| spec.id.clone()))
        .with_selection(only, exclude);
    if cli.memory.is_some() {
        source = source.with_memory();
    }
    for name in source.unknown() {
        eprintln!(
            "{}",
            palette.yellow(&format!("· unknown tool in --tools: {name}"))
        );
    }
    source
}

/// Print every source, its health, and the tools it offers.
fn print_catalog(catalog: &ToolCatalog, palette: Palette) {
    println!("工具来源（共 {} 个）：", catalog.entries().len());
    for entry in catalog.entries() {
        let source = entry.source();
        let health = match source.health() {
            SourceHealth::Healthy => palette.green("正常"),
            SourceHealth::Unhealthy(reason) => palette.yellow(&format!("异常（{reason}）")),
            // `SourceHealth` is `#[non_exhaustive]`: a future variant must not break this.
            _ => palette.yellow("未知"),
        };
        let prefix = entry
            .prefix()
            .map_or_else(String::new, |prefix| format!("  前缀 {prefix}"));
        println!(
            "  {} [{}] {health}{prefix}",
            source.id(),
            source.kind().as_str()
        );
    }

    println!();
    println!("已装配工具（共 {} 个）：", catalog.len());
    for tool in catalog.tools() {
        let spec = tool.spec();
        let approval = if tool.requires_approval() {
            palette.yellow("需审批")
        } else {
            palette.green("自动")
        };
        println!(
            "  {:<13} {approval} [{:<7}]  {}",
            spec.name,
            tool.permission(),
            truncate(&spec.description, 64)
        );
    }
    println!();
    println!(
        "来源在 rsagent.toml 的 [[tools.sources]] 中配置；用 --tools / --no-bash 收窄内置集。"
    );
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
    log: &mut log::EventLog,
    faux: Option<&FauxProvider>,
    input: &InputHandle,
    mut session: Option<&mut session::Session>,
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
                    if let Some(session) = session.as_deref_mut() {
                        session.save(agent, renderer.palette);
                    }
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
            // Leave a scripted `--faux-tool` queue in place for the first prompt; once it
            // drains, fall back to the plain text reply.
            if faux.pending_count() == 0 {
                faux.set_responses(vec![FauxResponse::text(format!(
                    "(faux) you said: {input_text}"
                ))]);
            }
        }

        // A model failure is already shown to the user (the renderer reports it as an
        // event), so keep the REPL alive rather than tearing the session down over one
        // bad turn. Any other error is fatal.
        match run_prompt(agent, rx, renderer, log, input_text.to_string()).await {
            Ok(()) => {}
            Err(error) if is_model_failure(&error) => {}
            Err(error) => return Err(error),
        }
        if let Some(session) = session.as_deref_mut() {
            session.save(agent, renderer.palette);
        }
    }

    Ok(())
}

/// Drive one prompt while concurrently rendering the agent's events.
async fn run_prompt(
    agent: &mut Agent,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    renderer: &mut Renderer,
    log: &mut log::EventLog,
    input: String,
) -> anyhow::Result<()> {
    let cancel = agent.cancel_token();
    let prompt = agent.prompt(input).fuse();
    tokio::pin!(prompt);

    // Register the interrupt handler once, outside the loop: rebuilding `ctrl_c()` on
    // every event would re-register it each iteration and risk dropping a signal that
    // arrives between two registrations. The guard stops polling it after it has fired
    // once — a completed future must not be polled again — which is fine, because one
    // abort cancels the run and so ends the loop.
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    let mut interrupted = false;

    // Hold the outcome rather than propagating it here: a failing run still has events
    // queued (the error itself is one), and they must be rendered before we return.
    let outcome = loop {
        tokio::select! {
            result = &mut prompt => break result,
            Some(event) = rx.recv() => {
                log.append(&event);
                renderer.render(&event);
            }
            _ = &mut interrupt, if !interrupted => {
                interrupted = true;
                eprintln!("{}", renderer.palette.yellow("\n· aborting…"));
                cancel.cancel();
            }
        }
    };

    // Drain any events that were queued right before completion.
    while let Ok(event) = rx.try_recv() {
        log.append(&event);
        renderer.render(&event);
    }
    renderer.flush_line();
    renderer.finish_prompt();
    outcome.map_err(anyhow::Error::from)
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

/// Resolve `--allow` into categories, exiting with code 2 on an unknown name.
///
/// `--yes` already means "allow everything", so this is for the finer case: trust some
/// kinds of side effects (say network fetches) while still confirming the rest.
fn allowed_categories(cli: &Cli) -> Arc<[Permission]> {
    let mut allowed = Vec::new();
    for name in &cli.allow {
        if let Some(permission) = Permission::parse(name) {
            allowed.push(permission);
        } else {
            let valid = Permission::ALL.map(Permission::as_str).join(", ");
            eprintln!("error: unknown permission category `{name}` (expected one of: {valid})");
            std::process::exit(2);
        }
    }
    allowed.into()
}

/// An [`Approver`] driven by [`ApprovalMode`].
#[derive(Debug, Clone)]
struct CliApprover {
    mode: ApprovalMode,
    /// Categories the user auto-approved with `--allow`; never prompted for.
    allowed: Arc<[Permission]>,
    /// Shared with the REPL so both read the terminal through one reader.
    input: InputHandle,
    /// Styling for the CLI's own messages.
    palette: Palette,
    /// Set once the user answers `a`: every later call is allowed without asking.
    allow_all: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl Approver for CliApprover {
    async fn approve(&self, tool_name: &str, permission: Permission, _args: &Value) -> bool {
        if self.allowed.contains(&permission) {
            return true;
        }
        match self.mode {
            ApprovalMode::Auto => true,
            ApprovalMode::Deny => {
                eprintln!(
                    "{}",
                    self.palette.yellow(&format!(
                        "· skipped {tool_name}: approval required (pass --yes or --allow {permission} to allow)"
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

    fn needs_prompt(&self, _tool_name: &str, permission: Permission) -> bool {
        !self.allowed.contains(&permission)
            && self.mode == ApprovalMode::Ask
            && !self.allow_all.load(Ordering::Relaxed)
    }
}

// The event renderer and its presentation helpers live in `render.rs`.

#[cfg(test)]
mod tests {
    use super::dropped_protected_vars;

    #[test]
    fn a_dotenv_may_not_introduce_a_protected_variable() {
        // Set in the real environment before `.env` loaded: dotenvy leaves it, so it is
        // kept. Absent before but present after: the `.env` introduced it, so it is dropped.
        let dropped = dropped_protected_vars(&|name| name == "RSAGENT_BASE_URL", &|_| true);
        assert_eq!(dropped, vec!["RSAGENT_USER_CONFIG"]);
    }

    #[test]
    fn nothing_is_dropped_when_the_environment_already_set_the_variable() {
        let dropped = dropped_protected_vars(&|_| true, &|_| true);
        assert_eq!(dropped, Vec::<&str>::new());
    }

    #[test]
    fn nothing_is_dropped_when_dotenv_adds_nothing() {
        let dropped = dropped_protected_vars(&|_| false, &|_| false);
        assert_eq!(dropped, Vec::<&str>::new());
    }
}
