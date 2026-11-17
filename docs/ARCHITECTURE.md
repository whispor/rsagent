# Architecture / 架构

> 中英对照：每节先英文，后中文。
> Bilingual: each section gives English first, then Chinese.

This document explains *how* `rsagent` works, so you can read the code with a map in
hand. For *how to use* it, see the [README](../README.md). For *how to extend* it, jump
to [§8](#8-extension-points--扩展点) or the runnable programs under `examples/`.

本文说明 `rsagent` 是**如何运作**的，让你带着地图去读源码。**怎么用**见
[README](../README.md)；**怎么扩展**见 [§8](#8-extension-points--扩展点) 或 `examples/`
下的可运行示例。

---

## 1. What is an "agent" here · 这里的「智能体」是什么

An agent is a loop. Send the transcript to a model; if the model asks to call tools,
run them, append the results, and call the model again — repeat until it stops asking
for tools. Everything else (files, search, shell, streaming, approvals) is support
machinery around that loop. `rsagent` is a deliberately small implementation of it.

智能体就是一个循环：把对话记录发给模型；若模型要求调用工具，就执行工具、把结果追加
回去、再次调用模型——直到它不再要求调用工具为止。其余一切（文件、搜索、命令、流式、
审批）都是围绕该循环的支撑设施。`rsagent` 是它的一个小巧实现。

The whole loop lives in one method: [`Agent::run_turns`](../crates/rsagent-agent/src/agent.rs).
Start there if you only read one function.

整个循环就在一个方法里：[`Agent::run_turns`](../crates/rsagent-agent/src/agent.rs)。
如果只想读一个函数，从这里开始。

---

## 2. Layering · 分层

```
            ┌───────────────────────────────────────────┐
            │  rsagent            (binary / CLI)        │
            │  main · render · input                    │  args, terminal, sessions
            └──────────────────────┬────────────────────┘
                                   │ depends on
            ┌──────────────────────▼────────────────────┐
            │  rsagent-agent      (loop)                │
            │  Agent · Tool · Approver · AgentEvent     │  turns, tools, events
            └──────────────────────┬────────────────────┘
                                   │ depends on
            ┌──────────────────────▼────────────────────┐
            │  rsagent-ai         (transport)           │
            │  Provider · Models · Message · StreamEvent│  providers, models, streaming
            └───────────────────────────────────────────┘
```

Dependencies point one way only: **each layer knows nothing about the layer above it.**
`rsagent-ai` never mentions agents or a terminal; `rsagent-agent` never mentions the CLI.
That is what makes the lower two crates reusable as libraries.

依赖单向流动：**每一层都不知道它上面的层。** `rsagent-ai` 从不提及智能体或终端；
`rsagent-agent` 从不提及 CLI。正因如此，下面两个 crate 才能作为库被复用。

| Crate | Responsibility · 职责 | Key types · 关键类型 |
| --- | --- | --- |
| `rsagent-ai` | Talk to models · 与模型通信 | `Provider`, `Models`, `Context`, `Message`, `StreamEvent` |
| `rsagent-agent` | Run the loop · 运行循环 | `Agent`, `Tool`, `Approver`, `AgentEvent` |
| `rsagent` | Be a CLI · 作为命令行 | `Cli`, `Renderer`, `InputHandle` |

---

## 3. Lifecycle of one run · 一次运行的生命周期

```
CLI                     Agent                 Models/Provider            Tool
 │  prompt(text)          │                         │                     │
 ├───────────────────────►│                         │                     │
 │                        │ push user message       │                     │
 │                        │ emit AgentStart         │                     │
 │                        │                         │                     │
 │                        │ ┌── for each turn ─────────────────────────┐ │
 │                        │ │ compact context (drop old turns)         │ │
 │                        │ │ emit TurnStart                           │ │
 │                        │ │ stream(model, ctx) ────►│                │ │
 │                        │ │ ◄──── StreamEvent* ─────┤  (SSE/HTTP)    │ │
 │                        │ │ emit MessageUpdate*                      │ │
 │                        │ │ if the model asked for tools:            │ │
 │                        │ │   approval? ────────────────────────────┼─► UI
 │                        │ │   execute ───────────────────────────────┼─►│
 │                        │ │   ◄──── ToolResult ──────────────────────┼──┤
 │                        │ │ emit TurnEnd                             │ │
 │                        │ └──────────────────────────────────────────┘ │
 │                        │ emit AgentEnd           │                     │
 │◄───────────────────────┤ Result<(), AgentError>  │                     │
```

One **turn** is one model call plus the tool executions it triggers. The loop stops when
the model returns no tool calls, a tool asks to terminate, the run is cancelled, or the
turn limit is hit.

一个**回合（turn）**＝一次模型调用 + 它触发的工具执行。循环在以下情况停止：模型不再
要求调用工具、某工具要求终止、运行被取消，或达到回合上限。

---

## 4. Two channels: result vs events · 两条通道：返回值与事件

The loop reports itself over **two independent channels**:

- The **return value** — `prompt()` returns `Result<(), AgentError>`. It is the
  *outcome*: did the run finish, or did it blow up (turn limit, no result)?
- The **event stream** — `AgentEvent`s flow through an unbounded channel you subscribe
  to with `Agent::subscribe`. They are the *narrative*: turn started, text streamed,
  tool running, approval needed, context compacted.

循环通过**两条独立通道**汇报自身：

- **返回值**——`prompt()` 返回 `Result<(), AgentError>`，表示**结局**：运行是否正常
  结束，还是出错（超过回合上限、无结果）。
- **事件流**——`AgentEvent` 经一条无界通道流动，用 `Agent::subscribe` 订阅。它们是
  **过程叙事**：回合开始、文本流式输出、工具执行、需要审批、上下文被裁剪。

This split is why the same `Agent` can drive a rich TUI, a plain CLI, or a headless
script without changing the core. Note a subtlety: a *model-level* failure (bad key,
network error) is **not** a `Result::Err` — it is a normal end of the run with the
reason recorded in `Agent::error_message()`. The CLI turns that into a non-zero exit
code for `--print` runs.

正是这一拆分，让同一个 `Agent` 能驱动富终端 UI、朴素 CLI 或无头脚本而无需改动内核。
一个细节：**模型级**失败（密钥错误、网络故障）**不是** `Result::Err`，而是一次正常
结束、原因记在 `Agent::error_message()` 里；CLI 会据此让 `--print` 以非零码退出。

---

## 5. Tool calls and approval · 工具调用与审批

A tool is a pair: a [`ToolSpec`](../crates/rsagent-ai/src/tool.rs) (what the model
sees: name, description, JSON Schema) and a [`Tool`](../crates/rsagent-agent/src/tool.rs)
implementation (what actually runs). They are deliberately separate, so the wire
contract and the execution logic can evolve independently.

一个工具由两部分组成：给模型看的 [`ToolSpec`](../crates/rsagent-ai/src/tool.rs)
（名称、描述、参数 JSON Schema），以及真正执行的
[`Tool`](../crates/rsagent-agent/src/tool.rs) 实现。二者刻意分离，使协议契约与执行
逻辑能各自演进。

Before a batch of calls runs, each one is **preflighted**: the agent asks the
[`Approver`](../crates/rsagent-agent/src/approval.rs) whether it may run. Tools that
mutate the filesystem or run commands return `requires_approval() == true`. Calls in a
batch run in parallel **unless** any tool in the batch declares `is_sequential()`
(then the whole batch runs in order).

一批调用执行前会逐个**预检**：智能体询问
[`Approver`](../crates/rsagent-agent/src/approval.rs) 是否放行。会改动文件系统或执行
命令的工具返回 `requires_approval() == true`。一批内的调用默认**并行**执行，**除非**
其中任一工具声明了 `is_sequential()`（此时整批按顺序执行）。

---

## 6. Context budget and compaction · 上下文预算与裁剪

Before each request the transcript is measured (a rough `chars / 4` token estimate) and,
if it would exceed three quarters of the model's context window, the **oldest whole
turns** are dropped — oldest first — while the first and most recent turns are always
kept. Turns are dropped whole so a tool call is never separated from its result. A
`ContextCompacted` event reports how many messages went away.

每次请求前都会估算对话记录长度（粗略按 `字符数 / 4` 估算 token）；若超过模型上下文
窗口的四分之三，就**按整轮丢弃最旧的回合**（从最旧开始），但始终保留第一轮与最近的
回合。按整轮丢弃可确保工具调用与其结果永不被拆散。`ContextCompacted` 事件会报告
丢弃了多少条消息。

---

## 7. Sessions · 会话

`Context` is plain, serializable data (system prompt + messages + tool specs). A session
file wraps it in a versioned envelope (`{"version":1,"context":{...}}`), so an older
build recognizes — and refuses rather than misreads — a file written by a newer one.
Because `Context` is just data, a transcript can be persisted, inspected, or handed to a
different provider at will.

`Context` 是纯数据、可序列化（system prompt + 消息 + 工具声明）。会话文件把它包在带
版本的信封里（`{"version":1,"context":{...}}`），因此旧版本能识别、并**拒绝而非误读**
新版本写出的文件。正因为 `Context` 只是数据，对话记录可被持久化、检视，或随意交给
另一个提供商。

---

## 8. Extension points · 扩展点

Four seams, all plain traits. Implement one and register it; the core does not change.

四处扩展缝，都是普通 trait。实现其一并注册即可，内核无需改动。

| Seam · 扩展点 | Trait | Register with · 注册方式 | Example · 示例 |
| --- | --- | --- | --- |
| Add a tool · 新增工具 | `rsagent_agent::Tool` | `Agent::with_tools` | `examples/custom_tool.rs` |
| Add a provider · 新增提供商 | `rsagent_ai::Provider` | `Models::register` | `examples/custom_provider.rs` |
| Change the approval policy · 改审批策略 | `rsagent_agent::Approver` | `Agent::with_approver` | `ScopedApprover` in `tests/agent_loop.rs` |
| Drive the loop headlessly · 无头驱动循环 | — (use `Agent` directly) | — | `examples/headless.rs` |

Each example is a real, compilable program:

```bash
cargo run -p rsagent-agent --example headless
cargo run -p rsagent-agent --example custom_tool
cargo run -p rsagent-ai    --example custom_provider
```

---

## 9. Reading order · 源码阅读顺序

If you are learning the codebase, read in this order — it follows the data flow:

若你在学习这套代码，建议按下面的顺序阅读——它顺着数据流走：

1. `rsagent-ai/src/types.rs` — the plain value types (`Model`, `Usage`, `StopReason`).
2. `rsagent-ai/src/message.rs` — `Message`, `ContentBlock`, `Context`.
3. `rsagent-ai/src/provider.rs` — the `Provider` trait and the `Models` registry.
4. `rsagent-ai/src/providers/openai.rs` — one concrete provider, end to end.
5. `rsagent-agent/src/tool.rs` — the `Tool` trait and `ToolContext`.
6. `rsagent-agent/src/agent.rs` — **the loop** (`run_turns`); the heart of the project.
7. `rsagent/src/main.rs` — how the CLI wires it all together.

Two shortcuts for orientation: `cargo test --workspace` runs 60+ tests that read like
usage examples, and `--list-tools` prints the built-in tool set at runtime.

两个定位捷径：`cargo test --workspace` 有 60 多个测试，读起来就像用例；`--list-tools`
可在运行时打印内置工具清单。
