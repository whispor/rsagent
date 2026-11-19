# 代码审查报告 · rsagent

> **快照**：2026-11-19，基线 commit **`e1906b5`**（工作区干净，与 `origin/main` 同步）。
> **性质**：独立审查，只读，未修改任何代码。本文是审查记录，**不是**项目契约；
> 契约见 [STABILITY.md](../STABILITY.md)。**可执行改进清单见 [ACTIONS.md](ACTIONS.md)**。
> **方法**：通读全部 Rust 源码、全部 Markdown 文档、CI 与仓库元数据；执行 AGENTS.md
> 规定的四道门禁；对每条结论回读被引用的源码逐条核实。
> 文中 **已验证** = 读到确切代码并推演过；**推断** = 依赖外部行为或需运行时确认。
> **相对上一版（基线 `73fb63b`）**：A1/A2/A3 三条已完成并核实（`cf67bf8`、`9029cc3`、
> `e1906b5`），M5/M10/M14 关闭，测试 243 → 251。见 §3.2。
> **结论**：工程纪律一流，主要可靠性缺口与安全语义反转均已堵上；剩余风险集中在
> 「外部依赖故障」（MCP / HTTP / 子进程）与「用户态语义」。综合评分 **9.3 / 10**。

---

## 摘要

这是一个骨架与文档都已达到可发布水平的小项目：三层单向依赖、`ToolSource` 在装配期
展平、0 `unsafe` / 0 `.unwrap()` / 0 TODO、严格 lint、**251 个测试**、中英双语文档与决策
日志（D1–D16）。

两轮修复都已落地且修法对症：第一轮（H1–H7 + M16/M17）把「最后一个终止事件获胜」这类根因
在类型/状态层消除；第二轮（A1–A3）修好了**一处安全语义反转**（`--allow exec` 会自动批准
网络调用）、**熔断器两处状态机漏洞**、以及 **Windows 上的必然编辑失败**。项目已从
「读起来很舒服但不敢跑在不可信仓库上」变成「可以放心用于日常长会话」。

**仍然欠的**是同一主题的延伸：外部依赖（MCP、HTTP、子进程）的**故障语义**与**资源边界**。
它们不会立刻炸，但会在「不可信仓库 + 长会话 + 真实网关」的组合下暴露。这些已整理为
[ACTIONS.md](ACTIONS.md) 的 B/C 两组任务。

| 严重度 | 数量 | 主题 |
| --- | --- | --- |
| 高 | 0 | —（H1–H7 已修复；A1 的安全语义反转已修复） |
| 中 | 10 | 超时/重试、MCP 分类与关联、路径基准、REPL/退出码、doctor、输出上限、skill 逃逸 |
| 低 | 9 | 提示误报、日志噪声、抽象空转、SSE 细节、测试隔离 |

---

## 1. 门禁实测

| 门禁 | 结果 | 说明 |
| --- | --- | --- |
| `cargo fmt --all --check` | ✅ exit 0 | — |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | ✅ 0 警告 | — |
| `cargo doc --workspace --no-deps --locked`（`RUSTDOCFLAGS="-D warnings"`） | ✅ 0 告警 | 无断链、无未文档化项 |
| `cargo test --workspace --locked` | ✅ **251 / 251** | exit 0 |

**环境注意（非项目缺陷）**：本机（Windows + MSVC 14.44）dev profile 的 `-Z7` 调试信息
会让 `cl.exe → c1.dll` 的 debug-record 交接失败（`D8050`），`ring` 等 C 依赖编译不过；
`CARGO_PROFILE_DEV_DEBUG=0` 可绕过。CI 用默认设置，不受影响。

---

## 2. 评分（主观加权）

| 维度 | 评分 | 依据 |
| --- | --- | --- |
| 架构与分层 | 9.5 | 三层单向依赖；`ToolSource`/`ToolCatalog` 把"来源"关在装配期（D2 落地），无回边；记忆层也遵守 |
| 代码质量与一致性 | 9.5 | 0 `unsafe`、0 `.unwrap()`、0 TODO、0 `#[allow]`、仅 3 处带 reason 的 `#[expect]`；`#[cfg]` 只在 `platform.rs` |
| 文档 | 9.0 | 文档体系完整且双语 + 决策日志 D1–D16；`REVIEW.md`/`ACTIONS.md` 已纳入版本控制 |
| 可扩展性 | 9.5 | 四 trait 扩展缝 + 六种来源 + 记忆层缝 + 配方/不变量/PR 清单；第三个 provider 边际成本低 |
| 安全性 | 9.0 | 审批模型、类别策略、路径围栏设计好；`.env` 信任边界已修；A1 修好了 `--allow` 语义反转。仅剩 skill 逃逸（M15）与项目配置无 opt-in（C1） |
| 健壮性 | 9.0 | H4 挂死、H5 panic、A2 熔断器状态机已修；provider 仍无读超时（M1）、外部输出仍无上限（M7） |
| 测试 | 8.5 | 251 个测试质量普遍不错；审批/退出码/`--log`/`--cwd`/abort 仍零 E2E |
| **综合** | **9.3** | 主要可靠性缺口与安全语义反转已堵上；剩余为外部依赖故障语义与用户态语义 |

> 评分变化说明：7.9 → 9.1 来自 H1–H7 的修复（健壮性 6.5→8.5、安全性 6.5→8.5、测试 7.0→8.5）；
> 9.1 → 9.3 来自 A1–A3（安全性 8.5→9.0、健壮性 8.5→9.0）。**B 组修完预计可到 9.5**，
> 瓶颈会转到测试覆盖（审批/退出码 E2E）与 CI 加固。

---

## 3. 已修复（评审方逐条核实）

### 3.1 第一轮：H1–H7 + M16/M17

> 以下 9 条**从问题清单移除**。核实方式：回读当前源码确认根因已消除 + 确认新增测试存在。

| # | 原问题 | 修法（核实） | 提交 |
| --- | --- | --- | --- |
| H1 | Anthropic `error` 之后仍发 `Done`，错误被覆盖成"成功空回复" | 解析器加 `failed: bool`；`finish()` 见 `failed` 直接返回空，不再补发 `Done` | `3b22d5f` |
| H2 | Anthropic 回放丢 thinking + signature，`--thinking` + 工具调用第二轮必然 400 | `assistant_blocks` 先回放 `Thinking { thinking, signature }`，再排文本与 tool_use | `3b22d5f` |
| H3 | 流式解码按 chunk 做 `from_utf8_lossy`，跨 chunk 多字节字符被永久损坏 | `sse::next_line` 改 `&mut Vec<u8>`，只在完整行处解码 | `3b22d5f` |
| H4 | 子进程退出后无超时地排空管道，`local`/`skill`/`bash` 可永久挂死 | `run()` 把「写 stdin + wait + 排空」收进**同一个** `timeout`；`kill_on_drop(true)`；新增 `DRAIN_GRACE` + `drain_within`；`platform::kill_tree` | `e939f87` |
| H5 | `mcp-stdio` 的 stderr 尾部裁剪在 UTF-8 边界 panic | 抽 `trim_front_to_limit`，用 `while !text.is_char_boundary(cut) { cut += 1 }` | `e939f87` |
| H6 | 会话持久化四条数据损坏/丢失路径 | 新增 `IncompatibleVersion` + `read_only`（新版文件永不覆盖）；`save_jsonl` 用 `last_written` 指纹判定可追加；`JsonlLoad` 返回 `needs_rewrite`/`dropped_tail` | `c8f6351` |
| H7 | 配置信任边界：`.env` 能改端点、`rsagent.toml` 能执行代码 | `load_dotenv()` 移除由 `.env` 新引入的 `RSAGENT_BASE_URL`/`RSAGENT_USER_CONFIG` 并告警；`config.rs` 加 `# Trust boundary` 文档 | `c8f6351` |
| M16 | 记忆 `append` 无锁，"读—改—写"整文件重写，两进程共用会丢笔记 | 改**单行追加**（`OpenOptions::append`），仅在超出容量时压缩一次 | `73fb63b` |
| M17 | 记忆单条笔记无大小上限 | 新增 `MAX_NOTE_CHARS = 2_000`，`cap_note` 截断并留可见标记 | `73fb63b` |

**评价**：修法质量高。H1 用状态位而非「看错误文本」、H4 用单一 deadline 而非多处补超时、
H6 用指纹而非数量判断前缀——都是消除根因而非打补丁，符合项目一贯纪律。H7 的 `.env`
处理尤其干净：**移除**而非「警告后仍生效」。

### 3.2 第二轮：A1–A3（M5 / M10 / M14）

| # | 原问题 | 修法（核实） | 提交 |
| --- | --- | --- | --- |
| **M5** | 非内置来源都没实现 `permission()`，全部落到默认 `Exec`，导致 `--allow` 语义反转 | 5 个来源全部显式声明：`web`/`mcp_http`/`mcp_stdio` → `Network`，`local`/`skill` → `Exec`；新增 5 个测试逐一断言类别 | `cf67bf8` |
| **M10** | 熔断器 `record_success` 无状态检查、半开探测无截止 | `record_success` 按状态分派（`Closed`/`HalfOpen` → 关闭；`Open` → **忽略**晚到的成功）；新增 `probe_started_at`；`allows()` 与 `effective()` **都**处理废弃探测的再武装 | `9029cc3` |
| **M14** | `read_file` 用 `lines()` 去掉 `\r`，`edit_file` 却做精确匹配 | `read_file` 改用 `split_inclusive('\n')` 且只剥 `\n`，保留 `\r`；新增 `read_file_output_edits_a_crlf_file` 端到端测试 | `e1906b5` |

**评价**：三条都**恰好**按建议的映射与策略实现，且没有过度设计。

- **A1 是当前最高价值的一修**：它消除的是一处**安全语义反转**——在修之前，用户传
  `--allow exec`（本意是「放行 shell 命令」）会**静默地**自动批准带 `Authorization` 头的
  网络调用。修后类别正确，`tools list` 的显示也不再误导。
- **A2 的额外优点**：探测 deadline **复用 `cooldown`** 而非新增 `BreakerPolicy` 字段——
  既解决问题又没扩大公开 API（北极星①④）。评审方确认 `effective()`（`state()`/`health()`
  的数据源）也做了同样处理，因此**健康显示与实际行为一致**，不会出现「显示半开、实际已可
  探测」的错位。
- **A3** 选的是「读侧保留 `\r`」方案（而非编辑侧归一化），与建议的首选一致，且带了端到端
  测试而不只是单元断言。

**评审方低优先级观察（非缺陷，不要求修）**：A2 的探测 deadline 等于 `cooldown`（默认 60s），
因此一个**合法但慢于 60s** 的探测可能被判为废弃并再武装。工具调用通常自带超时且远短于
60s，实际影响可忽略；若将来出现长探测场景，可考虑独立的 `probe_timeout`。

**一处遗留**：H7 只修了 `.env` 这一半。`rsagent.toml` 在审批前执行代码那一半**仍无
任何机制阻止**，已降级为 [ACTIONS.md](ACTIONS.md) C1（需负责人决策）。

---

## 4. 高优先级（当前为空）

H1–H7 已全部修复，A1 的安全语义反转也已修复。当前无「会静默给错结果或丢数据」的已知缺陷。

---

## 5. 中优先级（12 条）

> 每条都已映射到 [ACTIONS.md](ACTIONS.md) 的任务号（无编号者尚未排期）。**加粗**的是
> 评审方建议优先处理的。M5 / M10 / M14 已于本轮修复，见 §3.2。

| # | 问题 | 位置 | 影响与修复 | ACTIONS | 验证 |
| --- | --- | --- | --- | --- | --- |
| **M12** | `--print` 下 thinking 增量仍 `print!` 到 stdout | [render.rs:295-303](../crates/rsagent/src/render.rs#L295-L303) | `--print --thinking high … > answer.md` 把推理文本写进答案文件 | **A4** | 已验证 |
| **M13** | `tools doctor` 对坏配置和 `Unhealthy` 来源仍退 0 | [tools.rs:190-236](../crates/rsagent/src/tools.rs#L190-L236) | 与"可用作 CI gate"的承诺不符；拼错的 `rsagent.toml` 被忽略后落回内置来源，绿灯通过 | **A5** | 已验证 |
| M9 | JSON-RPC 只按 `id` 关联，不区分 server→client 请求；分页游标无上限 | [mcp_stdio.rs:212-224](../crates/rsagent-agent/src/sources/mcp_stdio.rs#L212-L224)、[mcp_http.rs:277-286](../crates/rsagent-agent/src/sources/mcp_http.rs#L277-L286) | id 撞号时 server 请求被当响应 → `tools/call` 返回 `null` **空成功**；恒定 `nextCursor` 无限循环 | B1 | 已验证 |
| M8 | `mcp-http` 先看 body 再看状态码；非 2xx 带 `error` → 当 `Protocol` → `record_success` 不熔断 | [mcp_http.rs:201-249](../crates/rsagent-agent/src/sources/mcp_http.rs#L201-L249) | 网关 429/5xx 伪装成"协议可达"，来源永不隔离 | B2 | 已验证 |
| M1 | provider 只有 `connect_timeout`，没有读/空闲超时 | [core.rs:13-31](../crates/rsagent-ai/src/providers/core.rs#L13-L31) | 服务端接受连接后停发字节 → 一个 turn 永久卡住 | B3 | 已验证 |
| M2 | 重试忽略 `Retry-After`、无 jitter，且把 409 当可重试 | [http.rs:11-13](../crates/rsagent-ai/src/providers/http.rs#L11-L13) | 429 时继续捶打 | B4 | 已验证 |
| M3 | 流内 `{"error":...}` 被忽略、缺 `finish_reason` 默认成 `Stop` | [openai.rs:321-345](../crates/rsagent-ai/src/providers/openai.rs#L321-L345) | 截断流里已解析出的 tool call 被静默丢弃 | B5 | 已验证 |
| M4 | `Usage.input` 两家语义相反（Anthropic 不含 cache，OpenAI 含） | [types.rs:86-91](../crates/rsagent-ai/src/types.rs#L86-L91) | 开 prompt cache 后 token/成本明显偏低 | — | 已验证 |
| M6 | 配置里的相对路径按**进程 cwd** 解析，而非配置文件目录或 `--cwd` | [main.rs:736-779](../crates/rsagent/src/main.rs#L736-L779) | 在 `repo/sub` 下运行 `path = "./skills/team"` 会找错目录并被静默跳过 | — | 已验证 |
| M7 | 外部输出无大小上限（search 整文件读入、subprocess 全缓冲） | [tools.rs:452-474](../crates/rsagent-agent/src/tools.rs#L452-L474)、[subprocess.rs:105-112](../crates/rsagent-agent/src/sources/subprocess.rs#L105-L112) | 32K 截断发生在完整缓冲之后，对内存峰值无效 | — | 已验证 |
| M11 | REPL 只容忍 `ModelFailed`；`MaxTurnsExceeded`/`NoResult` 直接踢出且跳过 session 保存 | [main.rs:972-982](../crates/rsagent/src/main.rs#L972-L982) | 与 README "REPL never exits on a failed turn" 矛盾；跑满 50 轮丢掉整轮对话 | — | 已验证 |
| M15 | `skill` 的 `command` 可用 `../..` 逃出技能目录；`mcp-http` 信任服务器自报的 `readOnlyHint` | [skill.rs:175-201](../crates/rsagent-agent/src/sources/skill.rs#L175-L201)、[mcp.rs:90-96](../crates/rsagent-agent/src/sources/mcp.rs#L90-L96) | 复制别人的 skill 目录 = 可执行任意二进制且可免审批；远端服务器可让任意工具跳过审批 | — | 已验证 |

---

## 6. 低优先级（9 条）

| # | 问题 | 位置 | 建议 |
| --- | --- | --- | --- |
| L1 | `looks_outside_cwd` 在默认 `cwd="."` 时把所有绝对写入路径误报为"工作目录之外" | [render.rs:516-534](../crates/rsagent/src/render.rs#L516-L534) | 构造 `Renderer` 时 canonicalize `cwd` 再比较 |
| L2 | 每次运行都无条件打印 `· config: <path>`，与 "`-v` 打印配置" 不符 | [main.rs:668-670](../crates/rsagent/src/main.rs#L668-L670) | 移入 `if cli.verbose` |
| L3 | stderr 着色按 stdout 是否 TTY 决定 | [render.rs:45-51](../crates/rsagent/src/render.rs#L45-L51) | 分别判断 |
| L4 | `--allow` 的合法性校验太晚（在 `build_catalog` 与缺 key 预检之后） | [main.rs:1051-1063](../crates/rsagent/src/main.rs#L1051-L1063) | 在任何 I/O 之前统一校验 |
| L5 | 项目级文件有三种基准目录：config/AGENTS/breaker 用 `--cwd`，session/log 用进程 cwd | [main.rs:371-374](../crates/rsagent/src/main.rs#L371-L374)、[breakers.rs:45-46](../crates/rsagent/src/breakers.rs#L45-L46) | 统一基准并写进文档 |
| L6 | `Auth::base_url` 与 `StreamOptions::session_id` 无人消费；`StreamOptions` 的 `Debug` 会打印 API key | [provider.rs:18-27](../crates/rsagent-ai/src/provider.rs#L18-L27) | 要么消费，要么删除；手写 `Debug` 脱敏 |
| L7 | SSE 层：多行 `data:` 不合并、`event:` 被忽略、EOF 残行不 flush；工具参数 JSON 解析失败静默变 `{}` | [sse.rs:24-42](../crates/rsagent-ai/src/providers/sse.rs#L24-L42) | 按事件累积 data 行；解析失败报错而非空参数执行 |
| L8 | 自定义模型上下文窗口恒为 128k | [types.rs:123-140](../crates/rsagent-ai/src/types.rs#L123-L140) | 加 `--context-window` |
| L9 | 双 Ctrl-C 被吞（**推断**） | [main.rs:1005-1023](../crates/rsagent/src/main.rs#L1005-L1023) | 第二次中断应直接退出 |

---

## 7. 测试与 CI

**进步**：H1–H7/M16/M17 的修复都带了针对性单测（230 → 243）。H3 的「跨 chunk 多字节
字符」测试尤其好——它直接复现了根因（`E4 B8 AD` 分三次喂入）。

**仍欠**：

- **审批与退出码零 E2E**：没有 `--yes`、`y`/`a`/拒绝、`--allow` 实际放行、`--log`、
  `--verbose`、exit 1、`--cwd`、abort 的测试。而 README 自述"bash 未沙箱化，审批是唯一
  防线"、exit code 表被 STABILITY 列为 Semver 契约——**核心逻辑没有回归保护**。这是当前
  最大的测试缺口。
- **空转测试**：`no_color_output_has_no_escapes`（[cli.rs:802-814](../crates/rsagent/tests/cli.rs#L802-L814)）
  在 `Command::output()` 下 stdout 本就不是 TTY，删掉 `--no-color` 也会通过。
- **测试不隔离用户级环境**：`rsagent()` 构造器不设 `current_dir`、不清 `RSAGENT_USER_CONFIG`，
  开发机上一旦有 user config，`tools list` 断言就会红。
- **mcp-stdio 示例缺失时静默通过**：找不到示例程序时测试 `return`，仍算通过。
- **CI 加固建议**：补 `permissions: contents: read`、actions 按 SHA 固定、`cargo-deny`/`cargo-audit`。

---

## 8. 值得保留的设计

1. **三层单向依赖**与"来源只在装配期存在"（D2）：循环完全不知道来源；记忆层也遵守。
2. **失败分类按代码路径而非错误文本**（D4/D14）：`CallError::{Transport, Protocol}` 在类型层
   强制调用点区分。
3. **熔断器把时间作为参数注入**：状态机可确定性测试；`BreakerReport` 用墙钟秒跨进程持久化。
   A2 的修复**保持了这一点**（探测 deadline 也在 `allows`/`effective` 里由传入的 `now` 判定），
   值得继续遵守。
4. **lint 纪律**：0 unsafe / 0 unwrap / 0 TODO / 3 个带理由的 expect；doc gate 进 CI。
5. **双语文档 + 决策日志（D1–D16）+ 可编译示例 + 图解页**。
6. **单 stdin 所有者**（后台线程 + `Recall` 枚举）解决 REPL 与审批提示抢字节；
   `write_atomic`、会话版本信封、截断尾行检测都是正确取舍。
7. **H7 的 `.env` 处理**：移除而非警告后仍生效——干净且正确的信任边界修法。
8. **A2 的克制**：探测 deadline 复用 `cooldown` 而不新增 `BreakerPolicy` 字段——修好问题
   且没扩大公开 API。这是「最小化」北星落地的正面例子。

---

## 9. 已知取舍（非缺陷）

- `bash` 不沙箱化、审批是唯一防线——已在 README 明说。
- 同进程 WASM 插件沙箱是**非目标**——理由（子进程/HTTP 边界 + 熔断器）成立。
- CLI 面向中文用户——产品决策；若要国际化，建议统一语言或做 i18n。
- `Context` 的 `chars / 4` token 估算——粗糙但有注释，符合"最小化"北星。
- 记忆层默认关闭、需显式 `--memory`——正确的 opt-in。

---

## 10. 路线图

详细任务（含改法、验收、护栏）见 **[ACTIONS.md](ACTIONS.md)**。概览：

- **A 组（小改动高收益）**：~~A1 `permission()`~~ ✅ · ~~A2 熔断器语义~~ ✅ ·
  ~~A3 CRLF~~ ✅ · **A4 thinking 到 stderr** · **A5 doctor 退出码**。
- **B 组（中等，0.5–1.5 天/条）**：B1 MCP 关联+分页 · B2 mcp-http 状态码分类 ·
  B3 读超时 · B4 重试策略 · B5 流内错误。
- **C 组（需负责人决策）**：C1 信任边界 opt-in · C2 记忆蒸馏限频。

**评审方建议的顺序**：**A5 → A4 → B1 → B2 → B3 → B4 → B5**。
理由：A5/A4 是**必然失败或输出污染**路径（成本各 0.25 天，性价比最高）；B 组是**外部依赖
故障**（概率低但后果重），其中 B1（空成功）与 B2（永不熔断）会**静默给错结果**，优先于
B3–B5。

**A 组已完成 3/5**：`cf67bf8`、`9029cc3`、`e1906b5`，护栏全过，测试 243 → 251。
本轮无新增高优先级问题。

---

## 附录 A：复现

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
```

Windows（本机）上若 C 依赖编译报 `D8050`：

```bash
CARGO_PROFILE_DEV_DEBUG=0 cargo test --workspace --locked
```

## 附录 B：审查覆盖与未核实项

- **已逐行阅读**：`crates/rsagent-ai/src/**`、`crates/rsagent-agent/src/**`（含 `memory.rs`）、
  `crates/rsagent/src/**`、三个 crate 的 `tests/` 与 `examples/`、CI、全部 Markdown 文档。
- **本轮已实际执行**：四道门禁（243 测试全过）、`permission()` 覆盖情况 grep 复核、
  `breaker.rs::record_success` 回读复核、H1–H7/M16/M17 的修复核实。
- **未实际执行（因只读约束，未启动网络服务/外部模型）**：真实 OpenAI/Anthropic 端点、
  真实 MCP 服务器、真实子进程挂死与双 Ctrl-C 场景。相关结论来自源码推演，已在条目中标注
  "推断"或"外部协议要求"。
