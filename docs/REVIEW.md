# 代码审查报告 · rsagent

> **快照**：2026-11-19，基线 commit `31b36ad`（工作区干净，与 `origin/main` 同步）。
> **性质**：独立审查，只读，未修改任何代码。本文是审查记录，**不是**项目契约；
> 契约见 [STABILITY.md](../STABILITY.md)。
> **方法**：通读全部 57 个 Rust 文件（约 17,500 行：`src` 14,511 + 测试 2,989）、
> [README](../README.md)、[ARCHITECTURE](ARCHITECTURE.md)、[EXTENSIBILITY](EXTENSIBILITY.md)、
> [STABILITY](../STABILITY.md)、[CHANGELOG](../CHANGELOG.md)、CI 与仓库元数据；执行
> AGENTS.md 规定的四道门禁；对每条结论回读被引用的源码逐条核实。
> 文中 **已验证** = 读到确切代码并推演过；**推断** = 依赖外部行为或需运行时确认。
> **相对上一版（`6546a15`）**：新增**记忆层**（决策 D16，见 §3）；修复了 `local` 来源
> 的跨平台 `echo` 测试。其余结论经复核**仍然成立**（H/M/L 三节逐条回读了源码）。
> **结论**：工程纪律一流、可靠性边界偏薄。综合评分 **7.9 / 10**。
> **后续（2026-11-19）**：本报告列出的 **H1–H7 全部已修**，记忆层的 **M16/M17 亦已修**
> （提交 `3b22d5f`…`0ee8d5e`，见 [CHANGELOG](../CHANGELOG.md) Fixed 节）。正文保留为
> 基线 `31b36ad` 的**当时快照**，未逐条改写；其余 H/M/L 结论的处置另行推进。

---

## 摘要

这是一个骨架与文档都已达到可发布水平的小项目：三层单向依赖、`ToolSource` 在装配期
展平、0 `unsafe` / 0 `.unwrap()` / 0 TODO、严格 lint、230 个测试、中英双语文档与决策
日志（D1–D16）。上一版列出的边界问题**一条未修**——流式协议的错误处理、子进程生命
周期、持久化一致性、配置的信任边界；其中 7 条会让用户**静默拿到错误结果或丢数据**，
应优先修复。本轮新增的记忆层质量不错（见 §3），但它也**继承**了同一类缺陷（并发写、
无大小上限）。

| 严重度 | 数量 | 主题 |
| --- | --- | --- |
| 高 | 7 | 协议错误被吞、thinking 回放、UTF-8 损坏、子进程挂死、stderr panic、会话损坏、配置信任边界 |
| 中 | 17 | 超时/重试、MCP 分类与关联、熔断器、`--allow` 语义、路径基准、REPL/退出码、doctor、记忆并发写与笔记上限 |
| 低 | 9 | 提示误报、日志噪声、抽象空转、SSE 细节、测试隔离 |

---

## 1. 门禁实测

| 门禁 | 结果 | 说明 |
| --- | --- | --- |
| `cargo fmt --all --check` | ✅ | — |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | ✅ 0 警告 | — |
| `cargo doc --workspace --no-deps --locked`（`RUSTDOCFLAGS="-D warnings"`） | ✅ | 无断链、无未文档化项 |
| `cargo test --workspace --locked` | ✅ 230 / 230 | 11 个测试二进制全绿 |

**环境注意（非项目缺陷）**：本机（Windows + MSVC 14.44）dev profile 的 `-Z7` 调试信息
会让 `cl.exe → c1.dll` 的 debug-record 交接失败（`D8050`），`ring` 等 C 依赖编译不过；
`CARGO_PROFILE_DEV_DEBUG=0` 可绕过。CI 用默认设置，不受影响。上一版记录的 `local` 来源
`echo` 测试（Windows 上 `echo` 是 shell 内建）**已修**：改为写一个只输出非 JSON 的
fixture 程序。

---

## 2. 评分（主观加权）

| 维度 | 评分 | 依据 |
| --- | --- | --- |
| 架构与分层 | 9.5 | 三层单向依赖；`ToolSource`/`ToolCatalog` 把"来源"关在装配期（D2 落地），无回边 |
| 代码质量与一致性 | 9.0 | 0 `unsafe`、0 `.unwrap()`、0 TODO、0 `#[allow]`、3 处带 reason 的 `#[expect]`；`#[cfg]` 只在 `platform.rs` |
| 文档 | 8.5 | 文档体系完整且双语；但至少 6 处与实现不符（见各条） |
| 可扩展性 | 9.0 | 四 trait 扩展缝 + 六种来源 + 记忆层缝（§3.1）+ 配方/不变量/PR 清单；第三个 provider 边际成本低 |
| 安全性 | 6.5 | 审批模型、类别策略、路径围栏设计好；配置信任边界、`readOnlyHint`、skill 逃逸有实质问题 |
| 健壮性 | 6.5 | 4 条可永久挂死路径；provider 无读超时；外部输出无大小上限；记忆层亦然（§3.3） |
| 测试 | 7.0 | 测试质量普遍不错（230 个）；审批/退出码/`--log`/`--cwd`/abort 零覆盖 |
| **综合** | **7.9** | 读起来很舒服，但还不足以放心跑在不可信仓库与长会话上 |

---

## 3. 本轮新增：记忆层（决策 D16）

`--memory <FILE>` 让智能体拥有一份**跨会话存活**的笔记：追加式 JSONL 文件（一行一条
`MemoryEntry { at, text }`，上限 200 条），最新若干条每请求折进 system prompt；裁剪上下文
时把被丢弃的回合蒸馏成一条 `[compacted]` 笔记。`remember`/`recall` 两个工具**仅在传入
该参数时**由 `BuiltinSource::with_memory()` 追加进装配结果。

### 3.1 值得肯定

- **贴合北极星**：300 行、无新依赖、无新时间来源（复用 `breaker::unix_now`）；存储沿用
  会话文件已验证的"追加 + 截断尾行丢弃"约定（`parse_entries` 跳空行/坏行），没有另造
  一套崩溃恢复逻辑（①④⑤）。
- **装配期决定工具集**：记忆工具**独立于 `builtin_tools()`**，默认不出现——与 D2 完全
  一致，循环仍只见扁平工具列表（`agent.rs` 里没有任何"记忆来源"概念）。
- **注入点干净**：`Option<Arc<Memory>>` 一路穿过 `Agent` 与 `ToolContext`，`with_memory`
  是纯 builder；`system_prompt_with_memory()` 每轮重读，因此**运行中写入的笔记下一轮即
  可见**（已验证）。
- **蒸馏方向正确**：`distill_messages` 只取用户消息（最多 5 条 × 120 字符），保留"问了
  什么"而不把工具输出拖回记忆——这是对的选择。
- **测试**：新增 17 个（`memory.rs` 7、`tools.rs` 3、`builtin.rs` 2、`agent_loop.rs` 3、
  `cli.rs` 2），覆盖往返、容量淘汰、坏行容忍、权限类别、无 memory 时的报错、压缩蒸馏。

### 3.2 值得商榷

- **每轮两次同步文件读**：`system_prompt_with_memory()` 在 `compact_context` 与
  `build_context` 各调一次，每次都 `read_to_string` 整个记忆文件。文件小、可接受，但
  是**每轮 O(文件) 的阻塞 I/O**，且与"上下文裁剪很省"的优化基调不一致。建议缓存一份
  或改异步读。
- **反馈回路的边界**：每次裁剪都追加一条 `[compacted]` 笔记，而该笔记下一轮进入 system
  prompt、**增大**预算占用。若转录长期超预算，会形成"裁剪→写笔记→prompt 变大→再裁剪"
  的软回路。有 200 条上限兜底，但建议给蒸馏笔记单独限频（如每 N 轮最多一条）。
- **`remember` 免审批**：文档理由（"只碰记忆文件、不碰工作树"）成立，但模型能借
  `remember` 把**工具输出里的对抗文本**固化成长期指令，且无需确认。属记忆功能的固有
  风险，至少在文档里点名更好。

### 3.3 与既有缺陷同类

- **新增 M16 · 记忆并发写丢笔记**：`Memory::append` 是"读—改—写"且**无锁**，两个进程
  共用同一 `--memory` 文件会相互覆盖（后写者胜），与 [README](../README.md) 对会话文件
  的承诺同属 H6.4 一类。会话文件至少是"追加"，记忆是"整文件重写"，丢得更多。
  *已验证（源码）。*
- **新增 M17 · 单条笔记无大小上限**：`append` 只 `trim()`，不限制长度；`render` 又把最多
  20 条原样折进 system prompt。一条超大笔记（模型可以写出）即可挤占预算。属 M7"外部输出
  无上限"的同类。*已验证（源码）。*

### 3.4 小结

记忆层的**方向与纪律都对**——它把"上下文裁剪会永久丢东西"这个真实缺口补上了，且没有
破坏 D2。欠的是**边界**：并发、大小、频率，正好是本文反复出现的主题。修 H6/M7 时顺手
把 M16/M17 一起修即可。

---

## 4. 高优先级：静默错误与数据丢失

### H1. Anthropic 的 `error` 之后仍发 `Done`，错误被覆盖成"成功空回复"

- **位置**：[anthropic.rs:387-397](../crates/rsagent-ai/src/providers/anthropic.rs#L387-L397)、
  [anthropic.rs:550-574](../crates/rsagent-ai/src/providers/anthropic.rs#L550-L574)、
  [agent.rs:478-480](../crates/rsagent-agent/src/agent.rs#L478-L480)、
  [provider.rs:212-217](../crates/rsagent-ai/src/provider.rs#L212-L217)
- **现象**：`error` 事件只被 `push` 进事件列表，不终止；SSE 关闭后 `finish()` 无条件
  `push(StreamEvent::Done)`。两个消费方都实现为"最后一个终止事件获胜"，于是 `Done`
  覆盖 `Error`：`stop_reason = Stop`、内容为空，`run_turns` 不走错误分支也不执行工具，
  `prompt()` 返回 `Ok(())`。
- **影响**：Anthropic 的 overloaded / 中途 5xx 是常态故障。终端会打印一行错误，但脚本
  与 `--json` 消费者认为运行成功（退出码 0），transcript 里也没有失败记录。
- **修复**：解析器遇 `error` 置 `failed`，`finish()` 在 failed 时不再发 `Done`；
  补断言事件序列为 `[..., Error]`。
- **验证**：已验证（源码；本轮回读 `anthropic.rs:568` 仍在无条件 `push(Done)`）。

### H2. Anthropic 回放丢掉 thinking + signature，`--thinking` + 工具调用第二轮必然 400

- **位置**：[anthropic.rs:242-255](../crates/rsagent-ai/src/providers/anthropic.rs#L242-L255)；
  数据来源见 [:419-425](../crates/rsagent-ai/src/providers/anthropic.rs#L419-L425)、
  [:497-505](../crates/rsagent-ai/src/providers/anthropic.rs#L497-L505)
- **现象**：`assistant_blocks` 只输出 Text 与 ToolCall，把已收集的 Thinking 与 signature
  丢掉。Anthropic 要求开启 extended thinking 的 assistant 轮在后续请求中原样带回
  thinking block，否则拒绝请求（400）。
- **影响**：`--thinking` 与工具调用这一对**受文档支持的功能组合完全不可用**——第一次
  工具调用后第二轮即失败。
- **修复**：`assistant_blocks` 先输出 `Thinking { thinking, signature }`，再输出
  text/tool_use；补"thinking + tool_use 回放"单测。
- **验证**：已验证（代码路径）+ 外部协议要求。

### H3. 流式解码按 chunk 做 `from_utf8_lossy`，跨 chunk 的多字节字符被永久损坏

- **位置**：[openai.rs:124](../crates/rsagent-ai/src/providers/openai.rs#L124)、
  [anthropic.rs:155](../crates/rsagent-ai/src/providers/anthropic.rs#L155)
- **现象**：`buffer.push_str(&String::from_utf8_lossy(&bytes))`。TCP/TLS chunk 边界可落在
  任意字节上，被切开的中文/emoji 变成 U+FFFD，且不可恢复。
- **影响**：中文回答、`--json` 事件流、会话文件中的文本都会被污染；英文场景完全看不出来，
  因此极难发现。
- **修复**：buffer 改为 `Vec<u8>`，按 `b'\n'` 切行后只对完整行做 UTF-8 解码；把
  "喂字节"抽成可单测的 `feed(&mut self, &[u8])`。
- **验证**：已验证（源码；本轮回读两处 `from_utf8_lossy` 仍在）。

### H4. 子进程退出后无超时地排空管道：`local` / `skill` / `bash` 可永久挂死

- **位置**：[subprocess.rs:70-88](../crates/rsagent-agent/src/sources/subprocess.rs#L70-L88)、
  [tools.rs:781-818](../crates/rsagent-agent/src/tools.rs#L781-L818)
- **现象**：`timeout` 只包住 `child.wait()`，随后无条件 `await` 两个排空任务。若脚本启动
  的后台进程继承了 stdout/stderr（`daemon &`、`nohup`、`cmd /C start`），EOF 永不到来，
  一个 turn 永久卡死——`timeout_secs` 的承诺在这条路径上失效。另外写入 stdin 发生在超时
  开始**之前**（[subprocess.rs:51-64](../crates/rsagent-agent/src/sources/subprocess.rs#L51-L64)、
  [mcp_stdio.rs:249-260](../crates/rsagent-agent/src/sources/mcp_stdio.rs#L249-L260)），
  大请求写不进去时同样阻塞。
- **影响**：`local`/`skill` 都是任意外部程序，属于高概率场景；且 `bash` 超时只杀直接子进程，
  进程树会残留。
- **修复**：一个总 deadline 覆盖"写 stdin + wait + drain"；超时后杀进程树（Unix 进程组 /
  Windows Job Object），再对 drain 任务加短超时后 `abort()`；让非内置来源接入 `CancelToken`。
- **验证**：已验证（源码；本轮回读 `subprocess.rs` 的 `select!` 仍只覆盖 `child.wait()`）。

### H5. `mcp-stdio` 的 stderr 尾部裁剪在 UTF-8 边界 panic

- **位置**：[mcp_stdio.rs:288-305](../crates/rsagent-agent/src/sources/mcp_stdio.rs#L288-L305)
- **现象**：`let cut = text.len() - 4096; text.drain(..cut)` 用字节长度切片。纯中文 stderr
  的 `len` 是 3 的倍数，`len - 4096 ≡ 2 (mod 3)`，`cut` 通常不在字符边界 → `String::drain`
  panic。
- **影响**：panic 发生在 `tokio::spawn` 的任务里，进程不崩，但该任务死掉、`sink` 的
  `Mutex` 被毒化，**stderr 诊断从此静默丢失**（而它正是连接失败时唯一的排障线索），
  同时默认 panic hook 会在终端打出噪声。
- **修复**：`while !text.is_char_boundary(cut) { cut += 1 }`；改用环形缓冲区避免 O(n) 前移。
- **验证**：已验证（源码）。

### H6. 会话持久化有四条数据损坏/丢失路径

1. **新版本文件被旧版本覆盖**：`load` 拒绝 `version > 1`
   （[session.rs:206-215](../crates/rsagent/src/session.rs#L206-L215)），
   `resume_session` 只打印警告（[main.rs:439-446](../crates/rsagent/src/main.rs#L439-L446)），
   随后 `--print` 结束仍会 `save()`（[main.rs:266-269](../crates/rsagent/src/main.rs#L266-L269)），
   把新版文件整体改写成 v1 空会话。STABILITY 承诺的"拒绝而非误读"退化为"读不到就毁掉"。
2. **已存在的空 `.jsonl` 永久损坏**：空文件加载返回 `(None, 0, false)`
   （[session.rs:229-234](../crates/rsagent/src/session.rs#L229-L234)），保存判定"可追加"
   而只写消息行、不写 header（[session.rs:168-182](../crates/rsagent/src/session.rs#L168-L182)）；
   此后再也加载不了，且每次运行继续追加。
3. **只按消息数量判断"文件是前缀"**：上下文裁剪丢弃中间回合后，若本轮又新增了足够多的
   消息，`written <= len` 仍成立，于是只追加最后几条——文件里保留着已被裁掉的旧回合，
   而真正新增的前几条被丢掉。
4. **并发 append 无锁**：两个进程同写一个 `.jsonl` 会交织，与
   [README.md:512-513](../README.md#L512-L513) 的"a concurrent run never leaves half a
   document behind"相反。
- **修复**：不兼容版本改为只读并跳过保存（或先 `.bak` 重命名）；header 缺失时强制整文件
  重写；记录"已写入的最后一条消息指纹"而不是数量；对会话文件加锁；修正 README 措辞。
- **验证**：已验证（源码）。

### H7. 配置信任边界：自动发现的 `.env` / `rsagent.toml` 能改端点、能执行代码

- **位置**：[main.rs:176](../crates/rsagent/src/main.rs#L176)、
  [config.rs:107-113](../crates/rsagent/src/config.rs#L107-L113)、
  [main.rs:659-694](../crates/rsagent/src/main.rs#L659-L694)、
  [local.rs:64-87](../crates/rsagent-agent/src/sources/local.rs#L64-L87)
- **现象**：
  - `.env` 在解析参数**之前**从 cwd 向上加载，dotenvy 只保护已存在的变量。仓库里的 `.env`
    可以设置 `RSAGENT_BASE_URL`，于是用户环境里真实的 `OPENAI_API_KEY` 会被发往该 URL；
    它还能设置 `RSAGENT_USER_CONFIG`，把用户级配置指向仓库内文件。
  - `rsagent.toml` 同样从 cwd 向上自动发现，而 `build_catalog` 在**任何审批发生之前**就执行
    `local` 的 `command --describe` 并 `spawn` `mcp-stdio`；`--list-tools` / `tools list` /
    `tools doctor` 都会触发。
- **影响**：`git clone && rsagent --list-tools` 等于执行仓库声明的任意命令；配合上面的
  `.env` 还能顺手把 key 送出去。README 反而鼓励把 key 放进项目 `.env`。
- **修复**：明确写出"项目配置 = 可执行代码"的信任假设，并加一道 opt-in（首次加载项目层
  来源时确认 / `--trust-project` / 记录配置哈希）；`.env` 至少拒绝设置 `RSAGENT_BASE_URL`、
  `RSAGENT_USER_CONFIG` 这类安全敏感变量，或改为只在显式 `--env-file` 时加载。
- **验证**：已验证（源码）。

---

## 5. 中优先级

| # | 问题 | 位置 | 影响与修复 | 验证 |
| --- | --- | --- | --- | --- |
| M1 | provider 只有 `connect_timeout`，没有读/空闲超时；注释自称"接了 TCP 不响应也不会挂住"是错的 | [core.rs:13-31](../crates/rsagent-ai/src/providers/core.rs#L13-L31) | 服务端接受连接后停发字节 → 一个 turn 永久卡住（CI 里即进程不退出）。加 `read_timeout` 或 per-request 超时 | 已验证 |
| M2 | 重试忽略 `Retry-After`、无 jitter，且把 409 当可重试 | [http.rs:11-13](../crates/rsagent-ai/src/providers/http.rs#L11-L13)、[http.rs:45-48](../crates/rsagent-ai/src/providers/http.rs#L45-L48) | 429 时继续捶打；解析 `Retry-After` 并加抖动 | 已验证 |
| M3 | 流内 `{"error":...}` 被忽略、缺 `finish_reason` 默认成 `Stop` | [openai.rs:321-345](../crates/rsagent-ai/src/providers/openai.rs#L321-L345)、[openai.rs:529-535](../crates/rsagent-ai/src/providers/openai.rs#L529-L535) | 截断流里已解析出的 tool call 会被静默丢弃，运行照常"成功"结束。缺终止标记时报 `stream truncated` | 已验证 |
| M4 | `Usage.input` 两家语义相反（Anthropic 不含 cache，OpenAI 含），`total_tokens` 又不计 cache | [types.rs:86-91](../crates/rsagent-ai/src/types.rs#L86-L91)、[anthropic.rs:360-365](../crates/rsagent-ai/src/providers/anthropic.rs#L360-L365)、[openai.rs:324-341](../crates/rsagent-ai/src/providers/openai.rs#L324-L341) | 开了 prompt cache 后 token/成本明显偏低；统一口径并补 cache 断言 | 已验证 |
| M5 | 非内置来源都没实现 `permission()`，全部落到默认 `Exec` | [tool.rs:239-249](../crates/rsagent-agent/src/tool.rs#L239-L249)、[web.rs:240-260](../crates/rsagent-agent/src/sources/web.rs#L240-L260)、[mcp_http.rs:451-465](../crates/rsagent-agent/src/sources/mcp_http.rs#L451-L465) | `--allow` 语义失效：`--allow network` 放行不了 `web`/`mcp-http`，`--allow exec` 却会自动批准带 `Authorization` 的 HTTP 调用；`tools list` 把网络工具标成 `[exec]` | 已验证 |
| M6 | 配置里的相对路径按**进程 cwd** 解析，而不是配置文件目录或 `--cwd` | [main.rs:736-779](../crates/rsagent/src/main.rs#L736-L779)、[config.rs:161-165](../crates/rsagent/src/config.rs#L161-L165) | 在 `repo/sub` 下运行，`path = "./skills/team"` 会找错目录并被"静默跳过"；`--cwd /repo` 从别处启动同理。应记住配置文件目录并以它为基准 | 已验证 |
| M7 | 外部输出无大小上限 | [tools.rs:452-474](../crates/rsagent-agent/src/tools.rs#L452-L474)（search 整文件读入）、[subprocess.rs:105-112](../crates/rsagent-agent/src/sources/subprocess.rs#L105-L112)、[mcp_http.rs:214](../crates/rsagent-agent/src/sources/mcp_http.rs#L214) | 32K 截断发生在完整缓冲之后，对内存峰值无效；一个大文件/大响应即可 OOM。读侧加 `take(limit)` | 已验证 |
| M8 | `mcp-http` 先看 body 再看状态码；非 2xx 带 `error` → 当 `Protocol` → `record_success` 不熔断；SSE 整体 `response.text()` 缓冲 | [mcp_http.rs:201-249](../crates/rsagent-agent/src/sources/mcp_http.rs#L201-L249)、[mcp_http.rs:254-274](../crates/rsagent-agent/src/sources/mcp_http.rs#L254-L274) | 网关 429/5xx 伪装成"协议可达"，来源永不隔离；保持 SSE 打开的服务器会一直超时。先按状态码分类，再增量解析 SSE | 已验证 |
| M9 | JSON-RPC 只按 `id` 关联，不区分 server→client 请求；分页游标无上限；stdio 的超时是"每读一行" | [mcp_stdio.rs:212-224](../crates/rsagent-agent/src/sources/mcp_stdio.rs#L212-L224)、[mcp_http.rs:277-286](../crates/rsagent-agent/src/sources/mcp_http.rs#L277-L286) | id 撞号时 server 请求被当响应 → `tools/call` 返回 `null` 空成功；恒定 `nextCursor` 无限循环；通知流可让请求永不超时 | 已验证 |
| M10 | 熔断器 `record_success` 无状态检查、半开探测无世代/截止 | [breaker.rs:123-178](../crates/rsagent-agent/src/breaker.rs#L123-L178) | 并发来源里晚到的成功会关掉刚跳闸的熔断器；探测 future 被丢弃后 `HalfOpen` 永久卡死 | 已验证 |
| M11 | REPL 只容忍 `ModelFailed`；`MaxTurnsExceeded`/`NoResult` 直接踢出且跳过 session 保存；abort 在 `--print` 下退 0 | [main.rs:972-982](../crates/rsagent/src/main.rs#L972-L982)、[main.rs:275-281](../crates/rsagent/src/main.rs#L275-L281) | 与 [README.md:530-531](../README.md#L530-L531) "REPL never exits on a failed turn" 矛盾；跑满 50 轮会丢掉整轮对话 | 已验证 |
| M12 | `--print` 下 thinking 增量仍 `print!` 到 stdout | [render.rs:295-303](../crates/rsagent/src/render.rs#L295-L303) | `rsagent --print --thinking high ... > answer.md` 会把推理文本写进答案文件。OneShot 下应走 stderr | 已验证 |
| M13 | `tools doctor` 对坏配置和 `Unhealthy` 来源仍退 0 | [tools.rs:190-236](../crates/rsagent/src/tools.rs#L190-L236) | 与"可用作 CI gate"的承诺不符：拼错的 `rsagent.toml` 整份被忽略后落回内置来源，绿灯通过 | 已验证 |
| M14 | `read_file` 用 `lines()` 去掉 `\r`，`edit_file` 却做精确匹配 | [tools.rs:154](../crates/rsagent-agent/src/tools.rs#L154)、[tools.rs:317-328](../crates/rsagent-agent/src/tools.rs#L317-L328) | CRLF 文件（`.bat`、`.sln`、git autocrlf 的源码）上"读出来再改"必然 `old_string not found`。读侧保留 `\r` 或编辑侧做 CRLF 归一 | 已验证 |
| M15 | `skill` 的 `command` 可用 `../..` 或绝对路径逃出技能目录；`mcp-http` 信任服务器自报的 `readOnlyHint` | [skill.rs:175-201](../crates/rsagent-agent/src/sources/skill.rs#L175-L201)、[mcp.rs:90-96](../crates/rsagent-agent/src/sources/mcp.rs#L90-L96) | 复制别人的 skill 目录 = 可执行任意二进制且可免审批；远端服务器可让任意工具跳过审批。canonicalize 后校验 `starts_with(dir)`；网络来源默认不信任注解 | 已验证 |
| M16 | 记忆 `append` 无锁，"读—改—写"整文件重写，两个进程共用会丢笔记 | [memory.rs:72-89](../crates/rsagent-agent/src/memory.rs#L72-L89) | 与 H6.4 同类；比会话文件更糟（会话是追加，记忆是重写）。加锁或改追加式 | 已验证（新增） |
| M17 | 记忆单条笔记无大小上限，`render` 原样折进 system prompt | [memory.rs:72-125](../crates/rsagent-agent/src/memory.rs#L72-L125) | 模型可写超大笔记挤占上下文预算；属 M7 同类。给单条笔记加上限 | 已验证（新增） |

---

## 6. 低优先级

| # | 问题 | 位置 | 建议 |
| --- | --- | --- | --- |
| L1 | `looks_outside_cwd` 在默认 `cwd="."` 时把所有绝对写入路径误报为"工作目录之外"（`Path::new("C:\\x").starts_with(".")` 恒 false） | [render.rs:516-534](../crates/rsagent/src/render.rs#L516-L534) | 构造 `Renderer` 时 canonicalize `cwd` 再比较 |
| L2 | 每次运行都无条件打印 `· config: <path>`，与 "`-v` 打印配置" 不符 | [main.rs:668-670](../crates/rsagent/src/main.rs#L668-L670)、[README.md:568-570](../README.md#L568-L570) | 移入 `if cli.verbose` |
| L3 | stderr 着色按 stdout 是否 TTY 决定 | [render.rs:45-51](../crates/rsagent/src/render.rs#L45-L51) | `2>err.log` 且 stdout 为终端时日志仍含 ANSI；分别判断 |
| L4 | `--allow` 的合法性校验太晚（在 `build_catalog` 与缺 key 预检之后） | [main.rs:1051-1063](../crates/rsagent/src/main.rs#L1051-L1063) | 在任何 I/O 之前统一校验 |
| L5 | 项目级文件有三种基准目录：config/AGENTS/breaker 用 `--cwd`，session/log 用进程 cwd | [main.rs:371-374](../crates/rsagent/src/main.rs#L371-L374)、[breakers.rs:45-46](../crates/rsagent/src/breakers.rs#L45-L46) | `--cwd /repo --continue` 会读写 shell 当前目录的 session；统一基准并写进文档 |
| L6 | `Auth::base_url` 与 `StreamOptions::session_id` 全仓库无人消费；`StreamOptions` 的 `Debug` 会打印 API key | [provider.rs:18-27](../crates/rsagent-ai/src/provider.rs#L18-L27)、[:30-49](../crates/rsagent-ai/src/provider.rs#L30-L49)、[:41-43](../crates/rsagent-ai/src/provider.rs#L41-L43) | 要么真正消费，要么删除；手写 `Debug` 脱敏 |
| L7 | SSE 层：多行 `data:` 不合并、`event:` 被忽略、EOF 时无换行的残行不 flush；工具参数 JSON 解析失败静默变 `{}` | [sse.rs:24-42](../crates/rsagent-ai/src/providers/sse.rs#L24-L42)、[openai.rs:488](../crates/rsagent-ai/src/providers/openai.rs#L488) | 按事件累积 data 行；解析失败报错而非空参数执行 |
| L8 | 自定义模型上下文窗口恒为 128k | [types.rs:123-140](../crates/rsagent-ai/src/types.rs#L123-L140) | `--model llama3.1` 接 8k 窗口的 Ollama 时裁剪器永不触发；加 `--context-window` |
| L9 | 双 Ctrl-C 被吞（**推断**）：第一次后 `if !interrupted` 永久关闭中断分支，非内置来源又不读 `CancelToken` | [main.rs:1005-1023](../crates/rsagent/src/main.rs#L1005-L1023) | 卡在 60s 超时的工具上只能外部 kill；第二次中断应直接退出 |

---

## 7. 测试与 CI

- **空转测试**：`no_color_output_has_no_escapes`（[cli.rs:802-814](../crates/rsagent/tests/cli.rs#L802-L814)）
  在 `Command::output()` 下 stdout 本就不是 TTY，`Palette` 早已关闭，删掉 `--no-color` 也会通过；
  `NO_COLOR` 完全未测。
- **审批与退出码零 E2E**：没有 `--yes`、`y`/`a`/拒绝、`--allow` 实际放行、`--log`、`--verbose`、
  exit 1、`--cwd`、abort 的测试。而 README 自述"bash 未沙箱化，审批是唯一防线"、exit code 表
  被 STABILITY 列为 Semver 契约——核心逻辑没有回归保护。
- **测试不隔离用户级环境**：`rsagent()` 构造器（[cli.rs:9-11](../crates/rsagent/tests/cli.rs#L9-L11)）
  不设 `current_dir`、不清 `RSAGENT_USER_CONFIG`，多个测试会从仓库根加载 `.env`；开发机上一旦
  有 user config，`tools list` 断言就会红。
- **mcp-stdio 示例缺失时静默通过**：找不到示例程序时测试 `return`，仍算通过；
  应显式 skip 或先构建 examples。
- **已修**：`local` 来源的 `echo` 测试（原为跨平台不可用）改为写 fixture 程序。
- **CI**：与 AGENTS.md 完全对齐（[ci.yml:29-34](../.github/workflows/ci.yml#L29-L34)、
  [:41-53](../.github/workflows/ci.yml#L41-L53)、[:60-70](../.github/workflows/ci.yml#L60-L70)），
  并新增了 `Docs` 门禁 job。建议补 `permissions: contents: read`、actions 按 SHA 固定、
  `cargo-deny`/`cargo-audit`。

---

## 8. 值得保留的设计

1. **三层单向依赖**与"来源只在装配期存在"（D2）：`ToolSource` → `ToolCatalog` → 扁平
   `Vec<Box<dyn Tool>>`，循环完全不知道来源。记忆层也遵守了它（工具集在装配期决定）。
2. **失败分类按代码路径而非错误文本**（D4/D14）：`CallError::{Transport, Protocol}` 在类型层
   强制调用点区分，`Err(String)` 的自由格式因此是安全的。
3. **熔断器把时间作为参数注入**：状态机可确定性测试；`BreakerReport` 用墙钟秒跨进程持久化。
4. **lint 纪律**：0 unsafe / 0 unwrap / 0 TODO / 3 个带理由的 expect，`#[cfg]` 只在
   `platform.rs`；doc gate 进 CI。
5. **双语文档 + 决策日志（D1–D16）+ 可编译示例 + 图解页**；
   [EXTENSIBILITY](EXTENSIBILITY.md) 的"配方 + 不变量 + PR 清单"是可复用资产。
6. **单 stdin 所有者**（后台线程 + `Recall` 枚举）解决 REPL 与审批提示抢字节；
   `write_atomic`、会话版本信封、截断尾行检测都是正确的取舍——记忆层复用了后者。
7. **审批提示的"人话"渲染**（[render.rs:357-411](../crates/rsagent/src/render.rs#L357-L411)）
   与 `--print` 的 stdout/stderr 分离，是对用户体验真正用心的部分。

---

## 9. 已知取舍（非缺陷）

- `bash` 不沙箱化、审批是唯一防线——已在 README 明说，属于设计取舍。
- 同进程 WASM 插件沙箱是**非目标**——理由（子进程/HTTP 边界 + 熔断器）成立。
- CLI 面向中文用户（审批提示、`tools list` 均为中文）——产品决策，不是 bug；若要面向
  国际用户，建议统一语言或做 i18n。
- `Context` 的 `chars / 4` token 估算——粗糙但有注释说明，符合"最小化"北星。
- 记忆层默认关闭、需显式 `--memory`——正确的 opt-in，副作用（写文件）不该默认发生。

---

## 10. 建议的修复路线图

**P0（静默错误/丢数据）**

1. H1 Anthropic `Error` 必须终止；H2 thinking 回放；H3 UTF-8 分帧——都在 `rsagent-ai`，
   可一起补"喂字节"单测。
2. H4 子进程总 deadline + 杀进程树；H5 stderr 边界 panic。
3. H6 会话四条损坏路径（优先"新版本被覆盖"与空 JSONL）；顺手 M16 记忆并发写。
4. H7 信任边界：至少写进 README/STABILITY，并让 `.env` 不覆盖端点/用户配置。

**P1（可用性/语义）**

M1 读超时、M5 `--allow` 类别、M6 路径基准、M8/M9 MCP 分类与关联、M11 REPL/退出码、
M12 thinking 污染 stdout、M13 doctor 退出码。

**P2（收尾）**

M14 CRLF、M7/M17 输出与笔记上限、M2/M3/M4 协议细节、M10 熔断器；补审批/退出码/`--cwd`/
`--log` E2E；CI 加固。

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

- **已逐行阅读**：`crates/rsagent-ai/src/**`、`crates/rsagent-agent/src/**`（含新增
  `memory.rs`）、`crates/rsagent/src/**`、三个 crate 的 `tests/` 与 `examples/`、CI、
  全部 Markdown 文档。
- **已实际执行**：四道门禁（230 测试全过）、`unsafe`/`unwrap`/`cfg`/`allow` 统计、
  依赖清单、H1/H3/H4/M1/M5 的回读复核。
- **未实际执行（因只读约束，未启动网络服务/外部模型）**：真实 OpenAI/Anthropic 端点、
  真实 MCP 服务器、真实子进程挂死与双 Ctrl-C 场景。相关结论来自源码推演，已在条目中标注
  "推断"或"外部协议要求"。
