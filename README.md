# rsagent

[![CI](https://github.com/whispor/rsagent/actions/workflows/ci.yml/badge.svg)](https://github.com/whispor/rsagent/actions/workflows/ci.yml)

A minimal Rust coding agent: an LLM turn loop that can read, write, and edit files
and run shell commands, exposed through a small terminal CLI.

It is organized as a three-crate workspace, mirroring the layering of the
[pi](https://github.com/badlogic/pi-mono) project:

| Crate | Role |
| --- | --- |
| `rsagent-ai` | Transport layer: providers, model catalog, normalized streaming events. Knows nothing about agents or UI. |
| `rsagent-agent` | Agent loop: turns, tool execution, event stream. Knows nothing about a specific UI. |
| `rsagent` | The CLI binary: argument parsing, terminal rendering, session handling. |

## Learn more · 深入学习

New to the code? Three entry points. · 初次读代码？三个入口。

- **[Architecture · 架构](docs/ARCHITECTURE.md)** — how the agent loop works, the
  layering, the event flow, and a suggested reading order. · 智能体循环如何运作、分层、
  事件流，以及建议的源码阅读顺序。
- **[Extensibility · 拓展性规范](docs/EXTENSIBILITY.md)** — the standard recipe for
  adding a tool, provider, tool source, or platform path, plus the invariants a change
  must not break and the PR checklist. · 新增工具 / 提供商 / 工具来源 / 平台适配的标准
  流程、必须遵守的不变量，以及提交前检查清单。
- **[Stability · 稳定性](STABILITY.md)** — the versioning promise, the public API, the
  MSRV, and the no-telemetry statement. · 版本承诺、公开 API、MSRV，以及不收集遥测的声明。
- **[Changelog · 变更日志](CHANGELOG.md)** — what changed in each version. · 每个版本的变更。
- **[The life of a tool call · 一次工具调用的完整旅程](docs/tool-call-journey.html)** —
  a visual walkthrough of one turn, step by step, with the real function and event names.
  Open the file in a browser. · 一个回合的逐步可视化讲解，标注真实的函数名与事件名。
  用浏览器打开该文件即可。
- **Runnable examples · 可运行示例** — each is a real, compilable program:

  ```bash
  cargo run -p rsagent-agent --example headless         # drive the loop without the CLI · 无头驱动循环
  cargo run -p rsagent-agent --example custom_tool      # add your own tool · 新增自定义工具
  cargo run -p rsagent-ai    --example custom_provider  # add your own provider · 新增自定义提供商
  cargo run -p rsagent-agent --example end_to_end       # provider + tool together · 提供商与工具协同
  ```

Extending `rsagent` means implementing one of four plain traits — `Tool`, `Provider`,
`Approver`, or driving `Agent` directly. See
[Architecture §8 · Extension points](docs/ARCHITECTURE.md#8-extension-points--扩展点),
and the [extensibility spec](docs/EXTENSIBILITY.md) for the step-by-step recipe and the
invariants a change must not break.
扩展 `rsagent` 就是实现四个普通 trait 之一——`Tool`、`Provider`、`Approver`，或直接
驱动 `Agent`。见[架构 §8 · 扩展点](docs/ARCHITECTURE.md#8-extension-points--扩展点)，
以及[拓展性规范](docs/EXTENSIBILITY.md)——逐步配方与不可破坏的不变量。

## Quick start

```bash
# Build everything and run the test suite
cargo build --workspace
cargo test --workspace
```

### Try it offline (no network, no API key)

The `faux` provider replays scripted responses, so the whole loop runs locally:

```bash
cargo run -p rsagent -- --faux --print "hello"
# (faux) you said: hello
```

`--faux-tool` scripts a tool call, so the approval path also runs offline. `SPEC` is a tool
name, or `name:JSON_ARGS`; repeat the flag for a sequence of calls:

```bash
cargo run -p rsagent -- --faux --print go --allow write \
  --faux-tool 'write_file:{"path":"out.txt","content":"hi"}'
```

### Talk to a real model

Any OpenAI Chat Completions-compatible endpoint works (OpenAI, DeepSeek, Groq,
Together, OpenRouter, Ollama, vLLM, ...). The easiest way is to put your key and model
in a `.env` file at the project root — it is loaded automatically at startup:

```bash
cp .env.example .env
# then edit .env:
#   RSAGENT_API_KEY=sk-...
#   RSAGENT_MODEL=deepseek-chat

cargo run -p rsagent -- --base-url https://api.deepseek.com/v1 --print "list the files"
```

`.env` is git-ignored. You can also pass everything as flags or export the same
variables (`RSAGENT_BASE_URL`, `RSAGENT_API_KEY`, `RSAGENT_MODEL`).

> **The base URL is not read from `.env`.** A `.env` that arrived with a `git clone`
> could otherwise point `RSAGENT_BASE_URL` at an attacker's server and send your real
> API key there. Set the endpoint with `--base-url` or `RSAGENT_BASE_URL` in your shell
> environment; a value in `.env` is ignored with a warning. See
> [Configuration](#configuration).

A local Ollama server:

```bash
cargo run -p rsagent -- \
  --base-url http://localhost:11434/v1 \
  --model llama3.1 \
  --print "explain this project"
```

An Anthropic endpoint (it speaks the Messages protocol, so pass `--anthropic`):

```bash
ANTHROPIC_API_KEY=sk-ant-... cargo run -p rsagent -- \
  --anthropic \
  --model claude-3-5-sonnet-latest \
  --print "explain this project"
```

If no key is found for the default endpoint, rsagent stops **before** sending anything and
prints the exact variable to set (`OPENAI_API_KEY` or `ANTHROPIC_API_KEY`) plus the
`--api-key` flag. A custom `--base-url` is treated as a local endpoint that may not need a
key, so that case only warns.

### Interactive mode

Run without `--print` to get a REPL:

```bash
cargo run -p rsagent -- --model gpt-4o-mini
> summarize Cargo.toml
...
> exit
```

When stdin is a terminal the REPL has line editing and history (the up arrow recalls
earlier prompts, Ctrl-R searches them). Piped input is read line by line as before.

A line starting with `/` is a command rather than a prompt:

| Command | Effect |
| --- | --- |
| `/help` | Show the command list. |
| `/clear` | Forget the current transcript (also rewrites the `--session` file). |
| `/exit` | Quit (also `exit`, `quit`). |

### Project instructions (`AGENTS.md`)

At startup `rsagent` looks for an `AGENTS.md` at or above the working directory and
folds the nearest one into the system prompt, under a `# Project instructions` heading.
This repository ships one at its root as an example.

Precedence is `--system` > `AGENTS.md` > built-in prompt: `--system` replaces the prompt
outright, and `AGENTS.md` is *added to* the built-in prompt rather than replacing it.

### Tool sources (`rsagent.toml`)

Tools are grouped into **sources**. The built-in tools (`read_file` … `bash`) are one
source; a project can add more without touching the loop. Sources are declared in a
project-local `rsagent.toml`, found at or above the working directory:

```toml
# rsagent.toml
[[tools.sources]]
kind = "builtin"          # the compiled-in tool set
id = "core"
exclude = ["bash"]        # same as --no-bash

[[tools.sources]]
kind = "local"            # a program run as a subprocess
id = "release"
command = "./scripts/release-tool"
args = ["--verbose"]      # fixed args, before the phase flag
timeout_secs = 60         # per call; default 60
prefix = "rel"            # optional: tools appear to the model as rel__<name>

[[tools.sources]]
kind = "mcp-stdio"        # a Model Context Protocol server, over stdio
id = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
prefix = "fs"             # MCP tool names are often generic, so a prefix is wise

[[tools.sources]]
kind = "mcp-http"         # a Model Context Protocol server, over HTTP
id = "remote"
url = "https://mcp.example.com/mcp"
prefix = "rm"
timeout_secs = 30
headers = { Authorization = "Bearer sk-..." }      # sent on every request

[[tools.sources]]
kind = "skill"            # a folder of scripts declared by a manifest
id = "team"
path = "./skills/team"    # holds SKILL.md and skill.toml
prefix = "team"           # tools appear to the model as team__<name>

[[tools.sources]]
kind = "web"              # one HTTP endpoint wrapped as one tool
id = "weather"
url = "https://api.example.com/weather"
method = "GET"            # GET/HEAD: args become query params; others: a JSON body
name = "get_weather"      # the tool name (default: the source id)
description = "Look up the weather for a city."
requires_approval = false
[tools.sources.parameters] # JSON Schema for the model
type = "object"
properties.city = { type = "string" }
required = ["city"]
```

With no `rsagent.toml` you get exactly the built-ins, narrowed by `--tools` / `--no-bash`.
A file that exists but does not parse is reported on stderr, never silently ignored; a
single source that fails to load is reported and **skipped**, so one broken source cannot
take the whole agent down. `--list-tools` prints every source, its health, and the
assembled tools.

The precedence is **CLI flags > environment > project `rsagent.toml` > user `rsagent.toml`
> built-in defaults**: `--tools` replaces a built-in source's `only`, and `--no-bash` adds
to its `exclude`. When both a user-level and a project-local file exist, their sources are
merged by `id` (the project entry wins).

#### Managing tool sources

`rsagent tools` edits `rsagent.toml` for you and probes what you have:

```bash
rsagent tools list                     # sources, health, and the assembled tools
rsagent tools doctor                   # probe every source; non-zero exit on failure
rsagent tools add weather --kind web --url https://example.com/api --name get_weather
rsagent tools remove weather
rsagent tools reset weather            # clear persisted circuit-breaker state
```

`add` / `remove` edit the file **in place**, preserving comments and formatting
(`toml_edit`), so a hand-written `rsagent.toml` stays readable. The semantics are
"edit config plus an idempotent preparation step": `add` writes the entry first, then
best-effort runs the source's install hook (an offline endpoint or a missing program is
reported, not rolled back), and `remove` only drops the entry and calls the source's
uninstall hook. **Neither ever deletes your data.** `add` refuses a duplicate `id`
rather than overwriting it. `doctor` is a usable CI gate: it exits non-zero if any source
fails to load, printing a kind-specific fix hint.

Each source guards itself with a circuit breaker (see `docs/EXTENSIBILITY.md` §3.5). Its
state is snapshotted to `.rsagent-breakers.json` in the working directory after every run
and restored on the next, so a source that was cooling down stays out of rotation.
`rsagent tools list` / `doctor` show that state, and `rsagent tools reset [id]` clears it
(omit `id` to clear all) so a cooling-down source is retried at once.

#### The `local` source protocol (version 1)

A `local` source is any program that speaks a small JSON protocol. `rsagent` invokes it
twice, appending a phase flag after `args`:

- `program <args...> --describe` prints one JSON object on stdout:

  ```json
  {"protocol": 1, "tools": [
    {"name": "release", "description": "Cut a release.",
     "parameters": {"type": "object", "properties": {}},
     "requires_approval": true}
  ]}
  ```

- `program <args...> --call` reads one JSON object on stdin
  (`{"tool": "release", "arguments": {...}}`) and prints one JSON object on stdout:
  `{"content": "..."}` (success) or `{"error": "..."}` (failure). `content` may be any
  JSON value; a string is used verbatim, anything else is rendered as compact JSON.

`requires_approval` defaults to `true`: an arbitrary program may do anything, so it must
opt *out* of the approval prompt, never accidentally into it. Tools from a `local` source
run one at a time (never concurrently), and each source has a circuit breaker: after
three consecutive *connection-class* failures (cannot start, timeout, non-zero exit,
non-JSON output) it is taken out of the tool list and probed again after a cooldown. A
well-formed `{"error": ...}` reply is a *business* failure — the program is working, so it
does not trip the breaker.

#### The `mcp-stdio` source (Model Context Protocol)

An `mcp-stdio` source is a **Model Context Protocol** server spoken to over stdio — the
same wire format most MCP servers already use, so an existing server works with no
adapter. `rsagent` launches `command args...` once, keeps the process alive for the whole
run, and speaks JSON-RPC 2.0 with it: one JSON message per line on stdout, UTF-8, with all
logging expected on stderr (stdout carries protocol traffic only).

The handshake follows the MCP lifecycle: `initialize` (the client sends its
`protocolVersion`, `capabilities`, and `clientInfo`) → the server echoes a version plus its
own `capabilities`/`serverInfo` → the client sends `notifications/initialized`. Tools are
then discovered with `tools/list` (following `nextCursor` pagination to the end) and
invoked with `tools/call`. A tool's `inputSchema` becomes its parameter schema, and
`annotations.readOnlyHint` sets `requires_approval` — a read-only tool skips the approval
prompt, anything else defaults to *requiring* it, consistent with the `local` source.

`timeout_secs` (default 60) bounds each request. If a call fails at the transport level
(the process died, output was not JSON, a request timed out) the session is dropped and
reconnected on the next call; `mcp-stdio` shares the same per-source circuit breaker as
`local`, so a server that cannot be reached is taken out of the tool list after three
consecutive connection-class failures rather than retried forever. The child is
`kill_on_drop`-ed, so it does not outlive the agent. Because MCP tool names are generic
(`read`, `list`, …), giving the source a `prefix` is strongly recommended.

#### The `mcp-http` source (Model Context Protocol over HTTP)

An `mcp-http` source is an MCP server reached over the network instead of a subprocess —
for a hosted server, or one you would rather not spawn locally. It implements the MCP
**Streamable HTTP** transport: `rsagent` POSTs JSON-RPC messages to the single `url`, and
accepts either reply encoding — a plain JSON object (`application/json`) or a Server-Sent
Events stream (`text/event-stream`), which the spec allows a server to use. The same
`initialize` → `notifications/initialized` → `tools/list` → `tools/call` lifecycle and the
same `inputSchema` / `readOnlyHint` handling apply as for `mcp-stdio`.

If the server answers `initialize` with an `Mcp-Session-Id` header, every later request
echoes it back; otherwise each request stands alone. `headers` are added to every request,
which is how you supply an `Authorization` token (or any other tenant/API header). Because
HTTP requests are independent, these tools are **not** sequential — the agent may run
several at once, unlike `mcp-stdio`.

`timeout_secs` (default 60) bounds each request. A transport failure (the endpoint is
unreachable, a request times out, a non-success status with no JSON-RPC error) drops the
session and re-initializes on the next call, and counts toward the same per-source circuit
breaker as the other MCP sources; a well-formed JSON-RPC `error` does not.

#### The `skill` source

A `skill` source is a **folder** of scripts, declared by a manifest — the smallest unit
for packaging a team's tools alongside its notes. Point `path` at a directory that holds

- `SKILL.md` — the skill's documentation, and the marker that the folder *is* a skill;
- `skill.toml` — one `[[tool]]` entry per tool:

```toml
# skills/team/skill.toml
[[tool]]
name = "lint"
description = "Lint a path with the team's rules."
command = "scripts/lint.sh"        # relative to the skill folder, or absolute
args = ["--strict"]                # fixed args, before the tool's own input
requires_approval = false          # default: true
[tool.parameters]                  # JSON Schema for the model; default is an empty object
type = "object"
properties.path = { type = "string" }
required = ["path"]
```

Each tool runs its `command` as a fresh subprocess with the tool's arguments written to
stdin as one JSON object, and reads the same `{"content": ...}` / `{"error": ...}` reply as
a `local` source — so a script is written the same way whichever source runs it. The
difference is where the tool list comes from: `local` asks the program (`--describe`),
while a skill reads its manifest, so the folder is self-describing **without being run**.
The manifest is validated at load time: a missing `SKILL.md`, a missing or malformed
`skill.toml`, or a `command` that does not exist is reported and the source is skipped.

`requires_approval` defaults to `true` for the same reason as `local` — an arbitrary
script may do anything. Because each call is its own process (not one long-lived program),
these tools are **not** sequential, and each source has the same circuit breaker as the
others: three consecutive connection-class failures (cannot start, timeout, non-zero exit,
non-JSON output) take it out of the tool list until a cooldown passes.

#### The `web` source

A `web` source wraps **one HTTP endpoint as one tool** — the lightest way to reach an
existing HTTP API, with no server changes. The model's arguments become the request:

- `GET` (the default) and `HEAD` send them as **query parameters** — a string verbatim,
  anything else as JSON, and a `null` dropped so an omitted optional field stays omitted;
- every other method sends the arguments object as the **JSON request body**.

`headers` are added to every request (how you supply an `Authorization` token). The
response body becomes the tool's result; when it parses as JSON it is also kept as
structured `details` for the UI. `name`, `description`, and `parameters` describe the tool
to the model — `name` defaults to the source id, `parameters` to an empty object.

`requires_approval` defaults to `true`: a `GET` on a read-only endpoint can opt out, but
anything else may change state on the far side. `timeout_secs` (default 60) bounds each
request.

The failure split differs from the subprocess sources on purpose. A **transport** failure
(the endpoint is unreachable, a request times out) is connection-class and trips the
breaker. An **HTTP response is not** — once the server has answered, it is reachable, so a
`4xx` or `5xx` is a business failure that is reported to the model as an error but leaves
the breaker closed. This keeps a `400` caused by bad arguments from taking the whole
source out of the tool list (see decision D9).

## CLI reference

| Flag | Env | Description |
| --- | --- | --- |
| `-m, --model <ID>` | `RSAGENT_MODEL` | Model id to use (default `gpt-4o-mini`). |
| `-p, --print <PROMPT>` | | Run one prompt and exit. |
| `--json` | | Emit the agent event stream as JSON lines on stdout, for CI and scripts (requires `--print`). |
| `--log <FILE>` | | Append the agent event stream to `FILE` as JSON lines, in every mode, for post-mortem diagnosis. |
| `-v, --verbose` | | Print extra diagnostics on stderr (resolved config, sources, per-turn timing). |
| `--base-url <URL>` | `RSAGENT_BASE_URL` | Base URL of an OpenAI-compatible endpoint. |
| `--api-key <KEY>` | `RSAGENT_API_KEY` | API key; overrides the provider's env lookup. |
| `--faux` | | Use the offline scripted provider. |
| `--faux-tool <SPEC>` | | Script a `--faux` tool call (repeatable); `SPEC` is `name` or `name:JSON`. |
| `--anthropic` | | Talk to an Anthropic Messages API endpoint instead of an OpenAI-compatible one. |
| `--cwd <DIR>` | | Working directory the agent operates in (default `.`). |
| `--system <PROMPT>` | | Override the system prompt (suppresses `AGENTS.md`). |
| `--max-turns <N>` | | Maximum model turns in a single run (default `50`). |
| `--temperature <F>` | | Sampling temperature (provider default when unset). |
| `--max-tokens <N>` | | Maximum tokens to generate per response (provider default when unset). |
| `--thinking <LEVEL>` | | Extended-reasoning budget: `off` (default), `minimal`, `low`, `medium`, `high`, `xhigh`, `max`. |
| `-y, --yes` | | Auto-approve every tool call. |
| `--allow <CATEGORY>[,...]` | | Auto-approve these categories without prompting: `read`, `write`, `exec`, `network`. |
| `--session <FILE>` | | Persist the conversation to a JSON file, resuming it if it exists. |
| `--continue` | | Resume the default session file (`.rsagent-session.json`). |
| `--memory <FILE>` | | Enable the `remember`/`recall` tools backed by an append-only JSONL note file, and fold recent notes into the system prompt. |
| `--no-color` | `NO_COLOR` | Disable ANSI colors. |
| `--allow-outside-cwd` | | Let read tools access paths outside the working directory. |
| `--tools <LIST>` | | Enable only the named tools (comma-separated); default is all built-ins. |
| `--no-bash` | | Disable the `bash` tool. |
| `--list-tools` | | List the tool sources and the assembled tools, then exit. |
| `-V, --version` | | Print the version and exit. |

**Subcommands** (run before anything else; they need no model or API key):

| Command | Description |
| --- | --- |
| `rsagent tools list` | List every source, its health, and the assembled tools. |
| `rsagent tools doctor` | Probe each source and report problems with a fix hint; non-zero exit on failure. |
| `rsagent tools add <id> --kind <KIND> [fields]` | Append a `[[tools.sources]]` entry to `rsagent.toml`. |
| `rsagent tools remove <id>` | Remove a source's entry from `rsagent.toml`. |
| `rsagent tools reset [id]` | Clear persisted circuit-breaker state (all sources when `id` is omitted). |

Run `rsagent tools add --help` for the per-kind fields (`--url`, `--command`, `--path`,
`--args`, `--prefix`, …).

## Built-in tools

| Tool | Approval | Category | Description |
| --- | --- | --- | --- |
| `read_file` | no | `read` | Read a file as UTF-8 text, each line prefixed with its line number; `offset`/`limit` read a range. |
| `list_dir` | no | `read` | List directory entries. |
| `search` | no | `read` | Search files under a directory for a literal substring (`path:line: text`). |
| `glob` | no | `read` | Find files by glob pattern (`*`, `?`, `**`), one path per line. |
| `webfetch` | yes | `network` | Fetch an HTTP(S) URL and return it as text (HTML is converted). |
| `write_file` | yes | `write` | Write a file, creating parent directories. |
| `edit_file` | yes | `write` | Replace an exact string in a file. |
| `bash` | yes | `exec` | Run a shell command (killed after a timeout). |

`search` and `glob` do not descend into build-output or vendor directories (`target`,
`node_modules`, `.git`, `.venv`, `venv`, `__pycache__`), so matches are not buried
under generated files. The directory you point them at is still searched.

Read tools (`read_file`, `list_dir`, `search`, `glob`) are confined to the working
directory by default: a path that resolves outside it — through `..` or a symlink — is
rejected. Pass `--allow-outside-cwd` to lift the restriction. The approval prompt for
`write_file` and `edit_file` flags any target outside the working directory, so a
mistaken path is visible before you approve it.

The tool set can be narrowed with `--tools <LIST>` (only the named tools are enabled)
and `--no-bash` (drops `bash` even if `--tools` listed it). An unknown name in `--tools`
is reported on stderr rather than silently ignored.

Two further tools, `remember` and `recall`, are offered only when `--memory <FILE>` is
set — they are not part of the default built-in set. See [Memory](#memory).

`webfetch` requires approval: it is the only tool that contacts an arbitrary external
host, so the prompt shows the exact URL. It follows redirects, times out after 30s, and
stops reading after 1 MiB. Only `http` and `https` URLs are allowed.

Tools marked **yes** require approval before they run. In interactive mode the agent
explains, in plain language, exactly what it is about to do and waits for your answer:

```
? 需要你确认
  操作：覆盖已有文件：notes.md（原有内容会被替换）
  说明：将写入约 128 字节内容。
  → 输入 y 再回车 = 只允许这一次
  → 输入 a 再回车 = 允许，且本次会话后续操作都不再询问
  → 直接回车 = 拒绝（我会换个方式继续）
```

The prompt is tailored per request: a shell command shows the exact command and a
safety note; `write_file` says whether it will create a new file or overwrite an
existing one; `edit_file` shows the exact text being replaced.

Three answers are accepted:

| Answer | Effect |
| --- | --- |
| `y` + Enter | Allow this call only. |
| `a` + Enter | Allow this call and stop asking for the rest of the session. |
| Enter (or `n`) | Deny; the model is told and usually tries another approach. |

To skip the prompts from the start, launch with `--yes` (allow every call). To trust
only some kinds of side effects, use `--allow <CATEGORY>[,...]`: the categories are
`read`, `write`, `exec`, and `network`, and a listed category is auto-approved without a
prompt. For example, `--allow network` lets `webfetch` run unattended while writes and
shell commands still ask:

```bash
rsagent --allow network,write "fetch the changelog and save it to notes.md"
```

Each tool's category is shown by `rsagent tools list`. With `--print`, mutating tools
are denied unless `--yes` (or a matching `--allow`) is passed, so scripts never mutate
state by surprise. Long-running `bash` output streams to the terminal line by line, and
the command is killed after 120s.

## Sessions

Pass `--session <FILE>` to save the transcript as JSON and resume it on the next
run, so a multi-step task can span several invocations:

```bash
rsagent --session work.json --print "read Cargo.toml"
rsagent --session work.json --print "now summarize what you read"
```

On resume the saved system prompt is restored, unless `--system` is passed to override
it. `--continue` is a shorthand for `--session .rsagent-session.json`, so a single
project-local transcript can be resumed without repeating the path.

The file is a versioned envelope (`{"version":1,"context":{...}}`), so an older build
recognizes — and refuses rather than misreads — a file written by a newer one. Files
in the original bare-`Context` format are still read.

Name the file with a `.jsonl` extension for **append-mode** sessions, which suit long
transcripts: the file is a header line followed by one message per line, and each run
appends only the new messages instead of rewriting the whole file. It resumes the same
way, and a half-written last line (from a crash mid-append) is dropped on load.

```bash
rsagent --session work.jsonl --print "read Cargo.toml"   # header + 2 message lines
rsagent --session work.jsonl --print "now summarize it"  # appends 2 more lines
```

All session writes are atomic (a temp file plus a rename), so a crash never leaves half a
document behind. The JSONL append mode is **not** safe for two concurrent runs sharing one
file — their lines can interleave — so point each run at its own session file.

## Memory

A session ends when the process does, and compaction throws turns away for good. Pass
`--memory <FILE>` to give the agent a small, durable memory that outlives both: an
append-only JSON Lines log of timestamped notes.

```bash
rsagent --memory notes.jsonl "remember that the release branch is release/2.0"
rsagent --memory notes.jsonl "what is the release branch?"   # the note is already in context
```

With `--memory` set, two tools become available:

| Tool | Category | Description |
| --- | --- | --- |
| `remember` | `write` | Append a timestamped note to the memory file. |
| `recall` | `read` | Return the most recent notes (up to `limit`). |

The newest notes are also folded into the system prompt on every request, so the model
sees them without having to call `recall`; the store keeps the most recent 200 notes and
drops older ones. A single note is capped at 2,000 characters, so one oversized note
cannot crowd the prompt. When the transcript is compacted, the dropped turns are distilled
into a single `[compacted]` note — what context management discards still survives on disk.

Memory is a plain file you own: read, edit, or delete it with any tool. Appending writes a
single line rather than rewriting the file, so two processes sharing one `--memory` file do
not overwrite each other's notes; the file is rewritten only once it has outgrown its cap.
A truncated last line (from a crash mid-append) is dropped on load, exactly like an
append-mode session.

## Exit codes

In `--print` mode the process exits non-zero when the run did not produce a usable
answer, so scripts and CI can detect failures:

| Code | Meaning |
| --- | --- |
| `0` | The run finished and the model produced an answer. |
| `1` | A model-level failure at run time (network error, provider error) or the turn limit was hit. |
| `2` | A usage or configuration error caught before the run: bad flags, an unreadable `--log` path, a missing API key for the default endpoint, or a failed subcommand. |

A missing API key is detected before anything is sent and exits `2` with the exact
variable to set. `--json` keeps its machine contract instead, so there the failure
surfaces as an error event on stdout (exit `1`).

The interactive REPL never exits on a failed turn; it prints the error and waits for
the next prompt.

In `--print` mode the answer owns stdout while everything else — tool activity, token
usage, approval prompts, warnings, and extended reasoning (`--thinking`) — goes to stderr,
so the output can be redirected cleanly:

```bash
rsagent --print "summarize this repo" > answer.md
```

### JSON output

Pass `--json` (with `--print`) to get the agent's event stream as JSON lines instead of
terminal output: one object per line, each tagged with a `type` field, in the order the
events occur (`agentStart`, then per-turn `turnStart` / `messageStart` / `messageUpdate`
/ `messageEnd` / `turnEnd`, and finally `agentEnd`). Messages and streaming deltas are
nested objects in the same camelCase, `type`-tagged shape, so one stream carries the
whole run:

```bash
rsagent --json --print "list the files here" | jq -c 'select(.type == "toolExecutionEnd")'
```

The exit-code semantics below are unchanged: a run that fails still exits non-zero, so a
consumer can rely on both the stream and the code.

### Event log

Pass `--log <FILE>` to append the same JSON-lines event stream to a file. Unlike
`--json`, it works in **every** mode — including the interactive REPL — so an unattended
run still leaves a machine-readable trace. The file is append-only, so a trace accumulates
across runs, and a crash mid-write only ever truncates the last line:

```bash
rsagent --log run.jsonl --yes --print "refactor the parser"
```

`-v, --verbose` prints extra diagnostics on stderr — the resolved config file(s), each
source with its tool count, and per-turn timing — which pairs well with `--log` when a run
behaves unexpectedly.

## Configuration

Environment variables are read from the process environment. A `.env` file in the
working directory (or any parent) is loaded at startup (simple `KEY=VALUE` lines;
existing environment variables win). Tool sources are configured by **two**
`rsagent.toml` files that are layered together: a **user-level** file (found at
`$XDG_CONFIG_HOME/rsagent/`, `%APPDATA%\rsagent\`, or `~/.config/rsagent/`; override with
`RSAGENT_USER_CONFIG`) and a **project-local** one found at or above the working directory.
Entries are merged by source `id`, and a project entry overrides a user entry with the same
id, so a repo can pin or override a globally-declared source. The overall precedence is
**CLI flags > environment > project `rsagent.toml` > user `rsagent.toml` > built-in defaults**.

| Variable | Purpose |
| --- | --- |
| `RSAGENT_BASE_URL` | Base URL of an OpenAI-compatible endpoint. **Not read from `.env`.** |
| `RSAGENT_API_KEY` | API key for that endpoint. |
| `RSAGENT_MODEL` | Model id to use. |
| `ANTHROPIC_API_KEY` | API key used when `--anthropic` is passed. |
| `RSAGENT_USER_CONFIG` | Override the user-level `rsagent.toml` path. **Not read from `.env`.** |
| `NO_COLOR` | Disable ANSI colors when set to a non-empty value. |

> **Trust boundary.** A project `rsagent.toml` and a `.env` are treated as **executable
> input**, not data. A `[[tools.sources]]` entry can spawn a program (`local`, `mcp-stdio`,
> `skill`), so `git clone && rsagent --list-tools` runs whatever the repo declares — before
> any approval prompt. Two variables are therefore refused from `.env` outright:
> `RSAGENT_BASE_URL` (it would redirect your API key) and `RSAGENT_USER_CONFIG` (it would
> point the user config at a repo-controlled file). Only run rsagent in a repository whose
> configuration you trust.

The `--help` output shows the env variable backing each flag but never its value.
Colors are also disabled automatically when stdout is not a terminal, so redirected
output and log files stay clean.

## Notes and limitations

- **No telemetry.** rsagent collects nothing: it sends data only to the model endpoint you
  configure, the tool sources you declare, and `webfetch` targets the model chooses. See
  [STABILITY.md](STABILITY.md).
- **Minimum supported Rust version: 1.95** (declared as `rust-version` in the workspace
  `Cargo.toml`, and checked by a dedicated CI job). The floor comes from the dependency
  tree — `rustyline` 19's `cfg_select!` macro, stabilized in 1.95 — not from a language
  feature we use.
- Two wire protocols are implemented natively: OpenAI Chat Completions and Anthropic
  Messages. Both normalize into the same `StreamEvent` sequence, so the agent loop and
  UI are protocol-agnostic.
- Requests that fail before the stream starts are retried with exponential backoff
  (transient HTTP statuses and connection errors); a failure mid-stream is surfaced
  as an error event rather than retried.
- Before each request the transcript is trimmed by whole turns if it would exceed the
  model's context window (roughly three quarters of it is used, leaving room for the
  response). The first and most recent turns are always kept, so a tool call is never
  separated from its result; a `ContextCompacted` event reports how many messages were
  dropped. With `--memory`, the dropped turns are also distilled into a memory note, so
  nothing the trim removes is lost entirely.
- The `bash` tool is not sandboxed; approval prompts are the only guardrail.
- **Same-process (WASM) plugin sandboxing is a non-goal.** Third-party code runs the same
  way the built-ins do — as a subprocess (`local`, `skill`, MCP) or an HTTP call (`web`) —
  so isolation is the operating system's job, not the agent's. A `wasmtime`-style sandbox
  would add a large dependency and a second execution model for little gain; the
  subprocess/HTTP boundary already keeps a broken source from taking the agent down and
  lets the circuit breaker quarantine it.
- `--thinking` is translated per protocol: OpenAI-compatible endpoints get
  `reasoning_effort` (`xhigh`/`max` saturate at `high`), while Anthropic gets an enabled
  `thinking` block with a `budget_tokens` derived from the level (capped below
  `max_tokens`). With thinking enabled Anthropic requires the default temperature, so a
  `--temperature` is dropped in that case.

## License

MIT — see [LICENSE](LICENSE).
