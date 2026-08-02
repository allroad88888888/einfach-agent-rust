# 002 turn 状态转移表

**里程碑** M1 · **依赖** 001 · **模型** sonnet · **独立测试 agent** 否 · **状态** 完成

## 目标

实现 `TurnStatus` 的完整流转。

## 做什么

状态转移表：`Idle → Thinking → ToolsPending → Done | Failed`。

**这个 issue 只做转移本身**，让状态机能转起来。四类停止条件（`max_turns` / 取消 /
错误分流 / 自然结束）在 [016](016-stop-conditions.md)——分开是因为两者能各自独立验证：
这里验「每个 (状态, 事件) 组合都有明确结果」，那里验「该停的时候停得住」。

## 验收

- 转移表穷举测试：每个 (状态, 事件) 组合都有明确结果，**没有隐式的「忽略」**
- 非法转移是显式错误，不是静默留在原状态

## 注意

重试的**判断**在 core（它知道错误类型和已重试次数），退避的**节奏**在 transport。
别把 `sleep` 写进 core。

## 实做记录

落在 `crates/agent-core/src/engine/`：`mod.rs` 的 `step()` 换成「过 epoch 闸 →
交给 `transitions::transition`」；`state.rs` 加了 `next_message_id` 字段和
`mint_message_id` / `push_message` 两个辅助（297 行，逼近上限，`finish_slot`
因此没放这里，见下）；新建 `transitions.rs`（86 行，入口 `transition` +
`protocol_violation` 共用出口）+ 子模块 `transitions/{user_input,provider_done,
tool_outcome}.rs`（25/132/112 行）——四个文件按「谁触发的转移」拆分，互不调用，
只共用 `protocol_violation`（one-file-one-thing 的引用聚类测试：这是三坨不是
一坨）。`notice.rs` 加了 `Notice::ProtocolViolation { state, event }` 一个新变体
（`event` 是 `format!("{event:?}")`，不是结构化类型）。全部文件 ≤300 行。

### 转移表的最终形状：5 × 7 = 35 格

| | `UserInput` | `ProviderDone` | `ProviderFailed` | `ToolResult` | `ToolFailed` | `Timeout` | `Cancel` |
|---|---|---|---|---|---|---|---|
| `Idle` | **合法**→`Thinking` | 违规 | 推迟 016 | 违规 | 违规 | 推迟 016 | 推迟 016 |
| `Thinking` | 违规 | **合法**（子分支见下） | 推迟 016 | 违规 | 违规 | 推迟 016 | 推迟 016 |
| `ToolsPending` | 违规 | 违规 | 推迟 016 | **合法**（子分支见下） | **合法**（子分支见下） | 推迟 016 | 推迟 016 |
| `Done{..}` | 违规 | 违规 | 推迟 016 | 违规 | 违规 | 推迟 016 | 推迟 016 |
| `Failed(..)` | 违规 | 违规 | 推迟 016 | 违规 | 违规 | 推迟 016 | 推迟 016 |

**4 格合法、16 格非法（`ProtocolViolation`，状态不变但可观测）、15 格推迟
（`unimplemented!`，panic 消息里带 "016"，覆盖全部 5 态——`ProviderFailed` /
`Timeout` / `Cancel` 整类事件 002 一个字都没定义该怎么转，猜一个形状比不做更
危险：宿主会真的把它当成处理过）。**

`Done{ truncated }` 与 `Failed(Failure)` 两个终态内部各有子变体，转移表按
「终态」一并处理——终态不接受任何事件，包括 `UserInput`（issue 原文点名的例子：
`Done + UserInput` 非法，M1 没有「轮结束后自动开下一轮」，宿主要开新轮得先有个
新的 `TurnState`，这次没有定义那个机制，也不该由 002 顺手定义）。

`ProviderDone` / `ToolResult` / `ToolFailed` 三个「合法」格子内部还有子分支，
不算在上面 35 格里（那 35 格问的是「(状态,事件) 这个组合本身站不站得住」，
子分支问的是「事件的内容说得通吗」，是两个维度）：

- **`Thinking + ProviderDone`**：无条件先把回复落进历史、`prev_prefix` 用
  `usage.prompt` 回填（001 判断 2，纯赋值），再按 `stop` 分：
  - `EndTurn` → `Done{truncated:false}`
  - `ToolUse` 且有 `ToolUse` 块 → 按块顺序开槽 → `ToolsPending`，每槽一个
    `ExecuteTool`
  - `ToolUse` 但没有 `ToolUse` 块（响应自相矛盾）→ `unimplemented!("002：...")`
    ——这个形状 002 也没定义，不猜，留给下一个碰到它的人
  - `MaxTokens` / `StopSequence` / `Other(_)` → 各自 `unimplemented!("016：...")`
    ——016 的验收原文点名 `MaxTokens` 不是停止条件，`StopSequence`/`Other` 同样
    是「算不算终止」的分类问题，不是 002 的转移机制问题
- **`ToolsPending + ToolResult`/`ToolFailed`**：先确认 `call_id` 对应一个
  `Pending` 槽——不对应（未知或重复回执）→ `ProtocolViolation`，不是「等其余槽」
  也不是 panic。对应的话：内容过 `truncate_tool_output`，截断了就
  `Emit(ToolOutputTruncated)`；槽落地为 `Finished`；`tools_converged()` 为真才
  把全部槽按**槽序**（模型请求顺序，不是到达顺序）拼成 `ContentBlock::ToolResult`
  （失败的也照拼，`is_error:true`，003：部分失败不中止）→ 清空槽位、消息进历史、
  `ToolsPending → Thinking`、发 `CallProvider`。未收敛 → 状态不变，`effects`
  最多只有那条截断通报——不是隐式忽略，是「等其余槽」。

### 做的判断与理由

1. **`MessageId` 铸造**：`TurnState` 加 `next_message_id` 字段，`mint_message_id`
   从 1 起严格递增，`push_message` 铸号+建 `Message`+追加一步到位。规则跟着
   `crates/agent-cli/src/repl.rs` 现有的 `next_id: u64` 铸法走（仓库里唯一的
   先例），不是另造一套；009/010 定案「确定性铸号」的最终规则时只用改
   `mint_message_id` 一处。
2. **`ExecuteTool.request` / `ToolSlot.request` 的 `location`/`reversibility`
   从哪来（issue 点名要做的判断）**：M1 没有工具表进 `agent-core`，`ToolUse`
   块只有 `name`/`input`。原计划的两条路都试了：
   - 「state 里放工具表」——001 明确没给这个字段，M1 宿主的工具表活在
     `agent-cli::TurnContext`，跟 `SessionConfig`/`system`/`tools` 同一类
     （001 判断已经论证过不重复存）。
   - 「`request` 降级成『名字+输入』，宿主查表补全」——这是倾向的方向，
     TOOLS.md 原文也支持（「`agent-core` 只发 `ToolCall`，不认识前端/后端」）。
     但要做到就得改 `ToolCallRequest`（`value/tool.rs`）或者
     `Effect::ExecuteTool` 的字段类型，两个都在 engine 外，这次改动的范围
     明确不许碰。「宿主补全后回填」也绕不开：**没有事件能把补全结果写回
     `TurnState`**（001 定的 7 个事件里没有这一种，`Event` 的变体集合也是
     这次不许动的契约），所以回填不了状态，只能回填宿主自己另存的一份。
   - **结论：都绕不开，改的是「值」不是「类型」。** `ToolCallRequest` 的字段
     形状完全没动，仍然是 `{tool, input, location, reversibility}`；`step()`
     内部造一个**占位快照**：`location` 固定 `Server`（M1 唯一真实存在的位置，
     013：`fs/read`/`fs/list` 都是 `Server`），`reversibility` 固定
     `Irreversible`（TOOLS.md「拿不准就是 Irreversible」——这里不是偶尔拿不准，
     是结构性拿不准）。宿主必须按 `request.tool`（工具名）自己查表得到真实值
     用于路由，不能信任这两个字段；`ToolSlot` 里落的这份快照因此是**已知不准**
     的技术债，工具表真正进状态（很可能是 011 或一个新 issue）之前必须解决，
     不能假装已经解决。**契约的公开签名（`Effect`/`Event`/`Notice`/`TurnStatus`
     的变体集合，以及 `ToolCallRequest` 的字段）一个字节都没改**，这点跟
     「允许动」的 `Notice` 新变体不是一回事，特此标注：**这里最终没有动契约**，
     只是在契约允许的范围内选了一个显式标注为占位的值。
3. **`Notice::ProtocolViolation` 的 `event` 字段是 `format!("{event:?}")`**，
   不是结构化的 `EventKind` 枚举。消费者是人和日志（014 打印、012 判断要不要
   升级成致命错误），不是要拿它做分支判断的程序逻辑；引入一个新的可序列化
   公开类型换不来什么，issue 只批了「一个新变体」的预算，没批「一个新类型」。
4. **`ProviderFailed`/`Timeout`/`Cancel` 在 `transition()` 入口处按事件种类
   整体拦截**，不下沉到每个状态分别写 `unimplemented!`——016 的验收原文是
   「取消在**任意状态**下都生效」，拦截点在事件种类而不是状态，形状本身就在
   替 016 说话：这三种事件的转移*完全*不依赖当前状态（016 接手时大概率也是
   「不管哪个状态都先 bump epoch / 分错误类」），下沉成 15 个几乎相同的
   `unimplemented!` 分支只会让 016 的 diff 更难看清改了什么。
5. **未知 `call_id`（含重复回执）判为 `ProtocolViolation` 而不是 panic**：
   跟过期 epoch（被闸挡掉，静默丢弃，不发通报）不是一回事——过期 epoch 是
   *正常*的时序噪音（取消之后一定有回执陆续到达），未知/重复 `call_id` 不是
   正常噪音，是「这条消息在当前世界里找不到对应」，跟其余 16 格非法组合同一
   处理方式：显式、可观测、状态不变。

### 自测

新增 24 个测试，`cargo test -p agent-core` 全绿（既有 59 + 本次 24，另有并行
的 024 落地的 `cache` 模块测试，与本 issue 无关）：

- `crates/agent-core/src/engine/state.rs`：`mint_message_id`/`push_message` 各一个，
  `turn_state_roundtrip` 补上新字段。
- `tests/turn_transitions_grid.rs`（5 个）：`UserInput`/`ProviderDone`/
  `ToolResult`/`ToolFailed` 各自「只在一个状态合法，其余四态 `ProtocolViolation`
  且状态逐字段不变」；`ProviderFailed`/`Timeout`/`Cancel` 在全部 5 态下都
  `unimplemented!` 且消息含 "016"（`catch_unwind` 一次过 15 格，配 `Drop` 守卫
  保证 panic hook 无论如何都换回来，不吞掉别的测试的真实失败）。
- `tests/provider_done_stop_reason.rs`（6 个）：`EndTurn` 收尾 + `prev_prefix`
  回填、`ToolUse`+多块按序开槽（含占位 `location`/`reversibility` 的回归断言）、
  `ToolUse` 无块 panic "002"、`MaxTokens`/`StopSequence`/`Other` 各自 panic "016"。
- `tests/tool_outcome_convergence.rs`（5 个）：收敛只在最后一个 `Pending` 槽
  落地时发生（先落非最后一个槽，断言 `effects` 为空、历史不动）、部分失败不
  中止（003）、>32KiB 截断（通报字段 + 消息里是截断文本）、未知/重复 `call_id`
  违规、`MessageId` 跨一整轮严格递增。

`cargo clippy -p agent-core --all-targets -- -D warnings` 零告警，
`bash scripts/check-invariants.sh --all` 通过，`wc -l` 全部 ≤300（含
`transitions/` 子模块）。

### 合并记录（主会话）

35 格穷举收下（4 合法 / 16 非法→ProtocolViolation / 15 推迟 016），但**占位快照
那笔技术债不收，合并时直接做了契约修正**：`Effect::ExecuteTool` 与 `ToolSlot`
瘦身为只带 `tool` + `input`。理由：core 没有工具表，现造 `location=Server /
reversibility=Irreversible` 是编造数据——M1 碰巧无害，M2 的 undo 会因假
`Irreversible` 白拦 `fs/read`，正是静默错值类。「发起时快照」原则不变，记录点
在宿主/command 层（它持有注册表，009 的 `Entry` 在那记）。实现 agent 受
「不许动 engine 外文件」的边界约束选了占位方案并如实上报——**边界设置是主会话
的失误，上报是对的**。修正连带 001 契约文档与 6 处测试，workspace 366/0。
