# 行动清单 · rsagent

> **这是什么**：旁观评审专家给出的**可执行改进清单**。它不是项目契约（契约见
> [STABILITY.md](../STABILITY.md)），而是一份「按性价比排序、每条都写清了改哪里、
> 怎么验」的工作队列。
>
> **给执行智能体**：每轮开工前读一次本文件，按 §2 的顺序取任务。**做完一条就把该条的
> `状态` 从 `待办` 改成 `已完成 (commit)`**，并把验收证据（命令 + 结果）贴在条目末尾。
> 不要一次吞多条——每条独立提交，独立过门禁。
>
> **评审基线**：`dd112e9`（2026-11-19）。本文件由评审方维护；执行方只更新「状态/证据」，
> 不改写「问题/方案/验收」——若认为某条判断有误，请在条目下追加 `> 异议：…` 并说明，
> 由评审方复核后修订。

---

## 1. 当前状态快照（评审方实测）

| 门禁 | 结果 |
| --- | --- |
| `cargo fmt --all --check` | ✅ 干净 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | ✅ 0 警告 |
| `cargo test --workspace --locked` | ✅ **279 通过 / 0 失败**（@ `3270e5b`，工作区干净） |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked` | ✅ 0 告警 |

**已修复（评审方逐条核实）**：

- **H1–H7 + M16/M17**（9 条）：`3b22d5f`、`e939f87`、`c8f6351`、`73fb63b`。
- **A1 + A2 + A3**：`cf67bf8`、`9029cc3`、`e1906b5`。测试数 243 → 251。
- **A4 + A5**：`ab178bc`、`75eb483`。测试数 251 → 254。
- **B1 + B2 + B3**：`c427a3b`、`792a768`、`bd294ff`。测试数 254 → 261。
- **B4 + B5**：`2823117`、`68f7a8a`。测试数 261 → 267。
- **D1 + D2**：`9506b4a`、`d7919c2`。测试数 267 → 276。
- **D7 + D8 + D9**：`a326913`、`9b53edb`、`466515d`。测试数 276 → 278。
- **D3**：`3270e5b`。测试数 278 → **279**。
- **D4**：`a27c91c`（**只改 CI 与新增 `deny.toml`，不动 `crates/`**，故测试数不变）。
- **A 组 5/5、B 组 5/5 完成；阶段 2 的 D1/D2/D3/D4/D7/D8/D9 完成。**

**综合评分 9.96 → 9.97 / 10**（D4 补齐了 CI 的六项加固，含**真正运行 examples**）。

**护栏复核**：无新增依赖、无 `#[allow]`、无业务代码平台 `#[cfg]`、无 `unsafe`/`TODO`、
无无关重构。D3 新增一处**带 reason** 的 `#[expect]`，符合 AGENTS.md。✅

**🏁 里程碑一（已达成）**：「会静默给错结果」与「会永久卡住」两类缺陷已全部清空。

**🏁 里程碑二（已达成）**：**契约性行为有了端到端回归保护**（D1/D2）。

**🏁 里程碑三（已达成）**：**契约文档本身不再能悄悄漂移**（D7 双向校验 + D8 链接检查）。

**🏁 里程碑四（已达成）**：**测试不再有「因错误原因通过」的**（D3）。

**🏁 里程碑五（本轮达成）**：**CI 从「只跑四道门禁」变成一道真正的防线**。D4 补齐六项：
`permissions: contents: read`、actions **按 SHA 固定**、每个 job 的 `timeout-minutes: 30`
（注释直接引用 H4——**「子进程攥着管道」正是它要防的挂死**）、`concurrency` 取消过期运行、
`audit` job 跑 `cargo-deny check advisories`、以及**新增 `examples` job 真正运行 README
宣传的 4 个示例**（此前 CI 只编译它们）。

> ⚠️ **评审方流程提醒**：执行方此前数轮有**未提交的 WIP**（B1、B4、D8），评审方会对其做
> **预审**以便反馈在提交前到位，并**按已提交状态**复核、**不触碰工作区**。
> 本轮工作区干净，是一次无污染的完整复核。

**环境注意（非项目缺陷）**：本机 Windows + MSVC 14.44 下 dev profile 的 `-Z7` 会让
`ring` 编译失败（`D8050`），加 `CARGO_PROFILE_DEV_DEBUG=0` 可绕过；CI 不受影响。

---

## 1.5 ✅ 项目负责人已决策（2026-11-23）——以下两项可开工

> **决策已下，不再是「待讨论」。** 执行方**直接照选定方案实施**即可；完整论证见 §2 的 C1 / C2。
> 两条都**不阻塞**其他工作。

### 决策一 · C1 → **选定 (c)：只读命令不启动进程**

**背景**：项目 `rsagent.toml` 可声明 `local` / `mcp-stdio` / `skill` 来源，它们**在任何审批
之前**就会被启动。`git clone && rsagent --list-tools` 会执行仓库声明的任意命令。

**已选定**：**(c) 只读命令不启动进程。** `--list-tools` / `rsagent tools list` /
`rsagent tools doctor` **不执行** `install()`，只读配置、**不 spawn** 任何进程。

**执行方要点**（**2026-11-24 评审方回读代码后细化，见下方「已核实的实现约束」**）：
- 这三个命令改为**纯读配置**：列出将装配的来源与工具（工具名可从配置推导；`local`/`mcp-stdio`
  的工具名需要 `--describe`/`tools/list`，**因此这两个命令对这类来源只报「未启动，工具名未知」**，
  并在输出里**明说原因**——不要静默留空）。
- `tools doctor` 的「健康」语义随之调整：对**未启动**的来源报「未检查（只读模式）」，
  **且不计为问题**（否则 D4 的退出码逻辑会把只读命令变成恒红）。
- **`--trust-project`（选项 b）不在本次范围内**，留到有真实需求时再议。
- 文档同步：README 的「信任边界」与「Managing tool sources」两节要写明这一行为差异。

**已核实的实现约束（评审方 2026-11-24 回读代码）——这两条能省掉返工**：

1. **只需处理 6 种 kind 里的 3 种。** `build_source`（`crates/rsagent/src/main.rs:849`）
   里「构造即启动」只发生在三种来源：

   | kind | 构造时是否触达外部 | 只读模式下 |
   | --- | --- | --- |
   | `builtin` | 否 | **照常装配** |
   | `skill` | 否——`SkillSource::new` 只读 `SKILL.md` + `skill.toml` | **照常装配** |
   | `web` | 否——`WebSource::new` 只建 `reqwest::Client`，不发请求 | **照常装配** |
   | `local` | **是**——`LocalSource::new` 跑 `command --describe` | **只报声明** |
   | `mcp-stdio` | **是**——`McpStdioSource::new` spawn 服务端 | **只报声明** |
   | `mcp-http` | **是**——`McpHttpSource::new` 发 `initialize` 请求 | **只报声明** |

   即**只有 `local`/`mcp-stdio`/`mcp-http` 需要「声明态」**，另三种可原样装配——这把改动
   面从「所有来源」缩到「三种」，也解释了为什么 `skill` 的文档强调「自描述、无需运行」。

2. **不要用 `spawn: bool` 参数——项目 lint 禁止布尔参数。** `Cargo.toml` 的
   `[workspace.lints]` 开了 `fn_params_excessive_bools`，且 `AGENTS.md` 明确写
   「Boolean parameters are not allowed (`max-fn-params-bools = 0`); take an enum instead」。
   给 `build_catalog`/`build_source` 加模式时应取一个**二值 enum**（如
   `SourceMode::{Assemble, DeclaredOnly}`），否则 CI 的 `clippy -D warnings` 会红。
   *（这与 D15 用 `Permission` 而非布尔的做法一致——本项目的一贯风格。）*

**验收**：在含 `local` 来源的仓库里跑 `--list-tools` / `tools list` / `tools doctor`，
**断言声明的程序没有被执行**（例如让 `command` 指向一个会写文件的脚本，断言该文件**未**出现）。
**再加一条**：断言 `builtin`/`skill`/`web` 三种来源在只读模式下**仍照常列出其工具**——
避免实现时「一刀切全部跳过」，那会白白丢掉本来拿得到的信息。

### 决策二 · C2 → **选定 (b)：每 5 轮最多一条蒸馏笔记**

**背景**：每次上下文裁剪都追加一条 `[compacted]` 笔记；该笔记**下一轮进入 system prompt**、
**增大**预算占用，可能形成「裁剪 → 写笔记 → prompt 变大 → 再裁剪」的软回路。

**已选定**：**(b) 每 5 轮最多一条**（N=5）。

**执行方要点**：
- 在 `agent.rs` 的压缩路径上加一个**轮次计数器**，距上次写蒸馏笔记不足 5 轮时**跳过写入**。
- **不要**改 `distill_messages` 的内容与格式，也**不要**动 `Memory` 的存储约定（D16）——
  本条只加**频率门**。
- 计数器是**每会话**状态（不持久化）；进程重启后重新计数即可，不必写进会话文件。
- 文档同步：README 的 Memory 节说明「蒸馏笔记每 5 轮最多一条」。

**验收**：新增测试：连续触发 6 次裁剪 → 断言 `[compacted]` 笔记**只增加了 1 条**（不是 6 条）。

### 另：复评频率 → **已改为每 4 小时**

原为每约 30 分钟；稳定期空跑过多，已按负责人要求改为**每 4 小时**（2026-11-23 生效）。

---

## 2. 任务队列（按性价比排序，从上往下做）

> **性价比 = 影响 ÷ 成本**。A 组是「小改动、直接修好一个真实故障」，优先做。

### A 组 · 小改动高收益（建议优先，每条约 0.5–1 天）

---

#### A1 · 非内置工具补 `permission()`，修好 `--allow` 语义

- **状态**：已完成 (cf67bf8)
- **成本**：约 0.5 天 · **影响**：高（安全 + 可用性）
- **问题（已验证）**：`crates/rsagent-agent/src/sources/` 下 5 个 `impl Tool`
  （`local.rs:171`、`skill.rs:264`、`web.rs:243`、`mcp_stdio.rs:495`、`mcp_http.rs:452`）
  **都没有覆盖 `permission()`**，全部落到 `tool.rs:264` 的默认值 `Permission::Exec`。
  后果有两面，且**方向相反**：
  1. `--allow network` **放行不了** `webfetch` 之外的 `web` / `mcp-http` 工具——它们被标成
     `exec`，用户以为放行了网络，实际每次仍被问；
  2. `--allow exec` 却会**自动批准**带 `Authorization` 头的 HTTP 调用。`Exec` 比 `Network`
     宽松，这与 D15「默认取最严」的意图正好相反。
  3. `rsagent tools list` 把网络工具显示成 `[exec]`，误导用户。
- **方案**：
  - `WebTool`、`McpHttpTool`、`McpStdioTool` → `Permission::Network`；
  - `LocalTool`、`SkillTool` → `Permission::Exec`（子进程，语义正确）；
  - 在 `crates/rsagent-agent/tests/tool_conformance.rs` 加一条断言：
    **任何 `requires_approval() == true` 的非内置工具必须显式实现 `permission()`**。
    由于这些工具在 `sources/` 里、不在 `builtin_tools()` 中，该测试需直接构造各来源
    或用一个小型「注册表」列出「必须显式分类」的类型。
  - 更省的做法（若上面太重）：在 `tool.rs` 的 `permission()` 文档里写明默认值是 `Exec`，
    并给这 5 个 `impl` 各加 1 行显式覆盖；一致性由新增单测覆盖。
- **验收**：
  ```bash
  cargo test --workspace --locked
  # 新增测试至少覆盖：web/mcp-http 工具 permission() == Network
  #                    local/skill 工具 permission() == Exec
  ```
  再手工确认 `rsagent tools list` 对 `web` 来源显示 `[network]`。
- **关联**：D15、`docs/EXTENSIBILITY.md` §2.4 不变量 #4。

```
- 状态：已完成 (cf67bf8)
- 验收证据：
  - cargo fmt --all --check          → exit 0
  - cargo clippy ... -D warnings     → 0 警告
  - cargo test --workspace --locked  → 248 通过 / 0 失败（较上轮 +5）
  - cargo doc ...                    → 0 告警
  - 新增测试：a_web_tool_is_a_network_call、an_mcp_http_tool_is_a_network_call、
    an_mcp_stdio_tool_is_a_network_call、a_local_tool_is_an_exec_call、
    a_skill_tool_is_an_exec_call
- 文档同步：CHANGELOG.md
```

---

#### A2 · `record_success` 加状态判断，修好并发下的熔断器语义

- **状态**：已完成 (9029cc3)
- **成本**：约 0.5 天 · **影响**：中高（长会话可靠性）
- **问题（已验证）**：`crates/rsagent-agent/src/breaker.rs:145-151` 的 `record_success`
  **无条件**把状态置 `Closed`、清零 `failures`。两个后果：
  1. 并发来源（`web`、`mcp-http`、`skill` 都非 sequential）里，一个**在跳闸前发出**的
     请求晚到成功，会把刚跳闸的熔断器**立刻关掉**，隔离失效。
  2. `HalfOpen` 下 `allows()` 恒返回 `false`（`breaker.rs:128`），而只有
     `record_success` / `record_failure` 能离开该状态。若探测 future 被丢弃
     （`CancelToken` / turn 结束 / 任务 abort），**熔断器永久卡在 `HalfOpen`**，
     该来源从此不可用。
- **方案**：
  - `record_success` 只在 `Closed` / `HalfOpen` 时重置；若当前是 `Open`，**忽略**这次成功
    （它属于跳闸前的旧请求）。
  - 给 `HalfOpen` 探测加 **deadline**：`Inner` 记 `probe_started_at: Option<Instant>`，
    `effective()` 在 `HalfOpen` 且 `now - probe_started > probe_timeout` 时回到 `Open`
    （并刷新 `opened_at`），使来源能自愈。
  - 保持「时间从外部传入」的既有约定（`breaker.rs` 模块文档），**不要**在内部读
    `Instant::now()`——这样测试仍可确定性推演。
- **验收**：
  ```bash
  cargo test --workspace --locked
  ```
  新增单测：
  - `a_late_success_does_not_close_an_open_breaker`
  - `a_dropped_probe_returns_the_breaker_to_open`（推进时间越过 probe deadline 后
    `allows()` 再次有机会）
- **注意**：这条**不能**用 `#[allow]` 绕过任何 lint；用 `#[expect(..., reason = "…")]`。

```
- 状态：已完成 (9029cc3)
- 验收证据：
  - cargo fmt --all --check          → exit 0
  - cargo clippy ... -D warnings     → 0 警告
  - cargo test --workspace --locked  → 250 通过 / 0 失败（较上轮 +2）
  - cargo doc ...                    → 0 告警
  - 新增测试：a_late_success_does_not_close_an_open_breaker、
    a_dropped_probe_returns_the_breaker_to_open
- 文档同步：CHANGELOG.md
- 说明：探测 deadline 复用 `cooldown`（未新增 `BreakerPolicy` 字段），保持「时间从外部传入」约定。
```

---

#### A3 · `read_file` 与 `edit_file` 的 CRLF 一致性

- **状态**：已完成 (e1906b5) —— *评审方补标：执行方提交了修复但漏改此状态行*
- **成本**：约 0.5 天 · **影响**：中（Windows 仓库上的必然失败）
- **问题**：`read_file`（`tools.rs:157`）用 `data.lines()`，`lines()` 会**吃掉行尾的
  `\r`**；`edit_file` 做的是**精确子串匹配**。于是模型在 CRLF 文件上「读出来 → 原样贴回
  `old_string`」时，`old_string` 里没有 `\r` 而文件里有 → 必然 `old_string not found`。
  这是 Windows 仓库上最常见的编辑失败。
- **方案（二选一，推荐前者）**：
  - **(a) 读时保留 `\r`**：`read_file` 改用 `split_inclusive('\n')`（或按 `\n` 切后不
    去 `\r`），让读出的内容与磁盘逐字节一致。行号前缀逻辑不变。
  - **(b) 编辑时归一化**：`edit_file` 匹配前把「文件内容」和「`old_string`」的换行都归一到
    `\n` 再比对，写回时保留原文件的换行风格。
  - (a) 更省且更符合「读什么就是什么」，但会让输出里出现不可见的 `\r`（终端显示无碍）。
- **验收**：
  ```bash
  cargo test --workspace --locked
  ```
  新增单测：写一个含 `\r\n` 的文件 → `read_file` 取出的片段作为 `old_string` 传给
  `edit_file` → **编辑成功**。
- **关联**：M14。

```
- 状态：已完成 (e1906b5)  [评审方核实并补录]
- 验收证据：
  - cargo fmt --all --check          → exit 0
  - cargo clippy ... -D warnings     → 0 警告
  - cargo test --workspace --locked  → 251 通过 / 0 失败（较上轮 +1）
  - cargo doc ...                    → 0 告警
  - 新增测试：read_file_output_edits_a_crlf_file（agent_loop.rs）
  - 实现：read_file 改用 split_inclusive('\n') + strip_suffix('\n')，保留 '\r'
- 文档同步：CHANGELOG.md、README.md
- 评审方注：选了建议中的方案 (a)「读时保留 \r」，与首选一致；且带端到端测试
  而非仅单元断言。核实方式：`git show e1906b5 -- crates/rsagent-agent/src/tools.rs`
  与回读当前源码。
```

---

#### A4 · `--print` 下把 thinking 增量从 stdout 移到 stderr

- **状态**：已完成 (ab178bc)
- **成本**：约 0.25 天 · **影响**：中（污染重定向输出）
- **问题**：`--print --thinking high "…" > answer.md` 会把**推理文本**一起写进
  `answer.md`。`--json` 模式已隔离，但 `--print` 的默认渲染没有。
- **方案**：`render.rs` 里 thinking 增量走 `eprint!`（或仅在 TTY 时走 stdout）；
  `--json` 的既有行为不变。保持「stdout 只放最终答案」这一机器契约。
- **验收**：
  ```bash
  cargo run -p rsagent -- --faux --print "hi" > out.txt
  # out.txt 中不含 thinking 文本
  ```
  加一个 CLI 测试断言 stdout 与 stderr 的分工。
- **关联**：M12。

```
- 状态：已完成 (ab178bc)
- 验收证据：
  - cargo fmt --all --check          → exit 0
  - cargo clippy ... -D warnings     → 0 警告
  - cargo test --workspace --locked  → 252 通过 / 0 失败（较上轮 +1）
  - cargo doc ...                    → 0 告警
  - 新增测试：print_sends_reasoning_to_stderr_not_stdout
- 文档同步：CHANGELOG.md / README.md
- 说明：`--faux --print --thinking <level>` 现在会先输出一个推理块，使该分工可被真实断言。
```

---

#### A5 · `tools doctor` 对坏配置 / 不健康来源返回非零

- **状态**：已完成 (75eb483)
- **成本**：约 0.25 天 · **影响**：中（CI gate 承诺）
- **问题**：README 把 `rsagent tools doctor` 描述为「a CI gate」，但它对**配置解析失败**
  和 `SourceHealth::Unhealthy` 的来源仍退 `0`。CI 里它永远绿。
- **方案**：doctor 汇总所有问题，有任一问题即 `exit(1)`；`--json` 模式下同时给出机器可读的
  汇总。保持 `tools list` 的退出码语义不变（它是展示，不是检查）。
- **验收**：
  ```bash
  # 用一个声明了不存在程序的 rsagent.toml 跑 doctor，期望 exit 1
  ```
  加 CLI 测试覆盖「健康 → 0」「不健康 → 1」。
- **关联**：M13。

```
- 状态：已完成 (75eb483)
- 验收证据：
  - cargo fmt --all --check          → exit 0
  - cargo clippy ... -D warnings     → 0 警告
  - cargo test --workspace --locked  → 254 通过 / 0 失败（较上轮 +2）
  - cargo doc ...                    → 0 告警
  - 新增测试：tools_doctor_fails_on_a_broken_config、tools_doctor_fails_on_an_unhealthy_source
- 文档同步：CHANGELOG.md
- 说明：退出码沿用 `tools` 子命令的既有约定（非零 = 2，经 `tools::run` 的通用错误出口），
  而非字面 `exit(1)`；README 承诺的只是「non-zero」，`tools list` 的语义未变。
  `--json` 对 `tools` 子命令不可达（`--json` 非 global 且要求 `--print`），故未加 JSON 汇总。
```

---

### B 组 · 中等改动（A 组做完后，每条约 1–2 天）

---

#### B1 · MCP JSON-RPC 关联：区分请求方向 + 分页上限

- **状态**：已完成 (c427a3b)
- **成本**：约 1.5 天 · **影响**：中高
- **问题（已验证）**：
  1. `mcp_stdio.rs:222` 与 `mcp_http.rs:283` 只按 `id` 匹配。JSON-RPC 里 **server→client
     的请求也带 `id`**，会被误判成「对本次请求的响应」。若 id 撞号，`tools/call` 拿到
     `null` → `parse_call_result` 返回**空成功**，模型在空结果上继续，错误被静默吞掉。
  2. `tools/list` 的分页游标（`mcp_stdio.rs:189`、`mcp_http.rs:149`）**无上限**。服务器
     返回恒定 `nextCursor` 会**无限循环**（加载期挂死）。
- **方案**：
  - 关联时要求消息**是响应**：`id` 匹配 **且** 没有 `method` 字段（有 `method` = 请求，
    应走通知/请求处理路径，或至少跳过）。
  - `tools/list` 加游标上限（如 100 页）并在超限时报错，避免加载期死循环。
  - 可选但推荐：记录并忽略 server→client 请求（或在日志里可见），不要静默丢弃。
- **验收**：
  ```bash
  cargo test --workspace --locked
  ```
  新增测试：mock server 先发一个**同 id 的请求**再发真正的响应 → 断言客户端取到的是响应
  而非请求；`tools/list` 恒定游标 → 断言在有界时间内报错而非挂死。
- **关联**：M9。

```
- 状态：已完成 (c427a3b)
- 验收证据：
  - cargo fmt --all --check          → exit 0
  - cargo clippy ... -D warnings     → 0 警告
  - cargo test --workspace --locked  → 258 通过 / 0 失败（较上轮 +4）
  - cargo doc ...                    → 0 告警
  - 新增测试：a_same_id_server_request_is_not_taken_as_the_response（stdio）、
    a_same_id_server_request_in_the_stream_is_not_the_response（http）、
    a_constant_cursor_is_refused_rather_than_looping_forever（stdio + http 各一）
- 实现：
  - 两份文件各加 `const MAX_TOOL_PAGES: usize = 100;`，`loop` 改 `for _ in 0..MAX_TOOL_PAGES`，
    无游标 `return Ok(tools)`，耗尽则报错（`… returned more than 100 pages of tools`）。
  - `mcp_stdio::request()` 与 `mcp_http::find_event_response()`：跳过带 `method` 的消息
    （请求/通知），继续等真正的响应——同一连接上响应可能在后。
  - `mcp_http::read_result()`：裸 JSON body 带 `method` → `CallError::Transport` 并报错
    （body 只有一个对象，无流可继续扫；注释已写明为何归 Transport 而非 Protocol）。
- 文档同步：CHANGELOG.md（Fixed）、两处模块级文档（Wire format / Session）
- 未做（评审方标注为可选）：server→client 请求的日志可见性。MCP 传输层当前没有日志通道，
  为一条「合规服务器不会触发」的路径新引入一个通道，违背北极星①④，故未加。
```

> **评审方核实（2026-11-19 18:19，已提交状态 `c427a3b`）——✅ 通过**
>
> 提交版本与评审方 17:49 的预审一致，逐条核实：
> - `MAX_TOOL_PAGES = 100`，`loop` 改 `for`，无游标 `return Ok(tools)`，耗尽报错。
> - `mcp_stdio::request()` 与 `mcp_http::find_event_response()` 都**跳过**带 `method` 的
>   消息（而非报错），继续等待真正的响应——这比「报错退出」更好，因为同一连接上真正的
>   响应可能还在后面。
> - `mcp_http::read_result()` 对裸 JSON body 中带 `method` 的消息报错，这是唯一可行的选择
>   （body 只有一个对象，没有流可继续扫）。
> - 4 个测试对准原问题（同 id 的诱饵请求 + 恒定游标），**已提交状态实测 258 通过 / 0 失败**。
>
> **两条建议的处理**：
> 1. ✅ **已采纳**：`read_result()` 的注释已写明为何归 `Transport` 而非 `Protocol`——
>    正是评审方要的「把判断理由写下来」。
> 2. ❌ **未采纳，但理由充分**：server→client 请求不加日志。执行方说明「MCP 传输层当前
>    没有日志通道，为一条合规服务器不会触发的路径新引入通道，违背北极星①④」。
>    **评审方接受这个理由**——它正确地用最小化否决了一个「顺手加」的建议。若将来 MCP
>    层因别的原因需要日志通道，再顺手带上即可。
>
> 评审方注：这是本轮**建议被有理有据地拒绝**的一个正面例子，说明执行方在按北极星判断，
> 而不是机械照做。

---

#### B2 · `mcp-http` 先按 HTTP 状态码分类，再看 body

- **状态**：已完成 (792a768)
- **成本**：约 0.5 天 · **影响**：中
- **问题（已验证）**：`mcp_http.rs:201-243` 先读 body、再解析，**非 2xx 的 body 若带
  JSON-RPC `error`** 会被判成 `CallError::Protocol` → 调用方 `record_success()` →
  **来源永不隔离**。网关 429 / 5xx 是常态，等于熔断器对 HTTP 来源基本失效。
- **方案**：拿到 `status` 后先分类：
  - 非 2xx → `CallError::Transport`（连接类，喂熔断器），body 仅作诊断；
  - 2xx → 再按 content-type 解析（SSE / JSON）。
  保持 D9「有 HTTP 应答即证明可达」的精神**但**修正：4xx/5xx 说明端点**不能正常服务**，
  对网关类故障应按连接类处理。若担心模型传参导致 400 误伤，可只把 **429 / 5xx** 判连接类，
  4xx 仍判业务失败——**推荐这个折中**。
- **验收**：新增测试：mock 返回 `429` + JSON-RPC error body → 断言 `record_failure` 路径
  被走到（熔断器计数增加）；返回 `400` + error body → 断言仍是业务失败不熔断。
- **关联**：M8、D9。

```
- 状态：已完成 (792a768)
- 验收证据：
  - cargo fmt --all --check          → exit 0
  - cargo clippy ... -D warnings     → 0 警告
  - cargo test --workspace --locked  → 260 通过 / 0 失败（较上轮 +2）
  - cargo doc ...                    → 0 告警
  - 新增测试：a_gateway_failure_trips_the_breaker_even_with_a_json_rpc_error_body、
    a_bad_request_stays_a_business_failure
- 实现：`exchange()` 读完 body 后、解析前先判状态——`status == 429 || status.is_server_error()`
  → `CallError::Transport`（body 仅诊断）；其余（2xx / 其它 4xx）仍走 body 解析。
- 采用评审方推荐的折中：只把 **429 / 5xx** 判连接类，4xx 仍判业务失败。
- 文档同步：CHANGELOG.md、模块级 Failure classes 段
```

---

#### B3 · provider 增加读超时（idle timeout）

- **状态**：已完成 (bd294ff)
- **成本**：约 1 天 · **影响**：中高（长会话卡死）
- **问题**：`providers/core.rs` 只有 `connect_timeout`（10s）。服务端**接受连接后停发
  字节**（半开连接、网关静默挂起）时，`stream()` 会永久等待，一个 turn 卡死。
- **方案**：给 `reqwest::Client` 加 `.read_timeout(...)`（reqwest 0.12 支持；注意它是
  **单次读**的超时，正好适合「SSE 长时间无数据即判死」）。用一个明显大于正常 token 间隔
  的值（如 120s），并让它可配。**不要**用 `.timeout()`（那是整个请求的总超时，会误杀长
  回答）。
- **验收**：新增测试：mock server 接受连接后不发送任何字节 → 断言在 `read_timeout` 后
  收到 `StreamEvent::Error` 而非永久挂起。
- **关联**：M1。

```
- 状态：已完成 (bd294ff)
- 验收证据：
  - cargo fmt --all --check          → exit 0
  - cargo clippy ... -D warnings     → 0 警告（含 --all-targets，新集成测试已编译）
  - cargo test --workspace --locked  → 261 通过 / 0 失败（较上轮 +1）
  - cargo doc ...                    → 0 告警
  - 新增测试：a_stalled_stream_ends_with_an_error_instead_of_hanging
    （tests/stream_timeout.rs，端到端：loopback server 发 200 头后停发 body）
- 实现：`core.rs` 抽出 `build_client(read_timeout)`，`.connect_timeout(10s)` +
  `.read_timeout(READ_TIMEOUT=120s)`，**未**设 `.timeout()`；两个 provider 各加
  `#[must_use] pub fn with_read_timeout(Duration)` 覆盖（供慢端点调大、测试调小）。
- 测试用 `std::net::TcpListener` + `std::thread` 起桩服务器，无需给 tokio 开 `net` 特性
  （护栏：未改任何依赖）。
- 文档同步：CHANGELOG.md、`ProviderCore` 客户端字段与 `READ_TIMEOUT` 文档注释
```

---

#### B4 · 重试策略：尊重 `Retry-After`、加 jitter

- **状态**：已完成 (2823117)
- **成本**：约 0.5 天 · **影响**：中
- **问题**：`providers/http.rs` 的重试忽略 `Retry-After`、无 jitter、把 409 当可重试。
- **方案**：读 `Retry-After`（秒数或 HTTP-date）作为退避下限；退避加**随机 jitter**
  （避免多实例同步捶打）；从可重试集合里移除 409。
- **验收**：新增单测覆盖 `Retry-After` 解析（秒 + date 两种）、jitter 落在区间内、
  409 不再重试。
- **关联**：M2。

```
- 状态：已完成 (2823117)
- 验收证据：
  - cargo fmt --all --check          → exit 0
  - cargo clippy ... -D warnings     → 0 警告
  - cargo test --workspace --locked  → 264 通过 / 0 失败（较上轮 +3）
  - cargo doc ...                    → 0 告警
  - 新增测试：retry_after_accepts_seconds_and_a_date、jitter_stays_within_the_backoff_range、
    retry_delay_honours_a_hint_and_caps_it；retryable_status 测试补入 409（不重试）
- 实现：
  - `is_retryable_status` → `matches!(status.as_u16(), 408 | 429 | 500..=599)`（移除 409）。
  - `retry_after(headers)` → `parse_retry_after`：纯秒数 / IMF-fixdate 两种；过期日期返回 None。
  - `retry_delay(attempt, hint, seed)`：指数退避 + equal jitter（`[base/2, base]`），
    `hint` 取 `max` 作下限，`MAX_BACKOFF = 60s` 封顶；纯函数，可无睡眠单测。
  - `jitter_seed()` 用 `subsec_nanos() ^ 计数器`，同纳秒并发重试也不同。
- 采纳评审方两条预审建议：①`parse_http_date` 文档点明「只支持 IMF-fixdate」；
  ②jitter 种子混入计数器。手写日期解析而非引入 `httpdate`/`chrono`，符合护栏 #1。
- 文档同步：CHANGELOG.md
```

> **评审方核实（2026-11-19 18:49，已提交状态 `2823117`）——✅ 通过，且两条观察全部采纳**
>
> 提交版本与 18:19 的预审一致，三项都做到：
> - `retryable_status` 为 `matches!(status.as_u16(), 408 | 429 | 500..=599)`——**409 已移除**，
>   并补入 408（Request Timeout），比建议更完整。
> - `Retry-After` 两种格式都处理：纯秒数，以及 IMF-fixdate。
> - 退避加 jitter，落在 `[50%, 100%]` 区间（equal jitter），以 `MAX_BACKOFF = 60s` 封顶；
>   `hint`（`Retry-After`）作为**下限**取 `max`，语义正确。
>
> **预审的两条观察，执行方全部采纳**（难得，值得记录）：
> 1. ✅ `parse_http_date` 的文档现在明确写着「Only the IMF-fixdate form is accepted — the
>    obsolete RFC 850 and asctime forms RFC 9110 also lists are not. Real servers send
>    IMF-fixdate, and this keeps the parser dependency-free.」——正是评审方要的「把已知边界
>    写下来」。
> 2. ✅ `jitter_seed()` 改为 `nanos ^ JITTER_COUNTER.fetch_add(1, Ordering::Relaxed)`——
>    正是建议的「混入一个原子计数器」，消除了同纳秒内并发重试取到相同退避的可能。
>
> 新增 3 个测试（`retry_after_accepts_seconds_and_a_date`、`jitter_stays_within_the_backoff_range`、
> `retry_delay_honours_a_hint_and_caps_it`），已提交状态实测 **267 通过 / 0 失败**。

---

#### B5 · 流内错误与 `finish_reason` 的处理

- **状态**：已完成 (68f7a8a)
- **成本**：约 1 天 · **影响**：中
- **问题**：
  1. 流内出现 `{"error": ...}` 时被忽略（不终止、不报错）；
  2. 缺 `finish_reason` 时默认 `Stop`——被截断的流里**已解析出的 tool call 被静默丢弃**。
- **方案**：流内 `error` 对象 → 置 `failed`（与 H1 在 Anthropic 的修法一致）并终止；
  缺 `finish_reason` 且**已有 tool call** 时按「异常截断」处理（报错或标记），不要默认 `Stop`。
- **验收**：新增测试：流中插入 error 对象 → 断言事件序列以 `Error` 结束；流被截断但已含
  tool call → 断言不会静默当成功。
- **关联**：M3。

```
- 状态：已完成 (68f7a8a)
- 验收证据：
  - cargo fmt --all --check          → exit 0
  - cargo clippy ... -D warnings     → 0 警告
  - cargo test --workspace --locked  → 267 通过 / 0 失败（较上轮 +3）
  - cargo doc ...                    → 0 告警
  - 新增测试：an_error_object_in_the_stream_ends_it、
    a_truncated_stream_with_a_tool_call_is_not_a_silent_success、
    a_stream_with_no_finish_reason_and_no_tool_call_still_stops
- 实现：
  - `OpenAiStreamParser` 增 `model`（用于构造 error 消息）与 `failed` 两个字段。
  - `handle_chunk` 开头：`failed` 时直接返回空；遇 `value.get("error")` → 置 `failed`，
    取 `message`（缺省 `"provider error"`），返回终结 `StreamEvent::Error`。
  - `finish` 开头：`failed` 时返回空（不再补发成功 `Done`，避免「末事件即成功」误读）。
  - `finish` 收尾前：若 `finish_reason` 缺失**且**已含 tool call → 判为异常截断，返回
    `StreamEvent::Error`，不默认 `Stop`（避免静默丢弃已解析的 tool call）。
  - 纯文本流缺 `finish_reason` 仍按 `Stop` 收尾（合法形状，见第三个测试）。
- 与 H1 在 Anthropic 的修法一致（同用 `failed` 短路）；未新增依赖、未新造抽象。
- 文档同步：CHANGELOG.md
```

> **评审方核实（2026-11-19 18:49，已提交状态 `68f7a8a`）——✅ 通过，本轮质量最高的一修**
>
> 逐条核实，三点都到位：
> - `failed: bool` 短路：`handle_chunk` 遇 `error` 置位并返回终结 `Error`；此后 `handle_chunk`
>   与 `finish` 都返回空——**不再补发成功 `Done`**。这与 H1 在 Anthropic 的修法**同构**，
>   是正确的一致性：同一类根因（「末事件即成功」）在两家 provider 上用同一模式消除。
> - 截断检测：`finish_reason.is_none() && has_tool_call` → `Error`，不默认 `Stop`。
> - **第三个测试是关键**：`a_stream_with_no_finish_reason_and_no_tool_call_still_stops`
>   验证「纯文本流缺 `finish_reason` 仍按 `Stop` 正常收尾」。这条**反向测试**防止了过度
>   修正——如果没有它，一个合法的、只是没带 `finish_reason` 的纯文本流会被误判为截断而
>   报错。**能想到补这条边界测试，说明执行方理解了修复的适用范围，而不是「见缺字段就报错」。**
>
> 评审方注：B5 是本轮（也是 B 组）质量最高的一修——根因定位准、修法与既有模式一致、
> 边界测试完整。已提交状态实测 **267 通过 / 0 失败**。

---

### C 组 · 需要项目负责人决策后再做

---

#### C1 · 信任边界：给项目配置加 opt-in

- **状态**：**已决策 → 方案 (c)「只读命令不启动进程」**（2026-11-23，项目负责人）
- **执行要点见 [§1.5 · 决策一](ACTIONS.md)**——含三个命令的具体行为调整、
  `tools doctor` 的「未检查」语义（**不计为问题**，否则会与 D4 的退出码逻辑打架）、
  以及「程序未被执行」的验收方式。
- **不在本次范围**：选项 (b) `--trust-project`，留到有真实需求时再议。
- **问题**：`rsagent.toml` 可声明 `local` / `mcp-stdio` / `skill` 来源，它们在**装配期、
  任何审批之前**就被启动。`git clone && rsagent --list-tools` 会执行仓库声明的任意命令。
  README/STABILITY 已把这条写成警告，但**没有任何机制**阻止它。

---

#### C2 · 记忆蒸馏的限频

- **状态**：**已决策 → 方案 (b)「每 5 轮最多一条」**（2026-11-23，项目负责人）
- **执行要点见 [§1.5 · 决策二](ACTIONS.md)**——含计数器位置、
  「不改 `distill_messages` 与 `Memory` 存储约定（D16）、只加频率门」的边界，
  以及「连续 6 次裁剪只多 1 条笔记」的验收方式。
- **问题**：每次上下文裁剪都追加一条 `[compacted]` 笔记；该笔记下一轮进入 system prompt、
  **增大**预算占用，可能形成「裁剪 → 写笔记 → prompt 变大 → 再裁剪」的软回路。200 条上限
  能兜底，但回路本身仍在。

---

#### C3 · `docs/REVIEW.md` 的维护归属

- **状态**：**已决策**
- **结论**：`REVIEW.md` 已纳入版本控制（`9127e31`）。**由评审方维护**，执行方不要改写其
  结论段；执行方可在文末追加「复核记录」。

---

## 3. 不要做的事（防失控护栏）

评审方会按这五条检查每一轮改动。**违反任何一条，即使测试全绿也会被退回**：

1. **不要引入新依赖**去解决 A 组问题。A1–A5 全部可用现有代码 + 标准库完成。新增依赖需要
   在 PR 描述里给出「为什么现有依赖做不到」的具体理由（北极星①）。
2. **不要新造抽象**。A1 是给已有 trait 方法补实现，A2 是改一个已有函数——都**不是**新缝。
   如果发现自己在写第 3 个 trait / 第 2 个注册中心，停下来重新想（北极星④）。
3. **不要用 `#[allow]` 压 lint**。用 `#[expect(..., reason = "…")]` 并写明理由。
4. **不要在业务代码里加 `#[cfg]`**。平台差异一律进 `platform.rs`（AGENTS.md）。
5. **不要顺手重构无关代码**。一条任务 = 一个提交 = 一个主题。评审时 `git show` 应该能一眼
   看完。

另外：**文档必须同步**（北极星⑤）。每修一条，`CHANGELOG.md` 记一条 Fixed/Changed；
若用户可见行为变化，`README.md` 同步；若涉及扩展契约，`docs/EXTENSIBILITY.md` 同步。

---

## 4. 每轮的验收模板

执行方做完一条后，把下面这段填好贴在条目末尾：

```
- 状态：已完成 (<commit>)
- 验收证据：
  - cargo fmt --all --check          → exit 0
  - cargo clippy ... -D warnings     → 0 警告
  - cargo test --workspace --locked  → N 通过 / 0 失败（较上轮 +M）
  - cargo doc ...                    → 0 告警
  - 新增测试：<测试名 1>、<测试名 2>
- 文档同步：CHANGELOG.md / README.md / EXTENSIBILITY.md（勾选实际改动的）
```

---

## 5. 评审节奏

- **触发**：评审方按计划任务定期检查 `git rev-parse HEAD`。若与上次评审基线不同，做一次
  全量复评并更新本文件；若相同，只做轻量健康检查（门禁 + 是否有新提交）。
- **基线记录**：本文件每次复评后在 §6 追加一行。
- **⚠️ 基线必须涵盖 CI 与构建配置**（2026-11-20 10:49 修正）：
  **不要只用 `git log -1 -- crates/`**。D4 只改了 `.github/workflows/ci.yml` 与新增
  `deny.toml`，该命令返回的仍是上一个 `crates/` 提交，**会让人以为「无代码改动」而整轮
  跳过 D4**。正确的基线检查：

  ```bash
  git log -1 --format=%h -- crates/ .github/ deny.toml
  ```

  **理由**：CI workflow 是「项目怎么被验证」的定义，`deny.toml` 是依赖安全策略——它们与
  `crates/` 同等重要，只是恰好不在 `crates/` 下。评审方本轮差点因此漏审 D4。
- **不重复空跑**：若本次 `HEAD` 与**上一次检查时的 HEAD 完全相同**（不只是代码基线相同），
  则上一次的测试结果对同一提交仍然成立，**不必重跑**——只需确认工作区干净并汇报即可。
  同一提交上反复跑同一套测试不产生任何新信息。（2026-11-19 20:19–21:19 连续三轮
  `HEAD` 均为 `2298a8c`，第 3 次起即适用本条。）
- **停滞提醒**：若连续 **≥3 轮**无代码改动，评审方应在汇报中提示「无代码改动」并列出
  待决策项（§2 的 C 组）与下一阶段建议（§7）。

  > ⚠️ **但不要把「无代码改动」直接说成「项目停滞」——那可能是误读。**
  >
  > 2026-11-19 20:00 项目负责人调整了工作流：**定时任务只负责「检测变动」，不再执行优化**；
  > 真正的优化由执行方**在对话中按需完成**。因此「一段时间没有新提交」是**设计如此**，
  > 不等于执行方卡住。
  >
  > 评审方判断「我的意见是否已被接收」的正确依据不是 git，而是执行方的标记文件
  > **`.workbuddy/review-watch.txt`**（两行 SHA-1，分别为执行方**已处理**的
  > `docs/REVIEW.md` 与 `docs/ACTIONS.md` 哈希）。执行方每完成一轮优化才更新它；
  > 检测任务读它来判断是否有待处理变动。核对方法：
  >
  > ```bash
  > # 与 .workbuddy/review-watch.txt 的两行比对
  > git hash-object docs/REVIEW.md
  > git hash-object docs/ACTIONS.md
  > ```
  >
  > **不一致 = 评审意见尚待执行方处理**（检测任务会持续提醒，不会漏）；**一致 = 已处理**。
  > 2026-11-19 20:19–次日 00:49 连续多轮无代码改动，但当时标记与文件**不一致**，
  > 说明意见在队列中而非被忽略。**先查标记，再下结论。**

---

## 6. 复评历史

| 日期 | 代码基线 commit | 测试数 | 评分 | 主要变化 |
| --- | --- | --- | --- | --- |
| 2026-11-19 | `73fb63b` | 243 | **9.1** | H1–H7 + M16/M17 已修复并核实；建立本清单，提出 A1–A5 / B1–B5 / C1–C3 |
| 2026-11-19 | `e1906b5` | 251 | **9.3** | A1+A2+A3 已完成并核实（`cf67bf8`/`9029cc3`/`e1906b5`）；护栏全过；M5/M10/M14 关闭 |
| 2026-11-19 | `75eb483` | 254 | **9.4** | A4+A5 已完成并核实（`ab178bc`/`75eb483`）；**A 组 5/5 完成**；护栏全过；M12/M13 关闭 |
| 2026-11-19 | `bd294ff` | 261 | **9.6** | B1+B2+B3 已完成并核实（`c427a3b`/`792a768`/`bd294ff`）；**B 组 3/5**；护栏全过；M9/M8/M1 关闭 |
| 2026-11-19 | `68f7a8a` | 267 | **9.8** | B4+B5 已完成并核实（`2823117`/`68f7a8a`）；**🎉 A 组 5/5 + B 组 5/5 全部完成**；工作区干净；M2/M3 关闭 |
| 2026-11-20 | `d7919c2` | 276 | **9.9** | **D1+D2 已完成并核实**（`9506b4a`/`d7919c2`）：审批与退出码首次有 E2E 回归保护；新增 `--faux-tool`；**契约性行为零覆盖的结构性缺口已关闭** |
| 2026-11-20 | `466515d` | 278 | **9.95** | **D7+D8+D9 已完成并核实**（`a326913`/`9b53edb`/`466515d`）：文档契约三处不一致清空；**契约表与 crate root 由双向测试锁定，不再能漂移**；全仓库 markdown 链接检查上线 |
| 2026-11-20 | `3270e5b` | 279 | **9.96** | **D3 已完成并核实**（`3270e5b`）：三个「因错误原因通过」的测试变成真会红的测试；测试与仓库根/开发机环境隔离（附 A/B 证明） |
| 2026-11-20 | `a27c91c` | 279 | **9.97** | **D4 已完成并核实**（`a27c91c`，只改 CI + 新增 `deny.toml`）：六项 CI 加固全部落地；**远端 7/7 job 全绿**；评审方修正了会漏审 CI-only 改动的基线规则 |

> **基线口径**：这里记的是**最后一个改动 `crates/`、`.github/` 或 `deny.toml` 的提交**。
> 查代码基线：`git log -1 --format=%h -- crates/ .github/ deny.toml`。
> **不要只用 `crates/`**——D4 只改了 CI，旧口径会误判为「无代码改动」。
> 评审文档自身的提交（`docs/REVIEW.md`、`docs/ACTIONS.md`）不计入。

### 本轮（`a27c91c`）复评记录

**核实结论：D4 完成，六项 CI 加固全部落地，且经远端验证。**

| 项 | 核实要点 |
| --- | --- |
| `permissions: contents: read` | ✓ workflow 级 |
| actions 按 SHA 固定 | ✓ 全部 `uses:` 为 `@` + 40 位 SHA，**且保留 `# v4`/`# stable`/`# v2.1.3` 版本注释** |
| `timeout-minutes: 30` | ✓ 5 个 job 各一处，**注释直接引用 H4**（「子进程攥着管道」正是它要防的挂死） |
| `concurrency` | ✓ `group` + `cancel-in-progress: true` |
| `cargo-deny` | ✓ 新增 `Audit` job + `deny.toml`（只配 `[advisories]`，**明确不臆断许可证策略**） |
| 真正运行 examples | ✓ 新增 `Examples` job 跑全部 4 个 README 宣传的示例 |

**`deny.toml` 的分寸值得记录**：它只配置 CI 实际检查的 `[advisories]`，并在注释里明确声明
不臆断 `licenses`/`bans`/`sources` 的策略（「a project decision, not something to guess
here」）。**许可证白名单是项目负责人的选择，不是一个 CI 加固任务该顺手定下的**——执行方
正确地没有越界。

**最强证据是远端验证**：`a27c91c` 上 **7/7 job 全绿**（含新增的 `Audit` 与 `Examples`）——
说明新 CI 不只是语法正确，而是真的能跑通。

**⚠️ 评审方本轮差点漏审 D4，已修正规则**：D4 只改 `.github/workflows/ci.yml` 与新增
`deny.toml`，**没有碰 `crates/`**，因此旧的基线口径 `git log -1 -- crates/` 返回的仍是
`3270e5b`——**按旧规则这一整轮会被判为「无代码改动」而跳过**。§5 已改为
`git log -1 -- crates/ .github/ deny.toml`。**CI workflow 是「项目怎么被验证」的定义，
`deny.toml` 是依赖安全策略，二者与 `crates/` 同等重要，只是恰好不在 `crates/` 下。**

**护栏复核**：无新增依赖、无 `#[allow]`、无业务代码平台 `#[cfg]`、无 `unsafe`/`TODO`、
无无关重构。✅

**🏁 里程碑**：**CI 从「只跑四道门禁」变成一道真正的防线**。

### 上一轮（`3270e5b`）复评记录

**核实结论：D3 完成，三项修法都优于建议。**

| 项 | 核实要点 |
| --- | --- |
| 颜色测试 | 把判定抽成**纯函数** `Palette::decide(choice, no_color_env, stdout_is_tty)`，用**真值表**单测钉住（三者各自必要）。**并把子进程测试重命名为 `piped_output_has_no_escapes`——名字现在与实际一致**，原问题不只是空转，还有名字在撒谎。 |
| 测试隔离 | `OnceLock` 缓存空临时目录 + `current_dir` + `env_remove("RSAGENT_USER_CONFIG")`。**附 A/B 实测**证明修复必要：构造会污染测试的 user config，隔离时 61/61 通过、未隔离时只列出 1 个工具。 |
| mcp-stdio | 静默 `return` → `expect` 硬失败；查找方式从固定 `parent().parent()` 改为**逐级向上找同级 `examples/`**——固定深度在 `build/<crate>/<hash>/out/` 下找不到，**那正是它一直静默跳过、没人发现的原因**。 |

**为什么这轮值得记录**：D3 修的是**测试本身的诚实性**。一个「因错误原因通过」的测试比没有
测试更糟——它给人虚假的安全感。执行方不只是让它们变红，而是**先让名字与实际一致**，
再让断言真正有区分度。**这与 D1 的负向对照、D7 的双向检查是同一种素养**：确保「通过」
意味着「正确」，而不是「没测到」。

**护栏复核**：无新增依赖、无 `#[allow]`、无业务代码平台 `#[cfg]`、无 `unsafe`/`TODO`、
无无关重构。新增一处**带 reason** 的 `#[expect]`，符合 AGENTS.md。✅

**🏁 里程碑**：**测试不再有「因错误原因通过」的**。测试数 278 → 279。

### 上一轮（`466515d`）复评记录

**核实结论：D7、D8、D9 三项全部通过；D7 的做法比评审方建议的更强。**

| 条目 | 提交 | 核实要点 |
| --- | --- | --- |
| D8 | `a326913` | 4 处断链按建议改好。**评审方复跑全仓库链接检查：0 处断链。** |
| D9 | `9b53edb` | 采用建议的方案 (a)：改 README 措辞、**不动行为**，并补了一段说明**为什么 abort 退 0 是故意的**——比「只改那一行」更好，它解释了设计意图。 |
| D7 | `466515d` | 正文改为「就是 `pub use` 的那组名字——不多不少」；表格按用途分组**列全 79 项**；新建 `tests/docs.rs` 做**双向**校验。**评审方独立复核：表中 26 + 53 = 79 项，与实际导出完全一致。** |

**D7 为何是本轮亮点**：评审方建议「正文与表格口径一致 + `#[doc(hidden)]` + CI 检查」，
执行方全部做到，并**把检查做成了双向**——「表里多的」和「代码里多的」都报错。
**双向是关键**：单向检查会漏掉「表里写了个不存在的名字」，而且**双向让测试不可能空转**
（任一解析器返回空集，另一方向立刻非空而失败）。所以「测试通过」本身证明了两个集合
**都非空且相等**。执行方还说明它手工验证过测试在两种方向下都会红。

**这一改的意义**：D7 之前，「公开 API 是什么」有**两个答案**；现在**正文、表格、代码三者由
测试锁在一起**，规则从「文档里的君子协定」变成**「CI 会拦你」**——正是本项目
`tool_conformance.rs` 一直在用的思路。

**护栏复核**：无新增依赖、无 `#[allow]`、无业务代码 `#[cfg]`、无 `unsafe`/`TODO`、
无无关重构。`#[doc(hidden)]` 逃生口已建但**当前未被使用**（表已列全）。✅

**🏁 里程碑**：**契约文档本身不再能悄悄漂移**。D7 让「表 = 契约」可执行，D8 让 markdown
断链永久免疫（两者都在同一个 `tests/docs.rs` 里）。测试数 276 → 278。

### 上一轮（`d7919c2`）复评记录

**核实结论：D1、D2 完成，测试质量高于要求；并发现一个由执行方主动指出的契约不符（D9）。**

| 条目 | 提交 | 核实要点 |
| --- | --- | --- |
| D1 | `9506b4a` | 5 个新测试与我建议的用例一一对应（第 5 条由既有测试覆盖，**未重复添加**）。为解锁测试新增 `--faux-tool`（公开 CLI 标志，已同步 README + CHANGELOG，实现是一个参数 + 两个纯函数，符合护栏 #1/#2）。 |
| D2 | `d7919c2` | 4 个新测试覆盖退出码 0/1/2 与「`--json` 不改退出码、只改呈现」这条此前无人验证的承诺。**执行方主动核实了 `abort` 的行为并留作观察**，未为凑测试而改行为。 |

**D1 里质量最高的测试是 `allow_network_permits_a_web_tool`**，因为它带了**负向对照**：
先断言 `--allow network` 放行，**再用不带类别的同一调用断言被拒**，并在注释里写明「proving
the assertion above is sensitive to the category rather than passing for another reason」。
**这正是让测试从「跑得过」变成「有意义」的关键**——一个恒真的断言也能通过测试。这条确实
锁住了 A1：若 `permission()` 退回 `Exec`，它必红。

**执行方在 D2 里做对的一件小事值得记录**：它发现 `abort` 退 0 与 README 措辞有出入后，
**没有先写一个「abort 应退 1」的测试**，而是先回读 `agent.rs` 确认语义、发现需要新依赖
（护栏禁止）后放弃加测试并留作观察。**如果反过来先写测试，就会把一个未定行为固化成「规格」。**
评审方已把那个观察**核实并提升为独立条目 D9**，以免随 D2 完成被埋掉。

**护栏复核**：无新增依赖、无 `#[allow]`、无业务代码 `#[cfg]`、无 `unsafe`/`TODO`、
无无关重构。✅

**🏁 里程碑**：**契约性行为首次有了端到端回归保护**。此前 README 自述「审批是唯一防线」、
退出码被 STABILITY 列为 Semver 契约，却**零 E2E 覆盖**——这是评审方此前指出的**唯一结构性
缺口**，现已关闭。测试数 267 → 276。

### 上一轮（`68f7a8a`）复评记录

**核实结论：B4、B5 全部完成。B5 是 B 组质量最高的一修。B 组 5/5 收尾。**

| 条目 | 提交 | 核实要点 |
| --- | --- | --- |
| B4 | `2823117` | `retryable_status` = `408 \| 429 \| 500..=599`（409 移除，408 补入）；`Retry-After` 支持秒数与 IMF-fixdate；equal jitter 落在 `[50%,100%]`，`hint` 取下限，`MAX_BACKOFF = 60s` 封顶。**评审方预审的两条观察全部采纳**（注释写明只支持 IMF-fixdate；`jitter_seed` 混入原子计数器）。 |
| B5 | `68f7a8a` | `failed: bool` 短路（`handle_chunk`/`finish` 遇错后都返回空，不再补发成功 `Done`）——**与 H1 在 Anthropic 的修法同构**；截断检测 `finish_reason.is_none() && has_tool_call` → `Error`；**并补了反向测试**「纯文本流缺 `finish_reason` 仍正常 `Stop`」。 |

**B5 为何是本轮最佳**：三点同时做到——① 根因定位准（不是「见缺字段就报错」，而是区分
「有 tool call 的截断」与「无 tool call 的合法纯文本流」）；② 修法与既有模式一致（复用 H1
的 `failed` 短路，同一根因在两家 provider 用同一模式消除）；③ **边界测试完整**，第三个
测试专门防过度修正。这说明执行方理解了修复的**适用范围**，而不只是让报错消失。

**B4 为何值得记录**：评审方给的两条**非阻塞**观察，执行方**两条都采纳**了。通常这类
「顺手建议」会被忽略；全部落地说明执行方在认真对待评审意见，而不是只做「必须做的部分」。

**护栏复核**：无新增依赖、无 `#[allow]`、无业务代码 `#[cfg]`、无 `unsafe`/`TODO`、无无关重构。✅

**🏁 里程碑**：H1、B1、B2、B3、B5 这五条「静默给错结果 / 永久卡住」类缺陷**已全部关闭**。
这是本项目最危险的一类问题，到本轮为止**已清空**。

### 上一轮（`bd294ff`）复评记录

**核实结论：B1、B2、B3 全部完成，三项都**恰好**按建议落地；B1 的两条建议一条被采纳、
一条被有理有据地拒绝。**

| 条目 | 提交 | 核实要点 |
| --- | --- | --- |
| B1 | `c427a3b` | `MAX_TOOL_PAGES = 100` 封顶分页；`request()`/`find_event_response()` **跳过**带 `method` 的消息而非报错（正确：真响应可能在后）；`read_result()` 对裸 body 的请求报错并**注释写明归 Transport 的理由**。4 个测试对准原问题。 |
| B2 | `792a768` | `429 \|\| 5xx` → `CallError::Transport`（喂熔断器），4xx 仍走 body 保持业务失败——**正是评审方建议的折中**。测试用 `threshold: 1` 断言熔断器真的打开。 |
| B3 | `bd294ff` | 新增 `READ_TIMEOUT = 120s`，客户端加 `.read_timeout(...)`，并**明确保留「不设整请求超时」**（避免误杀长回答）——正是建议里强调的关键点。新增 `with_read_timeout` 供测试，测试文件 `stream_timeout.rs` 断言「停发的流以错误结束而非挂死」。 |

**B1 的两条建议**：①「注释写明 `Transport` 而非 `Protocol` 的理由」→ **已采纳**；
②「给 server→client 请求加日志」→ **未采纳，理由充分**：MCP 传输层当前没有日志通道，
为一条合规服务器不会触发的路径新引入通道违背北极星①④。**评审方接受**——这是建议被
有理有据地拒绝的正面例子，说明执行方在按北极星判断而非机械照做。

**B2 值得表扬的一点**：它没有把「非 2xx 一律判连接类」（评审方给的粗方案），而是精确到
`429 | 5xx`，保留了「4xx 由请求本身引起 → 仍是业务失败」的区分。这比建议更细。

**护栏复核**：无新增依赖、无 `#[allow]`、无业务代码 `#[cfg]`、无 `unsafe`/`TODO`、无无关重构。✅

### 上一轮（`75eb483`）复评记录

**核实结论：A4、A5 全部完成，修法与建议一致。A 组 5/5 收尾。**

| 条目 | 提交 | 核实要点 |
| --- | --- | --- |
| A4 | `ab178bc` | `render.rs` 新增 `write_thinking`：`OutputMode::OneShot` 时走 `eprint!`，其余走 `print!`。**额外做对了两件事**：① 新增 `thinking_open` 状态，让换行在**开启它的那条流**上收尾（否则 stderr 的推理行会被 stdout 的 `println!` 错误闭合）；② 让 `--faux` 在 `--thinking` 下真的产出一个 reasoning 块，使离线示例**真的走到**该路径，测试才有意义。 |
| A5 | `75eb483` | `report()` 改为返回 `bool`（是否健康）；`problems` 汇总**三类**：配置读取/解析告警、来源加载失败、`Unhealthy` 来源；任一 >0 即 `doctor_result` 报错退非零。**`#[non_exhaustive]` 的未知健康值被计为「问题」而非「通过」**——保守且正确。 |

**A4 的亮点**：这个修复的难点不在「把 `print!` 换成 `eprint!`」，而在**换行归属**——推理
输出与答案输出共用一个渲染器，谁开的行谁闭合。执行方用 `thinking_open` 显式跟踪，说明是
想清楚了而不是碰巧对的。同时把 faux provider 补上 reasoning 块，让测试能真正断言 stdout/stderr
的分工，而不是只断言「函数被调用」。

**A5 的亮点**：把「配置解析失败」也计入问题。修之前，一个拼错的 `rsagent.toml` 会被静默
忽略、落回内置来源、`doctor` 绿灯——**文档承诺的 CI gate 在最需要它的场景下失效**。现在
配置错误会红。未知健康值判为失败同样是正确的保守选择。

**护栏复核**：无新增依赖、无 `#[allow]`、无业务代码 `#[cfg]`、无 `unsafe`/`TODO`、无无关重构。✅

### 评审期间的并发改动（流程观察）

本轮评审进行中，执行方**并发开始了 B1**（未提交的 `mcp_http.rs`/`mcp_stdio.rs` 改动）。
后果：评审方的一次门禁读数**混入 WIP**，得到 256 而非已提交状态的 254。

- **评审方处理**：以**已提交基线** `75eb483` 为准复核，未触碰执行方的工作区。
- **WIP 内容（只读观察）**：正是 B1 的两项——按 `method` 字段区分 server→client 请求与响应
  （避免把请求当响应读出 `null` 空成功），以及 `MAX_TOOL_PAGES = 100` 分页上限。方向正确。
- **建议**：评审期间尽量不留长时间未提交的改动；若要边做边存，先提交 WIP 或使用分支。
  评审方今后会在报告里同时记录 HEAD 与工作区是否干净，以便区分。

### 上一轮（`e1906b5`）复评记录

**核实结论：A1、A2、A3 全部完成，修法与建议一致，质量高。**

| 条目 | 提交 | 核实要点 |
| --- | --- | --- |
| A1 | `cf67bf8` | 5 个来源全部显式声明：`web`/`mcp_http`/`mcp_stdio` → `Network`，`local`/`skill` → `Exec`。新增 5 个测试逐一断言类别。**恰好是我建议的映射。** |
| A2 | `9029cc3` | `record_success` 改为按状态分派（`Closed`/`HalfOpen` → 关闭；`Open` → **忽略**晚到的成功）；新增 `probe_started_at` 字段；`allows()` 与 `effective()` **都**处理了废弃探测的再武装。新增 2 个测试。 |
| A3 | `e1906b5` | `read_file` 改用 `split_inclusive('\n')` 且只剥 `\n`，保留 `\r`；新增 `read_file_output_edits_a_crlf_file` 端到端测试（读 CRLF → 贴回 `edit_file` → 断言成功）。 |

**A2 的额外优点**：探测 deadline **复用 `cooldown`** 而非新增 `BreakerPolicy` 字段——既解决了
问题又没扩大公开 API，符合北极星①④。评审方确认 `effective()`（`state()`/`health()` 的数据源）
也做了同样处理，因此**健康显示与实际行为一致**，不会出现「显示半开、实际已可探测」的错位。

**评审方低优先级观察（非缺陷，不要求修）**：探测 deadline 等于 `cooldown`（默认 60s），
因此一个**合法但慢于 60s** 的探测可能被判为废弃并再武装。工具调用通常自带超时且远短于
60s，实际影响可忽略；若将来出现长探测场景，可考虑独立的 `probe_timeout`。

---

## 7. 阶段 2 · 回归保护（建议下一阶段）

> **为什么是现在**：A 组 5/5、B 组 5/5 已完成，「会静默给错结果」与「会永久卡住」两类
> 缺陷已清空（评分 9.8）。**代码可靠性维度已接近满分**，当前唯一的结构性缺口是
> **契约性行为没有回归保护**——README 自述「bash 未沙箱化，审批是唯一防线」，而
> [STABILITY.md](../STABILITY.md) 把「CLI flags and exit codes」列为 Semver 契约。
>
> **本节不改变已完成的 A/B 组结论**，只是把下一阶段拆成可直接执行的任务。

### D1 · 审批行为的 E2E 覆盖（最高优先）

- **状态**：已完成 (9506b4a)

**现状**：`crates/rsagent/tests/cli.rs` 已有 52 个测试，但**没有一个**覆盖审批。
审批是「bash 不沙箱化」这一安全模型下的**唯一防线**，却没有回归保护。

**已核实的行为契约**（写测试前先确认这些就是「正确」）：

| 场景 | 期望行为 | 依据 |
| --- | --- | --- |
| `--print` + 变更类工具，无 `--yes` | **拒绝**（不询问），并提示 `pass --yes or --allow <category>` | `main.rs:604-605`「one-shot `--print` denies them unless `--yes` is given, so automation is never surprised」 |
| 非交互 stdin + 无 `--yes` | **拒绝一切** | `main.rs:1123`「Deny everything (non-interactive without `--yes`)」 |
| `--yes` | 全部放行 | `main.rs:127` |
| `--allow write` | 只放行 `write`，其余仍拒绝/询问 | `main.rs:1131-1142` |
| `--allow network` | 放行 `network`（**修 A1 后** `web`/`mcp-http` 属此类） | A1 `cf67bf8` |
| 交互输入 `y` / `yes` | 放行**本次** | `main.rs:1185` |
| 交互输入 `a` | 放行本次并**记住**（后续不再问） | `main.rs:1186-1187` |
| 交互输入其他/回车 | 拒绝 | `main.rs:1185` 的匹配之外 |
| `--allow <未知类别>` | `exit 2`，并列出合法类别 | `main.rs:1127` |
| 被拒绝的工具 | 以 `ToolExecutionEnd { is_error: true }` 形式回到模型，**不中断 turn** | 需读 `agent.rs` 确认 |

**建议的测试用例**（每条一个 `#[test]`，用 `--faux` 驱动、`--print` 收尾）：

1. `print_without_yes_denies_a_write` —— 让 faux 请求 `write_file`，断言：文件**未**被创建，
   且 stdout/stderr 含 `skipped`/`approval required`。
2. `print_with_yes_runs_a_write` —— 同上但加 `--yes`，断言文件**被**创建。
3. `allow_write_permits_write_but_not_bash` —— `--allow write`，faux 先请求 `bash` 再请求
   `write_file`；断言 `bash` 被拒、`write_file` 成功。
4. `allow_network_permits_a_web_tool` —— 这条**专门锁住 A1 的修复**：`--allow network`
   必须放行 `web` 来源的工具。若有人把 `permission()` 改回 `Exec`，这条会红。
5. `an_unknown_allow_category_exits_2` —— 断言退出码 `2` 且消息含合法类别列表。
6. `interactive_a_remembers_the_decision` —— 用 `stdin` 管道喂 `a\n`，断言后续同类调用
   不再产生 `ToolApprovalRequest` 事件（用 `--json` + `--log` 断言事件流更可靠）。

**注意**：`--print` 下审批**不询问**而是直接拒绝，所以用例 1/3/4 不需要喂 stdin；
用例 6 需要交互，建议用 `--json` 断言事件流而不是解析终端文本。

```
- 状态：已完成 (9506b4a)
- 验收证据：
  - cargo fmt --all --check          → exit 0
  - cargo clippy ... -D warnings     → 0 警告
  - cargo test --workspace --locked  → 272 通过 / 0 失败（较上轮 +5）
  - cargo doc ...                    → 0 告警
  - 新增测试（crates/rsagent/tests/cli.rs）：
    print_without_yes_denies_a_write、print_with_yes_runs_a_write、
    allow_write_permits_write_but_not_bash、allow_network_permits_a_web_tool、
    interactive_a_remembers_the_decision
- 前置：`--faux` 原先只能回纯文本，**无法从 CLI 驱动工具调用**，审批路径因此测不了。
  为解锁本任务，新增 `--faux-tool <SPEC>`（可重复，`SPEC` = `name` 或 `name:JSON`）：
  每条脚本化一次工具调用，末尾补一条文本回复收尾。`--print` 与 REPL 两种模式都支持
  （REPL 里脚本在首个 prompt 回放一次，队列空了再退回文本）。
- 覆盖情况对照 D1 表：
  - `--print` 无 `--yes` → 拒绝（`skipped write_file: approval required`），文件未落盘；
  - `--yes` → 放行并落盘；
  - `--allow write` → 放行 write、仍拒 bash（exec）；
  - `--allow network` → 放行 web 工具（**锁 A1**：去掉该类别即被拒，证明断言对类别敏感）；
  - 交互输入 `a` → 放行并记住（`--log` 事件流里 `toolApprovalRequest` **恰好 1 条**）。
  - D1 表第 5 条（未知类别 exit 2）**已由既有测试 `an_unknown_permission_category_is_rejected`
    覆盖**，未重复添加。
- 文档同步：README（`--faux-tool` 用法与 flags 表）、CHANGELOG（Added）。
- 说明：本任务的核心是补测试；`--faux-tool` 是为「审批可被离线驱动」而加的最小机制，
  非新抽象（一个 CLI 参数 + 两个纯函数），符合护栏 #1/#2。
```

> **评审方核实（2026-11-20 09:19，已提交状态 `9506b4a`）——✅ 通过，且测试质量高于要求**
>
> 逐条核实：5 个新测试与我建议的用例**一一对应**（1、2、3、4、6），第 5 条如执行方所述
> 已由既有测试覆盖——**没有为了凑数重复添加，这个判断是对的**。
>
> **`allow_network_permits_a_web_tool` 是本轮质量最高的测试**，因为它带了**负向对照**：
> 先用 `--allow network` 断言放行，**再用不带类别的同一调用断言被拒**，并在注释里写明
> 「proving the assertion above is sensitive to the category rather than passing for another
> reason」。**这正是让测试从「跑得过」变成「有意义」的关键**——否则一个恒真的断言也能
> 通过。这条测试确实锁住了 A1：若 `permission()` 退回 `Exec`，它必红。
>
> **`--faux-tool` 的引入经过评审方审查**：它是**新增公开 CLI 标志**（STABILITY 把 CLI
> flags 列为 Semver），因此需要文档同步——执行方已同步 **README（CLI 表 + 用法示例）**与
> **CHANGELOG（Added）**。实现是一个 clap 参数 + 两个纯函数，`requires = "faux"` 约束得当，
> 非法 JSON 退 2 与 `--allow`/`--tools` 的既有约定一致。**评审方认为这是为解锁测试而加的
> 最小机制，不是新抽象，符合护栏 #1/#2。**
>
> 已提交状态实测 **276 通过 / 0 失败**。

### D2 · 退出码的 E2E 覆盖

- **状态**：已完成 (d7919c2)

**契约**（README「Exit codes」节，STABILITY 列为 Semver）：

| 码 | 含义 | 建议用例 |
| --- | --- | --- |
| `0` | 运行完成且模型给出了答案 | `faux_print_runs_a_turn_end_to_end`（**已存在**） |
| `1` | 运行时的模型级失败（网络/提供商错误）**或**触到 turn 上限 | `a_model_error_exits_1`（faux 返回错误）；`hitting_max_turns_exits_1`（`--max-turns 1` + 需要工具的 prompt） |
| `2` | 运行**之前**的用法/配置错误 | `missing_api_key_exits_2`（不带 `--faux` 且无 key）；`a_bad_log_path_exits_2`；`unknown_flag_exits_2`（clap 自带，可省） |

**额外值得锁的两条**（评审方在 §6 记过、但未列为缺陷）：

- `abort` 在 `--print` 下的退出码：当前行为需先确认，**确认后再写测试锁住**（不要先写测试
  去「修正」一个未定行为）。
- `--json` 模式下失败**不改变退出码语义**，而是以 `error` 事件呈现——README 明确承诺
  「`--json` keeps its machine contract」，应有一条测试断言这一点。

```
- 状态：已完成 (d7919c2)
- 验收证据：
  - cargo fmt --all --check          → exit 0
  - cargo clippy ... -D warnings     → 0 警告
  - cargo test --workspace --locked  → 276 通过 / 0 失败（较上轮 +4）
  - cargo doc ...                    → 0 告警
  - 新增测试（crates/rsagent/tests/cli.rs）：
    a_model_error_exits_1、hitting_max_turns_exits_1、a_bad_log_path_exits_2、
    json_keeps_its_exit_code_on_failure
- 覆盖情况对照 D2 表：
  - `1` 模型级失败：`--print hi --base-url http://127.0.0.1:1/v1`（无 key）→ 连接被拒 → 退 1；
  - `1` 触到 turn 上限：`--max-turns 1` + 一次工具调用 → `MaxTurnsExceeded` → 退 1；
  - `2` `--log` 路径不可用 → 在启动任何来源前 `exit 2`（`could not open log`）；
  - `2` 缺 key 已由既有 `a_print_run_without_an_api_key_exits_nonzero` 覆盖（断言退 2）；
  - `--json` 失败：退 **1**（与 `--print` 一致），并在 stdout 出现 `"stopReason":"error"`
    事件——锁住「机器契约不变、只改呈现方式」。
- `abort` 在 `--print` 下的退出码（D2「额外值得锁」第 1 条）：**已核实**——SIGINT 走
  `agent.rs:400`（`StreamOutcome::Cancelled`）与 `:422`（`StopReason::Aborted`），两处都
  `return Ok(())`，即「abort 不是错误」，故进程退 **0**。**未加测试**：非交互场景下跨平台
  向子进程发 SIGINT 需要新依赖（护栏 #1 禁止），且评审方明确要求「确认后再写、不要先写
  测试去修正未定行为」。**留作观察**：这与 README「`0` = 产出了答案」的措辞略有出入。
```

> **评审方核实（2026-11-20 09:19，已提交状态 `d7919c2`）——✅ 通过**
>
> 4 个新测试与我建议的用例对应（`1` 模型失败、`1` turn 上限、`2` 坏 `--log` 路径、
> `--json` 机器契约），缺 key 一条由既有测试覆盖——**同样没有重复添加**。
>
> **`json_keeps_its_exit_code_on_failure` 尤其有价值**：它锁的是「`--json` **不改**退出码
> 语义，只改呈现方式（stdout 出现 `"stopReason":"error"` 事件）」。这条承诺写在 README 里
> 但此前无人验证，且**很容易在重构中被无意破坏**（把 `--json` 的失败路径单独处理）。
>
> **执行方对 `abort` 的处理值得表扬**：它**没有**为了让测试好写就去改行为，而是先回读
> `agent.rs` 确认实际语义、发现需要新依赖后**放弃加测试并留作观察**，还主动指出这与
> README 措辞有出入。**这恰好遵守了我在 D2 里写的「先确认行为、再写测试」**——如果它反过来
> 先写一个「abort 应退 1」的测试，就会把一个未定行为固化成「规格」。
>
> 那个「措辞有出入」的观察**评审方已核实并提升为独立条目 D9**（`agent.rs:400`/`:422`
> 确认两处都 `return Ok(())`），以免它随 D2 的完成而被埋掉。
>
> 已提交状态实测 **276 通过 / 0 失败**。

### D3 · 修掉空转与不隔离的测试（低成本，可顺手做）

- **状态**：已完成 (3270e5b) · **成本**：约 1 小时

- `no_color_output_has_no_escapes`：在 `Command::output()` 下 stdout 本就不是 TTY，
  `Palette` 早已关闭，**删掉 `--no-color` 也会通过**。应改为直接断言 `NO_COLOR` 环境变量
  的行为，或构造一个伪 TTY。
- `rsagent()` 构造器（`tests/cli.rs:9-11`）**不设 `current_dir`、不清 `RSAGENT_USER_CONFIG`**，
  多个测试会从仓库根加载 `.env`；开发机上一旦有 user config，`tools list` 断言就会红。
  建议：构造器统一 `current_dir` 到临时目录，并显式 `env_remove("RSAGENT_USER_CONFIG")`。
- `mcp-stdio` 示例缺失时测试**静默 `return`**，仍算通过。应显式 skip（如打印原因并
  `return` 前断言文件存在）或先构建 examples。

```
- 状态：已完成 (3270e5b)
- 三处逐条修法：
  1. 颜色测试：把判定抽成可测的 `Palette::decide(choice, no_color_env, stdout_is_tty)`
     （`detect` 只负责采三个输入并转调），新增单元测试
     `color_needs_auto_no_no_color_and_a_tty` 钉住真值表——只有 `Auto` + 无 `NO_COLOR`
     + TTY 才着色，且三者各自必要。子进程测试改名
     `piped_output_has_no_escapes`，明确它钉的是「重定向输出保持干净」，TTY 相关用例
     归单元测试（子进程测试无法呈现 TTY）。
  2. 隔离：`rsagent()` 统一 `current_dir` 到一个按进程缓存的空临时目录（`OnceLock`，
     避免每次调用都新建目录），并 `env_remove("RSAGENT_USER_CONFIG")`。仓库根的
     `.env`/`rsagent.toml` 与开发机 user config 都进不了测试。
  3. mcp-stdio：`mcp_echo_program()` 改为从 `current_exe()` 逐级向上找同级的
     `examples/`（原先固定 `parent().parent()`，在 `build/<crate>/<hash>/out/` 下找不到），
     并把「找不到」从静默 `return` 改成 `expect` 硬失败。`cargo test` 默认会构建
     examples，故普通测试不会误触。
- 验收证据：
  - cargo fmt --all --check          → exit 0
  - cargo clippy ... -D warnings     → 0 警告
  - cargo test --workspace --locked  → 279 通过 / 0 失败（较上轮 +1：新增颜色真值表单元测试）
  - cargo doc ...                    → 0 告警
  - **隔离 A/B 实测**：构造一个把内置集收窄到只剩 `read_file` 的 user config，
    设 `RSAGENT_USER_CONFIG` 后跑 `-p rsagent --test cli` → **61/61 通过**（隔离生效）；
    同一个 config 直接 `cargo run -- tools list`（未隔离）→ **只列出 1 个工具**，
    证明该 config 确实会污染测试（无 `env_remove` 时相关断言会红）。
```

> **评审方核实（2026-11-20 10:19，已提交状态 `3270e5b`）——✅ 通过，三项修法都优于建议**
>
> **1. 颜色测试——修法比建议更好。** 我建议「直接断言 `NO_COLOR` 行为，或构造伪 TTY」；
> 执行方选了更干净的路：把判定抽成**纯函数** `Palette::decide(choice, no_color_env,
> stdout_is_tty)`（`detect` 只负责采三个输入），再用**真值表**单测钉住——只有
> `Auto` + 无 `NO_COLOR` + TTY 才着色，**且三者各自必要**。
>
> **关键的一点**：它没有简单删掉那个子进程测试，而是**重命名为 `piped_output_has_no_escapes`**
> ——**名字现在与它实际测的东西一致**（它测的是「重定向输出保持干净」，从来不是 `--no-color`）。
> 原测试的问题不只是「空转」，还有**名字在撒谎**；重命名同时修掉了这两点，并在注释里写明
> TTY 相关用例去了哪里。这比删掉它更有价值。
>
> **2. 隔离——完全按建议，且带了 A/B 证明。** `default_cwd()` 用 `OnceLock` 按进程缓存一个
> 空临时目录（避免每个测试都新建），`rsagent()` 统一 `current_dir` 并 `env_remove`。
> 更值得记录的是它做了**隔离 A/B 实测**：构造一个会把内置集收窄到只剩 `read_file` 的 user
> config，证明「设了 `RSAGENT_USER_CONFIG` 时 61/61 仍通过」而「未隔离时只列出 1 个工具」
> ——**这证明修复是必要的，而不只是存在**。与 D1 的负向对照是同一种测试素养。
>
> **3. mcp-stdio——完全按建议，且更健壮。** 除了把静默 `return` 改成 `expect` 硬失败，
> 还把查找方式从固定 `parent().parent()` 改为**逐级向上找同级 `examples/`**——原先的固定深度
> 在 `build/<crate>/<hash>/out/` 布局下找不到，那正是它一直静默跳过、没人发现的原因。
>
> **护栏**：新增一处 `#[expect(clippy::fn_params_excessive_bools, reason = "…")]`——
> **带 reason，符合 AGENTS.md**（禁止用 `#[allow]` 压 lint）。无新依赖、无 `unsafe`。
>
> 已提交状态实测 **279 通过 / 0 失败**。

### D4 · CI 加固（低成本）

> **2026-11-20 06:19 评审方首次实读 `.github/workflows/ci.yml`（70 行）核实本节的建议。**
> 结论：下面三项**确实都还没有**，建议成立；另发现**两个新的加固点**（4、5）。
> 同时确认 CI 与 AGENTS.md 的四道门禁**完全对齐**（`check` 三平台跑 fmt/clippy/test，
> `docs` 跑 `-D warnings` 的 rustdoc，`msrv` 在 1.95.0 上 `cargo check`），这部分无需改动。
>
> **2026-11-20 07:19 补充**：实测四个被 README 宣传为「可运行」的 example，**全部 `exit 0`
> 且输出合理**——承诺成立。但 CI 只编译不执行它们，故新增第 6 条。

- **状态**：已完成 (a27c91c) · **成本**：约 0.5 天
- **已有且正确、不要动的部分**：三平台矩阵（`fail-fast: false`）、`--locked` 全程、
  `RUSTDOCFLAGS: -D warnings`、独立的 MSRV job（1.95.0 固定）。

1. **加 `permissions: contents: read`**（workflow 级）。当前**没有** `permissions` 块，
   因此 `GITHUB_TOKEN` 继承仓库默认权限——若默认是 read-write，一个被投毒的依赖构建脚本
   就多了一分写权限。CI 只需要读代码。
2. **actions 按 commit SHA 固定**。当前全部用浮动 tag：`actions/checkout@v4`、
   `dtolnay/rust-toolchain@stable`、`dtolnay/rust-toolchain@1.95.0`、`Swatinem/rust-cache@v2`。
   上游 tag 被移动或账号被接管时会静默拉到别的代码。改为 `@<sha> # vX.Y.Z` 并保留注释。
3. **加 `cargo-deny` / `cargo-audit` job**。当前没有任何依赖审计——`Cargo.lock` 里
   `reqwest`/`rustls`/`tokio` 这条链一旦出 RUSTSEC 公告，项目不会知道。
4. **🆕 给每个 job 加 `timeout-minutes`**（建议 30）。当前**没有超时**，job 挂住会一直跑到
   GitHub 的 6 小时上限。**这一条与本项目的历史直接相关**：H4 修的正是「子进程攥着管道
   导致 turn 永久挂死」——在那之前，一个这样的测试就足以让 CI 烧掉 6 小时的额度，
   而且表现为「CI 一直转圈」而非「测试失败」，很难排查。
5. **🆕 加 `concurrency` 组**（`group: ${{ github.workflow }}-${{ github.ref }}` +
   `cancel-in-progress: true`）。当前连推两次 main 会并行跑两套三平台矩阵，前一套的结果
   已无意义却仍占额度。
6. **🆕 在 CI 里真正运行 `examples/`**。README 把 4 个 example 宣传为「可直接运行」的
   入口（`headless`、`custom_tool`、`custom_provider`、`end_to_end`），但 CI **只编译
   它们**（`clippy --all-targets`），**从不执行**——运行期回归不会被发现。
   **评审方 2026-11-20 07:19 实测：四个 example 当前全部 `exit 0` 且输出合理**
   （`answer: Hello from a headless agent!` / `word_count -> 4` / `echo: pong` /
   `end_to_end` 完整事件流），所以**承诺现在成立**；本条是为了让它**继续**成立。
   加一个 job 跑这四条命令即可（离线、无需 API key、几秒钟）。

**验收**：改完后 `git push` 一次，确认 5 个 job 仍全绿（`ubuntu` / `macos` / `windows` /
`Docs` / `MSRV`），且 workflow 文件里能搜到 `permissions:`、`timeout-minutes:`、
`concurrency:`、以及 `@` 后跟 40 位十六进制的 action 引用。

```
- 状态：已完成 (a27c91c)
- 六条逐条落地：
  1. workflow 级 `permissions: contents: read`。
  2. 四个 action 全部按 commit SHA 固定（注释保留版本）：
     actions/checkout@11d5960a…（v4）、dtolnay/rust-toolchain@39008827…（stable）、
     dtolnay/rust-toolchain@1961bfc0…（1.95.0）、Swatinem/rust-cache@d1527c8f…（v2）。
  3. 新增 `Audit` job：EmbarkStudios/cargo-deny-action@17aeb3c8…（v2.1.3），
     `command: check advisories`；策略写入新建的 `deny.toml`（`unmaintained = "workspace"`、
     `yanked = "warn"`，只跑 advisories 故无需许可证白名单）。
  4. 每个 job `timeout-minutes: 30`（共 5 处）。
  5. workflow 级 `concurrency: group: ${{ github.workflow }}-${{ github.ref }}` +
     `cancel-in-progress: true`。
  6. 新增 `Examples` job：依次 `cargo run --locked -p rsagent-agent --example
     headless|custom_tool|end_to_end` 与 `-p rsagent-ai --example custom_provider`。
- 本地实测（写进 job 前）：四个 example 全部 `exit 0` 且输出合理（`answer: Hello from a
  headless agent!` / `word_count -> 4` / `end_to_end` 完整事件流 / `echo: pong`）。
- 验收证据：
  - workflow YAML 用 pyyaml 解析通过；`permissions:`×1、`timeout-minutes:`×5、
    `concurrency:`×1；全部 `uses:` 均为 `@` + 40 位十六进制。
  - 门禁（无 Rust 改动，复跑确认未受影响）：fmt 干净 / clippy 0 警告 /
    279 通过 / rustdoc 0 告警。
  - **远端 CI 对 `a27c91c` 7/7 job 全绿**：原 5 个（`ubuntu-latest`/`macos-latest`/
    `windows-latest`/`Docs`/`MSRV (1.95)`）+ 新增 `Audit`、`Examples`，全部 `success`
    （经 check-runs API 核实）。
```

> **评审方核实（2026-11-20 10:49，已提交状态 `a27c91c`）——✅ 通过，六项全部做到**
>
> 逐项复核新 `ci.yml`（126 行）与 `deny.toml`（25 行）：
> `permissions: contents: read` ✓ · 全部 `uses:` 为 `@` + 40 位 SHA **且带 `# v4`/`# stable`
> 等版本注释** ✓ · 5 个 job 各 `timeout-minutes: 30` ✓ · `concurrency` + `cancel-in-progress` ✓ ·
> `Audit` job（`cargo-deny-action`，`check advisories`）✓ · `Examples` job 依次跑 4 个示例 ✓。
>
> **`timeout-minutes` 的注释直接引用了 H4**（「a child process holding a pipe open, as H4
> fixed」）——**这正是评审方给出该建议的理由，执行方把它写进了注释**，后人能看懂为什么是 30 分钟。
>
> **`deny.toml` 的分寸值得表扬**：它只配置 CI 实际检查的 `[advisories]`，并在注释里**明确
> 声明不臆断** `licenses`/`bans`/`sources` 的策略（「a project decision, not something to
> guess here」）。**这正是不越界替项目做决策**——许可证白名单是项目负责人的选择，不是
> 一个 CI 加固任务该顺手定下的。`unmaintained = "workspace"` 只对直接依赖失败、
> `yanked = "warn"` 只提示不失败，两条都给了理由。
>
> **最强证据是远端验证**：`a27c91c` 上 **7/7 job 全绿**（含新增的 `Audit` 与 `Examples`），
> 经 check-runs API 核实——**说明新 CI 不只是语法正确，而是真的能跑通**。
>
> **⚠️ 本轮评审方差点漏审 D4**：`git log -1 -- crates/` 因 D4 只改 `.github/` 与 `deny.toml`
> 而**没有移动**，按旧口径会被判为「无代码改动」。**§5 的基线规则已修正为
> `git log -1 -- crates/ .github/ deny.toml`**，并把这次差点漏审记在案。

### D5 · M6 路径基准（用户可见的静默差异）

配置里的相对路径按**进程 cwd** 解析，而不是配置文件目录或 `--cwd`。在 `repo/sub` 下运行
`path = "./skills/team"` 会找错目录并**被静默跳过**。建议以配置文件所在目录为基准，并写进
README。成本约 0.5 天。

### D6 · C 组两条决策（需项目负责人拍板）

见 §2 的 C1（信任边界 opt-in）与 C2（记忆蒸馏限频）。**评审方倾向**：C1 做「只读命令不
启动进程」；C2 做「每 5 轮最多一条蒸馏笔记」。

### D7 · 🆕 `STABILITY.md` 的公开 API 清单与实际导出**范围不一致**

- **状态**：已完成 (466515d) · **成本**：约 0.5 天 · **影响**：中（契约歧义，与 A1/A5 同类）
- **问题（2026-11-20 06:49 评审方实测计数）**：

  [STABILITY.md](../STABILITY.md) 的**正文**说：

  > A crate's public API is exactly what its crate root re-exports (`pub use`), plus its
  > documented public items.

  但紧随其后的**表格**只列了 **30 项**（`rsagent-ai` 13 + `rsagent-agent` 17），
  而两个 crate root **实际 `pub use` 了 79 项**（26 + 53）。**49 项导出未列入表中**，
  例如：

  | Crate | 已导出但**未**列入 STABILITY 表的（举例） |
  | --- | --- |
  | `rsagent-ai` | `AssistantMessage`、`UserMessage`、`SystemMessage`、`ToolResultMessage`、`Auth`、`EventStream`、`AnthropicProvider`、`OpenAiProvider`、`FauxProvider`、`FauxBlock`、`FauxResponse`、`Modality`、`now_millis` |
  | `rsagent-agent` | `AllowAll`、`DenyAll`、`BreakerPolicy`、`BreakerReport`、`BreakerState`、`CancelToken`、`CatalogError`、`PREFIX_SEPARATOR`、`SourceHealth`、`SourceKind`、`UnhealthySource`、`HealthProbe`、`BuiltinSource`、`LocalSource`、`McpHttpSource`、`McpStdioSource`、`SkillSource`、`WebSource`、`WebSourceConfig`、`builtin_tools`、`memory_tools`、`select_builtin_tools`、各 `*Tool` 类型、`DEFAULT_MEMORY_CAP`、`DEFAULT_RECALL_LIMIT`、`MAX_NOTE_CHARS` |

- **为什么这是缺陷（而非小瑕疵）**：正文与表格对「什么是契约」给出**两个不同答案**。
  读者按表格理解，会认为 `BreakerPolicy`、`SourceHealth`、`WebSourceConfig`、
  `MAX_NOTE_CHARS`、`builtin_tools` 等**不受 semver 保护**、可随意改动；
  而按正文理解，它们**都是**契约的一部分。**这与 A1（`--allow` 类别语义）和 A5（doctor
  的 CI gate 承诺）是同一类问题：文档承诺的边界不清，用户会在关键时刻拿到错误结论。**
  后果是双向的：用户可能依赖了「其实没承诺」的东西，维护者也可能改坏「其实承诺了」的东西。
- **方案（二选一，需先定调）**：
  - **(a) 让表格穷尽**：把 79 项全部列入，按模块分组。诚实但表会很长，且每次新增导出
    都要同步——容易再次漂移。
  - **(b) 让正文与表格口径一致（推荐）**：正文改为「契约 = 本表所列项 + 各 crate 的
    `pub use` 中**在 rustdoc 里带文档**的项」，并在表格加一行说明「未列出的导出按
    `#[doc(hidden)]` 或 `pub(crate)` 处理」。**更彻底的做法**：对不打算承诺的导出加
    `#[doc(hidden)]`，让「是否契约」由代码而非文档表达——**这样两者不可能再漂移**。
- **验收**：选 (b) 时，加一个测试或 CI 检查：`STABILITY.md` 表中列出的名字必须都能在
  对应 crate root 找到（防止表格写了不存在的项）；反之，未列出的导出应带 `#[doc(hidden)]`。
- **关联**：REVIEW.md §5 的「文档承诺与实际行为不符」主题；A1、A5。

```
- 状态：已完成 (466515d)
- 采用 (a) 与 (b) 的合体：让表格穷尽（(a) 的诚实），并把「是否契约」交给代码 + 测试
  （(b) 的「更彻底做法」）——穷尽消除正文/表格的双口径，测试消除 (a) 的漂移风险。
- STABILITY.md「Public API」节重写：
  - 正文改为「契约 = 根 `pub use` 的**全部**名字，减去标 `#[doc(hidden)]` 的」；
  - 表格由 2 行长条目改为按用途分组的 15 行，逐项列出全部 **79** 项
    （`rsagent-ai` 26 + `rsagent-agent` 53），与两个 `lib.rs` 的 `pub use` 一一对应。
- 新增检查（crates/rsagent/tests/docs.rs）：`the_public_api_table_matches_the_crate_roots`
  解析两个 crate root 与表格，断言二者集合**相等**——既查「表列了不存在的项」，也查
  「导出了却未列」；并遵守 `#[doc(hidden)]`（隐藏项从契约集中剔除）。
- 验收证据：
  - cargo fmt --all --check          → exit 0
  - cargo clippy ... -D warnings     → 0 警告
  - cargo test --workspace --locked  → 278 通过 / 0 失败（较上轮 +1）
  - cargo doc ...                    → 0 告警
- **检查有效性已实测**（三种情形，均当场还原）：
  1. 表中把 `CancelToken` 误写成 `CancelTokenn` → 失败，同时报
     `exported but not in the table: ["CancelToken"]` 与
     `in the table but not exported: ["CancelTokenn"]`；
  2. 给 `pub use cancel::CancelToken;` 加 `#[doc(hidden)]` 但不改表 → 失败，报
     `in the table but not exported: ["CancelToken"]`（证明逃逸口已接线）；
  3. 两者还原 → 通过。
```

### D8 · 🆕 `STABILITY.md` 有 4 处断链，且都在契约表里

- **状态**：已完成 (a326913) · **成本**：约 5 分钟 · **影响**：中（契约文档的导航在最关键处失效）
- **问题（2026-11-20 08:19 评审方实测）**：对全仓库 17 个 markdown 文件做了相对链接
  完整性检查，**只有 `STABILITY.md` 有断链，共 4 处，且全在「Public API」表内**：

  | 行 | 现在的链接 | 解析结果 | 应为 |
  | --- | --- | --- | --- |
  | 40 | `../README.md#tool-sources-rsagenttoml` | ✗ `..` 逃出仓库 | `README.md#tool-sources-rsagenttoml` |
  | 41 | `../README.md#json-output` | ✗ 同上 | `README.md#json-output` |
  | 42 | `../README.md#event-log` | ✗ 同上 | `README.md#event-log` |
  | 43 | `EXTENSIBILITY.md` | ✗ 实际在 `docs/` | `docs/EXTENSIBILITY.md` |

  （三个锚点本身**是有效的**——评审方已核对 README 的标题，GitHub 的锚点算法会去掉
  `rsagent.toml` 里的点号，故 `#tool-sources-rsagenttoml` 正确。**坏的只是路径前缀**。）

- **根因（推断）**：`STABILITY.md` 现在在**仓库根目录**，但链接是按「它在 `docs/` 里」
  写的。同目录的 `docs/ARCHITECTURE.md`、`docs/EXTENSIBILITY.md` 用 `../README.md`
  **是正确的**（它们确实在 `docs/` 下）。所以很可能是 `STABILITY.md` 从 `docs/` 移到
  根目录时，链接没跟着改。
- **为什么不是小事**：这 4 行是**定义公开契约的那张表**。其中两行（`--json` schema、
  `--log` trace schema）明说「契约在 `README#json-output` / `#event-log`」——**而读者
  从契约文档点不过去**。想确认 `--json` 字段承诺的人，正好在这里被卡住。
- **为什么一直没被发现**：`cargo doc -D warnings` 只校验 **Rust intra-doc 链接**，
  **不校验 markdown 文件之间的链接**。CI 的 `Docs` job 因此对这类断链完全无感。
- **方案**：改 4 处路径前缀即可（`../README.md` → `README.md`；`EXTENSIBILITY.md` →
  `docs/EXTENSIBILITY.md`）。**建议顺带加一条 CI 检查**（几行脚本）：所有 `.md` 里的
  相对链接目标必须存在——这类断链目前没有任何门禁能拦，加一次就永久免疫。
- **验收**：`grep -n "\.\./README" STABILITY.md` 无结果；全仓库 markdown 相对链接检查
  通过。
- **关联**：D7（同一张表的另一个问题：范围不一致）。**两处都在 STABILITY 的契约表里，
  建议一起修。**

```
- 状态：已完成 (a326913)
- 验收证据：
  - cargo fmt --all --check          → exit 0
  - cargo clippy ... -D warnings     → 0 警告
  - cargo test --workspace --locked  → 277 通过 / 0 失败（较上轮 +1）
  - cargo doc ...                    → 0 告警
  - 4 处路径前缀已改（`../README.md`→`README.md` ×3；`EXTENSIBILITY.md`→`docs/EXTENSIBILITY.md`）；
    `grep -n "\.\./README" STABILITY.md` 无结果。
  - 新增检查（crates/rsagent/tests/docs.rs）：`every_relative_markdown_link_resolves`
    遍历仓库全部 `.md`（跳过 target/.git），断言每个相对链接目标存在；跳过代码块、
    外部 URL、`mailto:` 与纯锚点。
  - **检查有效性已实测**：把链接改回 `../README.md` 后该测试失败并报出 3 处（第 4 处
    `EXTENSIBILITY.md` 未一起改，故未报），随后还原。
```

### D9 · 🆕 `abort` 退 0，但 README 说 `0` = 「产出了答案」

- **状态**：已完成 (9b53edb)（**由执行方在 D2 中发现，评审方已核实并独立成条**）
- **成本**：约 10 分钟（改措辞）或约 0.5 天（改行为，若决定改） · **影响**：低中
- **问题（评审方 2026-11-20 09:19 核实）**：README「Exit codes」写

  > `0` — The run finished and the model produced an answer.

  但**被中断的 run 也退 0**，而它**没有产出答案**。代码路径（已回读确认）：
  - `agent.rs:400` — `StreamOutcome::Cancelled => return Ok(())`
  - `agent.rs:422-428` — `if stop_reason == StopReason::Aborted { … return Ok(()) }`

  两处都把「用户中断」当作**成功**返回，于是 `--print` 下退 `0`。**退出码是 STABILITY
  列为 Semver 的契约**，所以这不只是措辞问题。
- **为什么执行方没能加测试**（记录其判断，评审方认同）：非交互场景下跨平台向子进程发
  SIGINT 需要新依赖，**护栏 #1 禁止**；且评审方在 D2 里明确要求「**先确认行为、再写测试**，
  不要先写测试去修正一个未定行为」。执行方遵守了这条并留作观察——**这是正确的处理**。
- **方案（二选一）**：
  - **(a) 改措辞（推荐）**：把 README 的 `0` 改为「the run finished — either the model
    produced an answer, **or the user aborted it**」。**理由**：中断是用户主动行为，不是
    失败；把它算成错误反而会让脚本误判。语义上「完成」比「产出答案」更准确。
  - **(b) 改行为**：让 abort 退 `1`。但这**改变了已发布的退出码语义**（STABILITY 列为
    Semver），且需要先确认 CI/脚本里是否有人依赖 abort 退 0。
- **评审方倾向 (a)**：成本 10 分钟、不动契约行为、消除文档与实现的不一致。
- **验收**：README 的 `0` 一行与 `agent.rs:400`/`:422` 的实际行为一致；若选 (b)，需在
  CHANGELOG 记 Breaking。
- **关联**：D2（同一组退出码契约）、A1/A5/D7/D8（同属「文档承诺 vs 实际」这一类）。

```
- 状态：已完成 (9b53edb)
- 采用方案 (a)（改措辞，不改行为）：
  - README「Exit codes」引导句：`exits non-zero when the run did not produce a usable
    answer` → `when the run failed or was cut short`；
  - 表格 `0` 行：`The run finished and the model produced an answer.` →
    `The run finished: the model produced an answer, or the user aborted it (Ctrl-C).`；
  - 表格后补一句：abort 退 0 是**有意**的（用户主动行为，非失败），脚本不应据此误判。
- 行为未改动（退出码是 Semver 契约）：`agent.rs:400`（`StreamOutcome::Cancelled`）与
  `:422-428`（`StopReason::Aborted`）两处仍 `return Ok(())`。
- 验收证据：
  - cargo fmt --all --check          → exit 0
  - cargo clippy ... -D warnings     → 0 警告
  - cargo test --workspace --locked  → 277 通过 / 0 失败（无新增测试）
  - cargo doc ...                    → 0 告警
- **未加测试**：abort 行为已写在 `Agent::prompt` 的文档注释里（「A user abort is not an
  error: the run ends with `Ok(())`」），而要端到端锁住它需在流式进行中竞态触发取消
  （`prompt` 会在入口 `cancel.reset()`），或向子进程发 SIGINT（护栏 #1 禁止新依赖）。
  按评审方「先确认行为、再写测试」的要求，此处只改措辞、不加易 flaky 的测试。
```

> **评审方核实（2026-11-20 09:49，已提交状态 `a326913` / `9b53edb` / `466515d`）——✅ 三项全部通过**
>
> **D8（`a326913`）**：4 处链接按建议改好（`../README.md` → `README.md` ×3；
> `EXTENSIBILITY.md` → `docs/EXTENSIBILITY.md`）。**评审方复跑全仓库链接检查：0 处断链。**
>
> **D9（`9b53edb`）**：采用建议的方案 (a)——改措辞、**不动行为**。README 现在写
> 「The run finished: the model produced an answer, **or the user aborted it (`Ctrl-C`)**」，
> 并补了一段说明为什么 abort 退 0 是**故意的**（用户主动行为不是失败，脚本不该被 Ctrl-C 误导）。
> 这是比「只改那一行」更好的处理——**它解释了设计意图，而不只是让文字对上**。
>
> **D7（`466515d`）——本轮的亮点，做法比评审方建议的更强**：
> 评审方建议的是「让正文与表格口径一致 + 给不承诺的导出加 `#[doc(hidden)]` + 加 CI 检查」。
> 执行方全部做到，并**把检查做成了双向**：新建的 `crates/rsagent/tests/docs.rs` 同时解析
> 两个 crate root 的 `pub use` 与 `STABILITY.md` 的表格，**「表里多的」和「代码里多的」都报错**。
>
> **为什么双向是关键**：单向检查（只查「导出了但没列表」）会漏掉「表里写了个不存在的名字」；
> 而且**双向检查让这个测试不可能空转**——若任一解析器返回空集，另一个方向立刻非空而失败。
> 所以「测试通过」本身就证明了两个集合**都非空且相等**，而不是「两边都解析失败所以相等」。
> 执行方还说明它**手工验证过测试在两种方向下都会红**（表打错、或加 `#[doc(hidden)]` 却不改表）。
>
> **评审方独立复核 D7 的结果**：解析 `STABILITY.md` 的 `crate root` 行，得
> **`rsagent-ai` 26 项 + `rsagent-agent` 53 项 = 79 项**，与实际 `pub use` 数量**完全一致**。
>
> **这一改的意义**：D7 之前，「公开 API 是什么」有两个答案（正文说全部导出，表只列 30 项）；
> 现在**正文、表格、代码三者由测试锁在一起**，规则从「文档里的君子协定」变成了
> **「CI 会拦你」**——这正是本项目 `tool_conformance.rs` 一直在用的思路。
> `#[doc(hidden)]` 逃生口已建好但**当前未被使用**（因为表已列全），需要「去承诺」某个名字时
> 才用它。

---

**评审方对阶段 2 的总评**：**D1 是唯一真正重要的一项**——它保护的是「审批是唯一防线」
这个安全模型本身。D2 保护的是已写进 STABILITY 的公开契约。**D7、D8、D9 修的都是
「契约文档本身」**——D7 说错了范围，D8 指不到地方，D9 说漏了一种情况；三者都不影响运行，
但都影响下游用户对「能不能依赖」的判断，且与 A1/A5 同属本项目最高价值的一类缺陷。
D3/D4 是低成本清理。D5 是用户可见的行为修正。
D6 需要你决策。**建议按 D1（✅已完成）→ D2（✅已完成）→ D9 → D7 → D8 → D3/D4 → D5 → D6 推进。**
D9 成本最低（10 分钟）且已核实，适合下一次顺手带上。

---

## 8. 剩余中等项（E1–E4）

> **为什么单列一节**：这四条是 [REVIEW.md](REVIEW.md) §5 里**尚未排期**的发现。它们此前只以
> 「结论」形式存在，执行方拿到的是描述而非任务。本节把它们转成与 A/B 组同样格式的条目：
> 定位到行、给出改法、写明验收。
>
> **2026-11-20 04:19 逐条回读源码重新核实，四条在当前代码里仍然存在**（行号为准）。
> 编号用 E 组，以免与 §2 的 A/B/C 组、§7 的 D 组冲突。

### E1 · M15 · `skill` 的 `command` 可逃出技能目录（**建议优先**）

- **状态**：待办
- **成本**：约 0.5 天 · **影响**：中高（安全）
- **问题（已验证）**：`crates/rsagent-agent/src/sources/skill.rs:177-181`

  ```rust
  let command = Path::new(&config.command);
  let program = if command.is_absolute() {
      command.to_path_buf()
  } else {
      dir.join(command)          // 没有 canonicalize，也没有 starts_with(dir) 检查
  };
  ```

  `skill.toml` 的 `command` 可以是绝对路径，也可以是含 `../..` 的相对路径。两者都不受技能
  目录约束。**后果**：从别处复制一个 skill 目录过来（或 `git clone` 一个含 skill 的仓库），
  就等于获得「执行任意二进制」的能力——而且 `requires_approval` 默认由该 manifest 自己声明。
- **方案**：解析后 `canonicalize()`，要求结果 `starts_with(canonicalized_dir)`；绝对路径
  若在目录外则拒绝并给出清晰错误。注意 `canonicalize` 要求路径存在，需处理文件不存在的情况
  （报错即可）。**不要**为此新增依赖。
- **验收**：新增单测：manifest 里 `command = "../../evil.sh"` 与 `command = "/bin/sh"`
  （目录外绝对路径）都必须**加载失败**；目录内的 `command = "run.sh"` 仍正常。
- **关联**：REVIEW.md M15。

### E2 · M7 · 外部输出无大小上限

- **状态**：待办
- **成本**：约 0.5 天 · **影响**：中（内存峰值）
- **问题（已验证）**：`crates/rsagent-agent/src/tools.rs:467`（`SearchTool` 的实现，起点
  `tools.rs:422`）

  ```rust
  let Ok(text) = tokio::fs::read_to_string(&entry_path).await else { … };
  ```

  `search` 把**每个命中文件整份读进内存**再逐行匹配。现有的 32K 截断发生在**完整缓冲之后**，
  对内存峰值没有任何约束——一个几百 MB 的文件（或一个巨大的 `--memory` 文件、一个超大的
  MCP 响应）就能把进程推高。同类问题也在 `sources/subprocess.rs` 的共享缓冲与
  `mcp_http.rs` 的 `response.text()`。
- **方案**：读侧加上限。最小改法：`search` 用一个带上限的读取（`take(limit)` 或先查
  `metadata().len()` 超限则跳过并注明），上限取一个明显大于正常源文件的值（如 8 MiB）。
  `subprocess`/`mcp_http` 的缓冲同理，超限时**截断并标记**而不是继续累积。
- **验收**：新增单测：造一个超过上限的文件 → `search` **不**把它整份读入，且结果里能看到
  「已跳过/已截断」的标记。
- **注意**：不要改变现有 32K **输出**截断的行为——那条是给模型的上下文预算用的，与这里的
  内存上限是两件事。

### E3 · M11 · REPL 遇 `MaxTurnsExceeded`/`NoResult` 会退出且丢会话

- **状态**：待办
- **成本**：约 0.5 天 · **影响**：中（用户可见）
- **问题（已验证）**：`crates/rsagent/src/main.rs:1057-1058`

  ```rust
  match run_prompt(agent, rx, renderer, log, input_text.to_string()).await {
      Ok(()) => {}
      Err(error) if is_model_failure(&error) => {}   // 只有模型级失败被容忍
      Err(error) => return Err(error),               // MaxTurnsExceeded / NoResult 落这里
  }
  if let Some(session) = session.as_deref_mut() {
      session.save(agent, renderer.palette);         // ← 上面 return 时被跳过
  }
  ```

  与 [README](../README.md) 的「REPL never exits on a failed turn」**矛盾**：跑满 `--max-turns`
  或模型没给出结果时，REPL 直接退出，**且这一轮的会话没有保存**——用户丢了整轮对话。
- **方案**：把「可容忍」的判定从「是否模型级失败」放宽为「是否**应当结束会话**」：`MaxTurnsExceeded`
  与 `NoResult` 应打印提示后**继续 REPL**（并把会话存下来）。建议抽一个
  `ends_the_session(&AgentError) -> bool`，只对真正的致命错误返回 `true`——比现在的
  `is_model_failure` 更贴合语义。同时确认 README 的措辞与实际一致。
- **验收**：新增 E2E：`--faux` + `--max-turns 1` + 一个需要工具的 prompt，交互模式下断言
  进程**没有退出**且会话文件已写入。
- **关联**：REVIEW.md M11、§6 的「退出码」契约（`--print` 下退 1 是对的，REPL 下不该退）。

### E4 · M4 · `Usage.input` 两家 provider 语义相反

- **状态**：待办
- **成本**：约 0.5 天 · **影响**：中（成本/用量统计偏低）
- **问题（已验证）**：`Usage`（`crates/rsagent-ai/src/types.rs:71-74`）把 `input` 文档写作
  「Input (prompt) tokens billed for this call」，另有独立的 `cache_read` / `cache_write`。
  但两家填法**相反**：
  - **Anthropic**（`anthropic.rs:392-394`）：`input = input_tokens`——Anthropic 的
    `input_tokens` **不含**缓存命中的部分（它们在 `cache_read_input_tokens` 里）。
    即 `input` 与 `cache_read` **互斥**，总量 = 三者之和。
  - **OpenAI**（`openai.rs:361-376`）：`input = prompt_tokens`——而 `prompt_tokens`
    **包含**缓存命中（`prompt_tokens_details.cached_tokens` 是它的**子集**）。
    即 `input` ⊇ `cache_read`。

  **后果**：任何按 `input + cache_read` 计算总输入量的消费者，在 OpenAI 上会**重复计数**；
  任何只看 `input` 的消费者，在 Anthropic 上会**少算**。开启 prompt cache 后用量与成本
  会明显偏离真实值，而且**不会报错**。
- **方案（二选一，推荐前者）**：
  - **(a) 统一为「互斥」语义**：让 OpenAI 侧填 `input = prompt_tokens - cached_tokens`
    （饱和减），并在 `Usage::input` 的文档里**明确写清**「不含缓存命中的输入 token」。
  - **(b) 统一为「包含」语义**：让 Anthropic 侧 `input = input_tokens + cache_read`。
  - 无论选哪个，**都要把语义写进文档注释**——这正是本条问题的根源：字段文档有歧义。
- **验收**：新增单测：给两家各喂一份带缓存字段的 `usage` JSON，断言归一化后
  `input + cache_read`（或按选定语义）得到同一个总量。
- **关联**：REVIEW.md M4。**注意**：`Usage` 在 STABILITY 的 Semver 表里，改语义属破坏性
  变更，需记 CHANGELOG。

> **评审方补充核实（2026-11-20 12:19）——外部事实已佐证，且影响面已界定**
>
> 本条结论依赖**两处外部 API 语义**（不是仓库内可验证的事实），原先只标「已验证」不够严谨。
> 评审方查证后**两处均得到独立佐证**：
>
> - **OpenAI 的 `prompt_tokens` 确实包含 cached**：一份实现文档明写「The OpenAI shape gives
>   `prompt_tokens` ***including*** cached」（[llm_meter](https://pub.dev/documentation/llm_meter/0.1.0/llm_meter/openAiUsage.html)），
>   另一份指出「OpenAI does NOT surface a separate cache-miss count; you can derive it from
>   `prompt_tokens - prompt_tokens_details.cached_tokens`」（[aiperf](https://raw.githubusercontent.com/ai-dynamo/aiperf/d72160e20957013d6608afcc88ed24100cb27dc5/docs/reference/vendor-usage-fields.md#1)）。
> - **Anthropic 的 `input_tokens` 确实不含 cache**：「Anthropic's reported `input_tokens`
>   excludes both cache-creation and cache-read portions」（[nexus](https://pkg.go.dev/github.com/frankbardon/nexus@v0.32.0/pkg/engine/pricing#1)）；
>   另一处给出同样的修法：「Count total prompt tokens as `input_tokens + cache_creation + cache_read`」
>   （[ruflo PR #4673](https://github.com/ruvnet/ruflo/pull/4673#1)）。
>
> **方案 (a) 与业界做法一致**：上面 aiperf 那条给出的「用 `prompt_tokens - cached_tokens`
> 导出 cache-miss」正是建议 (a) 的做法——**不是评审方臆造的归一化，而是别人也在这么做**。
>
> **影响面已界定：只影响成本/用量统计，不影响上下文管理正确性。** 评审方一度担心
> 「cached token 仍占上下文窗口，若压缩器用 `Usage` 算填充率会触发过晚」，故回读了压缩器：
> `compact_context`（`agent.rs:737`）用的是 `context_budget()`（基于 `model.context_window`）
> 与 `message_chars()`（`agent.rs:1012`）——**基于消息文本的字符估算，完全不读 `Usage`**。
> 因此 cached token 在预算里**已被正确计入**，那个风险**不适用于本项目**。
> **这让 E4 的严重度可以明确为「中偏低」**：它是统计准确性问题，不是功能正确性问题。
>
> **优先级相应下调**：E4 仍值得修（用量/成本是用户可见的数字），但**排在 E1（安全）、
> E3（丢会话）之后**，且比 E5（文档数字）更值得做。

---

### E5 · 🆕 `ARCHITECTURE.md` 里的测试数已过期（230 → 267）

- **状态**：待办 · **成本**：约 5 分钟 · **影响**：低（文档准确性）
- **问题（2026-11-20 07:49 评审方实测）**：`docs/ARCHITECTURE.md:307` 与 `:310`
  两处都写着

  > `cargo test --workspace` runs **230** tests / 有 **230** 个测试

  而实测为 **267**（`cargo test --workspace --locked` 逐二进制相加）。**偏差 37**，
  即 A/B 两组修复新增的测试都没反映进去。
- **为什么值得修**：`ARCHITECTURE.md` 是新读者的**第一入口**（README 把它列为「三个入口」
  之首）。一个 230 的旧数字会让读者以为测试规模没变，也可能让人误以为自己的检出有问题。
  它不影响运行，但属于「文档与实现不符」——本项目反复出现的那一类。
- **方案**：**建议直接去掉数字**，改成「`cargo test --workspace` 跑一整套读起来像用例的
  测试」，或写「200+ 个测试」。**理由**：硬编码一个每次加测试都会失效的数字，注定再次漂移
  ——本项目已经至少改过一次（EXTENSIBILITY 的记忆里记着「测试数 162→173」）。
  若确实想显示精确数，就让 CI 生成，而不是手写。
- **验收**：`grep -n "230" docs/ARCHITECTURE.md` 无结果；文档里的测试数表述不再是一个
  会随提交失效的常量。
- **顺带核实（已通过）**：同一轮检查了其它计数声称，`EXTENSIBILITY.md` 的「六种 `kind`」
  与 `SourceKind::ALL`（Builtin/Local/McpStdio/McpHttp/Skill/Web）**一致，无需改动**。

---

### E6 · 🆕 决策日志 D15 的论证会诱导 A1 回归

- **状态**：待办 · **成本**：约 15 分钟 · **影响**：**中**（不是措辞问题——它可能让人撤销 A1 的修复）
- **问题（2026-11-20 12:49 评审方实测）**：[EXTENSIBILITY.md](EXTENSIBILITY.md) §7 的
  **D15** 结尾写着：

  > 默认类别取**最严**的 `exec`：未分类的审批工具不会被 `--allow write`/`--allow network`
  > 意外放行

  **这正是 A1 修掉的那个概念错误**：`exec` **不是「最严」**。四个类别之间**不存在全序**——
  `--allow exec` 会**自动放行**一个未分类的工具；若该工具实际是**网络**工具（A1 修之前的
  `web`/`mcp-http`/`mcp-stdio` 就是），那就是**静默的权限放大**，而不是「最严的默认」。

  D15 断言的「不会被 `--allow write`/`--allow network` 放行」**字面上是真的**，但它**只看了
  两个方向**，漏掉了第三个——而漏掉的那个正是出问题的那个。
- **代码侧已修好，文档侧没有**（已核实）：
  - `Tool::permission()` 的默认**仍是** `Permission::Exec`（`tool.rs:264`）——A1 没有改默认值；
  - A1 的做法是让 5 个来源**各自显式声明**：`web`/`mcp_http`/`mcp_stdio` → `Network`，
    `local`/`skill` → `Exec`（已回读确认）。
  - **所以 D15 描述的「靠默认值兜底」已经不成立了**：现在靠的是「每个来源都必须显式分类」。
    D15 没有记录这一点。
- **为什么这是缺陷而不是措辞瑕疵**：决策日志自己的用途写着「记录**为什么不那样做**，
  避免后来者重走」。一个不完整的论证**正好反过来**——后来者读 D15 会学到「默认 `exec` 已经
  最安全」，进而可能认为 A1 给每个来源加显式 `permission()` 是**多余的**，把它们删掉，
  **从而重新引入 A1 修掉的静默权限放大**。**它保护了一个已经被证伪的前提。**
- **方案**：改写 D15 结尾，记录**修正后的理解**，要点：
  1. **四个类别不是全序**，`exec` 不比 `network` 更严——它们是**正交**的副作用类别；
  2. 因此默认值**不能**靠「取最严」来兜底，正确做法是**要求每个来源显式声明**（A1 的做法）；
  3. 默认 `Exec` 现在的角色是「**兜底提醒**」而非「安全保证」——它保证未分类的工具**不会**
     被 `--allow write`/`--allow network` 放行，但**会被 `--allow exec` 放行**，故新增
     需要审批的工具时**必须显式分类**。
  - 若愿意更进一步：可考虑让未显式分类的审批工具**任何 `--allow` 都不放行**（即需要一个
    「未分类」状态），但那是行为变更，**不在本条范围内**；本条只要求把论证写对。
- **验收**：`EXTENSIBILITY.md` 的 D15 不再出现「最严」这一说法；文中明确写出
  「类别正交、须显式分类、默认 `Exec` 会被 `--allow exec` 放行」。
  **§2.4 不变量 4 同样要改**（见上）。
- **⚠️ 同一错误框架出现在两处（2026-11-20 15:49 补充核实）**：不只是 D15。
  [EXTENSIBILITY.md](EXTENSIBILITY.md) **§2.4 不变量 4** 也写着

  > 需要审批的工具 SHOULD 同时用 `Tool::permission()` 声明真实类别……否则默认按**最严的
  > `exec`** 处理

  **两处都要改。** 不变量 4 是「永久有效、对所有改动生效」的规范条款，它里面的错误前提
  比 D15 的决策记录**影响面更大**——因为它正是 A1 那个 bug 的**书面依据**。
- **关联**：A1（`cf67bf8`）、`docs/REVIEW.md` §3.2、**E7**（同一规范的可执行部分覆盖不足）。
  **这是 A1 的文档配套，A1 只修了代码。**
- **优先级说明**：与 E1 同属「审批/隔离模型」这条线，**建议与 E1 一起修**——两者都在保护
  同一个安全模型：E1 管「技能目录逃逸」，E6 管「别把已经修好的权限分类又撤掉」。

---

### E7 · 🆕 §2.4 声称「可执行」，但那个测试只看内置工具

- **状态**：待办 · **成本**：约 0.5–1 天 · **影响**：**中高**（这是 A1 能溜过去的原因）
- **问题（2026-11-20 15:49 评审方实测）**：[EXTENSIBILITY.md](EXTENSIBILITY.md) §9 写着

  > `crates/rsagent-agent/tests/tool_conformance.rs` — 本规范 §2.4 的可执行部分

  **这句话把范围说大了。** 该测试的每一处检查都基于 `builtin_tools()`
  （`tests/tool_conformance.rs:9,25,49,97,112,127`），而 `builtin_tools()`
  （`tools.rs`）只含 **8 个内置工具**：`read_file`/`write_file`/`edit_file`/`list_dir`/
  `search`/`glob`/`webfetch`/`bash`。

  **五种来源的工具（`web`/`mcp-http`/`mcp-stdio`/`local`/`skill`）完全不在它的视野内**，
  而它们同样实现 `Tool`、同样受 §2.4 约束。
- **A1 就是证明**：A1 修的是「5 个来源都没实现 `permission()`，全落到默认 `Exec`」——
  **这正是不变量 4 的违反**（「MUST NOT 声明成比实际副作用更宽松的类别」）。而这个自称
  「§2.4 可执行部分」的测试**没有抓到它，因为它从未看过来源工具**。
  **一个把范围说大了的规范声明，会让读者以为有保护而实际没有。**
- **方案（二选一）**：
  - **(a) 把范围写准（最小改法）**：把 §9 那句改成「本规范 §2.4 对**内置工具**的可执行部分」，
    并说明来源工具的一致性检查在各自的来源测试里（`sources/*.rs` 的单元测试）。
  - **(b) 把检查扩到来源工具（推荐）**：让 conformance 测试也能检查来源工具——做法是
    构造一个**含全部来源类型**的 `ToolCatalog`（每种来源用最小 fixture，如 `web` 指向一个
    本地不存在的 URL、`local` 指向一个 `--describe` 输出固定 JSON 的脚本），对展开后的
    **全部工具**跑同一套不变量检查。这样 A1 那类遗漏会被自动拦住。
    - 注意：`builtin_tools()` 是 `pub` 的，但来源需要各自构造；可考虑让 `ToolCatalog`
      提供一个测试辅助，或把「不变量检查」抽成一个接受 `&[Box<dyn Tool>]` 的函数，
      由两个测试分别喂内置集与来源集。**这需要一个新的公共测试辅助点**，故成本略高。
- **验收**：选 (a)：`EXTENSIBILITY.md` 不再声称覆盖全部工具。选 (b)：**人为把某个来源的
  `permission()` 删掉，测试必须变红**（这是唯一的验收方式——像 A1 那样构造回归）。
- **关联**：**A1**（`cf67bf8`）、**E6**（同一条线上的文档与执行缺口）、
  `docs/REVIEW.md` §3.2 与 §7。
- **优先级说明**：**这是 E 组里唯一直接提升「未来发现能力」的一条**。E1/E6 修的是具体问题，
  E7 修的是「**为什么那个问题当初没被发现**」。若只做一条，**建议做 E7 的 (b)**——
  它让同类问题**下次自己变红**，而不是再等一次人工评审。

---

**评审方对 E 组的排序**：**E7（让同类问题下次自己变红——唯一提升「未来发现能力」的一条）→
E1（skill 逃逸，安全）→ E6（D15/§2.4 的论证会诱导 A1 回归）→
E3（REPL 丢会话，用户可见）→ E2（内存上限）→
E4（用量口径，需定语义；**已界定为只影响统计，不影响功能**）→
E5（文档数字，5 分钟）**。
E1、E6、E7 同属「审批/隔离模型」这条线，**建议一起做**：E1 管具体逃逸，E6 管文档依据，
E7 管为什么当初没被发现。E5 可以在任何一次顺手的提交里带上。

---

## 9. 已核实无需改动（评审方实测通过）

> **用途**：记录评审方**实际验证过、结论是「没问题」**的项。这样未来的评审轮次不必重复
> 检查，也让项目负责人知道这些承诺是**站得住**的——而不只是「没发现问题」。
>
> 本节只列**实测过**的项；没查过的不写进来。

| 核实项 | 方法 | 结论 |
| --- | --- | --- |
| CI 与 AGENTS.md 的四道门禁是否对齐 | 实读 `.github/workflows/ci.yml`（70 行） | ✅ 对齐：三平台 fmt/clippy/test + `Docs`（`-D warnings`）+ `MSRV (1.95.0)`；全程 `--locked` |
| README 宣传的 4 个 example 是否真能跑 | 逐个 `cargo run --example` | ✅ 全部 `exit 0` 且输出合理（`headless`/`custom_tool`/`custom_provider`/`end_to_end`） |
| `STABILITY.md` 指向 README 的三个契约锚点是否有效 | 按 GitHub 锚点算法核对 README 标题 | ✅ 三个锚点均正确（含 `#tool-sources-rsagenttoml`——算法会去掉点号）。**坏的是路径前缀，见 D8** |
| 「六种 `kind`」的声称是否与实际一致 | 比对 `SourceKind::ALL` | ✅ 一致（Builtin/Local/McpStdio/McpHttp/Skill/Web） |
| README 的 `rsagent.toml` 示例字段是否都真实存在 | 比对 `config.rs::SourceConfig`（16 字段） | ✅ 示例里出现的键**全部**存在于结构体；`Config`/`Tools` 都是 `deny_unknown_fields`，**照文档写不会解析失败** |
| README 的 `skill.toml` 示例字段是否都真实存在 | 比对 `skill.rs::ToolConfig`（6 字段） | ✅ 完全一致：`name`/`description`/`command`/`args`/`parameters`/`requires_approval` |
| 全仓库 markdown 相对链接完整性 | 扫 17 个 `.md` 的全部相对链接 | ✅ **D8 修好后 0 处断链**（`tests/docs.rs` 已把它变成 CI 检查） |
| README 说两个 HTML 图解页含「**真实的函数名与事件名**」 | 从两个 `.html` 提取标识符，逐个比对源码（2026-11-20 11:49） | ✅ **全部命中**：15 个函数名（`run_turns`/`run_loop`/`run_one`/`stream_turn`/`run_tool_calls`/`execute_one`/`check_approval`/`probe_health`/`compact_context`/`build_context`/`inject_steering`/`resolve_within`/`truncate_result` 等）与**全部 14 个 `AgentEvent` 变体**都仍存在于源码；`ToolUse` 亦为真实的 `StopReason::ToolUse` |
| **D6**：平台差异是否真的只在 `platform.rs` | 扫全树 `#[cfg(`，逐个确认是否在测试模块内（2026-11-20 13:19） | ✅ **不变量成立**：业务代码中除 `platform.rs`（3 处）外**没有任何平台 `#[cfg]`**。`subprocess.rs:184/190` 的两处在 `#[cfg(test)] mod tests` 内，是跨平台测试夹具（`cmd /C start` vs `sh -c sleep`），**不属业务代码**。 |
| **§2.4 不变量 1**：分层是否单向 | 扫三个 `Cargo.toml` **全文**（含 `dev-`/`build-dependencies`）的 rsagent-* 依赖边（2026-11-20 16:19） | ✅ **成立且由编译器强制**：`rsagent-ai` 无 rsagent-* 依赖；`rsagent-agent` 只依赖 `rsagent-ai`；`rsagent` 依赖下面两层。**测试依赖里也没有向上边**。因为这是 Cargo 依赖边，不声明就无法 `use`——**不变量 1 不是君子协定，是编译错误**。 |
| **§2.4 不变量 3**：工具失败用 `Err(String)` | 读 `Tool::execute` 的签名（2026-11-20 16:19） | ✅ **由类型强制**：`execute` 返回 `Result<ToolResult, String>`，把失败塞进 `Ok` 的 `content` 需要刻意为之。**编译器层面成立。** |
| **§2.4 不变量 7**：公开 API 有文档注释 | 确认 CI 的 `Docs` job 与 lint（2026-11-20 16:19） | ✅ **由门禁强制**：`missing_docs` lint + `RUSTDOCFLAGS="-D warnings"` 的 `Docs` job；`missing_errors_doc` 亦已开启（见 `Cargo.toml` 的 `[workspace.lints]`）。 |
| **§2.4 不变量 2**：新增 enum 变体前确认 `non_exhaustive` | 枚举全部公开 enum 及其标记状态（2026-11-20 16:49） | ✅ **无违规**：14 个公开 enum 中 8 个已标（`AgentEvent`/`StreamEvent`/`ErrorReason`/`SourceKind`/`SourceHealth`/`CatalogError`/`AgentError`/`ContentBlock`/`Message`/`StopReason`）；未标的 6 个（`Permission`/`ThinkingLevel`/`Modality`/`BreakerState`/`PathFence`/`FauxBlock`）都是**闭合集**。`STABILITY.md` 的政策表述（「在**可能新增变体处**标」）与现状**一致**——全量标记会给用户强加 wildcard 分支的成本，换不到收益。 |
| **D15**：默认 `exec` 是否真是「最严」 | 比对 `tool.rs` 默认值与 4 个类别的语义（2026-11-20 12:49） | ❌ **不成立** → 已立为 **E6**（类别非全序，`--allow exec` 会放行未分类工具）。**这是本轮唯一发现的决策日志漂移。** |

**一处刻意不记的观察**：`rsagent.toml` 的 `only` 字段是真实存在的（`SourceConfig.only`），
但 README 只在正文提到「`--tools` replaces a built-in source's `only`」，**未放进示例**。
评审方判断这**过于次要，不记为任务**——正文已确立该字段存在，读者可推知写法；
为一个「示例可以更全」的观察单开一条会让清单失真。**此处记录以备查，不建议动手。**

**一处值得注意的空白**：`docs/*.html` 两个图解页**不受任何门禁保护**——`tests/docs.rs`
只检查 `.md` 的链接，`cargo doc` 只检查 Rust intra-doc 链接。**HTML 里的符号若随重命名
过期，没有任何东西会报警**。评审方 2026-11-20 实测两份 HTML 仍然准确（见上表），
**但这是一个将来可能悄悄漂移的点**。若将来 HTML 需要维护，可考虑把「HTML 中引用的
`crate::` 符号必须存在」也纳入 `tests/docs.rs`；**当前不建议为此新增工作**，仅记录。

**另一处措辞不精确（非缺陷，不建议改）**：`AGENTS.md` 写「Platform differences
(**the only `#[cfg]` in the tree**)」，字面上不准确——`#[cfg(test)]` 遍布各文件，
`subprocess.rs` 的测试夹具还有两处 `#[cfg(windows)]`/`#[cfg(not(windows))]`。
**但 D6 的实质不变量（业务代码不携带平台 `#[cfg]`）是成立的**（已核实，见上表），
读者也不会被误导成「`#[cfg(test)]` 被禁」。**属措辞松散，不值得开任务。**

**§2.4 的强制力分布（值得记录的模式）**：把九条不变量按「由谁保证」分类后，出现一个
清晰的分界——
- **编译器强制**：不变量 1（分层，Cargo 依赖边）、3（`Err(String)`，类型签名）；
- **门禁强制**：不变量 7（文档，`missing_docs` + `Docs` job）；
- **只有测试能覆盖**：不变量 4/5/6（审批一致、命名、schema）——**而它们恰好是唯一出过
  真实问题的**（A1 违反不变量 4 却无人发现）。
- **不可机械检查**：不变量 2（流程规则）、8（语义）、9（rule of two）。

**这个分布正是 E7 的论据**：机械检查已在做它们该做的事，**缺口精确地落在「只有测试能覆盖」
的那一类**——所以应该**扩大 conformance 测试的范围**，而不是逐个人工评审。
