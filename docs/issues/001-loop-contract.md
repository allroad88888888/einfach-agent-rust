# 001 定 loop 的事件与 effect 契约

**里程碑** M1 · **依赖** 022 · **模型** opus · **独立测试 agent** ✅ · **状态** 完成

## 目标

把「core 决定该发生什么，adapter 决定怎么发生」这条划分落成具体类型：一个纯函数式
状态机 `step(state, event) -> (state, Vec<Effect>)`。

## 为什么

`Effect` 是**描述**不是执行。core 不调 HTTP、不跑工具、不读时钟，只产出「请去调这个
provider」「请去执行这个工具」。这是红线 7 能成立的原因——不是刻意约束，是这个形状的
自然结果。带来的好处很实在：整个 loop 可零 IO 单元测试、超时可模拟、状态机可穷举。

## 做什么

在 `crates/agent-core/src/engine/` 下定义两个枚举与 step 的签名。

core 发出的 effect（至少这些）：

```
CallProvider { agent, epoch }
ExecuteTool { agent, call_id, request, epoch }
SpawnChild { parent, id, definition }
CancelInFlight { epoch }
Compact { agent, strategy }
Emit { event }
Persist { entries }
```

**`CallProvider` 里没有 payload**，这是决策 15 的直接后果：请求由 adapter 组装，
core 说的是「该调了」不是「照这个调」。宿主收到这个 effect 后，在 actor 线程上
让 adapter 从状态取料、按自己的能力组装，产出能跨线程带走的东西再发出去。
接缝见 [../ADAPTER.md](../ADAPTER.md)。

上一版在 core 里写了个 `build_request()` 想把 payload 塞进来，做出来的是个不做任何
模型相关判断的搬运函数。**effect 变胖是接缝错位的第一个症状。**

同理，事件那边要能带回 adapter 的**调整报告**（决策 17）：`ProviderDone` 里有
`adjustments: Vec<Adjustment>`，core 不事先问「你能不能强制指定工具」，
只在事后知道「它降级了」。

core 接收的事件：

```
UserInput / ProviderChunk / ProviderDone / ProviderFailed
ToolResult / ToolFailed / ChildFinished
Undo / Redo / Cancel
Timeout { call_id, epoch }
```

## 验收

- 两个枚举定义完整，`step` 签名确定
- **core 里没有 `Instant::now()`**：超时是注入的 `Timeout` 事件，计时器活在外面。
  于是测试能在零时间内模拟任意超时序列
- `cargo test -p agent-core` 无网络通过

## 注意

`Effect` 里不能出现不可序列化的活对象（红线 3 的精神）——`ExecuteTool` 带的是
`ToolCallRequest` 快照，不是执行句柄。

红线 12：`step()` 里一条模型相关判断都不许有。转移表对三家必须完全一致——
**如果写着写着需要「这家不一样」，那是接缝漏了，回去改 adapter，不是在这里加分支。**

## 实做记录

落在 `crates/agent-core/src/engine/`：`event.rs`（254）/ `state.rs`（246）/
`effect.rs`（141）/ `mod.rs`（177，含 `step` 与 epoch 闸）/ `notice.rs`（85）/
`epoch.rs`（66），另在 `ids.rs` 加了 `AgentId`。全部 ≤300 行。

### 定了什么

| 词汇 | 变体 |
|---|---|
| `Effect` | `CallProvider { agent, epoch }`（无 payload）/ `ExecuteTool { agent, call_id, request, epoch }` / `CancelInFlight { epoch }` / `Emit(Notice)` |
| `Notice` | `TurnStatusChanged { status }` / `ToolOutputTruncated { call_id, original_bytes, kept_bytes }` |
| `Event` | `UserInput` / `ProviderDone`（带 `adjustments` + `prefix`）/ `ProviderFailed { class: ErrorClass, .. }` / `ToolResult` / `ToolFailed` / `Timeout { call_id: Option<_>, .. }` / `Cancel` |
| `TurnState` | `agent` / `status` / `messages: imbl::Vector<Message>` / `tool_slots: Vec<ToolSlot>` / `epoch` / `prev_prefix` |
| `TurnStatus` | `Idle` / `Thinking` / `ToolsPending` / `Done { truncated }` / `Failed(Failure)`，`Failure = Cancelled \| Provider(ErrorClass)` |

`step(&mut TurnState, Event) -> Vec<Effect>`：**签名用 `&mut` 而不是返回新状态**——
语义仍然是纯的（不读时钟、不做 IO、同输入同输出），只是不想每步 clone 整份历史；
M2 上原子图后这里变成 command 层的调用点，形状不用改。

### 推迟了什么、推给谁

- **`SpawnChild` → 006（M3）**：谁 spawn 都没拍板，两种形态字段不同。
- **`Compact` → 决策 18（M2/M3）**：压缩是状态变更、要走 command 层进 undo log，M1 没有 store；阈值也还是 ROADMAP §四的未决问题。
- **`Persist` → 011（M2）**：落盘单位是 `Entry`（009 定），现在连 `Entry` 都不存在；012 已写明 M1 阶段丢弃。
- **`Undo` / `Redo` → 017（M2）**：没有 undo log，`Undo` 唯一能做的（bump epoch）没有任何东西可回滚。
- **`ChildFinished` → 006（M3）**：没有子 agent；且「等子 agent 完成」在原子图上是 derived atom，未必长成事件。
- **`Cancel` 的转移（`CancelInFlight` + bump） → 016**：016 的验收原文是「取消在**任意状态**下都生效」，那是转移表的一格，单一所有者应该是 016。**闸本身（丢过期）这次就做了**，bump 的原语是 `TurnState::bump_epoch()`。
- **重试通报 → 016**：重试几次、按哪些 `ErrorClass` 重试都是 016 的设计，现在定通报只能猜字段。
- **缓存兜底告警 → 024**：判读输入（drift / predicted_cache / usage）宿主全部持有，第 1 层甚至发生在请求发出**之前**——那时 loop 还没被 step 过，走 `Emit` 出不来。
- **`max_turns` / 已用轮数 → 016**、**`SessionConfig` / `system` / `tools` → 011**（M1 宿主 `TurnContext` 已持有且会话中途不变）、**`RequestIntent` 的存放处 → 有真实 `MustUse` 场景时**（M1 恒 `Free`）：都不进 `TurnState`。
- **`AgentId` 的路径语义 → M3**：类型定成 `Arc<str>`（STATE-MODEL 的路径编码 `root/a1/a1.2`，换类型的代价会摊到所有事件/effect/快照上），但 `is_ancestor_of` 之类判定不给——现在写等于凭空猜规则，且没有任何 M1 代码会调。

### 做的判断与理由

1. **流式增量不是事件**（推翻了 001 清单里的 `ProviderChunk` 和 012 那句「流式增量转成
   `ProviderChunk` 事件」）。三条理由：(a) 它不改任何状态、不产任何 effect——累积器活在
   宿主那边（ADAPTER.md §时序），`Message` 又写死了「只放完成的消息」，喂进来只能原样弹回
   一条 `Emit`，一轮几千次空转；(b) 它会在 002 的穷举表里凿洞——002 的验收是「没有隐式的
   『忽略』」，一个天生什么都不做的事件在每一行都只能写「忽略」；(c) 014 的「流式实时打印」
   在宿主的流回调里就能做，022 的 CLI 已经在做。**翻案条件**：core 哪天要基于半截生成做
   决策（看到前几个 token 就抢跑取消），那时 chunk 才第一次有转移可写。
   **012 落地时注意：增量打印保持在 runner 里，不要为它加事件。**
2. **`ProviderDone` 带 `prefix`**。宿主要「从状态取料」组装（012），`Ingredients::prev_prefix`
   就得在状态里；而镜像是 adapter 在 `encode` 时产出的，只能经事件进来。core **只存不判读**
   （ADAPTER.md），`prompt_tokens` 用 `usage.prompt` 回填是纯赋值不是判断。
3. **`ToolResult` 与 `ToolFailed` 分成两个事件**。宿主那边本来就是 `Ok`/`Err` 两条路径，
   合并成一个 `is_error` 布尔会逼它现造错误字符串；core 这边两者殊途同归，都变成一条
   `ContentBlock::ToolResult`（失败的 `is_error: true`），符合 003「部分失败不中止」。
4. **`Timeout.call_id` 是 `Option`**：`None` = provider 调用超时，`Some(id)` = 那个工具超时。
   两者转移不同（前者是可重试失败，后者只让一个槽落地），不分就得在 016 里猜。
5. **`Notice` 只定两条**，判据是「宿主自己看不见的才在这里」：drift / predicted_cache /
   usage / adjustments / 增量文本宿主全都有，绕一圈进 core 再发回来是白走一趟。
   `ToolOutputTruncated` 反过来只有 core 说得出——截断在 core 边界做（决策 19），
   不打出来的话「人看到 10MB、模型看到 32KiB」这件事查起来会先怀疑模型。
   `TurnStatusChanged` 同时是 **loop 说「停」的唯一出口**：effect 列表为空是歧义的
   （可能在等在飞结果，也可能是过期事件被闸挡了），runner 靠 `TurnStatus::is_terminal()` 收工。
6. **「答完了 vs 被截断了」放状态不放通报**（`Done { truncated }`）：016 的验收原文是
   「**状态里**能看出」，而且事后任何时候都该问得出，通报是一次性的。
7. **收敛判断是 `tools_converged()` 扫槽位**，槽位状态用显式的 `SlotState::Pending` 而不是
   `Option::None`。计数器是 undo 之后最容易对不上的东西（回滚了槽位没回滚计数，不报错）；
   扫的形状 M2 能原样搬成 derived atom，计数的搬不过去（003 的注意条）。
8. **闸判 `!=` 不判 `<`**：epoch 只增不减，未来的 epoch 同样只可能来自不该存在的世代。
   **过期事件不发通报**——取消之后一定有一批回执陆续到达，这是正常现象，每条喊一声只会刷屏。
9. **`Emit(Notice)` 不带 `agent`**：M1 单 agent，CLI 打印时无处可用；M3 多 agent 并行输出
   要分辨「谁说的」时再加。其余 effect 带 `agent` 是 issue 原文要求的路由字段。
10. **effect 里的 `agent` 取自 `TurnState.agent`，不取自事件**：事件是宿主喂进来的，
    让它决定 effect 发给谁等于把路由权交出去。

### 自测

`cargo test -p agent-core` 35 绿（新增 12 个）：四个词汇各自 serde 往返、
`CallProvider` 序列化后 key 只有 `agent`/`epoch`（决策 15 的最小实检）、
`Event::epoch()` 逐变体穷举、过期 `ProviderDone`/`ProviderFailed`/`ToolResult`/
`ToolFailed`/`Timeout` 全部丢弃且状态逐字节不变、bump 后旧世代回执立刻作废、
`tools_converged()` 三种输入、终态判定穷举。
`cargo clippy -p agent-core --all-targets -- -D warnings` 零告警，
`scripts/check-invariants.sh --all` 通过。

> 收工时 `cargo test --workspace` 编不过，原因在 `agent-providers`：并行的 023 正在
> 加 `glm` / `kimi` 两个模块，`lib.rs` 已经 `pub mod` 但目录还是空的。与本 issue 无关，
> `agent-core` 单独全绿。

### 合并记录（主会话）

独测 24 个新测试一次全绿（agent-core 59/59），与实做记录零分歧。独测的接口探知
只用 `cargo doc` 产物 + 一次性探针测试，未读 engine 源码——这个办法记下来，
后续独测照抄。顺带发现并已修检查脚本两个缺口：红线 7 的 IO 检查现在
(a) 豁免 `*/tests/*`（元测试跑脚本是正当 IO），(b) 同时抓全限定调用
（`std::fs::read(...)` 不写 use 也能用，只查 use 红线就是摆设）。

### 契约修正（002 合并时）

`Effect::ExecuteTool { request: ToolCallRequest }` → `{ tool: Arc<str>, input:
Arc<Value> }`。原形状要求 core 给出 `Location`/`Reversibility`，而 core 没有
工具表——「带快照不带句柄」的原则保留，但快照的构造与记录归持有注册表的宿主
/command 层（M2 009）。`ToolCallRequest` 类型本身保留在 value/tool.rs 给那时用。
