# 016 停止条件与取消

**里程碑** M1 · **依赖** 002 · **模型** sonnet · **独立测试 agent** 否 · **状态** 完成

## 目标

002 给出的是一台能转的状态机，这个 issue 让它**能停**。

## 做什么

四类停止条件，后续动作各不相同：

| 条件 | 动作 |
|---|---|
| 模型自然结束（`StopReason::EndTurn`） | `Done` |
| 撞 `max_turns` | `Done`，且状态里能看出是被截断的 |
| 用户取消 | 发 `CancelInFlight`，落 `Failed(Cancelled)` |
| 错误 | 按 `ErrorClass` 分流：可重试的退避重来，其余 `Failed` |

`StopReason::MaxTokens`（本次响应被 `max_tokens` 截断）**不是**停止条件——
要不要续写是策略，不是终止。

## 验收

- `max_turns` 到了能停住，且调用方能区分「答完了」和「被截断了」
- 取消在**任意状态**下都生效，且一定发出 `CancelInFlight`——包括工具在飞时
- 错误分流走 `ErrorClass`，不是自己写一套状态码判断
- `MaxTokens` 不会被误当成 `EndTurn`

## 注意

重试的**判断**在 core（它知道错误类型和已重试次数），退避的**节奏**在 transport。
**别把 `sleep` 写进 core**——红线 7，而且那会让「模拟任意超时序列」的测试没法零时间跑。

## 实做记录

落在 `crates/agent-core/src/engine/`：`state.rs` 加了 `turns_used`/`max_turns`/
`retries_used`/`max_retries` 四个字段（默认 32/2）和两个原语方法
`record_turn_attempt`/`record_retry_attempt`（243 行，逼近上限——为腾地方把
`#[cfg(test)] mod tests` 整体搬进 `tests/turn_state.rs`，见判断 6）；
`transitions.rs` 补了 `try_call_provider`（跟已有的 `protocol_violation` 并列的
第二个共用出口）和三个新子模块 `cancel.rs`/`provider_failed.rs`/`timeout.rs`
（55/55/55 行）；`tool_outcome.rs`/`user_input.rs` 的收敛/开局分支改调
`try_call_provider`；`provider_done.rs` 填了 `MaxTokens`/`StopSequence`/`Other`
三个 `stop` 分支，外加 002 留白的「`ToolUse` 无块」那格（135 行）；`notice.rs`
加了 `Notice::Retrying` 一个新变体。全部文件 ≤300 行。

### 转移表的最终形状：10 合法 / 25 非法 / 0 推迟

在 002 定的 4 格基础上新增 6 格合法：`{Idle,Thinking,ToolsPending}+Cancel`、
`Thinking+ProviderFailed`、`{Thinking,ToolsPending}+Timeout`（provider 超时落
`Thinking`，工具超时落 `ToolsPending`，`call_id` 决定走哪条，同一个事件变体
按状态区分）。其余 25 格全部 `Notice::ProtocolViolation`，状态不变但显式可见
——`unimplemented!` 清零。`turn_transitions_grid.rs` 跟着扩到 8 个测试函数
（新增 `provider_failed_legal_only_from_thinking`/
`timeout_provider_leg_legal_only_from_thinking`/
`timeout_tool_leg_legal_only_from_tools_pending`/
`cancel_legal_from_non_terminal_states_violation_from_terminal`，删掉了 002
那个基于 `catch_unwind` 的 15 格 `unimplemented!` panic 测试）。

### 做的判断与理由

1. **`max_turns` 卡的是「发了几次 `CallProvider`」，不是「用户回合数」**：
   新一轮开局（`UserInput`）、工具收敛之后接着调、以及重试，三个入口全部计数
   ——issue 原文「每次 CallProvider 计一轮」是直接指令，不是留白。三个入口
   都改调同一个 `try_call_provider`（新增的第二个共用出口，跟 `protocol_violation`
   并列），不各自重复一份闸判断，理由跟 002 判断 4 相同：散着写等于给这条闸
   开好几个漏判的机会。`try_call_provider` 只在状态**真的变了**才发
   `TurnStatusChanged`（`prev != new` 才发）——重试路径是 `Thinking → Thinking`，
   没有变化，不该喊一声；这也是为什么它能被三种截然不同的调用点直接复用而不用
   传参数说明"这次要不要发通报"。
2. **重试预算（`retries_used`/`max_retries`）与轮数预算（`turns_used`/
   `max_turns`）是两个独立计数器**：前者是「当前这条失败链连续失败了几次」，
   `ProviderDone` 成功时清零（`provider_done.rs` 顶部，紧跟在状态合法性确认
   之后、`push_message` 之前）；后者是「这一整轮总共调了几次 provider」，
   永不清零。拆开的理由：如果合并成一个计数器，一次成功的中间响应会把
   `max_turns` 的进度也一起清零，撞不上真实的轮数上限；如果只留 `max_turns`
   不留 `retries_used`，一条不断重试的失败链会在耗尽 `max_turns` 之前被判定
   为"继续重试"，`Exhausted`/`BadRequest` 之类不可重试错误反而没有独立判断
   （必须靠 `ErrorClass` 分流，不能靠计数器兜底）。两者都在拿到新失败/新调用
   时各自的 `record_*_attempt` 里 `>=` 判断——不用 `checked_sub`/溢出检查，
   跟 `Epoch::next()` 的判断同理（u32 溢出需要 40 亿次才会绕回来）。
3. **重试预算耗尽 → `Failed(Provider(class))`，`class` 是那次触发耗尽的
   `ErrorClass`（含 `Retryable`）**：`Failure::Provider` 的既有文档注释就是
   「按 `ErrorClass` 分流之后判定不该重试」，`Retryable` 但预算耗尽正是这句话
   字面的意思——不需要给"重试次数耗尽"单开一个新的 `Failure` 变体或者
   `ErrorClass` 变体。`retry_or_fail`（`provider_failed.rs`）因此只有一个
   判断分支：`class == Retryable && record_retry_attempt()` 决定重试，其余
   （含 `Exhausted`/`BadRequest`/`Auth`/`Unknown`，含预算耗尽的 `Retryable`）
   全部落同一行 `Failed`。**`Exhausted` 永不重试**是这个形状的推论，不是
   单独写的判断——`Exhausted != Retryable`，天然走不进重试分支。
4. **`Timeout` 的 provider 分支直接复用 `retry_or_fail`，`class` 写死传
   `ErrorClass::Retryable`**：验收原文「`call_id=None` 按 Retryable 走同一条
   重试路」是直接指令。工具分支直接复用 `tool_outcome::on_tool_outcome`
   （超时文案 `"工具执行超时，未在预期时间内返回结果。"` 当 `content`、
   `is_error:true`），不新开一条收敛逻辑——003 的部分失败语义本来就不关心
   "为什么失败"，超时只是失败原因的一种。两个复用都要求 `timeout.rs` 能看到
   `provider_failed`/`tool_outcome` 两个兄弟子模块的 `pub(super)` 项：Rust 的
   `pub(super)` 落在 `transitions` 这一级时，对 `transitions` 的**全部**子孙
   模块可见，不止对 `transitions.rs` 自己——兄弟互调因此不需要放宽任何可见性
   声明，`cargo clippy -D warnings` 也没有为此报任何警告。
5. **`Notice::Retrying` 只在真的要发 `CallProvider` 时才报**：`retry_or_fail`
   决定重试之后调 `try_call_provider`，只有当它返回的 effects 最后一条真的是
   `CallProvider`（不是撞上 `max_turns` 被兜底成 `Done{truncated:true}`）才在
   前面插入 `Retrying` 通报。这个判断（`retry_blocked_by_max_turns_falls_back_
   to_done_truncated`，`provider_error_classification.rs`）是设计过程里唯一
   容易漏掉的边角：重试预算够、但轮数预算不够时，"决定重试"和"真的重试了"
   是两件事，通报只报后者，不然会跟紧跟着落地的 `Done{truncated:true}`
   自相矛盾。**加这个 `Notice` 变体本身是可选项**——016 的验收原文没有点名
   要它，但 001 实做记录明确把"重试通报"的形状留白给 016（"016 补，是加变体
   不是改形状"），`notice.rs` 自己的注释也这么写。权衡之后决定加：字段极小
   （`attempt`/`max_retries` 两个 `u32`），且补上了一个真实的可观测性缺口
   ——不加的话，宿主要知道"这次 CallProvider 是新一轮还是重试"，只能拿两次
   `step()` 调用的返回值自己对照，脆弱且间接。
6. **`state.rs` 的内联单元测试整体搬进 `tests/turn_state.rs`**：016 加完四个
   新字段和两个新方法后，继续内联测试会顶破 300 行硬上限（草稿量过 340+）。
   INVARIANTS.md 红线 9 自己写明"本仓的取向是把集成测试挪到 `tests/`，源文件
   里只留最贴身的单元测试"，而这批测试用到的类型（`TurnState`/`TurnStatus`/
   `SlotState`/`ToolSlot`/`Message`/`Epoch`/`AgentId`/`MessageId`/
   `ContentBlock`/`PrefixImage`/`Role`/`ErrorClass`/`Failure`）本来就全部是
   `agent_core::` 顶层重导出的公开类型，搬家零损失可见性、断言内容逐字未改
   ——这是"搬家"不是"改测试语义"，虽然物理上是把 002 写的测试从 `src/`
   移到了 `tests/`，但没有一条断言的期望值变了。这条我判定不需要在这里
   "逐条说明语义变化"，因为压根没有语义变化；说明写在这里是为了讲清楚
   为什么一份 002 的产物出现在 016 的 diff 里。
7. **终态收到 `Cancel` → `ProtocolViolation`，不是静默 no-op**（issue 原文点名
   要我判断的一格）：终态（`Done`/`Failed`）没有任何东西在飞，`Cancel` 落在
   这里字面上无事可做，"静默返回空"看起来是最省事的答案。但选了
   `ProtocolViolation`，理由三条：(a) 跟"过期 epoch 被闸挡掉"不是一回事——
   过期 epoch 是**必然会发生**的正常时序噪音（取消之后一定有回执陆续到达），
   静默丢弃是对的；`Cancel` 不带 epoch（`Event::epoch()` 对它返回 `None`），
   它是"针对当前世界"表达的用户意图，终态收到它不是必然噪音，是"针对一个
   已经不存在的东西表达意图"，跟 002 判 `Done+UserInput` 非法是同一类问题、
   同一个答案。(b) 002 的验收原文"没有隐式的『忽略』"没有说"除了 016 加的
   格子"——`Cancel` 不该因为是后加的就破例享受静默 no-op。(c) 这不会把宿主
   逼进死角：`Notice` 的文档注释早就写明"012 决定要不要把 `ProtocolViolation`
   升级成致命错误"，宿主真的踩到"双击 Ctrl-C"这种场景，看到并忽略这条通报
   就是了；`cancel_any_state.rs::double_cancel_second_one_hits_the_terminal_case`
   专门测了这个真实场景，断言第二次不 panic、不重复 bump epoch，只是显式
   可见地什么都不做。
8. **非终态的 `Cancel`（`Idle`/`Thinking`/`ToolsPending`）统一处理，不按状态
   分三条路径**：`Idle` 单独拎出来看会觉得奇怪——没有任何东西在飞，取消
   一个还没开始的轮次意味着什么？但 M1 没有"轮结束后自动开下一轮"（002 判断
   记录），`Idle` 只在一个 `TurnState` 生命周期的最开头出现一次，`Cancel`
   在这里的语义就是"放弃这个 `TurnState`，宿主要继续得开一个新的"——跟
   `Thinking`/`ToolsPending` 收到 `Cancel` 的语义（放弃当前这轮，不管当前
   进展到哪）完全一致，没有理由单独分支。`bump_epoch()` 在 `Idle` 时是
   无意义但无害的操作（没有在飞 effect 依赖旧 epoch），`CancelInFlight`
   在宿主那侧也是无意义但无害的指令（没有东西可取消）——"总是发生，多数
   时候是空操作"比"看情况发生"更容易验证、更不容易漏判。`ToolsPending`
   多做一步「槽全弃」（`state.tool_slots.clear()`）：不清的话终态里会留着
   一堆再也不会被回执认领的 `Pending` 槽（epoch 已经 bump，未来的回执会被
   闸挡掉），是自身即会误导的死数据。
9. **`Thinking + ProviderDone` 里"`ToolUse` 无块"（002 留白的形状）判
   `ProtocolViolation`，不是 `Failed`**（issue 原文给的默认选项，我确认了它
   而不是另选）：这不是 provider 报错——没有 `ProviderFailed` 事件，走的是
   成功路径，响应本身自相矛盾（`stop` 说要用工具，`blocks` 里却一个 `ToolUse`
   都没有）。把它塞进 `Failed(Provider(_))` 相当于编一个 provider 没说过的
   错误分类，跟 `Other(_)` 那种"provider 明确说了一个我们不认识的 stop"不是
   同一类问题。选 `ProtocolViolation` 还有一个只有在 016 落地之后才成立的
   理由：之前 002 独立实现时，这一格判 `ProtocolViolation` 会把 `status`
   卡在 `Thinking` 且没有任何路径能再推进（`Cancel`/`ProviderFailed`/
   `Timeout` 当时全是 `unimplemented!`）——如果 002 就选了这个答案，"卡死"
   会是个真问题。016 落地后 `Cancel` 在任意非终态都生效，是这一格万一被
   触发时的逃生舱，"选 `ProtocolViolation` 会不会把宿主逼进死角"这个顾虑
   因此被消解了，这也是我确认这个默认选项而不是改选 `Failed` 的关键论据。
10. **`MaxTokens` → `Done{truncated:true}`，续写策略明确不做**：issue 原文
    「要不要续写是策略不是终止」是直接指令，`Done{truncated:true}` 只是如实
    标记"这次响应被截断了"，跟 `max_turns` 撞顶共用同一个字段——调用方只需要
    看 `truncated` 这一个布尔，不需要分辨"因为轮数还是因为 token 数"。
    **翻案条件**：真要做"自动续写"（M2+）时，这个分支要变成隐式开一个下一轮
    `CallProvider`（还可能要处理输出续接的拼接逻辑），是全新的转移形状，不是
    这一行改一下能带出来的；现在没有任何 M1 代码需要它，`agent-cli` 目前也
    没有调用 `engine::step` 的路径（`grep` 确认过），不存在"提前做"的驱动力。
11. **`StopSequence` → `Done{truncated:false}`，`Other(_)` →
    `Failed(Provider(Unknown))`**：前者是配置生效（模型在配置好的停止点
    停下），语义上等价于"答完了"，跟 `EndTurn` 的唯一区别只在 `truncated`
    该填 `false`。后者复用 `ErrorClass::Unknown` 既有的定义（"没认出来，
    保守处理，不自动重试"）——"没认出来"的对象从"HTTP 错误"换成了
    "finish_reason"，判据一致，不是借用错了字段；不认识的 stop 当成功处理
    会静默吞掉一段可能被截断/出错的回复，这是 issue 原文点名的理由。

### 自测

新增/改动测试，`cargo test --workspace` 393/393 全绿（002 合并时的 366 +
016 净增 27）：

- `crates/agent-core/tests/turn_state.rs`（新建，10 个）：002 原有 6 个测试
  原样搬入 + 016 新增 4 个（默认上限、`record_turn_attempt`/
  `record_retry_attempt` 各自的撞顶行为、`max_turns=0` 的边界）。
- `tests/turn_transitions_grid.rs`（8 个，002 的 5 个减 1 加 4）：删掉基于
  `catch_unwind` 的 15 格 `unimplemented!` panic 测试，新增
  `provider_failed_legal_only_from_thinking`/`timeout_provider_leg_...`/
  `timeout_tool_leg_...`/`cancel_legal_from_non_terminal_states_...`。
- `tests/provider_done_stop_reason.rs`（6 个，语义改动）：`tool_use_claimed_
  without_blocks_panics_with_002` → `..._is_a_protocol_violation`；
  `max_tokens_panics_with_016`/`stop_sequence_panics_with_016`/
  `unknown_stop_reason_panics_with_016` 三个从"断言 panic"改成"断言真实
  转移结果"（判断 9/10/11 的对应测试）。
- `tests/provider_error_classification.rs`（新建，5 个）：四个非 `Retryable`
  类立即失败且不耗预算、`Exhausted` 永不重试（哪怕预算异常充裕）、
  `Retryable` 重试到耗尽为止、`ProviderDone` 成功清零重试计数、重试预算够但
  `max_turns` 不够时的兜底行为。
- `tests/timeout_transitions.rs`（新建，5 个）：provider 超时复用同一条重试
  预算、工具超时落地为 `is_error:true` 的结果、多槽部分超时不中止、未知
  `call_id` 的超时事件是违规、超时文案逐字节确定。
- `tests/cancel_any_state.rs`（新建，5 个）：`Idle`/`Thinking`/`ToolsPending`
  三态各自的取消行为（含 `CancelInFlight` 带的是取消前的旧 epoch）、
  `ToolsPending` 的槽全弃、终态违规且不改任何字段（含 epoch）、连续两次取消。
- `tests/max_turns.rs`（新建，5 个）：默认上限 32、工具收敛之后撞顶改落
  `Done{truncated:true}`、答完与截断可区分、`max_turns=0` 边界、`turns_used`
  精确随每次 `CallProvider` 递增。
- `src/engine/notice.rs`：`roundtrip_all_variants` 加了 `Notice::Retrying`。

`cargo clippy --workspace --all-targets -- -D warnings` 零告警，
`bash scripts/check-invariants.sh --all` 通过，`wc -l` 全部 ≤300（含
`transitions/` 子模块，最大的是 `event.rs` 254 行，016 没有改动它）。

### 合并记录（主会话）

表 10 合法 / 25 非法 / 0 推迟，workspace 393/0。裁决全收：终态 Cancel 判
ProtocolViolation（一致性 + Cancel 自身是逃生舱）；MaxTokens 诚实截断、续写
留 M2+ 且写了翻案条件；Other(_) 保守判 Failed(Unknown)——没见过的 stop 当
成功会静默吞半截回复。max_turns 轮数闸落地 = 死循环的第一道事前闸就位
（ROADMAP §四预算闸那条的前半）。
