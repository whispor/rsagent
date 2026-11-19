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
| `cargo test --workspace --locked` | ✅ **254 通过 / 0 失败**（@ `75eb483`） |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked` | ✅ 0 告警 |

**已修复（评审方逐条核实）**：

- **H1–H7 + M16/M17**（9 条）：`3b22d5f`、`e939f87`、`c8f6351`、`73fb63b`。
- **A1 + A2 + A3**：`cf67bf8`、`9029cc3`、`e1906b5`。测试数 243 → 251。
- **A4 + A5**：`ab178bc`、`75eb483`。测试数 251 → 254（A4 +1、A5 +2）。
- **A 组已全部完成（5/5）**。

**综合评分 9.3 → 9.4 / 10**（A4 修好了输出污染，A5 让文档承诺的 CI gate 真正生效）。

**护栏复核**：无新增依赖、无 `#[allow]`、无业务代码 `#[cfg]`、无 `unsafe`/`TODO`、无无关
重构——五条修复都是**给已有方法补实现或改已有函数**，没有新造抽象。✅

> ⚠️ **评审方流程提醒（本轮实际遇到）**：执行方在评审进行中**并发提交了新工作**（B1 的
> 未提交改动落在 `mcp_http.rs`/`mcp_stdio.rs`），导致评审方的门禁跑**混入了 WIP**，一次
> 读数比已提交状态多 2 个测试（256 vs 254）。评审方已按**已提交基线**复核。建议执行方：
> 提交时把状态行与证据一起提交，评审期间尽量不留长时间未提交的改动，或先提交 WIP。
> 评审方今后会在报告里同时记录「HEAD」与「工作区是否干净」。

**环境注意（非项目缺陷）**：本机 Windows + MSVC 14.44 下 dev profile 的 `-Z7` 会让
`ring` 编译失败（`D8050`），加 `CARGO_PROFILE_DEV_DEBUG=0` 可绕过；CI 不受影响。

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

> **评审方预审（2026-11-19 17:49，针对未提交的 WIP，提交后需重新核实）**
>
> 两份文件都已按方案改到位，方向正确：
> - `MAX_TOOL_PAGES = 100`，`loop` 改 `for`，无游标时 `return Ok(tools)`，耗尽时返回错误。
> - `mcp_stdio::request()` 与 `mcp_http::find_event_response()` 都改为**跳过**带 `method`
>   的消息（而非报错），继续等待真正的响应——这比「报错退出」更好，因为同一个连接上
>   可能还有真正的响应在后面。
> - `mcp_http::read_result()` 对裸 JSON body 中带 `method` 的消息**报错**，这是唯一可行的
>   选择（body 只有一个对象，没有流可继续扫）。
> - 新增 4 个测试，名字与断言都对准了原问题（同 id 的诱饵请求 + 恒定游标），
>   评审方实测 **258 通过 / 0 失败**（已提交 254 + WIP 4）。
>
> **两点小建议（非阻塞，提交前可顺手考虑）**：
> 1. `read_result()` 把「收到请求而非响应」归为 `CallError::Transport`（会喂熔断器）。
>    D9 的精神是「HTTP 一旦应答即证明可达」，按此可归 `Protocol`（不熔断）。归 `Transport`
>    也说得通（同一故障会重复发生，隔离掉反而止损）。**两种都可接受，但请在注释里点明
>    选它的理由**，以免后人误改。
> 2. server→client 请求被**静默跳过**。本客户端的 `initialize` 声明 `capabilities: {}`，
>    合规服务器不会发这类请求，因此可接受；但若能顺手记一条日志（哪怕 `--verbose` 才显示），
>    排障时会省很多时间。
>
> 提交后评审方会按已提交状态重新核实并补正式验收证据。

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

- **状态**：待办
- **成本**：约 0.5 天 · **影响**：中
- **问题**：`providers/http.rs` 的重试忽略 `Retry-After`、无 jitter、把 409 当可重试。
- **方案**：读 `Retry-After`（秒数或 HTTP-date）作为退避下限；退避加**随机 jitter**
  （避免多实例同步捶打）；从可重试集合里移除 409。
- **验收**：新增单测覆盖 `Retry-After` 解析（秒 + date 两种）、jitter 落在区间内、
  409 不再重试。
- **关联**：M2。

---

#### B5 · 流内错误与 `finish_reason` 的处理

- **状态**：待办
- **成本**：约 1 天 · **影响**：中
- **问题**：
  1. 流内出现 `{"error": ...}` 时被忽略（不终止、不报错）；
  2. 缺 `finish_reason` 时默认 `Stop`——被截断的流里**已解析出的 tool call 被静默丢弃**。
- **方案**：流内 `error` 对象 → 置 `failed`（与 H1 在 Anthropic 的修法一致）并终止；
  缺 `finish_reason` 且**已有 tool call** 时按「异常截断」处理（报错或标记），不要默认 `Stop`。
- **验收**：新增测试：流中插入 error 对象 → 断言事件序列以 `Error` 结束；流被截断但已含
  tool call → 断言不会静默当成功。
- **关联**：M3。

---

### C 组 · 需要项目负责人决策后再做

---

#### C1 · 信任边界：给项目配置加 opt-in

- **状态**：**待决策**
- **问题**：`rsagent.toml` 可声明 `local` / `mcp-stdio` / `skill` 来源，它们在**装配期、
  任何审批之前**就被启动。`git clone && rsagent --list-tools` 会执行仓库声明的任意命令。
  README/STABILITY 已把这条写成警告，但**没有任何机制**阻止它。
- **三个候选方案**：
  | 方案 | 做法 | 成本 | 代价 |
  | --- | --- | --- | --- |
  | **(a) 仅文档** | 保持现状，加粗警告 | 0 | 风险照旧 |
  | **(b) `--trust-project`** | 首次加载项目层来源时要求确认；CI 用 flag 跳过 | 1 天 | 交互式流程变复杂 |
  | **(c) 只读命令不启动进程** | `--list-tools` / `tools list` / `tools doctor` **不执行** `install()`，只读配置、不 spawn | 0.5 天 | 这两个命令无法报告来源真实健康 |
- **评审方倾向**：**(c) + (b) 的组合**。先做 (c)（成本低、直接堵住「clone 即执行」的最常见
  路径），把 (b) 留到有真实需求时。理由：`--list-tools` 是 README 里推荐的「定位捷径」，
  也是最容易被无意执行的命令；让它只读，收益最大。
- **请项目负责人选定后，把本条的 `状态` 改为 `已决策：<方案>`，执行方再动手。**

---

#### C2 · 记忆蒸馏的限频

- **状态**：**待决策**
- **问题**：每次上下文裁剪都追加一条 `[compacted]` 笔记；该笔记下一轮进入 system prompt、
  **增大**预算占用，可能形成「裁剪 → 写笔记 → prompt 变大 → 再裁剪」的软回路。200 条上限
  能兜底，但回路本身仍在。
- **候选**：(a) 保持现状；(b) 每 N 轮最多一条蒸馏笔记；(c) 蒸馏笔记不进入 system prompt，
  只进 `recall` 可查。
- **评审方倾向**：**(b)**，N=5。成本约 0.5 天，既保留「不丢信息」的核心价值，又切断回路。
- **请项目负责人确认。**

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
- **输出**：每次复评更新 `docs/REVIEW.md`（结论与评分）与本文件的 §1、§2（条目状态）。
- **基线记录**：本文件每次复评后在 §6 追加一行。

---

## 6. 复评历史

| 日期 | 代码基线 commit | 测试数 | 评分 | 主要变化 |
| --- | --- | --- | --- | --- |
| 2026-11-19 | `73fb63b` | 243 | **9.1** | H1–H7 + M16/M17 已修复并核实；建立本清单，提出 A1–A5 / B1–B5 / C1–C3 |
| 2026-11-19 | `e1906b5` | 251 | **9.3** | A1+A2+A3 已完成并核实（`cf67bf8`/`9029cc3`/`e1906b5`）；护栏全过；M5/M10/M14 关闭 |
| 2026-11-19 | `75eb483` | 254 | **9.4** | A4+A5 已完成并核实（`ab178bc`/`75eb483`）；**A 组 5/5 完成**；护栏全过；M12/M13 关闭 |

> **基线口径**：这里记的是**最后一个改动 `crates/` 的提交**，不是 HEAD。
> 查代码基线：`git log -1 --format=%h -- crates/`。
> 评审文档自身的提交（`docs/REVIEW.md`、`docs/ACTIONS.md`）不计入——若两次复评之间
> 只有文档提交，视为「无代码改动」，走轻量路径，不必重跑全量门禁。

### 本轮（`75eb483`）复评记录

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
