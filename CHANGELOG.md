# Changelog · 变更日志

All notable changes to `rsagent` are recorded here.
本文件记录 `rsagent` 的所有重要变更。

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project adheres to [Semantic Versioning](https://semver.org/). Because the workspace is
pre-1.0, a **minor** bump may contain breaking changes and a **patch** bump may not — see
[STABILITY.md](STABILITY.md) for the full promise and the public-API surface.
格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本遵循
[语义化版本](https://semver.org/lang/zh-CN/)。由于尚未到 1.0，**minor** 版本可含破坏性变更、
**patch** 版本不得含——完整承诺与公开 API 清单见 [STABILITY.md](STABILITY.md)。

---

## [Unreleased]

### Added
- **`CHANGELOG.md`** — this file, so breaking changes have a documented home (the
  versioning rules in `STABILITY.md` already referred to a changelog).
- **`--log <FILE>`** — append the agent event stream to a file as JSON Lines, for
  post-mortem diagnosis of long or unattended runs. Reuses the same `Serialize`
  implementation as `--json`, so the on-disk trace and the stdout stream share one shape.
- **`--verbose`** — emit extra diagnostics on stderr (resolved config paths, assembled
  sources, per-turn timing) without changing stdout.
- **User-level configuration** — `rsagent.toml` is now also read from the user config
  directory (`$XDG_CONFIG_HOME/rsagent/` or `~/.config/rsagent/` on Unix,
  `%APPDATA%\rsagent\` on Windows) and merged under the project file, completing the
  precedence chain promised in `docs/EXTENSIBILITY.md` §3.3:
  **CLI flags > environment > project config > user config > built-in defaults**.

### Changed
- **HTTP providers reuse one `reqwest::Client`** (and therefore one connection pool) for
  the whole provider lifetime, instead of building a fresh client per request. This
  removes repeated TCP/TLS setup on multi-turn runs. The client is created with no
  request timeout on purpose, so long-lived streams are never cut off.

## [0.1.0] - 2026-11-18

The first version: a minimal, extensible Rust coding agent.
首个版本：一个小巧、可扩展的 Rust 编码智能体。

### Added
- **Three-crate workspace** with one-way layering: `rsagent-ai` (transport) →
  `rsagent-agent` (loop) → `rsagent` (CLI). Each lower layer knows nothing about the one
  above it.
- **Providers** — native OpenAI Chat Completions and Anthropic Messages, both normalized
  into one `StreamEvent` sequence; a `faux` scripted provider for offline runs; a
  `Models` registry for routing.
- **Agent loop** (`Agent::run_turns`) — turns, parallel (or sequential) tool execution,
  approval preflight, mid-run steering and follow-ups, cancellation, per-request retry
  with backoff, and whole-turn context compaction.
- **Built-in tools** — `read_file`, `list_dir`, `search`, `glob`, `webfetch`,
  `write_file`, `edit_file`, `bash`; read tools confined by `PathFence`.
- **Tool sources** — `ToolSource` + `ToolCatalog` assemble six kinds into a flat tool
  list at assembly time: `builtin`, `local`, `mcp-stdio`, `mcp-http`, `skill`, `web`.
  Name prefixes and collision detection keep the model's tool namespace unambiguous.
- **Per-source circuit breaker** — connection-class failures trip a source out of
  rotation with exponential cooldown; state is snapshotted to `.rsagent-breakers.json`
  and restored on the next run.
- **Approval policy** — `Approver` trait with `AllowAll` / `DenyAll` and a CLI approver
  (auto / ask / deny), plus `y` / `a` / `n` prompt answers.
- **CLI** — one-shot `--print` and an interactive REPL (line editing, history, slash
  commands), `--json` event stream, versioned JSON and append-mode JSONL sessions
  (atomic writes), `AGENTS.md` project instructions, `--thinking` budgets, and exit-code
  semantics for CI.
- **`rsagent tools`** subcommands — `list`, `doctor` (a CI gate with kind-specific fix
  hints), `add` / `remove` (in-place `rsagent.toml` edits that preserve comments), and
  `reset` (clear persisted breaker state).
- **Documentation** — `docs/ARCHITECTURE.md`, `docs/EXTENSIBILITY.md` (with a decision
  log), `STABILITY.md`, two visual walkthroughs (`docs/tool-call-journey.html`,
  `docs/approval-and-fence.html`), and runnable `examples/`.
- **CI** — a three-platform matrix (ubuntu / windows / macos) running
  `cargo fmt --check`, `cargo clippy -D warnings`, and `cargo test`.
