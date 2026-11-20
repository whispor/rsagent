# Stability · 稳定性承诺

What you can rely on, and how it is allowed to change. · 你可以依赖什么，以及它允许如何变化。

`rsagent` is pre-1.0, so it is honest about the rules rather than pretending they do not
exist: the surface below is treated as a contract, and a change to it follows the process
in this file. · `rsagent` 尚未到 1.0，与其假装没有规则，不如把规则写清楚：下列表面被视为
契约，改动须走本文件描述的流程。

## Versioning · 版本策略

The workspace follows [Semantic Versioning](https://semver.org/). All three crates share
one version (`[workspace.package] version`). · 工作区遵循语义化版本，三个 crate 共用一个
版本号。

- **Pre-1.0 (`0.x`)**: a **minor** release (`0.2` → `0.3`) MAY contain breaking changes;
  a **patch** release (`0.2.1` → `0.2.2`) MUST NOT. Breaking changes are listed in the
  [changelog](CHANGELOG.md). · **1.0 之前**：**minor** 版本可含破坏性变更，**patch** 版本
  不得含。破坏性变更记录在[变更日志](CHANGELOG.md)中。
- **Deprecation**: a deprecated item keeps working for **at least one minor release**
  with a warning, and is removed no earlier than the minor after that. · **弃用**：被弃用的
  项至少再保留一个 minor 版本并给出警告，最早在其后的 minor 中移除。

## Public API · 公开 API

A crate's public API is **exactly the set of names its crate root re-exports** (`pub use`)
— no more, no less. The table below lists every one of them, grouped by role; a test
(`crates/rsagent/tests/docs.rs`) keeps the table and the crate roots in lock-step, so the
two cannot drift. A re-export that is deliberately *not* promised carries `#[doc(hidden)]`
and is left out of the table; anything `pub(crate)` is internal and may change at any time.
Public items carry doc comments, and CI builds the docs with warnings denied, so a missing
doc or a broken intra-doc link fails the build. · 一个 crate 的公开 API **就是其根 `pub use`
导出的那组名字**——不多不少。下表按用途分组逐一列出；有测试（`crates/rsagent/tests/docs.rs`）
令表格与各 crate 根保持同步，二者不会漂移。刻意**不**作承诺的再导出会标 `#[doc(hidden)]`
并从表中略去；`pub(crate)` 一律为**内部**实现，可随时变更。公开项均带文档注释，且 CI 以
「警告即错误」构建文档，因此缺失文档或断链会让构建失败。

| Surface | Where | Stability |
| --- | --- | --- |
| `rsagent-ai` · messages — `AssistantMessage`, `ContentBlock`, `Context`, `Message`, `SystemMessage`, `ToolResultMessage`, `UserMessage` | crate root | Semver |
| `rsagent-ai` · streaming — `ErrorReason`, `StreamEvent` | crate root | Semver |
| `rsagent-ai` · provider — `Auth`, `EventStream`, `Models`, `Provider`, `StreamOptions`, `AnthropicProvider`, `OpenAiProvider` | crate root | Semver |
| `rsagent-ai` · models & types — `Model`, `Modality`, `StopReason`, `ThinkingLevel`, `Usage`, `now_millis` | crate root | Semver |
| `rsagent-ai` · tool spec — `ToolSpec` | crate root | Semver |
| `rsagent-ai` · test doubles — `FauxBlock`, `FauxProvider`, `FauxResponse` | crate root | Semver |
| `rsagent-agent` · loop — `Agent`, `AgentError`, `AgentEvent`, `HealthProbe` | crate root | Semver |
| `rsagent-agent` · approval — `AllowAll`, `Approver`, `DenyAll`, `Permission` | crate root | Semver |
| `rsagent-agent` · tools — `CatalogError`, `PathFence`, `PREFIX_SEPARATOR`, `SourceEntry`, `SourceHealth`, `SourceKind`, `Tool`, `ToolCatalog`, `ToolContext`, `ToolResult`, `ToolSource`, `UnhealthySource` | crate root | Semver |
| `rsagent-agent` · sources — `BUILTIN_SOURCE_ID`, `BuiltinSource`, `LocalSource`, `MCP_PROTOCOL_VERSION`, `McpHttpSource`, `McpStdioSource`, `PROTOCOL_VERSION`, `SkillSource`, `WebSource`, `WebSourceConfig` | crate root | Semver |
| `rsagent-agent` · breaker — `BreakerPolicy`, `BreakerReport`, `BreakerState`, `CircuitBreaker` | crate root | Semver |
| `rsagent-agent` · memory — `DEFAULT_MEMORY_CAP`, `DEFAULT_RECALL_LIMIT`, `MAX_NOTE_CHARS`, `Memory`, `MemoryEntry` | crate root | Semver |
| `rsagent-agent` · cancellation — `CancelToken` | crate root | Semver |
| `rsagent-agent` · built-in tools — `BashTool`, `EditFileTool`, `GlobTool`, `ListDirTool`, `ReadFileTool`, `SearchTool`, `WebFetchTool`, `WriteFileTool`, `builtin_tools`, `select_builtin_tools` | crate root | Semver |
| `rsagent-agent` · memory tools — `RecallTool`, `RememberTool`, `memory_tools` | crate root | Semver |
| CLI flags and exit codes | `rsagent --help` | Semver |
| `rsagent.toml` schema | [README](README.md#tool-sources-rsagenttoml) | Semver |
| `--json` event schema | [README](README.md#json-output) | Semver (additive only) |
| `--log` trace schema | [README](README.md#event-log) | Semver (additive only) |
| `local` / `skill` subprocess protocol (`protocol: 1`) | [EXTENSIBILITY §3](docs/EXTENSIBILITY.md) | Versioned separately |

Both crates are `#[non_exhaustive]` where a future variant is likely (events, stop
reasons, source health). Matching on them must include a wildcard arm; adding a variant
is therefore **not** a breaking change. · 两个 crate 在可能新增变体处都标了
`#[non_exhaustive]`；对它们做匹配须带通配臂，因此新增变体**不算**破坏性变更。

## Wire formats · 线格式

- **Session file**: a versioned JSON envelope. The `version` field is bumped whenever the
  stored shape changes incompatibly; an older build **rejects** a newer file instead of
  misreading it — and, because it cannot be re-saved safely, the session is then left
  read-only so the newer file is never overwritten. · **会话文件**：带版本号的 JSON 信封；
  不兼容变更时递增 `version`，旧版构建会**拒绝**而非误读新版文件；由于无法安全回写，该会话
  随即转为**只读**，绝不会覆盖新版文件。
- **`--json`**: one `AgentEvent` per line, internally tagged with `type`, camelCase
  fields. **Adding** a field or event kind is allowed within a minor version; **removing
  or renaming** one is breaking. · **`--json`**：每行一个 `AgentEvent`，`type` 内部标签 +
  camelCase 字段。minor 版本内**新增**字段或事件类型是允许的，**删除或改名**属破坏性变更。
- **`--log`**: the same JSONL shape as `--json`, appended to a file, in every output mode.
  It shares the `AgentEvent` schema, so the same additive-only rule applies. · **`--log`**：
  与 `--json` 同形的 JSONL，追加写入文件，适用于任意输出模式；二者共用 `AgentEvent`
  schema，故同样只允许增量变更。
- **Subprocess protocol** (`local`, `skill`): the `protocol` number in the `--describe`
  reply gates compatibility, independently of the crate version. · **子进程协议**：以
  `--describe` 回复中的 `protocol` 号判断兼容性，与 crate 版本解耦。

## Minimum Supported Rust Version · 最低支持的 Rust 版本

**MSRV: Rust 1.95.** Declared as `rust-version` in the workspace `Cargo.toml` and
inherited by every crate, so `cargo` itself enforces it. A dedicated CI job builds the
whole workspace on 1.95, so the floor is *checked*, not merely asserted. · **MSRV：Rust
1.95**，在 `Cargo.toml` 的 `[workspace.package]` 声明并由各 crate 继承，`cargo` 会强制检查；
CI 有独立任务在 1.95 上构建整个 workspace，因此下限是被**校验**的，而非仅声明。

It is set by the dependency floor, not by a language feature we use. The binding
constraint is `rustyline` 19 (the REPL line editor), whose `cfg_select!` macro was
stabilized in Rust 1.95 (`rust-lang/rust#115585`); everything else, including `icu_*`
via `reqwest` → `url`, builds on older toolchains. Raising it is treated as a **minor**
change and is announced in the changelog. · 它由依赖下限决定，而非我们使用的语言特性：
真正的约束是 `rustyline` 19（REPL 行编辑器）用到的 `cfg_select!` 宏，它在 Rust 1.95 才稳定
（`rust-lang/rust#115585`）；其余依赖（含 `reqwest` → `url` → `icu_*`）在更旧的工具链上即可
构建。提高 MSRV 视为 **minor** 变更并在 changelog 说明。

## Telemetry · 遥测

**rsagent collects no telemetry.** It sends data only where you point it: the model
endpoint you configure, the tool sources you declare, and `webfetch` targets the model
chooses. There is no analytics, no phone-home, and no background update check. · **rsagent
不收集任何遥测。** 它只向你指定的地方发送数据：你配置的模型端点、你声明的工具来源，以及
模型选择的 `webfetch` 目标。没有分析上报、没有回连、没有后台更新检查。

## Trust boundary · 信任边界

**Project configuration is executable input.** A project `rsagent.toml` can declare
`local` / `mcp-stdio` / `skill` sources, which are spawned while the tool set is assembled —
*before* any approval prompt. Treat a repository you have not reviewed the same way you
would treat a script in it: `git clone && rsagent --list-tools` runs what the repo declares.
· **项目配置即可执行输入。** 项目 `rsagent.toml` 可声明 `local` / `mcp-stdio` / `skill`
来源，它们会在装配工具集时（**任何审批之前**）被启动。未经审阅的仓库，应按其中的脚本一样
对待：`git clone && rsagent --list-tools` 会执行仓库声明的命令。

An auto-discovered `.env` is likewise untrusted. Two credential-sensitive variables are
**refused** from it: `RSAGENT_BASE_URL` (would redirect where your API key is sent) and
`RSAGENT_USER_CONFIG` (would point the user config at a repo-controlled file). Both are
still honored from the real process environment or a CLI flag. · 自动发现的 `.env` 同样
不可信：两个与凭据相关的变量会被**拒绝**——`RSAGENT_BASE_URL`（会改变 API key 的发送目标）
与 `RSAGENT_USER_CONFIG`（会把用户配置指向仓库内文件）。二者仍可从真实进程环境或命令行参数
生效。

## Changing this file · 修改本文件

Treat the tables above as the checklist for a breaking change: if a change touches a row
marked *Semver*, follow the versioning and deprecation rules above. · 把上表当作破坏性变更
的检查清单：若改动触及标注 *Semver* 的行，须遵循上述版本与弃用规则。
