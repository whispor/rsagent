# Extensibility · 拓展性规范

> **How to add to rsagent without breaking it.** · 如何给 rsagent 加东西而不弄坏它。
>
> Audience: anyone adding a tool, a provider, a tool source, or a platform-specific
> path. 读者：准备新增工具 / 提供商 / 工具来源 / 平台适配代码的人。
>
> **约定 · Conventions.** `MUST` = 必须（违反即视为 bug，会被一致性测试或评审拦下）；
> `SHOULD` = 应当（有正当理由可例外，但要在 PR 里写明）；`MAY` = 可以。
> 每节的英文段是摘要，中文段是正文。

---

## 0. 这份规范的前提 · The premise

This document is subordinate to the project's five north stars. A rule that makes
the code bigger, harder to follow, or harder to learn is **wrong**, even if it looks
tidier on a diagram. Whenever this document and north star ① (minimalism) conflict,
minimalism wins and this document gets amended.

本规范**服从**五条北极星。任何一条规则如果让代码更大、更难跟、更难学，那它就是错的——
哪怕它在架构图上更好看。本规范与北极星①（最小化）冲突时，以最小化为准，并回头修订本规范。

Concretely, that rules out three tempting but rejected designs:

| 被否决的方案 · Rejected | 原因 · Why |
| --- | --- |
| 动态库插件（`cdylib` + ABI） | 需要稳定的 C ABI、跨平台加载、版本协商——代价远超收益，且破坏③的"任何人可定制"（要会写 unsafe FFI）。 |
| 全功能 DI 容器 / 组件注册中心 | 抽象层数 > 实际扩展点数。北极星①④直接否决。 |
| 把所有扩展统一成"一个 trait" | `Tool` 与 `Provider` 解决的是不同问题，强行统一只会得到两个都别扭的 trait。 |

**结论：本项目的"组件化"= 少量**缝**（seam）+ 明确的契约 + 可执行的检查，不是框架。**

---

## 1. 现状盘点 · Where we are

Grounded in the tree at the time of writing. · 以下是写作时的真实状态。

| 关注点 · Concern | 现状 · Current | 缺口 · Gap |
| --- | --- | --- |
| 工具执行 | `Tool` trait（`crates/rsagent-agent/src/tool.rs`），`ToolSpec`（给模型）与执行分离 | — |
| 工具注册 | `Agent::with_tools(Vec<Box<dyn Tool>>)` 仍是唯一入口；`ToolCatalog` 在装配期把来源展开成该扁平列表 | — |
| 工具来源 | ✅ `ToolSource` trait + `ToolCatalog`（`source.rs` / `catalog.rs`）；`BuiltinSource` 与 `LocalSource`（`sources/`）两种实现 | `mcp-stdio` / `mcp-http` / `skill` / `web` 待 P2 |
| 提供商 | `Provider` trait + `Models::register`（`rsagent-ai/src/provider.rs`） | — |
| 审批 | `Approver` trait + `Agent::with_approver` | 语义未文档化（默认 `AllowAll`） |
| 路径安全 | `PathFence`（`WorkingDirectory` / `Unrestricted`） | — |
| 平台差异 | ✅ 收敛到 `platform.rs`（工作区唯一 `#[cfg]`） | — |
| CI | ✅ 三平台矩阵（ubuntu / windows / macos） | — |
| 配置 | ✅ `rsagent.toml`（`crates/rsagent/src/config.rs`），优先级 CLI > env > project > 默认 | 用户级配置待补 |
| 项目指令 | ✅ `AGENTS.md` 并入 system prompt（`main.rs`） | — |
| 机器可读输出 | 无 | 无 `--json` |
| 稳定性标记 | `AgentEvent` / `StreamEvent` / `ErrorReason` 已 `#[non_exhaustive]` | 无公开 API 清单与 semver 承诺 |

---

## 2. 功能拓展性 · Feature extensibility

### 2.1 重点评估 · Assessment

**结论：不要做组件框架；做三件低成本的事。**

The honest read: for a project this size, the cost of "componentization" is almost
never the abstraction — it is the *boundary*. What actually makes a new feature slow
to add is not a missing trait; it is (a) not knowing which files to touch, (b) not
knowing which invariants you must not break, and (c) having no way to check you got it
right before review. So:

| 手段 · Lever | 成本 | 收益 |
| --- | --- | --- |
| **配方（recipe）**：每类扩展一张"改哪几个文件 + 必须满足什么"的清单 | 纯文档 | 消除"不知道从哪下手" |
| **一条被强制的缝**：每类扩展只有一个注册入口 | 极小 | 消除"两个地方都能改"的分裂 |
| **一致性测试**：跑一个测试就自证合规 | 一个测试文件 | 把契约从"文档里的君子协定"变成"CI 会拦你" |

第 3 项是关键。没有它，前两项只是建议；有了它，规范才**可执行**。

**为什么不做动态库插件：** 它把"扩展 rsagent"的门槛从"会写 Rust"抬高到"会写
unsafe FFI 并处理跨平台 ABI"，与北极星③（任何人可定制）背道而驰。真正需要进程隔离的
扩展（MCP、本地程序）应该走 **§3 的 `ToolSource`**，用子进程而非同进程 ABI 来隔离——
这也正是 MCP 协议本身的设计取向。

### 2.2 标准流程 · The recipe

Adding anything follows four steps. · 任何新增都走这四步。

```
1. 选缝 (pick a seam)   → 下表四选一；不要新造抽象
2. 实现 (implement)     → 只动该缝允许的文件
3. 注册 (register)      → 只走该缝唯一入口
4. 过检 (conform)       → 跑 §2.5 的一致性测试；新增变体要过 §2.4 的清单
```

### 2.3 各扩展类型的配方 · Per-type recipes

| 扩展类型 | 实现位置 | 注册入口 | 必须补的测试 |
| --- | --- | --- | --- |
| 新工具 · Tool | `crates/rsagent-agent/src/tools.rs`（内置）或独立 crate（外部） | `builtin_tools()` 里加一行 / `Agent::with_tools` | 一个端到端 `agent_loop` 风格测试（脚本化工具调用 → 断言结果） |
| 新提供商 · Provider | `crates/rsagent-ai/src/providers/<name>.rs` | `Models::register` | 流式解析单测（喂真实 SSE 片段）+ `thinking` 映射单测 |
| 新工具来源 · ToolSource | `crates/rsagent-agent/src/sources/<name>.rs` | `ToolCatalog::build(Vec<SourceEntry>)` | 来源解析单测 + 健康/熔断单测（见 §3） |
| 新审批策略 · Approver | `crates/rsagent-agent/src/approval.rs` 或调用方 | `Agent::with_approver` | 决策表测试（哪些名字/参数通过） |
| 新 CLI 参数 | `crates/rsagent/src/main.rs` 的 `Cli` | — | `tests/cli.rs` 里加一条（含 `--help` 断言） |
| 新事件 · AgentEvent 变体 | `crates/rsagent-agent/src/event.rs` | `Agent::emit` | 在对应 `agent_loop` 测试里断言事件序列 |
| 平台适配 | `crates/rsagent-agent/src/platform.rs`（§4） | — | 三平台 CI（§4.3） |

### 2.4 不变量 · Invariants (MUST)

These hold for every change, forever. · 以下每条对所有改动永久有效。

1. **分层方向 MUST 单向。** `rsagent-ai` 不得引用 `rsagent-agent`；`rsagent-agent`
   不得引用 `rsagent`。下层不知道上层。
2. **新增 enum 变体 MUST 先确认该 enum 是 `#[non_exhaustive]`。** 否则就是对下游的破坏性变更。
3. **工具失败 MUST 用 `Err(String)`，MUST NOT 把失败塞进成功结果的 `content`。**
   循环据此把结果标成 `is_error: true` 交回模型。
4. **`Tool::requires_approval()` MUST 与实际副作用一致。** 会写盘、跑命令、发网络请求的工具必须返回 `true`。
5. **工具名 MUST 全局唯一且为 `snake_case`。** 模型靠名字调用，重名即歧义。
6. **工具的参数 schema MUST 是 JSON object schema**，且 `required` 里的键 MUST 在 `properties` 中声明。
7. **公开 API MUST 有文档注释**；返回 `Result` 的 MUST 有 `# Errors` 段（`missing_errors_doc` 已开）。
8. **模型级失败 MUST NOT 变成 `Result::Err`。** 缺密钥、网络失败记在 `Agent::error_message()`，
   `prompt()` 仍返回 `Ok(())`——这是第 4 节"两条通道"的语义（见 `docs/ARCHITECTURE.md` §4）。
9. **不得为单一调用方引入抽象。** 抽象 MUST 至少有两个真实使用者（rule of two）。
10. **不得新增 `unsafe`。** 目前全仓库零 `unsafe`，这是特性，不是巧合。

### 2.5 一致性测试 · The conformance test

`crates/rsagent-agent/tests/tool_conformance.rs` 把 §2.4 里可机检的部分固化成测试。
任何新增内置工具都会自动被它检查。**这是"规范"与"建议"的分界线。**

Adding a new kind of extension SHOULD come with a matching conformance test in the same
PR. · 新增一类扩展时，应当在同一个 PR 里补上对应的一致性测试。

---

## 3. 工具拓展性 · Tool extensibility

### 3.1 统一对接方式：`ToolSource`

The single abstraction that makes MCP, Skills, local programs, and web tools all the
same shape. · 让 MCP、Skill、本地程序、网页工具变成同一种东西的那一层。

```rust
/// Where a group of tools comes from, and how to keep it healthy.
pub trait ToolSource: Send + Sync {
    /// What kind of source this is (drives config, docs, and the `doctor` command).
    fn kind(&self) -> SourceKind;
    /// Stable id, unique among sources. Used for config and event attribution.
    fn id(&self) -> &str;
    /// The tools this source currently offers.
    fn tools(&self) -> Vec<Box<dyn Tool>>;
    /// Current health. Defaults to `Healthy` for sources that cannot fail to load.
    fn health(&self) -> SourceHealth { SourceHealth::Healthy }
    /// Install anything this source needs (e.g. an MCP server). Default: nothing to do.
    fn install(&self) -> Result<(), String> { Ok(()) }
    /// Undo `install`. Default: nothing to do.
    fn uninstall(&self) -> Result<(), String> { Ok(()) }
}
```

**关键决策：`Agent` 仍然只接收 `Vec<Box<dyn Tool>>`。** `ToolSource` 在**装配期**
（`ToolCatalog`）展开成扁平工具列表，`Agent` 完全不知道它的存在。理由：循环不应关心
工具来自哪里——这既保住北极星①，也让 `ToolSource` 的失败模式（连接失败、超时）不会
渗进循环。

**Decision:** `Agent` keeps taking a flat `Vec<Box<dyn Tool>>`. `ToolSource` is expanded
at assembly time by `ToolCatalog`; the loop never learns where tools come from.

### 3.2 来源分类 · Source taxonomy

| kind | 例子 | 传输 · Transport | "安装"是什么 | 默认需要审批 | 三平台 |
| --- | --- | --- | --- | --- | --- |
| `builtin` | `read_file` … `bash` | 同进程 | — | 按工具 | ✅ **已实现** |
| `local` | 项目自带的脚本/二进制 | 子进程 stdin/stdout（JSON 行） | 检查可执行文件存在 | ✅ | ✅ **已实现** |
| `mcp-stdio` | `@modelcontextprotocol/server-filesystem` | 子进程 + MCP 协议 | 配置即安装（`npx`/`uvx` 按需拉取） | 按工具注解 | 待 P2 |
| `mcp-http` | 远端 MCP 服务 | HTTP + SSE | 配置即安装（写 URL + 令牌） | 按工具注解 | 待 P2 |
| `skill` | 一个文件夹 + `SKILL.md` + 脚本 | 文件夹扫描 → 展开为工具 | 目录存在性检查 | ✅ | 待 P2 |
| `web` | 把某个 HTTP 端点包装成工具 | HTTP | 配置即安装 | ✅ | 待 P2 |

**一种对接方式，多种来源。** 新增来源类型 = 实现 `ToolSource` 一次，而不是改循环。
`SourceKind` 是 `#[non_exhaustive]`，P2 的四种来源会作为新变体加入，不影响下游。

### 3.3 配置 · Configuration

One file, `rsagent.toml`, project-local, with CLI flags overriding it. **已实现**
（`crates/rsagent/src/config.rs`）；当前只解析 `builtin` 与 `local` 两种 `kind`，下面
示例里的 `mcp-stdio` / `skill` 是 P2 的目标形态。

```toml
# rsagent.toml
[[tools.sources]]
kind = "builtin"
id = "core"
exclude = ["bash"]              # 与 --no-bash 等价

[[tools.sources]]
kind = "mcp-stdio"              # ← P2
id = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
# 该来源的工具名统一加前缀，避免与内置冲突
prefix = "fs"

[[tools.sources]]
kind = "skill"                  # ← P2
id = "team-conventions"
path = "./skills/conventions"

[[tools.sources]]
kind = "local"                  # ← 已实现
id = "release"
command = "./scripts/release-tool"
timeout_secs = 60
```

`local` 来源的线协议（version 1）见 README「The `local` source protocol」：程序被调用
两次，`--describe` 列出工具，`--call` 执行一次调用，均以单行 JSON 通信。

**优先级 MUST 为：CLI 参数 > 环境变量 > 项目配置 > 用户配置 > 内置默认。** 已实现
前三档（用户级配置待补）；这条 MUST 写进 `--help` 与 README（§5）。

### 3.4 命名与冲突 · Naming and collisions

- 来源可声明 `prefix`；带前缀的工具对模型呈现为 `fs__read_file`。
- 不带前缀时，工具名 MUST 全局唯一。**冲突 MUST 在装配期报错并指出两个来源**，
  MUST NOT 静默取其一——静默覆盖会让模型调用到"另一个"工具，是最难查的一类 bug。
- 前缀分隔符用 `__`（双下划线），因为它不会出现在 `snake_case` 工具名里。

### 3.5 健康与故障自愈 · Health and self-healing

"自动排除调用故障及修复"落地为**熔断器**，每个来源一个：

```
连续失败 N=3 次
   → 状态 unhealthy：从工具列表摘除该来源的全部工具
   → 发一次事件（AgentEvent::SourceUnhealthy）通知 UI/日志
   → 冷却 60s
冷却结束
   → 半开：放行一次探测调用
      成功 → 恢复 healthy，工具重新可用
      失败 → 回到 unhealthy，冷却翻倍（上限 10min）
```

**实现状态（P1）：** 状态机本身已实现（`crates/rsagent-agent/src/breaker.rs`：
Closed / Open / HalfOpen、连续失败计数、冷却翻倍且有上限、半开只放行一次探测）。
`LocalSource::health()` 会据此报告 `Unhealthy`，`LocalTool::execute()` 在熔断打开时
直接拒绝调用。**尚未实现的两点**（留到 P3 的 `doctor`）：运行时把工具从装配好的列表
里摘除、以及 `AgentEvent::SourceUnhealthy` 事件——因为当前没有消费者，写了就是"没有
调用者的代码"（北极星①）。

要点：

- **只对"连接类"失败计数**（来源不可达、协议错误、超时），不对"业务失败"计数
  （工具正常返回 `Err` 说明它在工作）。这条区分不做，一个总是报错的工具会把自己的来源
  熔断掉，反而掩盖问题。**已实现**：`LocalTool::execute()` 只在"无法启动 / 超时 /
  非零退出 / 输出非 JSON"时 `record_failure`，合法的 `{"error": ...}` 回复只算业务失败。
- 熔断器状态 MUST 可通过 `rsagent tools`（§3.6）查询与手动重置。**待 P3。**
- 实现上复用 `rsagent-ai/src/providers/http.rs` 里 `send_with_retry` / `is_retryable_status`
  的分类思路：**先分清"可重试"与"不可重试"，再谈重试与熔断。**

### 3.6 安装 / 卸载 / 体检 · Install, uninstall, doctor

CLI 只加一个子命令组，避免参数爆炸：

```bash
rsagent tools list            # 每个来源、每个工具、健康状态
rsagent tools doctor          # 逐个来源做一次探测，报告问题与修复建议
rsagent tools add <source>    # 交互式写入 rsagent.toml
rsagent tools remove <id>     # 从 rsagent.toml 移除并调用 uninstall()
```

**安装/卸载的语义 MUST 是"改配置 + 幂等的准备动作"**，MUST NOT 在卸载时删除用户数据。
MCP 的 `npx -y` 会在首次运行时自行拉包，所以"安装"对多数来源就是写一行配置——
这是刻意的：**配置简单、操作便捷**优先于"完整的包管理器体验"。

---

## 4. 平台适配拓展性 · Platform portability

### 4.1 目标 · Goal

Windows / macOS / Linux 上**行为一致、构建通过、测试通过**。兼容性承诺：

| 维度 | 承诺 |
| --- | --- |
| OS | Windows 10+、macOS 12+、主流 Linux（glibc 2.31+） |
| 架构 | x86_64 必测；aarch64 应当可用（Apple Silicon、ARM Linux） |
| Rust | stable，`rust-version` 声明 MSRV |
| 终端 | ANSI 彩色 + `NO_COLOR` 降级；非 TTY 时不输出控制序列 |

### 4.2 差异集中 · Centralize the differences

**所有 `#[cfg(...)]` MUST 收敛到 `crates/rsagent-agent/src/platform.rs`**（业务代码里
MUST NOT 出现平台条件编译）。工作区当前唯一的平台差异是 `BashTool` 的 shell 选择，已
移入 `platform.rs::shell_command`——业务代码里现在**零** `#[cfg]`。

下表里只有第一行是**今天真实存在的**差异，其余是"等真有需要时再加"的目标：MUST NOT
为了填满这张表而预先写出没有调用者的代码（北极星①）。

| 差异点 | 现状 | 目标（`platform.rs` 提供） |
| --- | --- | --- |
| shell 选择 | ✅ 已收敛：`platform::shell_command`（`cmd /C` / `sh -c`） | `fn shell_command(command: &str) -> Command` |
| 路径规范化 | 暂无差异：`resolve_within` 两侧都经 `canonicalize`，`\\?\` 前缀一致，比较正确 | `fn normalize_path(&Path) -> PathBuf`（剥 `\\?\` 前缀），仅在需要拿规范化路径与非规范化路径比较时引入 |
| 可执行后缀 | 暂无差异：当前只经 shell 启动，不直接 spawn 命名可执行文件 | `fn executable(name: &str) -> String`（Windows 加 `.exe`） |
| 换行 | 无处理 | 读文件保留原样；写文件沿用原换行（不强制 LF） |
| 终端能力 | `NO_COLOR` + `--no-color`（在 CLI 层） | `fn supports_color() -> bool`（合并 TTY、`TERM=dumb`、`NO_COLOR`） |
| 中断信号 | tokio 取消令牌 | `fn install_interrupt_handler()`（Unix 信号 / Windows 控制台事件） |

**已知陷阱（MUST 检查）：**

1. `std::fs::canonicalize` 在 Windows 返回 `\\?\C:\...` 前缀形式。`PathFence` 的
   `resolve_within` 两侧都走 `canonicalize`，所以前缀一致、比较正确；**规则是**：一旦
   你把规范化路径与非规范化路径放在一起比较，MUST 先让两边都过同一个 `normalize_path`，
   否则围栏会误判。
2. 测试里构造路径 MUST 用 `Path::join` / `PathBuf`，MUST NOT 手拼 `/` 或 `\`。
3. 会话文件写入 SHOULD 走"写临时文件 + 原子 rename"，避免断电/并发下写出半个 JSON。
4. 行尾：仓库当前由 Git 处理 CRLF（`.gitattributes` SHOULD 显式声明 `* text=auto eol=lf`），
   代码里 MUST NOT 假定 `\n`。
5. 大小写：Windows/macOS 默认大小写不敏感，**工具名与路径比较 MUST NOT 依赖大小写敏感**。

### 4.3 CI 矩阵 · CI matrix

`.github/workflows/ci.yml` 的 `check` 任务 SHOULD 展开为三平台矩阵：

```yaml
strategy:
  fail-fast: false
  matrix:
    os: [ubuntu-latest, windows-latest, macos-latest]
runs-on: ${{ matrix.os }}
```

`fail-fast: false` 是刻意的：一个平台红了不应掩盖另外两个的结果。

---

## 5. 对照主流产品的规范性缺口 · Normative gaps vs. mainstream

Compared against Claude Code, Codex CLI, aider, and opencode. · 对照结论如下。

| 项 | 主流做法 | 本项目 | 决策 |
| --- | --- | --- | --- |
| 项目指令文件 | `AGENTS.md`（事实标准，多产品通用） | ✅ 已支持：启动时读取工作目录向上的最近 `AGENTS.md`，并入 system prompt（`--system` 优先） | **MUST 支持**：启动时读取 `AGENTS.md`，内容并入 system prompt |
| 配置优先级 | CLI > env > project > user | ✅ 已实现前三档并文档化（`rsagent.toml`，README） | **MUST 文档化**（§3.3） |
| 机器可读输出 | `--json` / `--output-format` | 无 | **SHOULD 支持**：`--json` 输出事件流，供 CI/脚本消费 |
| 权限模型 | 按工具类别分级 + 白/黑名单 | `Approver` + `PathFence`，语义未文档化 | **MUST 文档化**；工具按 `read` / `write` / `exec` / `network` 归类 |
| 会话持久化 | JSONL 追加式，可 resume | 版本化 JSON 信封 | 保持；**SHOULD 增加 JSONL 追加模式**以便长会话 |
| 稳定性承诺 | semver + 弃用周期 | 无 | **MUST 增加**：`STABILITY.md` 列公开 API，破坏性变更走 minor+弃用期 |
| 遥测 | 默认关或显式选择加入 | 无遥测 | **MUST 声明**：README 写明"不收集任何遥测" |
| 非交互模式 | `-p` / `--print` + 退出码 | 已有 | 保持（已实现退出码语义） |
| 子命令结构 | `tools` / `mcp` / `config` 分组 | 全平铺在顶层参数 | **SHOULD 分组**（§3.6） |

---

## 6. 路线图 · Roadmap

Ordered by (value ÷ risk). 每项标注对北极星①（最小化）的影响。

### P0 — 低风险，立刻可做

| 项 | 内容 | 对①的影响 |
| --- | --- | --- |
| P0-1 | 一致性测试 `tests/tool_conformance.rs`（§2.5） | 中性（只加测试） |
| P0-2 | ✅ `AGENTS.md` 支持（§5） | +约 20 行 |
| P0-3 | ✅ 平台差异收敛到 `platform.rs`（§4.2） | 中性（移动代码，不新增逻辑） |
| P0-4 | ✅ CI 三平台矩阵（§4.3） | 中性（只改 CI） |
| P0-5 | ✅ `.gitattributes` 固定行尾 | 中性 |

### P1 — 工具来源骨架

| 项 | 内容 | 对①的影响 |
| --- | --- | --- |
| P1-1 | ✅ `ToolSource` + `ToolCatalog`（§3.1） | +约 150 行，但换来"新增来源不动循环" |
| P1-2 | ✅ `rsagent.toml` 读取与优先级（§3.3） | +约 80 行 |
| P1-3 | ✅ 命名冲突检测与 `prefix`（§3.4） | +约 40 行 |
| P1-4 | ✅ 健康熔断器（§3.5） | +约 120 行 |
| P1-5 | ✅ `local` 来源（子进程 JSON 行协议）——作为**活样板** | +约 120 行 |

### P2 — 主流工具接入

| 项 | 内容 |
| --- | --- |
| P2-1 | `mcp-stdio` 来源（MCP 协议：`initialize` / `tools/list` / `tools/call`） |
| P2-2 | `mcp-http` 来源 |
| P2-3 | `skill` 来源（文件夹 + `SKILL.md` → 工具） |
| P2-4 | `web` 来源（HTTP 端点 → 工具） |
| P2-5 | `--json` 输出（§5） |

### P3 — 运营与体验

| 项 | 内容 |
| --- | --- |
| P3-1 | `rsagent tools list / doctor / add / remove`（§3.6） |
| P3-2 | 会话文件原子写（§4.2 陷阱 3） |
| P3-3 | `STABILITY.md` + MSRV 声明（§5） |
| P3-4 | 会话 JSONL 追加模式（§5） |

**P1 是分水岭**：P0 只加检查与文档，P1 才真正引入新抽象。P1 的每一项都应单独提交，
便于逐项评审其对北极星①的代价。

---

## 7. 决策记录 · Decision log

记录"为什么不那样做"，避免后来者重走。· Records the roads not taken.

| 决策 | 选择 | 否决项与原因 |
| --- | --- | --- |
| D1 | 不做动态库插件 | `cdylib` + 稳定 ABI 的成本与 unsafe 风险，违背③ |
| D2 | `ToolSource` 在装配期展开，`Agent` 只见扁平工具列表 | 让循环知道"来源"会把连接失败、超时等状态渗进核心循环，违背①④ |
| D3 | 扩展隔离靠**子进程**而非同进程 ABI | 与 MCP 的设计取向一致；崩溃/挂起可被杀掉 |
| D4 | 熔断器只统计"连接类"失败 | 否则一个业务上总报错的工具会熔断掉整个来源，掩盖真问题 |
| D5 | 命名冲突报错而非静默覆盖 | 静默覆盖会让模型调到"另一个"工具，是最难排查的一类缺陷 |
| D6 | 平台差异集中在 `platform.rs`，业务代码禁止 `#[cfg]` | 让平台问题只有一处可看，也让三平台 CI 的失败定位更快 |
| D7 | 抽象必须满足 rule of two | 防止为单一调用方引入抽象（北极星①） |

---

## 8. 提交前检查清单 · PR checklist

Copy this into your PR description. · 复制到 PR 描述里。

```markdown
- [ ] 只动了 §2.3 配方里允许的文件，没有新造抽象
- [ ] 走的是该扩展缝的唯一注册入口
- [ ] 满足 §2.4 全部不变量（分层 / non_exhaustive / Err(String) / 审批一致 / 命名）
- [ ] 新增 enum 变体前确认了 `#[non_exhaustive]`
- [ ] 补了对应测试；内置工具会被 `tool_conformance` 自动检查
- [ ] 没有新增 `unsafe`；没有为单一调用方引入抽象
- [ ] 业务代码里没有新增 `#[cfg(...)]`（平台差异进 `platform.rs`）
- [ ] `cargo fmt --all --check` 干净
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings` 零告警
- [ ] `cargo test --workspace --locked` 全过
- [ ] 公开 API 有文档；返回 `Result` 的有 `# Errors`
- [ ] 用户可见的行为变化写进了 README / `docs/`
```

---

## 9. 相关文档 · See also

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — 三层分层、一次运行的生命周期、两条通道、扩展点
- [`tool-call-journey.html`](tool-call-journey.html) — 一次工具调用的逐步可视化
- `crates/rsagent-agent/tests/tool_conformance.rs` — 本规范 §2.4 的可执行部分
