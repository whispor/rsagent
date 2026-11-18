# AGENTS.md

Project instructions for any agent working in this repository. Read this first.

## What this is

`rsagent` is a minimal Rust coding agent, laid out as a three-crate workspace:

- `rsagent-ai` — transport layer: providers, model catalog, normalized streaming events.
- `rsagent-agent` — the agent loop: turns, tool execution, the event stream.
- `rsagent` — the CLI binary: argument parsing, terminal rendering, session handling.

The one rule that keeps the layering honest: **each layer must not know about the layer
above it.** `rsagent-ai` knows nothing about agents or UI; `rsagent-agent` knows nothing
about a specific UI.

## The north stars

Every change is judged against these. When a change makes one of them worse, it needs a
reason in the PR description.

1. **Minimize.** Prefer less code, fewer abstractions, fewer dependencies.
2. **Match pi's shape.** Where a design choice exists, follow
   [pi](https://github.com/badlogic/pi-mono) unless there is a concrete reason not to.
3. **One obvious way.** A reader should not have to choose between two equivalent paths.
4. **No new abstraction without two callers.** Extract only when a second real use exists.
5. **Documentation is part of the change.** A new public item has a doc comment and an
   example; a new behavior is reflected in `README.md` or `docs/`.

## Before you push

The full gate, all of it required:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
```

Lints are strict on purpose (`Cargo.toml` `[workspace.lints]`, adapted from BeautifulRust):

- A `Result`-returning function needs an `# Errors` section; a panicking one needs `# Panics`.
- Do not silence a lint with `#[allow]`; use `#[expect(..., reason = "...")]` and justify it.
- Boolean parameters are not allowed (`max-fn-params-bools = 0`); take an enum instead.

## Where things live

| Concern | File |
| --- | --- |
| Platform differences (the only `#[cfg]` in the tree) | `crates/rsagent-agent/src/platform.rs` |
| Built-in tools | `crates/rsagent-agent/src/tools.rs` |
| The turn loop | `crates/rsagent-agent/src/agent.rs` |
| SSE framing shared by providers | `crates/rsagent-ai/src/providers/sse.rs` |

Adding a tool, a provider, or a platform path has a recipe in
`docs/EXTENSIBILITY.md`; follow it, and keep the invariants it lists intact.
