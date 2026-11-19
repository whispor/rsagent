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
- **Publishable crate metadata** — each crate declares `keywords`, `categories`, and a
  `readme`, and the intra-workspace path dependencies now carry a `version`, so the three
  crates can be packaged and published to crates.io (in order: `rsagent-ai`,
  `rsagent-agent`, `rsagent`).
- **Credential preflight** — before a run, rsagent checks that the selected provider has a
  usable API key. If not, it stops with exit `2` and prints the exact variable to set
  (`OPENAI_API_KEY` / `ANTHROPIC_API_KEY`) and the `--api-key` / `--base-url` / `--faux`
  alternatives, instead of letting the first prompt come back as a terse model error. A
  custom `--base-url` (a local endpoint may need no key) only warns; `--json` keeps its
  machine contract and lets the failure surface as an error event.
- **`--allow <CATEGORY>[,...]`** — auto-approve only some kinds of side effects instead
  of everything. Tools now declare a `Permission` category (`read`, `write`, `exec`,
  `network`), and a listed category runs without a prompt while the rest still ask. For
  example, `--allow network` lets `webfetch` run unattended but still confirms writes and
  shell commands. The category is shown per tool by `rsagent tools list`.
- **`--memory <FILE>`** — a durable, append-only memory of timestamped notes (JSONL) that
  outlives a session. When set, two tools (`remember`, `recall`) join the tool set and the
  newest notes are folded into the system prompt on every request; context compaction also
  distills the turns it drops into a `[compacted]` note, so nothing the trim removes is lost
  for good. The store keeps the most recent 200 notes. New public API: `Memory`,
  `MemoryEntry`, `RememberTool`, `RecallTool`, `memory_tools`, and the `DEFAULT_MEMORY_CAP`
  / `DEFAULT_RECALL_LIMIT` constants (decision D16 in `docs/EXTENSIBILITY.md`).

### Changed
- **`--faux --print --thinking <level>` now scripts a reasoning block.** The offline demo
  emits a short reasoning block before the answer when a thinking level is requested, so
  the reasoning rendering can be seen (and tested) without a network model.
- **Breaking: `Approver::approve` / `needs_prompt` now take the tool's `Permission`.**
  The category (`read` / `write` / `exec` / `network`) is passed alongside the name so a
  policy can vary by kind — the mechanism behind `--allow`. A custom `Approver` adds the
  parameter; `AllowAll` and `DenyAll` are unchanged in behaviour.
- **HTTP providers reuse one `reqwest::Client`** (and therefore one connection pool) for
  the whole provider lifetime, instead of building a fresh client per request. This
  removes repeated TCP/TLS setup on multi-turn runs. The client is created with no
  request timeout on purpose, so long-lived streams are never cut off.
- **Breaking: `Agent::prompt` / `continue_run` now return `AgentError::ModelFailed` when
  the model reports a terminal error**, instead of returning `Ok(())`. A model failure was
  already reported as an event and via `Agent::error_message`, but returning it as well
  means a caller writing `prompt(..).await?` can no longer mistake the failure for
  success. A user-initiated `abort` is still not an error. The CLI is unchanged in
  behaviour (it still exits non-zero on `--print`, and the REPL still survives a failed
  turn).
- **Breaking: `AgentEvent::AgentEnd` carries `message_count: usize` instead of the full
  `messages` transcript.** The transcript is available via `Agent::messages`, so cloning
  it into every subscriber — and, with `--log`, onto disk — was pure overhead at the end
  of every run. Update `AgentEnd { messages }` matches to `AgentEnd { message_count }`.
- **Breaking: MSRV raised from 1.88 to 1.95.** The floor is now set by `rustyline` 19
  (the REPL line editor), which uses the `cfg_select!` macro stabilized in Rust 1.95
  (`rust-lang/rust#115585`); every other dependency builds on older toolchains. A new CI
  job builds the whole workspace on 1.95, so the floor is verified rather than asserted.
  Per [STABILITY.md](STABILITY.md) a MSRV rise is a **minor** change.

### Fixed
- **An OpenAI stream's mid-stream error, and a truncated tool call, now fail the turn.** An
  `{"error": ...}` object in the stream was ignored, so the turn ended as an empty success and
  the failure was invisible; it now ends with a terminal error, and nothing after it is read.
  And a stream that carried a tool call but no `finish_reason` (a cut-off connection) defaulted
  to `Stop`, silently dropping the call; it now fails instead.
- **Provider retries respect `Retry-After`, jitter the delay, and skip `409`.** The backoff
  ignored the server's `Retry-After` (both the delta-seconds and HTTP-date forms), used a
  fixed schedule that let many clients retry in lockstep, and treated `409 Conflict` as
  transient. A retry now waits at least as long as the server asks (clamped to a minute) and
  jitters the delay within the backoff window; a `409` is no longer retried.
- **A stalled provider stream now fails instead of hanging the turn.** Only the *connect*
  phase was bounded, so a server that accepted the request and then went silent (a half-open
  connection, a gateway that stops sending) left the stream waiting forever. The HTTP client
  now also sets a per-read (idle) timeout — generous by default, and overridable with
  `with_read_timeout` on either provider — so a silent connection ends the stream with an
  error. The whole-request timeout is deliberately *not* set, so a long answer is never cut
  off mid-stream.
- **`mcp-http` treats a 429 or 5xx as an outage, not a business failure.** The transport
  parsed the body first, so a gateway error that carried a JSON-RPC `error` (a common shape
  for a rate-limited or overloaded endpoint) was read as the server refusing the call — the
  breaker stayed closed and the source was never isolated during an outage. A 429 or 5xx now
  trips the breaker regardless of the body; a 4xx the request itself caused still reads as a
  business failure.
- **MCP replies are matched by direction, and `tools/list` pagination is bounded.** Both
  transports matched a response by `id` alone, but a server→client *request* also carries an
  `id`; a same-id request was taken as the answer and read as a silent empty success. A reply
  must now echo the `id` *and* carry no `method`. The `tools/list` cursor loop is also capped
  at 100 pages, so a server that returns a fresh cursor forever fails at load time instead of
  hanging.
- **`rsagent tools doctor` now fails the gate on every problem.** It previously exited `0`
  for a `rsagent.toml` that could not be parsed (falling back to the built-ins) and for a
  source that reports `Unhealthy`, so the documented CI gate always passed. It now counts
  config problems and unhealthy sources and exits non-zero if there are any.
- **`--print` sends extended reasoning to stderr.** With `--thinking`, the reasoning
  deltas were written to stdout, so `rsagent --print --thinking high "…" > answer.md`
  mixed the reasoning into the answer file. Reasoning now goes to stderr in a one-shot run
  (stdout stays the answer alone, as the contract already promised); the REPL is unchanged.
- **`read_file` no longer strips `\r`, so editing a CRLF file works.** The reader kept the
  `\r` out of its output while `edit_file` matched byte-for-byte, so reading a Windows file
  and pasting a line back as `old_string` always failed with `old_string not found`. The
  reader now returns each line exactly as it is on disk.
- **The circuit breaker can no longer be closed by a stale success, or wedge half-open.**
  A `record_success` now only resets a `Closed` or `HalfOpen` breaker; a late success from
  a request issued before the trip is ignored, so a concurrent source cannot re-open the
  gate the breaker just shut. And a half-open probe whose future is dropped now returns the
  breaker to `Open` after one cooldown (instead of staying half-open forever), so a source
  that failed a probe can still heal.
- **Non-built-in tools now declare their real `Permission` category.** The `web`,
  `mcp-http`, and `mcp-stdio` tools were falling back to the default `exec` category, so
  `--allow network` could not let them through while `--allow exec` silently auto-approved
  authenticated HTTP calls (a permission *widening*, against D15's "most restrictive by
  default"). They now report `network`; `local` and `skill` correctly stay `exec`. This also
  makes `rsagent tools list` show the right category.
- **Anthropic: an `error` event no longer ends as a success.** The parser emitted a
  terminal `Done` after an `error`, and consumers that take the last terminal event read
  the failure as an empty success (exit `0`, no error in the transcript). `finish` now
  suppresses `Done` once an error has been seen.
- **Anthropic: extended thinking is replayed.** The assistant turn now re-sends its
  `thinking` blocks (with signatures) before text and tool calls, so `--thinking` combined
  with tool use works past the first turn instead of being rejected with a 400.
- **Streaming no longer corrupts multi-byte characters split across network chunks.** The
  SSE buffer holds raw bytes and decodes only complete lines, so a UTF-8 character straddling
  two chunks (common for Chinese and emoji) is no longer replaced with U+FFFD.
- **A subprocess can no longer hang a turn.** One deadline now covers writing the request,
  waiting for exit, and draining the pipes; after a short grace the drain is abandoned, and a
  timed-out `bash` command is killed as a process tree on Windows. A command that backgrounds
  a daemon holding the inherited pipes no longer blocks forever.
- **`mcp-stdio` stderr trimming no longer panics on a UTF-8 boundary.** The tail is cut on a
  character boundary instead of a raw byte offset, so non-ASCII server diagnostics are kept
  instead of silently killing the drain task.
- **Sessions from a newer rsagent are no longer overwritten.** Such a file is refused and the
  session becomes read-only, instead of being rewritten as an empty v1 transcript. An existing
  empty `.jsonl` file is rewritten with a header rather than being left permanently unreadable,
  and an append is only taken when the on-disk file is still a clean prefix of the transcript
  (checked by fingerprint, so context compaction cannot strand dropped turns).
- **A project `.env` can no longer redirect credentials.** `RSAGENT_BASE_URL` and
  `RSAGENT_USER_CONFIG` are ignored when they come from an auto-discovered `.env` (a value in
  the real environment or a flag still wins), closing a way for a cloned repo to send your API
  key to an attacker's endpoint.
- A failing run's error event is now rendered before the process returns. Previously a
  model error could be queued but never printed when the run ended in the same poll.
- HTTP providers apply a 10-second **connect** timeout, so an unreachable endpoint fails
  fast instead of hanging the first turn. There is still deliberately no *request*
  timeout, so long-lived streams are never cut off.
- The provider's `missing API key` error now names the environment variable(s) it looks
  for, so a library caller gets a fix hint too, not just a diagnosis.
- Resuming a `.jsonl` session that ends in a truncated line (a crash mid-write) now says
  so on stderr. The partial record was already dropped and the file rewritten, but the
  loss was silent.
- The `search` and `glob` tool docs linked a private constant, producing a `rustdoc`
  warning. Public docs no longer link a private item, and CI now builds the docs with
  warnings denied, so a broken intra-doc link fails the build instead of reaching a reader.
- The `local` source test `a_program_that_does_not_speak_the_protocol_fails_to_load` no
  longer relies on `echo` being an executable: `echo` is a shell builtin, so on Windows it
  resolves to no program (and under MSYS to one that may not start). It now writes a tiny
  fixture program instead, so the test runs on every platform.
- **Memory: a note is appended, not rewritten, so concurrent writers no longer lose notes.**
  `Memory::append` was a read-modify-write of the whole file, so two processes sharing one
  `--memory` file clobbered each other (last writer wins). It now appends a single line and
  rewrites only once the file has outgrown its cap.
- **Memory: a single note is capped** at `MAX_NOTE_CHARS` (2000). An oversized note — which
  the model can write — was folded verbatim into the system prompt and could crowd the
  context budget; it is now truncated with a visible marker, like an oversized tool result.

### Performance
- The agent indexes its tools by name, so a tool call resolves its tool in O(1) instead
  of scanning the list (which grows with MCP sources) up to four times per call.
- Context compaction measures each message once, instead of re-serializing every message
  it considers while trimming.

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
