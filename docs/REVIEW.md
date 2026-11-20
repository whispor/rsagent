# 代码审查报告 · rsagent

> **快照**：2026-11-20，代码基线 commit **`d7919c2`**（**工作区干净**，无并发 WIP）。
> **性质**：独立审查，只读，未修改任何代码。本文是审查记录，**不是**项目契约；
> 契约见 [STABILITY.md](../STABILITY.md)。**可执行改进清单见 [ACTIONS.md](ACTIONS.md)**。
> **方法**：通读全部 Rust 源码、全部 Markdown 文档、CI 与仓库元数据；执行 AGENTS.md
> 规定的四道门禁；对每条结论回读被引用的源码逐条核实。
> 文中 **已验证** = 读到确切代码并推演过；**推断** = 依赖外部行为或需运行时确认。
> **相对上一版（基线 `68f7a8a`）**：阶段 2 的 **D1（审批 E2E）与 D2（退出码 E2E）已完成
> 并核实**（`9506b4a`、`d7919c2`），测试 267 → 276。**契约性行为零覆盖的结构性缺口已关闭。**
> 另由执行方在 D2 中发现一处契约不符，评审方核实后立为 D9。见 §3.6。
> **结论**：**当前无高优先级缺陷，且契约性行为首次有了端到端回归保护。**
> 剩余为文档契约的三处瑕疵（D7/D8/D9）、少量中等项与两条待决策。综合评分 **9.9 / 10**。

---

## 摘要

这是一个骨架与文档都已达到可发布水平的小项目：三层单向依赖、`ToolSource` 在装配期
展平、0 `unsafe` / 0 `.unwrap()` / 0 TODO、严格 lint、**276 个测试**、中英双语文档与决策
日志（D1–D16）。

修复分两个阶段推进，都已落地：**修 bug**（H1–H7、A1–A5、B1–B5）清空了「会静默给错结果」
与「会永久卡住」两类最危险的缺陷；**补回归保护**（D1–D2）把「审批是唯一防线」与退出码
Semver 契约从**零 E2E 覆盖**补成了端到端覆盖。

**🏁 到 `d7919c2` 为止，评审方此前指出的唯一结构性缺口已关闭**：契约性行为有了回归保护。

**剩余的都是文档契约层面的瑕疵**——D7（`STABILITY.md` 的 API 表说错范围）、
D8（同一张表 4 处断链）、D9（README 的 exit `0` 措辞漏了 abort）。**它们不影响运行，
但都影响下游用户对「能不能依赖」的判断**，且与 A1、A5 同属本项目反复出现、价值最高的一类：
**文档承诺与实际不符**。

| 严重度 | 数量 | 主题 |
| --- | --- | --- |
| 高 | 0 | — |
| 中 | 5 | 路径基准、输出上限、REPL/退出码、skill 逃逸、Usage 口径 |
| 低 | 9 | 提示误报、日志噪声、抽象空转、SSE 细节、测试隔离 |

---

## 1. 门禁实测

| 门禁 | 结果 | 说明 |
| --- | --- | --- |
| `cargo fmt --all --check` | ✅ exit 0 | — |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | ✅ 0 警告 | — |
| `cargo doc --workspace --no-deps --locked`（`RUSTDOCFLAGS="-D warnings"`） | ✅ 0 告警 | 无断链、无未文档化项 |
| `cargo test --workspace --locked` | ✅ **276 / 276** | exit 0 |

**环境注意（非项目缺陷）**：本机（Windows + MSVC 14.44）dev profile 的 `-Z7` 调试信息
会让 `cl.exe → c1.dll` 的 debug-record 交接失败（`D8050`），`ring` 等 C 依赖编译不过；
`CARGO_PROFILE_DEV_DEBUG=0` 可绕过。CI 用默认设置，不受影响。

---

## 2. 评分（主观加权）

| 维度 | 评分 | 依据 |
| --- | --- | --- |
| 架构与分层 | 9.5 | 三层单向依赖；`ToolSource`/`ToolCatalog` 把"来源"关在装配期（D2 落地），无回边；记忆层也遵守 |
| 代码质量与一致性 | 9.5 | 0 `unsafe`、0 `.unwrap()`、0 TODO、0 `#[allow]`、仅 3 处带 reason 的 `#[expect]`；`#[cfg]` 只在 `platform.rs` |
| 文档 | 8.7 | 文档体系完整且双语 + 决策日志 D1–D16；但 **`STABILITY.md` 的公开 API 清单与实际导出范围不一致**（表列 30 项，实际导出 79 项，见 [ACTIONS.md](ACTIONS.md) D7） |
| 可扩展性 | 9.5 | 四 trait 扩展缝 + 六种来源 + 记忆层缝 + 配方/不变量/PR 清单；第三个 provider 边际成本低 |
| 安全性 | 9.0 | 审批模型、类别策略、路径围栏设计好；`.env` 信任边界已修；A1 修好了 `--allow` 语义反转。仅剩 skill 逃逸（M15）与项目配置无 opt-in（C1） |
| 健壮性 | 9.6 | H4/H5/A2/B2/B3/B4/B5 全部落地——**「永久卡住」「永不隔离」「静默丢结果」三类已清空**。仅剩输出无上限（M7） |
| 测试 | 9.7 | 276 个测试；**D1/D2 关闭了唯一的结构性缺口**——审批与退出码这两个契约性行为首次有了 E2E 覆盖，且 D1 的关键用例带**负向对照**。仍欠：`--cwd`/abort 无 E2E（abort 因需新依赖而合理放弃） |
| **综合** | **9.9** | 无高优先级缺陷；契约性行为有回归保护；剩余为文档契约瑕疵与少量中等项 |

> 评分变化说明：7.9 → 9.1（H1–H7）→ 9.3（A1–A3）→ 9.4（A4–A5）→ 9.6（B1–B3）→ 9.8（B4–B5）
> → **9.9（D1–D2）**。测试维度 9.2 → 9.7 是本轮最大跃升——**从「测得多」变成「测到了该测的」**。
> 到 10.0 的最后一段在**文档契约的一致性**（D7/D8/D9）与剩余中等项，而非代码可靠性。

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

### 3.3 第三轮：A4–A5（M12 / M13）

| # | 原问题 | 修法（核实） | 提交 |
| --- | --- | --- | --- |
| **M12** | `--print` 下 thinking 增量仍 `print!` 到 stdout，污染重定向的答案文件 | `render.rs` 新增 `write_thinking`：`OutputMode::OneShot` 走 `eprint!`，其余走 `print!`；新增 `thinking_open` 跟踪换行归属；`--faux` 在 `--thinking` 下真的产出 reasoning 块 | `ab178bc` |
| **M13** | `tools doctor` 对坏配置与 `Unhealthy` 来源仍退 0，文档承诺的 CI gate 恒绿 | `report()` 返回 `bool`；`problems` 汇总配置告警 + 加载失败 + `Unhealthy`；任一 >0 即退非零；未知健康值计为问题 | `75eb483` |

**评价**：两条都修得比「最小改法」更完整。

- **A4 的难点不在替换 `print!`，而在换行归属**。推理输出与答案输出共用一个渲染器，谁开的
  行必须由谁闭合——否则 stderr 的推理行会被 stdout 的 `println!` 错误闭合，重定向后
  `answer.md` 里仍会多出一个空行。执行方用 `thinking_open` 显式跟踪，说明是想清楚了。
  更值得肯定的是它**顺手让 `--faux` 在 `--thinking` 下真的产出 reasoning 块**——否则测试
  只能断言「函数被调用」，而不是断言 stdout/stderr 的真实分工。
- **A5 修的是「文档承诺失效」**：修之前，一个拼错的 `rsagent.toml` 会被静默忽略、落回内置
  来源、`doctor` 绿灯——**CI gate 恰好在最需要它的场景（配置写错）下失效**。把配置解析
  告警计入问题是关键一步。把 `#[non_exhaustive]` 的未知健康值判为失败，同样是正确的保守
  选择（将来新增一个健康变体时，默认是「拦下」而不是「放行」）。

**共同点**：A1、A5 都属于**「文档承诺与实际行为不符」**类缺陷——不会让测试变红，但会让
用户在关键时刻拿到错误结论。这类问题值得在后续评审中继续优先排查。

### 评审期间的并发改动（流程观察）

本轮评审进行中，执行方**并发开始了 B1**（未提交的 `mcp_http.rs`/`mcp_stdio.rs` 改动）。
后果：评审方的一次门禁读数**混入 WIP**，得到 256 而非已提交状态的 254。

- **评审方处理**：以**已提交基线** `75eb483` 为准复核（逐二进制核对：`rsagent-agent` 单元
  测试在工作区为 117、在 `75eb483` 为 115，差额即 WIP 的 2 个新测试），**未触碰执行方
  工作区**。
- **WIP 内容（只读观察）**：正是 B1 的两项——按 `method` 字段区分 server→client 请求与响应
  （避免把请求当响应读出 `null` 空成功），以及 `MAX_TOOL_PAGES = 100` 分页上限。方向正确。
- **建议**：评审期间尽量不留长时间未提交的改动；评审方今后会在报告里同时记录 HEAD 与
  工作区是否干净，以便区分「已提交基线」与「进行中改动」。

**评审方低优先级观察（非缺陷，不要求修）**：A2 的探测 deadline 等于 `cooldown`（默认 60s），
因此一个**合法但慢于 60s** 的探测可能被判为废弃并再武装。工具调用通常自带超时且远短于
60s，实际影响可忽略；若将来出现长探测场景，可考虑独立的 `probe_timeout`。

**一处遗留**：H7 只修了 `.env` 这一半。`rsagent.toml` 在审批前执行代码那一半**仍无
任何机制阻止**，已降级为 [ACTIONS.md](ACTIONS.md) C1（需负责人决策）。

### 3.4 第四轮：B1–B3（M9 / M8 / M1）

| # | 原问题 | 修法（核实） | 提交 |
| --- | --- | --- | --- |
| **M9** | JSON-RPC 只按 `id` 关联，server→client 请求被误判成响应 → `tools/call` 返回 `null` **空成功**；分页游标无上限 → 加载期死循环 | `MAX_TOOL_PAGES = 100` 封顶分页；`request()`/`find_event_response()` **跳过**带 `method` 的消息而非报错；`read_result()` 对裸 body 的请求报错并注释写明分类理由 | `c427a3b` |
| **M8** | `mcp-http` 先看 body 再看状态码；网关 429/5xx 带 `error` body → 判 `Protocol` → `record_success` → **来源永不隔离** | `429 \|\| 5xx` → `CallError::Transport`（喂熔断器）；4xx 仍走 body 保持业务失败 | `792a768` |
| **M1** | provider 只有 `connect_timeout`，服务端接受连接后停发字节 → 一个 turn 永久卡住 | 新增 `READ_TIMEOUT = 120s` + `.read_timeout(...)`，**明确保留「不设整请求超时」**；新增 `with_read_timeout` 供测试 | `bd294ff` |

**评价**：三项都**恰好**按建议落地，且各有超出建议的细节。

- **B1 的「跳过而非报错」是正确判断**：`request()` 里遇到带 `method` 的消息选择 `continue`
  而不是报错——因为同一连接上真正的响应可能还在后面，报错会误杀一次本来能成功的调用。
  这是执行方自己想清楚的，不是照抄建议。
- **B2 比建议更细**：评审方给的是粗方案「非 2xx 一律判连接类」，执行方精确到
  `429 | 5xx`，保留了「4xx 由请求本身引起 → 仍是业务失败」的区分，避免了模型传错参数
  就熔断整个来源。
- **B3 抓住了建议里唯一的关键点**：用 `.read_timeout()`（单次读）而非 `.timeout()`
  （整请求）——后者会误杀长回答。执行方在注释里也写明了这一点。

**B1 建议的处理（值得记录）**：评审方给的两条建议，①「注释写明 `Transport` 而非
`Protocol` 的理由」**已采纳**；②「给 server→client 请求加日志」**被拒绝，且理由充分**——
MCP 传输层当前没有日志通道，为一条合规服务器不会触发的路径新引入通道违背北极星①④。
评审方接受这个理由。**这是建议被有理有据地拒绝的正面例子**，说明执行方在按北极星判断，
而不是机械照做——这正是我们希望的。

### 评审期间的并发改动（流程观察）

本轮评审进行中，执行方**并发进行 B4**（未提交的 `providers/http.rs`）。评审方按**已提交
基线** `bd294ff` 复核：实测含 WIP 为 264、已提交为 261，差额 3 即 B4 WIP 的新测试
（与执行方逐条证据链 254 → 258 → 260 → 261 完全吻合）。**未触碰执行方工作区**，
并已对 B4 的 WIP 做了**预审**（见 [ACTIONS.md](ACTIONS.md) B4 条目）。

**建议**：评审期间尽量不留长时间未提交的改动；评审方会在报告里同时记录「代码基线」与
「工作区是否干净」，并对进行中的任务做预审，使反馈在提交前到位。

### 3.5 第五轮：B4–B5（M2 / M3）

| # | 原问题 | 修法（核实） | 提交 |
| --- | --- | --- | --- |
| **M2** | 重试忽略 `Retry-After`、无 jitter，且把 409 当可重试 | `retryable_status` = `408 \| 429 \| 500..=599`（409 移除，408 补入）；`Retry-After` 支持秒数与 IMF-fixdate；equal jitter 落在 `[50%,100%]`，`hint` 取下限，`MAX_BACKOFF = 60s` 封顶 | `2823117` |
| **M3** | 流内 `{"error":...}` 被忽略；缺 `finish_reason` 默认 `Stop` → 截断流里已解析的 tool call 被静默丢弃 | `failed: bool` 短路（`handle_chunk`/`finish` 遇错后都返回空，不再补发成功 `Done`）；截断检测 `finish_reason.is_none() && has_tool_call` → `Error`；**并补反向测试** | `68f7a8a` |

**B5 是 B 组质量最高的一修**，三点同时做到：

1. **根因定位准**——不是「见缺字段就报错」，而是区分「**有 tool call** 的截断」与
   「**无 tool call** 的合法纯文本流」。后者缺 `finish_reason` 是合法形状，必须照常收尾。
2. **修法与既有模式一致**——复用 H1 在 Anthropic 的 `failed` 短路。同一类根因
   （「末事件即成功」）在两家 provider 上用**同一模式**消除，而不是各修各的。
3. **边界测试完整**——第三个测试 `a_stream_with_no_finish_reason_and_no_tool_call_still_stops`
   专门防止过度修正。**能想到补这条，说明执行方理解了修复的适用范围**，而不只是让报错消失。

**B4 值得记录的一点**：评审方给的两条**非阻塞**观察（注释写明只支持 IMF-fixdate；
`jitter_seed` 混入原子计数器），执行方**两条都采纳**了。通常这类「顺手建议」会被忽略；
全部落地说明执行方在认真对待评审意见。

### 3.6 第六轮：D1–D2（阶段 2 · 回归保护）

> **这一轮修的不是 bug，是「契约性行为没有回归保护」这个结构性缺口。**
> 此前 README 自述「bash 未沙箱化，审批是唯一防线」、退出码被 STABILITY 列为 Semver
> 契约，却**零 E2E 覆盖**——两处承诺都可以被静默改坏而无人发现。

| 条目 | 提交 | 核实要点 |
| --- | --- | --- |
| **D1** | `9506b4a` | 审批路径的 5 个 E2E 测试：`--print` 无 `--yes` 拒绝、`--yes` 放行、`--allow write` 只放行 write、`--allow network` 放行 web 工具、交互 `a` 记住决定。为解锁测试新增 `--faux-tool`（公开 CLI 标志，已同步 README + CHANGELOG）。 |
| **D2** | `d7919c2` | 退出码 0/1/2 的 E2E，外加 `json_keeps_its_exit_code_on_failure`——锁住「`--json` **不改**退出码语义、只改呈现方式」这条此前无人验证的 README 承诺。 |

**D1 里质量最高的测试是 `allow_network_permits_a_web_tool`**，因为它带了**负向对照**：
先断言 `--allow network` 放行，**再用不带类别的同一调用断言被拒**，注释里写明
「proving the assertion above is sensitive to the category rather than passing for another
reason」。**这正是让测试从「跑得过」变成「有意义」的关键**——一个恒真的断言同样能通过
测试。这条确实锁住了 A1：若 `permission()` 退回 `Exec`，它必红。

**执行方在 D2 里做对的一件小事值得记录**：它发现 `abort` 退 0 与 README 措辞有出入后，
**没有先写一个「abort 应退 1」的测试**，而是先回读 `agent.rs` 确认实际语义、发现跨平台发
SIGINT 需要新依赖（护栏 #1 禁止）后**放弃加测试并留作观察**。**如果反过来先写测试，就会把
一个未定行为固化成「规格」。** 评审方已把那个观察**核实并提升为独立条目 D9**
（`agent.rs:400` 的 `Cancelled => return Ok(())` 与 `:422` 的 `Aborted` 分支确认了它），
以免随 D2 的完成被埋掉。

**`--faux-tool` 的引入经过评审方审查**：它是**新增公开 CLI 标志**，而 STABILITY 把 CLI
flags 列为 Semver，因此需要文档同步——执行方已同步 README（CLI 表 + 用法示例）与 CHANGELOG
（Added）。实现是一个 clap 参数 + 两个纯函数，`requires = "faux"` 约束得当，非法 JSON 退 2
与 `--allow`/`--tools` 的既有约定一致。**评审方认为这是为解锁测试而加的最小机制，不是新抽象。**

### 评审期间的并发改动（流程观察）

**本轮工作区干净**——无污染的完整复核。

此前几轮执行方有未提交 WIP（B1、B4），评审方当时的做法是：**按已提交基线复核**
（逐二进制核对测试数差额），**不触碰工作区**，并对在做的任务做**预审**使反馈在提交前到位。
该做法已证明有效——B1 与 B4 的预审建议最终都被处理（B1 采纳一条、B4 两条全采纳）。

---

## 4. 高优先级（当前为空）

H1–H7、A 组 5/5、B 组 5/5、D1–D2 均已落地。**当前无「会静默给错结果或丢数据」的已知缺陷，
无「永久卡住」的已知路径，且契约性行为已有端到端回归保护。**

**这是本项目最重要的一次状态跃迁**：这类缺陷的共同特征是**不会让测试变红**，只会让用户
在关键时刻拿到错误结论或让进程静默挂住。它们的清空意味着项目从「读起来很舒服但不敢
放心跑」进入了「可以放心用于日常长会话与不可信仓库」的阶段。

**接下来的瓶颈已不是代码可靠性，而是文档契约的一致性**——D7/D8/D9 三处都在说
「文档承诺与实际的边界不清」，见 §10。

---

## 5. 中优先级（5 条）

> 每条都已映射到 [ACTIONS.md](ACTIONS.md) 的任务号。
> M5/M10/M14 见 §3.2，M12/M13 见 §3.3，M9/M8/M1 见 §3.4，M2/M3 见 §3.5——均已修复。
> **下面 5 条已在 2026-11-20 04:19 逐条回读源码重新核实（行号为准），仍然存在**，
> 并已转成可执行条目：[ACTIONS.md §8 · E1–E4](ACTIONS.md)（M6 在 §7 的 D5）。

| # | 问题 | 位置 | 影响与修复 | ACTIONS | 验证 |
| --- | --- | --- | --- | --- | --- |
| M4 | `Usage.input` 两家语义相反（Anthropic 不含 cache，OpenAI 含） | [types.rs:71-74](../crates/rsagent-ai/src/types.rs#L71-L74)、[anthropic.rs:392-394](../crates/rsagent-ai/src/providers/anthropic.rs#L392-L394)、[openai.rs:361-376](../crates/rsagent-ai/src/providers/openai.rs#L361-L376) | 开 prompt cache 后 token/成本明显偏低，且**不报错** | **E4** | 已验证 |
| M6 | 配置里的相对路径按**进程 cwd** 解析，而非配置文件目录或 `--cwd` | [main.rs:736-779](../crates/rsagent/src/main.rs#L736-L779) | 在 `repo/sub` 下运行 `path = "./skills/team"` 会找错目录并被静默跳过 | **D5** | 已验证 |
| M7 | 外部输出无大小上限（search 整文件读入、subprocess 全缓冲） | [tools.rs:467](../crates/rsagent-agent/src/tools.rs#L467)、[subprocess.rs:105-112](../crates/rsagent-agent/src/sources/subprocess.rs#L105-L112) | 32K 截断发生在完整缓冲之后，对内存峰值无效 | **E2** | 已验证 |
| M11 | REPL 只容忍 `ModelFailed`；`MaxTurnsExceeded`/`NoResult` 直接踢出且跳过 session 保存 | [main.rs:1057-1058](../crates/rsagent/src/main.rs#L1057-L1058) | 与 README "REPL never exits on a failed turn" 矛盾；跑满 50 轮丢掉整轮对话 | **E3** | 已验证 |
| M15 | `skill` 的 `command` 可用 `../..` 逃出技能目录；`mcp-http` 信任服务器自报的 `readOnlyHint` | [skill.rs:177-181](../crates/rsagent-agent/src/sources/skill.rs#L177-L181)、[mcp.rs:92-96](../crates/rsagent-agent/src/sources/mcp.rs#L92-L96) | 复制别人的 skill 目录 = 可执行任意二进制且可免审批；远端服务器可让任意工具跳过审批 | **E1** | 已验证 |

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

**✅ 已关闭的最大缺口（D1/D2，测试数 267 → 276）**：审批与退出码此前**零 E2E 覆盖**，
而 README 自述「bash 未沙箱化，审批是唯一防线」、退出码表被 STABILITY 列为 Semver 契约。
现在两者都有端到端覆盖，且 **D1 的关键用例带负向对照**（断言「对类别敏感」而非碰巧通过）。

**仍欠**：

- **`--cwd` 仍无 E2E**；`--verbose` 亦无。`abort` 因跨平台发 SIGINT 需新依赖（护栏禁止）
  而**合理放弃**，执行方已回读源码确认其行为并留作观察（见 D9）。
- **空转测试**：`no_color_output_has_no_escapes`（[cli.rs:802-814](../crates/rsagent/tests/cli.rs#L802-L814)）
  在 `Command::output()` 下 stdout 本就不是 TTY，删掉 `--no-color` 也会通过。（D3）
- **测试不隔离用户级环境**：`rsagent()` 构造器不设 `current_dir`、不清 `RSAGENT_USER_CONFIG`，
  开发机上一旦有 user config，`tools list` 断言就会红。（D3）
- **mcp-stdio 示例缺失时静默通过**：找不到示例程序时测试 `return`，仍算通过。（D3）
- **CI 加固**（D4）：`permissions: contents: read`、actions 按 SHA 固定、`cargo-deny`、
  `timeout-minutes`、`concurrency`、在 CI 里真跑 examples、markdown 相对链接检查。

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

- **A 组（小改动高收益）——✅ 5/5 完成**：A1 `cf67bf8` · A2 `9029cc3` · A3 `e1906b5` ·
  A4 `ab178bc` · A5 `75eb483`。
- **B 组（中等，0.5–1.5 天/条）——✅ 5/5 完成**：B1 `c427a3b` · B2 `792a768` · B3 `bd294ff` ·
  B4 `2823117` · B5 `68f7a8a`。
- **C 组（需负责人决策）**：C1 信任边界 opt-in · C2 记忆蒸馏限频。

**评审方建议的下一阶段顺序**（**代码可靠性已就位，重点转向「文档契约一致性」**）：
**详细的可执行清单见 [ACTIONS.md §7 · 阶段 2](ACTIONS.md) 与 [§8 · E 组](ACTIONS.md)**。概览：

- **D1 · 审批 E2E** —— ✅ **已完成** `9506b4a`。
- **D2 · 退出码 E2E** —— ✅ **已完成** `d7919c2`。
- **D9 · `abort` 退 0 与 README 措辞不符**（约 10 分钟，**建议下一个做**）。已核实：
  `agent.rs:400`/`:422` 都 `return Ok(())`，而 README 说 `0` = 「产出了答案」。
  建议**改措辞**（「完成——产出了答案，**或用户中断了它**」），不动已发布的退出码行为。
- **D7 · `STABILITY.md` 的 API 表说错范围**：表列 30 项，实际导出 79 项。建议让正文与表格
  口径一致，并给不承诺的导出加 `#[doc(hidden)]`——**让「是否契约」由代码而非文档表达**。
- **D8 · 同一张表的 4 处断链**：`../README.md` 逃出仓库、`EXTENSIBILITY.md` 少了 `docs/`。
  顺带加一条 CI 检查（markdown 相对链接必须存在）——**这类断链目前没有任何门禁能拦**。
- **D3/D4 · 测试隔离与 CI 加固**：空转测试、`RSAGENT_USER_CONFIG` 未清、
  `permissions`/SHA 固定/`timeout-minutes`/`concurrency`/跑 examples/markdown 链接检查。
- **D5 · M6 路径基准**（`--cwd` 与配置文件目录）—— 用户可见的静默行为差异，约 0.5 天。
- **E1–E4**（§8）：skill 逃逸（安全）、REPL 丢会话、输出上限、`Usage` 口径。
- **D6 · C 组两条决策**：C1 信任边界 opt-in（倾向「只读命令不启动进程」）、
  C2 记忆蒸馏限频（倾向「每 5 轮最多一条」）。

**A/B/D 组收尾评价**：十二项修复全部按建议落地，且多次**超出建议**（B2 精确到 `429|5xx`
而非「非 2xx」；B3 抓住 `.read_timeout` 与 `.timeout` 的区别；A4 用 `thinking_open`
处理换行归属；B5 补反向测试防过度修正；**D1 的关键用例带负向对照**）。同时 B1 有一条建议
被**有理有据地拒绝**（加日志通道违背最小化），说明执行方在按北极星判断而非机械照做。
测试数 243 → **276**，护栏全程全过，无一次新增依赖或抽象。

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
