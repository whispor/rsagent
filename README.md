# rsagent

A minimal Rust coding agent: an LLM turn loop that can read, write, and edit files
and run shell commands, exposed through a small terminal CLI.

It is organized as a three-crate workspace, mirroring the layering of the
[pi](https://github.com/badlogic/pi-mono) project:

| Crate | Role |
| --- | --- |
| `rsagent-ai` | Transport layer: providers, model catalog, normalized streaming events. Knows nothing about agents or UI. |
| `rsagent-agent` | Agent loop: turns, tool execution, event stream. Knows nothing about a specific UI. |
| `rsagent` | The CLI binary: argument parsing, terminal rendering, session handling. |

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

### Talk to a real model

Any OpenAI Chat Completions-compatible endpoint works (OpenAI, DeepSeek, Groq,
Together, OpenRouter, Ollama, vLLM, ...). The easiest way is to put your settings in
a `.env` file at the project root — it is loaded automatically at startup:

```bash
cp .env.example .env
# then edit .env:
#   RSAGENT_BASE_URL=https://api.deepseek.com/v1
#   RSAGENT_API_KEY=sk-...
#   RSAGENT_MODEL=deepseek-chat

cargo run -p rsagent -- --print "list the files in this directory"
```

`.env` is git-ignored. You can also pass everything as flags or export the same
variables (`RSAGENT_BASE_URL`, `RSAGENT_API_KEY`, `RSAGENT_MODEL`).

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

### Interactive mode

Run without `--print` to get a REPL:

```bash
cargo run -p rsagent -- --model gpt-4o-mini
> summarize Cargo.toml
...
> exit
```

## CLI reference

| Flag | Env | Description |
| --- | --- | --- |
| `-m, --model <ID>` | `RSAGENT_MODEL` | Model id to use (default `gpt-4o-mini`). |
| `-p, --print <PROMPT>` | | Run one prompt and exit. |
| `--base-url <URL>` | `RSAGENT_BASE_URL` | Base URL of an OpenAI-compatible endpoint. |
| `--api-key <KEY>` | `RSAGENT_API_KEY` | API key; overrides the provider's env lookup. |
| `--faux` | | Use the offline scripted provider. |
| `--anthropic` | | Talk to an Anthropic Messages API endpoint instead of an OpenAI-compatible one. |
| `--cwd <DIR>` | | Working directory the agent operates in (default `.`). |
| `--system <PROMPT>` | | Override the system prompt. |
| `-y, --yes` | | Auto-approve every tool call. |
| `--session <FILE>` | | Persist the conversation to a JSON file, resuming it if it exists. |

## Built-in tools

| Tool | Approval | Description |
| --- | --- | --- |
| `read_file` | no | Read a file as UTF-8 text. |
| `list_dir` | no | List directory entries. |
| `search` | no | Search files under a directory for a literal substring. |
| `write_file` | yes | Write a file, creating parent directories. |
| `edit_file` | yes | Replace an exact string in a file. |
| `bash` | yes | Run a shell command (killed after a timeout). |

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

To skip the prompts from the start, launch with `--yes`. With `--print`, mutating
tools are denied unless `--yes` is passed, so scripts never mutate state by surprise.
Long-running `bash` output streams to the terminal line by line, and the command is
killed after 120s.

## Sessions

Pass `--session <FILE>` to save the transcript as JSON and resume it on the next
run, so a multi-step task can span several invocations:

```bash
rsagent --session work.json --print "read Cargo.toml"
rsagent --session work.json --print "now summarize what you read"
```

## Configuration

Environment variables are read from the process environment. A `.env` file in the
working directory is loaded at startup (simple `KEY=VALUE` lines; existing
environment variables win).

## Notes and limitations

- Two wire protocols are implemented natively: OpenAI Chat Completions and Anthropic
  Messages. Both normalize into the same `StreamEvent` sequence, so the agent loop and
  UI are protocol-agnostic.
- Requests that fail before the stream starts are retried with exponential backoff
  (transient HTTP statuses and connection errors); a failure mid-stream is surfaced
  as an error event rather than retried.
- The `bash` tool is not sandboxed; approval prompts are the only guardrail.

## License

MIT.
