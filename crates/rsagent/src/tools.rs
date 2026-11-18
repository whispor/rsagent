//! The `rsagent tools` subcommand group: list, doctor, add, remove, reset
//! (`docs/EXTENSIBILITY.md` §3.6).
//!
//! `add` and `remove` edit `rsagent.toml` in place, preserving comments and formatting
//! (`toml_edit`). Their semantics are "edit config plus an idempotent preparation step"
//! — never "delete user data": `remove` only drops the config entry and calls the
//! source's own [`uninstall`](rsagent_agent::ToolSource::uninstall) hook.
//!
//! `list` and `doctor` report each source's health, seeded from the persisted breaker
//! snapshot; `reset` clears that snapshot so a cooling-down source is retried at once
//! (§3.5).

use std::path::PathBuf;

use clap::{Args, Subcommand};
use rsagent_agent::{BreakerState, SourceHealth, ToolSource};
use toml_edit::{value, Array, ArrayOfTables, DocumentMut, Item, Table};

use crate::breakers::BreakerStore;
use crate::config::{find_upward, Config, CONFIG_FILE};
use crate::fs::write_atomic;
use crate::render::Palette;
use crate::Cli;

/// A top-level subcommand. Today the only group is `tools`.
#[derive(Subcommand, Debug)]
pub(crate) enum Command {
    /// Manage tool sources: list, doctor, add, remove, reset.
    Tools {
        /// Which operation to run.
        #[command(subcommand)]
        action: ToolsAction,
    },
}

/// One `rsagent tools` operation.
#[expect(
    clippy::large_enum_variant,
    reason = "the `add` variant mirrors the config schema; boxing it would only obscure the CLI"
)]
#[derive(Subcommand, Debug)]
pub(crate) enum ToolsAction {
    /// List every source, its health, and the tools it offers.
    List,
    /// Probe each source and report problems with a suggested fix.
    Doctor,
    /// Add a source to `rsagent.toml`.
    Add(AddArgs),
    /// Remove a source from `rsagent.toml` by id.
    Remove {
        /// The id of the source to remove.
        id: String,
    },
    /// Clear persisted circuit-breaker state so a cooling-down source is retried now.
    Reset {
        /// The id of the source to reset; omit to reset every source.
        id: Option<String>,
    },
}

/// The fields `rsagent tools add` accepts. They mirror [`SourceConfig`](crate::config) so
/// the command is a thin, scriptable front-end for the file.
#[derive(Args, Debug)]
pub(crate) struct AddArgs {
    /// A stable id, unique among sources.
    id: String,
    /// The source kind: `builtin`, `local`, `mcp-stdio`, `mcp-http`, `skill`, or `web`.
    #[arg(long)]
    kind: String,
    /// An optional prefix applied to this source's tool names.
    #[arg(long)]
    prefix: Option<String>,
    /// The program to run (`local` and `mcp-stdio`).
    #[arg(long)]
    command: Option<String>,
    /// Fixed arguments passed to `command` (comma-separated).
    #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
    args: Vec<String>,
    /// The skill folder (`skill`): a directory holding `SKILL.md` and `skill.toml`.
    #[arg(long)]
    path: Option<String>,
    /// The endpoint URL (`mcp-http` and `web`).
    #[arg(long)]
    url: Option<String>,
    /// The HTTP method (`web`; default `GET`).
    #[arg(long)]
    method: Option<String>,
    /// The tool name the model sees (`web`; default: the id).
    #[arg(long)]
    name: Option<String>,
    /// A one-line description for the model (`web`).
    #[arg(long)]
    description: Option<String>,
    /// Per-call timeout in seconds.
    #[arg(long)]
    timeout: Option<u64>,
    /// Whether a call needs approval first (`web`; default `true`).
    #[arg(long)]
    requires_approval: Option<bool>,
}

/// Run a top-level subcommand.
///
/// # Errors
///
/// Returns a message for the caller to print and turn into a non-zero exit.
pub(crate) async fn run(command: &Command, cli: &Cli, palette: Palette) -> anyhow::Result<()> {
    match command {
        Command::Tools { action } => run_tools(action, cli, palette).await,
    }
}

/// Run one `tools` operation.
async fn run_tools(action: &ToolsAction, cli: &Cli, palette: Palette) -> anyhow::Result<()> {
    match action {
        ToolsAction::List => {
            let catalog = crate::build_catalog(cli, palette).await?;
            // Seed before printing so a cooling-down source shows as unhealthy (§3.5).
            let store = BreakerStore::load(&cli.cwd);
            store.seed(catalog.entries());
            crate::print_catalog(&catalog, palette);
            print_breakers(&store, palette);
            Ok(())
        }
        ToolsAction::Doctor => doctor(cli, palette).await,
        ToolsAction::Add(args) => add(cli, args, palette).await,
        ToolsAction::Remove { id } => remove(cli, id, palette).await,
        ToolsAction::Reset { id } => reset(cli, id.as_deref(), palette),
    }
}

/// Clear persisted breaker state, for one source or all of them.
///
/// Idempotent by design: resetting a source that has no stored breaker (a `builtin`
/// source never has one, for instance) reports "nothing to clear" rather than failing,
/// so the command is safe to run in a loop or a CI step.
fn reset(cli: &Cli, id: Option<&str>, palette: Palette) -> anyhow::Result<()> {
    let mut store = BreakerStore::load(&cli.cwd);
    let cleared = store.reset(id);
    if cleared.is_empty() {
        match id {
            Some(id) => println!("{} 来源 `{id}` 没有熔断状态", palette.dim("·")),
            None => println!("{} 没有熔断状态需要清除", palette.dim("·")),
        }
        return Ok(());
    }
    store.save()?;
    println!(
        "{} 已清除 {} 个来源的熔断状态（{}）：{}",
        palette.green("✓"),
        cleared.len(),
        store.path().display(),
        cleared.join("、")
    );
    Ok(())
}

/// Print the persisted breaker snapshot, when there is one to show.
fn print_breakers(store: &BreakerStore, palette: Palette) {
    if store.reports().is_empty() {
        return;
    }
    println!();
    println!("熔断器状态（来自 {}）：", store.path().display());
    for (id, report) in store.reports() {
        println!(
            "  {id}  {}  失败 {} 次  冷却 {}s",
            breaker_label(report.state, palette),
            report.failures,
            report.cooldown_secs
        );
    }
}

/// A short, coloured label for one breaker state.
fn breaker_label(state: BreakerState, palette: Palette) -> String {
    match state {
        BreakerState::Closed => palette.green("关闭"),
        BreakerState::Open => palette.yellow("开启"),
        BreakerState::HalfOpen => palette.yellow("半开"),
    }
}

/// Probe each configured source and report its health, with a fix hint on failure.
///
/// # Errors
///
/// Returns a message when at least one source failed to load, so `doctor` can be used as
/// a CI gate.
async fn doctor(cli: &Cli, palette: Palette) -> anyhow::Result<()> {
    // Seeding makes a persisted, still-cooling-down source show as unhealthy here too.
    let store = BreakerStore::load(&cli.cwd);
    let config = match Config::load(&cli.cwd) {
        Ok((config, path)) => {
            match &path {
                Some(path) => println!("配置：{}", path.display()),
                None => println!("配置：未找到，使用内置来源"),
            }
            config
        }
        Err(error) => {
            println!("配置：无法读取（{error}），使用内置来源");
            Config::default()
        }
    };

    if config.tools.sources.is_empty() {
        let source = crate::builtin_source(cli, None, palette);
        store.seed_source(&source);
        report(&source, palette);
        print_breakers(&store, palette);
        return Ok(());
    }

    let mut failures = 0_usize;
    for spec in &config.tools.sources {
        match crate::build_source(cli, spec, palette).await {
            Ok(source) => {
                store.seed_source(source.as_ref());
                report(source.as_ref(), palette);
            }
            Err(error) => {
                failures += 1;
                println!("{} {} [{}]：{error}", palette.red("✗"), spec.id, spec.kind);
                if let Some(hint) = fix_hint(&spec.kind) {
                    println!("  → {hint}");
                }
            }
        }
    }
    print_breakers(&store, palette);
    if failures > 0 {
        anyhow::bail!("{failures} 个来源有问题");
    }
    Ok(())
}

/// Print one healthy source line.
fn report(source: &dyn ToolSource, palette: Palette) {
    let health = match source.health() {
        SourceHealth::Healthy => palette.green("正常"),
        SourceHealth::Unhealthy(reason) => palette.yellow(&format!("异常（{reason}）")),
        // `SourceHealth` is `#[non_exhaustive]`: a future variant must not break this.
        _ => palette.yellow("未知"),
    };
    println!(
        "{} {} [{}] {} 个工具 · {health}",
        palette.green("✓"),
        source.id(),
        source.kind().as_str(),
        source.tools().len()
    );
}

/// A one-line, kind-specific suggestion for fixing a source that will not load.
fn fix_hint(kind: &str) -> Option<&'static str> {
    match kind {
        "local" | "mcp-stdio" => Some("检查 `command` / `args`，并确认该程序能在本机直接运行"),
        "mcp-http" | "web" => Some("检查 `url`，并确认端点可达（网络与鉴权头）"),
        "skill" => Some("检查 `path`，并确认该目录含 `SKILL.md` 与 `skill.toml`"),
        "builtin" => Some("检查 `only` / `exclude` 里的工具名是否拼写正确"),
        _ => None,
    }
}

/// Add a source to `rsagent.toml`, then best-effort prepare it.
async fn add(cli: &Cli, args: &AddArgs, palette: Palette) -> anyhow::Result<()> {
    let path = config_path(cli);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: DocumentMut = existing
        .parse()
        .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;

    let sources = sources_array_mut(&mut doc)?;
    let duplicate = sources
        .iter()
        .any(|table| table.get("id").and_then(Item::as_str) == Some(args.id.as_str()));
    if duplicate {
        anyhow::bail!(
            "a source with id `{}` already exists in {}",
            args.id,
            path.display()
        );
    }
    sources.push(entry_table(args));

    let rendered = doc.to_string();
    write_atomic(&path, &rendered)?;
    println!(
        "{} 已添加来源 `{}` 到 {}",
        palette.green("✓"),
        args.id,
        path.display()
    );

    // Preparation is best-effort: a source that cannot load right now (an offline
    // endpoint, say) is still written, because "config first" is the point (§3.6).
    prepare_after_add(cli, &rendered, &args.id, palette).await;
    Ok(())
}

/// Build the just-added source and run its idempotent `install` hook, reporting failures.
async fn prepare_after_add(cli: &Cli, rendered: &str, id: &str, palette: Palette) {
    let Ok(config) = Config::parse(rendered) else {
        return;
    };
    let Some(spec) = config.tools.sources.iter().find(|spec| spec.id == id) else {
        return;
    };
    match crate::build_source(cli, spec, palette).await {
        Ok(source) => match source.install() {
            Ok(()) => println!("  · 已准备就绪"),
            Err(error) => println!(
                "{}",
                palette.yellow(&format!("  · 已写入配置，但准备失败：{error}"))
            ),
        },
        Err(error) => println!(
            "{}",
            palette.yellow(&format!("  · 已写入配置，但无法加载：{error}"))
        ),
    }
}

/// Remove a source from `rsagent.toml`, then best-effort clean it up.
///
/// Only the config entry is removed; the source's own `uninstall` hook decides what (if
/// anything) to clean up, and must not delete user data (§3.6).
async fn remove(cli: &Cli, id: &str, palette: Palette) -> anyhow::Result<()> {
    let path = config_path(cli);
    if !path.exists() {
        anyhow::bail!("no {} to remove `{id}` from", path.display());
    }
    let text = std::fs::read_to_string(&path)?;
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;

    let sources = sources_array_mut(&mut doc)?;
    let Some(index) = sources
        .iter()
        .position(|table| table.get("id").and_then(Item::as_str) == Some(id))
    else {
        anyhow::bail!("no source with id `{id}` in {}", path.display());
    };
    sources.remove(index);

    write_atomic(&path, &doc.to_string())?;
    println!(
        "{} 已从 {} 移除来源 `{id}`",
        palette.green("✓"),
        path.display()
    );

    if let Ok(config) = Config::parse(&text) {
        if let Some(spec) = config.tools.sources.iter().find(|spec| spec.id == id) {
            if let Ok(source) = crate::build_source(cli, spec, palette).await {
                if let Err(error) = source.uninstall() {
                    println!(
                        "{}",
                        palette.yellow(&format!("  · 已移除配置，但清理失败：{error}"))
                    );
                }
            }
        }
    }
    Ok(())
}

/// The `rsagent.toml` to edit: the nearest one upward, or a new one in the working dir.
fn config_path(cli: &Cli) -> PathBuf {
    find_upward(&cli.cwd, CONFIG_FILE).unwrap_or_else(|| cli.cwd.join(CONFIG_FILE))
}

/// The `[[tools.sources]]` array, creating the `[tools]` table and the array if absent.
fn sources_array_mut(doc: &mut DocumentMut) -> anyhow::Result<&mut ArrayOfTables> {
    if doc.get("tools").is_none() {
        // Implicit, so the file reads `[[tools.sources]]` with no empty `[tools]` header.
        let mut table = Table::new();
        table.set_implicit(true);
        doc.insert("tools", Item::Table(table));
    }
    let tools = doc
        .get_mut("tools")
        .expect("`tools` was just inserted or already present")
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("`tools` in rsagent.toml must be a table"))?;
    let sources = tools
        .entry("sources")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()));
    sources.as_array_of_tables_mut().ok_or_else(|| {
        anyhow::anyhow!("`tools.sources` in rsagent.toml must be an array of tables")
    })
}

/// Render one `[[tools.sources]]` entry from the CLI arguments, omitting unset fields.
fn entry_table(args: &AddArgs) -> Table {
    let mut table = Table::new();
    table["kind"] = value(args.kind.as_str());
    table["id"] = value(args.id.as_str());
    if let Some(prefix) = &args.prefix {
        table["prefix"] = value(prefix.as_str());
    }
    if let Some(command) = &args.command {
        table["command"] = value(command.as_str());
    }
    if !args.args.is_empty() {
        let mut array = Array::new();
        for arg in &args.args {
            array.push(arg.as_str());
        }
        table["args"] = value(array);
    }
    if let Some(path) = &args.path {
        table["path"] = value(path.as_str());
    }
    if let Some(url) = &args.url {
        table["url"] = value(url.as_str());
    }
    if let Some(method) = &args.method {
        table["method"] = value(method.as_str());
    }
    if let Some(name) = &args.name {
        table["name"] = value(name.as_str());
    }
    if let Some(description) = &args.description {
        table["description"] = value(description.as_str());
    }
    if let Some(timeout) = args.timeout {
        table["timeout_secs"] = value(i64::try_from(timeout).unwrap_or(i64::MAX));
    }
    if let Some(requires_approval) = args.requires_approval {
        table["requires_approval"] = value(requires_approval);
    }
    table
}
