# 003 多工具并发的收敛与部分失败

**里程碑** M1 · **依赖** 016 · **模型** sonnet · **独立测试 agent** 否 · **状态** 完成

## 目标

模型一次可能请求 N 个工具（三家实测都支持并行）。实现「全部回来才继续」的收敛，
以及部分失败的处理。

## 做什么

- 每个 tool call 一个槽位，在飞时持 `Pending`
- 收敛条件：所有槽位都不是 `Pending`
- **部分失败不中止**：3 个工具 2 成功 1 失败，把失败当作 `tool_result`（`is_error: true`）
  喂回模型，让它决定要不要紧

## 为什么部分失败不中止

模型比我们更知道这个工具失败要不要紧。一个可选的补充查询失败了就整轮报错，是把
判断权从模型手里抢走。

## 验收

- 3 个工具、1 个失败：loop 继续，错误作为 `tool_result` 进了下一轮的 prompt
- 全部失败：仍然继续（让模型看到全貌再决定），不是直接 `Failed`
- 收敛判断不依赖计数器——用「有没有槽位还是 `Pending`」判，计数器是 undo 之后最容易
  对不上的东西

## 注意

P3 上了原子图之后，收敛判断会变成一个 derived atom（`Pending` 沿依赖图自动汇聚）。
P1 阶段用普通结构，但**判断逻辑要写成能直接搬过去的形状**——即「扫槽位」而非「维护计数」。

## 实做记录（2026-08-01，收尾/补测）

**机制零改动。** 002/016 落地时已经把 003 要的三条机制建完：`TurnState::tools_
converged()`（`crates/agent-core/src/engine/state.rs`）是纯扫描，`on_tool_outcome`
（`crates/agent-core/src/engine/transitions/tool_outcome.rs`）把失败槽跟成功槽
一样拼进 `ContentBlock::ToolResult`（`is_error` 照落），且拼接严格按 `tool_slots`
的顺序（= 模型请求顺序）不是到达顺序。这次收尾没有改 `crates/agent-core/src/`
任何一行，只在 `crates/agent-core/tests/` 新增 4 个文件、9 个测试，把既有机制的
语义钉死；`agent-providers` 只读，零改动。

### 验收 1：3 个工具 1 个失败，错误进下一轮 prompt

既有覆盖：`tests/tool_outcome_convergence.rs::partial_tool_failure_does_not_
abort_the_turn`（003 落地时随 002 一起写的）证明 `is_error: true` 落进了
`ContentBlock::ToolResult`。这次补的是链路剩下那一截——**新增**
`tests/tool_convergence_error_reaches_prompt.rs`：
- `failed_tool_error_text_survives_verbatim_into_the_next_prompt_message`：
  3 槽 1 败，断言失败槽的 `content` 与喂给 `Event::ToolFailed` 的原始错误文本
  逐字节相等（不只是 `is_error` 为真）。
- `multiline_error_text_is_preserved_exactly`：多行/含特殊字符的错误文本同样
  原样保留，跟截断机制（004/决策 19，只在超 32KiB 时触发）是两回事。

**is_error 与 wire 的关系——读了 `agent-providers`，只读没改，裁决如下。**
`crates/agent-providers/src/wire/messages.rs` 第 65–69 行，`push_message` 编码
`ContentBlock::ToolResult` 时用的是 `ContentBlock::ToolResult { id, content, .. }`
——`is_error` 被 `..` 显式丢弃，不进 wire 的 JSON。这不是这次收尾发现的新问题，
是 025 落地时就做过的记录在案的取舍（该文件模块头注释、`docs/issues/025-
provider-seam.md` 实做记录都写着「`ToolResult.is_error` 不进 wire」）。

**裁决：003 和 025 不冲突，因为两者管的不是同一层。** 003 的验收原文是「错误
作为 `tool_result`（`is_error: true`）进了下一轮的 prompt」，这句话在 M1 的落点
是「进了 `TurnState.messages`」——`ContentBlock` 是 provider-agnostic 的 prompt
表示，`is_error` 有没有作为一个独立字段跨进某一家的 wire 字节，是 adapter 层
「这家 wire 协议长什么样」的问题，红线 12 定的接缝就是把这类判断挡在 core 外面。
003 要保证的是**模型看得到这次失败**，不是「`is_error` 这个 Rust 布尔值必须以
某种形式出现在 HTTP body 里」——而模型看到失败靠的是 `content` 里的错误文本，
这份文本原样进了 wire 的 `content` 字段（`push_message` 里 `ToolResult` 分支
`"content": &**content`，跟 `is_error` 是否传是两条独立的赋值），验收 1 因此
成立：**错误进了 prompt，靠的是错误文本本身在 content 里**——issue 原文提示
的读法完全对应实测代码路径。

如果哪天真的需要 `is_error` 本身（而不只是错误文本）影响模型的判断（比如某家
支持 tool 消息带错误标记、想利用它做更精确的重试策略），那是 adapter 一侧
针对那一家的翻译规则要不要跟进的问题，不是 core 或 003 的机制要改——003 的
契约止于「`TurnState.messages` 里有一条 `is_error: true` 的 `ToolResult`」。

### 验收 2：全部失败仍然继续，不是 Failed

002/016 的现状已经是这样（`on_tool_outcome` 里没有任何按失败个数/占比分支的
代码，只问 `slot.state` 是不是 `Pending`），**不需要修**，但之前没有测试单独
钉住这个边界（存量测试只测过「3 个里 1 个失败」）。**新增**
`tests/tool_convergence_all_failed.rs`：
- `all_tools_failing_still_converges_to_thinking_not_failed`：3 槽全败，断言
  `status == Thinking`（不是 `Failed`）、发了 `CallProvider`、三个块全部
  `is_error: true`。
- `a_single_tool_call_that_fails_alone_still_converges`：唯一一个工具调用失败
  的最小复现，同一断言。

### 验收 3：收敛判断不依赖计数器（回滚式测试）

`tools_converged()` 的实现（`state.rs` 197 行起）已经是纯扫描——文档注释原文
「形状是刻意的：扫，不是维护一个计数器」。既有 `tests/tools_converged.rs` 只测
了三个静态形状（全 `Finished` / 一个 `Pending` / 空槽位），没有证明「它是不是
每次都真的扫」（一个偷偷缓存结果的实现一样能让那三个测试通过）。**新增**
`tests/tool_convergence_scan_not_counter.rs`：
- `flipping_a_slot_back_to_pending_immediately_flips_convergence`：issue 原文
  点名的「回滚式」测试——收敛之后不经过 `step()`，直接把一个槽的 `state` 手动
  改回 `Pending`（模拟 undo 回滚了这个槽的回执），断言 `tools_converged()` 立刻
  从 `true` 翻成 `false`，再翻回 `Finished` 立刻恢复 `true`。
- `repeated_calls_without_mutation_are_idempotent`：不改动时连续问三次答案不变
  ——跟上一条合起来才是「扫」的完整形状（对修改敏感 + 对不修改幂等，只有一半
  不足以排除「维护了一个只在某些路径更新的缓存」这种半吊子实现）。
- `appending_a_pending_slot_directly_is_seen_without_going_through_step`：直接
  `push` 一个新槽（不经过任何维护路径），断言立刻被看见——计数器类实现在这里
  最容易漏（`push` 不会触发任何计数维护，但会被下一次扫描看见）。

### 边角 1：重复 `call_id` 不覆盖第一次结果

`tests/tool_outcome_convergence.rs::unknown_or_duplicate_call_id_is_a_protocol_
violation` 测过重复回执违规，但那个测试只有一个槽，第一次落地时就已经收敛、
`tool_slots` 被 `clear()`——第二次重复回执落在的是「`call_id` 在 `tool_slots`
里找不到」这条分支，跟「槽还在但已经是 `Finished`」是同一行判断
（`has_pending_slot` 的 `any(... && matches!(Pending))`）但不同触发条件，
issue 原文点名的「第一次的结果不被覆盖」这层没有被真正走到过。**新增**
`tests/tool_convergence_duplicate_call_id.rs`：
- `second_result_for_an_already_finished_slot_is_a_violation_and_does_not_
  overwrite`：2 槽，call_1 先落地（call_2 还 `Pending`，不收敛），对 call_1
  发第二次、内容不同的 `ToolResult`——断言 `ProtocolViolation`、整个 `TurnState`
  逐字段不变；再让 call_2 落地触发收敛，断言最终拼进消息的 call_1 内容是
  **第一次**的内容，第二次的内容没有留下任何痕迹。
- `second_delivery_via_tool_failed_after_a_successful_first_result_is_also_
  rejected`：同一形状但第二次投递换成 `ToolFailed`（跟第一次的事件类型不同），
  照样拒绝——`ToolResult`/`ToolFailed` 殊途同归（002 判断记录）也包括「拒绝
  重复」这件事。

### 边角 2：乱序回来仍按槽序拼接

已有覆盖，判定不需要新测试：`tests/tool_outcome_convergence.rs::convergence_
happens_only_when_the_last_slot_lands` 就是按「call_2 先于 call_1 到达」构造的
（测试内注释「顺序倒着来，证明收敛看的是槽位状态而不是第几个到」），断言最终
消息的块顺序是 `[call_1, call_2]`（槽序）而不是 `[call_2, call_1]`（到达序）。
这份收尾没有重复造一个等价测试。

### 自测与收工

新增 4 个文件、9 个测试（`tool_convergence_all_failed.rs` 2 个、
`tool_convergence_duplicate_call_id.rs` 2 个、
`tool_convergence_error_reaches_prompt.rs` 2 个、
`tool_convergence_scan_not_counter.rs` 3 个）。`cargo test --workspace`
402/402 全绿（393 + 9）。`cargo clippy --workspace --all-targets -- -D
warnings` 零告警。`bash scripts/check-invariants.sh --all` 通过。

issue 状态维持**待办**不改——这是收尾验证与补测，不是把 003 判定为完成的
決定权在这份记录里；验收三条现在都有测试名可查，缺口（验收 2 无测试、边角 1
的「槽存在但非 Pending」分支未测）已按上面的清单补齐。

### 合并记录（主会话）

纯验证收官：src 零改动，机制 002/016 已建对；补 9 个钉子测试（错误文本逐字节
进 prompt、全败仍收敛、回滚式扫描证明、重复 call_id 不覆盖），402/402。
is_error 与 025「不进 wire」取舍的关系已裁清：不同层，不冲突。
