# 代码审查报告 · rsagent

> **快照**：2026-11-19，基线 commit **`dd112e9`**（工作区干净，与 `origin/main` 同步）。
> **性质**：独立审查，只读，未修改任何代码。本文是审查记录，**不是**项目契约；
> 契约见 [STABILITY.md](../STABILITY.md)。**可执行改进清单见 [ACTIONS.md](ACTIONS.md)**。
> **方法**：通读全部 Rust 源码、全部 Markdown 文档、CI 与仓库元数据；执行 AGENTS.md
> 规定的四道门禁；对每条结论回读被引用的源码逐条核实。
> 文中 **已验证** = 读到确切代码并推演过；**推断** = 依赖外部行为或需运行时确认。
> **相对上一版（基线 `31b36ad`）**：H1–H7 与 M16/M17 **共 9 条已全部修复并核实**
> （`3b22d5f`、`e939f87`、`c8f6351`、`73fb63b`），见 §3。剩余条目的结论经复核**仍然成立**。
> **结论**：工程纪律一流，主要可靠性缺口已被堵上；剩余风险集中在「外部依赖故障」与
> 「用户态语义」。综合评分 **9.1 / 10**。

---

## 摘要

这是一个骨架与文档都已达到可发布水平的小项目：三层单向依赖、`ToolSource` 在装配期
展平、0 `unsafe` / 0 `.unwrap()` / 0 TODO、严格 lint、**243 个测试**、中英双语文档与决策
日志（D1–D16）。

上一版列出的 7 条高优先级缺陷（**静默错误与数据丢失**）本轮已全部修复，且修法对症：
不是打补丁，而是把「最后一个终止事件获胜」这类根因在类型/状态层消除。这让项目从
「读起来很舒服但不敢跑在不可信仓库上」变成了「可以放心用于日常长会话」。

**仍然欠的**是同一主题的延伸：外部依赖（MCP、HTTP、子进程）的**故障语义**与**资源边界**。
它们不会立刻炸，但会在「不可信仓库 + 长会话 + 真实网关」的组合下暴露。这些已整理为
[ACTIONS.md](ACTIONS.md) 的 A/B/C 三组任务。

| 严重度 | 数量 | 主题 |
| --- | --- | --- |
| 高 | 0 | —（H1–H7 已修复） |
| 中 | 13 | 超时/重试、MCP 分类与关联、熔断器语义、`--allow` 类别、路径基准、REPL/退出码、doctor、CRLF、输出上限、skill 逃逸 |
| 低 | 9 | 提示误报、日志噪声、抽象空转、SSE 细节、测试隔离 |

---

## 1. 门禁实测

| 门禁 | 结果 | 说明 |
| --- | --- | --- |
| `cargo fmt --all --check` | ✅ exit 0 | — |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | ✅ 0 警告 | — |
| `cargo doc --workspace --no-deps --locked`（`RUSTDOCFLAGS="-D warnings"`） | ✅ 0 告警 | 无断链、无未文档化项 |
| `cargo test --workspace --locked` | ✅ **243 / 243** | exit 0 |

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
| 安全性 | 8.5 | 审批模型、类别策略、路径围栏设计好；`.env` 信任边界已修；但 `permission()` 缺失（M5）与 skill 逃逸（M15）仍在 |
| 健壮性 | 8.5 | H4 挂死、H5 panic 已修；provider 仍无读超时（M1）、外部输出仍无上限（M7）、熔断器语义仍有洞（M10） |
| 测试 | 8.5 | 243 个测试质量普遍不错；审批/退出码/`--log`/`--cwd`/abort 仍零 E2E |
| **综合** | **9.1** | 主要可靠性缺口已堵上；剩余为外部依赖故障语义与用户态语义 |

> 评分变化说明：7.9 → 9.1 的 +1.2 全部来自 H1–H7 的修复（健壮性 6.5→8.5、安全性
> 6.5→8.5、测试 7.0→8.5）。**剩余项修完预计可到 9.5**，瓶颈会转到测试覆盖与 CI 加固。

---

## 3. 本轮已修复（评审方逐条核实）

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

**一处遗留**：H7 只修了 `.env` 这一半。`rsagent.toml` 在审批前执行代码那一半**仍无
任何机制阻止**，已降级为 [ACTIONS.md](ACTIONS.md) C1（需负责人决策）。

---

## 4. 高优先级（当前为空）

H1–H7 已全部修复。当前无「会静默给错结果或丢数据」的已知缺陷。

**但请注意**：M5（`permission()` 缺失）有**安全语义反转**——`--allow exec` 会自动批准
网络调用。它被归在中优先级是因为「需要用户显式传 `--allow exec` 才触发」，但对**已经这么
用的用户**，这是一个静默的权限放大。评审方建议把它当作**事实上的高优先级**处理，即
[ACTIONS.md](ACTIONS.md) 的 A1。

---

## 5. 中优先级（13 条）

> 每条都已映射到 [ACTIONS.md](ACTIONS.md) 的任务号。**加粗**的是评审方建议优先处理的。

| # | 问题 | 位置 | 影响与修复 | ACTIONS | 验证 |
| --- | --- | --- | --- | --- | --- |
| **M5** | 非内置来源都没实现 `permission()`，全部落到默认 `Exec` | [tool.rs:239-249](../crates/rsagent-agent/src/tool.rs#L239-L249)、[web.rs:240-260](../crates/rsagent-agent/src/sources/web.rs#L240-L260)、[mcp_http.rs:451-465](../crates/rsagent-agent/src/sources/mcp_http.rs#L451-L465) | `--allow network` 放行不了 `web`/`mcp-http`；`--allow exec` 却会自动批准带 `Authorization` 的 HTTP 调用；`tools list` 把网络工具标成 `[exec]` | **A1** | 已验证（本轮 grep 复核：5 个 `impl Tool` 均无 `permission()`） |
| **M10** | 熔断器 `record_success` 无状态检查、半开探测无截止 | [breaker.rs:145-151](../crates/rsagent-agent/src/breaker.rs#L145-L151)、[:123-142](../crates/rsagent-agent/src/breaker.rs#L123-L142) | 并发来源里晚到的成功会关掉刚跳闸的熔断器；探测 future 被丢弃后 `HalfOpen` 永久卡死（`allows()` 在 `HalfOpen` 恒 false） | **A2** | 已验证（本轮回读源码） |
| **M14** | `read_file` 用 `lines()` 去掉 `\r`，`edit_file` 却做精确匹配 | [tools.rs:154](../crates/rsagent-agent/src/tools.rs#L154)、[:317-328](../crates/rsagent-agent/src/tools.rs#L317-L328) | CRLF 文件上"读出来再改"必然 `old_string not found` | **A3** | 已验证 |
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
   **修 M10 时务必保持这一点。**
4. **lint 纪律**：0 unsafe / 0 unwrap / 0 TODO / 3 个带理由的 expect；doc gate 进 CI。
5. **双语文档 + 决策日志（D1–D16）+ 可编译示例 + 图解页**。
6. **单 stdin 所有者**（后台线程 + `Recall` 枚举）解决 REPL 与审批提示抢字节；
   `write_atomic`、会话版本信封、截断尾行检测都是正确取舍。
7. **H7 的 `.env` 处理**：移除而非警告后仍生效——干净且正确的信任边界修法。

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

- **A 组（小改动高收益，0.25–1 天/条）**：A1 `permission()` · A2 熔断器语义 ·
  A3 CRLF · A4 thinking 到 stderr · A5 doctor 退出码。
- **B 组（中等，0.5–1.5 天/条）**：B1 MCP 关联+分页 · B2 mcp-http 状态码分类 ·
  B3 读超时 · B4 重试策略 · B5 流内错误。
- **C 组（需负责人决策）**：C1 信任边界 opt-in · C2 记忆蒸馏限频。

**评审方建议的顺序**：A1 → A2 → A3 → A5 → A4 → B1 → B2 → B3 → B4 → B5。
理由：A1/A2 是**语义正确性**问题（用户会拿到错误行为），A3/A5 是**必然失败**路径，
B 组是**外部依赖故障**（概率低但后果重）。

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
