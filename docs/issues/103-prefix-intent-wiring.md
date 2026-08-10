# 103 `PrefixIntent::Intentional` 接线

**里程碑** M12 · **依赖** [102](102-clear-tool-results-policy.md) · **模型** sonnet · **独立测试 agent** 是 · **状态** 完成

## 目标

压缩那一轮（第 2、3 档都算）把 `PrefixIntent` 置成 `Intentional`，
让缓存兜底第 1 层把预期内的漂移判成 `Expected` 而不是 `Unexpected`。

## 为什么这不是可选的

`cache/drift.rs:21` 那段注释就是为今天写的：

> M1 恒为 `PrefixIntent::Reuse`：还没有任何一处会有意改前缀。压缩重写历史、
> 换 skill 集、晚加的工具被并进顶层，都是后面才出现的 `Intentional` 来源。
> 字段现在就留出来，是因为漏了它的那一天，表现是「**压缩一次报一次假警报**」，
> 然后人开始无视这一层的告警。

漏了它不报错、不影响功能——只是把一个真能救命的告警变成噪音。M12 是第一个真的会
有意改前缀的里程碑，这条到期了。

## 做什么

在压缩生效的那一轮把意图传成 `Intentional`，其余轮次维持 `Reuse`。
第 2、3 档共用同一条路径，不各写一份。

## 定死的接口（2026-08-10 主会话定）

### 怎么知道「这一轮有意改了前缀」

**比较这一轮用的 `SendPlan` 和上一次请求用的那份。** `SendPlan` 只被 command
改（101/104/107），所以「跟上次请求时不一样」⟺「这中间压缩开过火」。

为此**新增一个槽位** `Slot::PrevSendPlan`：存「上一次 `CallProvider` 时用的那份
`SendPlan`」，**在 `PrevPrefix` 被写的同一个地方一起写**（两者是同一件事的两半：
上一次发出去的长什么样）。默认值 = pristine。

```rust
impl Session {
    /// 上一次发请求时用的发送计划。从没发过 → pristine。
    pub fn prev_send_plan_of(&self, agent: &AgentId) -> SendPlan;
}
```

判定：

```rust
let intent = if session.send_plan_of(&agent) == session.prev_send_plan_of(&agent) {
    PrefixIntent::Reuse
} else {
    PrefixIntent::Intentional
};
```

接线点是 `crates/agent-runtime/src/provider_call.rs:190`
（现在写死 `PrefixIntent::Reuse`，注释还说「M1 恒 Reuse」——那句要改掉）。

**为什么不用一个「本轮压缩过」的 bool 标志**：那个标志要在每次请求后清掉，
清就是状态变更、就要进 undo log，等于每请求多一条 entry。比较法一个额外写都不需要
——`PrevSendPlan` 跟 `PrevPrefix` 本来就要在同一时刻更新。

**为什么不用运行时内存里的标志**：崩溃恢复之后标志丢了，恢复后的第一轮会误报一次
`Unexpected`。持久化的槽位没有这个问题。

### 槽位记账

`Slot::ALL` 16 → 17，可见性 `Private`（跟 `PrevPrefix`、`SendPlan` 同类）。
六个槽位计数触发线测试要跟着从 16 改到 17——**那是应尽的记账，不是「改 golden」**
（093 加 `ExecutionProfile`、100 加 `SendPlan` 都做过同样的事）。

## 验收

- 第 2 档开火那轮：第 1 层判 `DriftVerdict::Expected { segment: History }`，
  **不是** `Unexpected`
- 第 3 档开火那轮：同上
- **紧接着的下一轮**（没有压缩）：意图回到 `Reuse`；此时若仍有漂移，
  判 `Unexpected` 并告警——**这条是反向锁**，防止有人图省事一直传 `Intentional`
  把这一层永久关掉
- Tools / System 段在压缩轮**不漂**（压缩只动 History）
- **压缩轮单独不触发 `ChronicMiss`**：一轮压缩（低命中）+ 紧跟一轮正常高命中 →
  不告警，证明一次性的压缩代价不会被误判成慢性失效
  - **⚠️ 原文这条写的是「一次压缩后紧跟两轮正常低命中不触发 `ChronicMiss`」，
    那是错的**：`cache/window.rs:130` 只让**失明轮**（`TurnHit::Blind`，provider
    根本没报 `cached`）不打断连续性；压缩轮报的是 `cached: Some(0)`，是
    `TurnHit::Observed`、是实打实的低命中轮，**本来就该计入**。而且那是对的设计
    ——压缩完还连着两轮低命中说明这次压缩没起作用，正是慢性失效该报的

## 注意

- 这条本身不碰红线 1–6/11/12，但**漏了它的后果跟红线同级**（静默地让告警失效），
  所以照样派独立测试 agent
- 反向锁那条验收是本 issue 的核心。只测「压缩轮不告警」的话，
  一个「永远传 `Intentional`」的实现也能全绿
- 第 2 层（对账）不用改：压缩轮 adapter 预测 0、实际 0，对得上。
  GLM 上实际可能远好于预测，落 `BetterThanExpected`，信息级不告警
  （`wire/prefix.rs` 模块文档「保守低估」那段）

## 实做记录（实现 agent + 独立测试 agent 并行，2026-08-10）

与 [106](106-summary-via-subagent.md) 同时开工（`provider_call.rs` 归本条、
`dispatch.rs` 归 106）。

### 落地的文件

| 文件 | 行数 | 职责 |
|---|---|---|
| `agent-core/src/graph/slot.rs` | 295（+26） | `Slot::PrevSendPlan` + 默认值（pristine，复用 `Slot::SendPlan` 的 codec）+ 进 `Slot::ALL`（16→17） |
| `agent-core/src/graph/visibility.rs` | 191（+9） | 归 `Private`，同 `PrevPrefix`/`SendPlan` |
| `agent-core/src/command/txn.rs` | **300**（+14） | `Txn::record_prev_send_plan()` |
| `agent-core/src/command/read.rs` | 271（+11） | `prev_send_plan_of`，紧挨着 `prev_prefix_of`（它的另一半） |
| `agent-core/src/command/transitions/provider_done.rs` | 124（+4） | 在 `set_prev_prefix` 之后立刻记 `PrevSendPlan` |
| `agent-runtime/src/provider_call.rs` | 247（+18/−4） | 比较判定 + 改掉「M1 恒 Reuse」那句 |
| `agent-core/src/cache/drift.rs` | 146（+7/−4） | `PrefixIntent` 文档注释刷新（那句「还没有任何一处会有意改前缀」到期了） |
| 7 个既有测试 | 机械 | 槽位计数 16→17（比预想多一个：`subagent_indep_snapshot.rs` 里内联算的 `3 * 16`） |
| 独测 6 个文件 | 33–154 | 见下 |

**⚠️ `command/txn.rs` 现在正好 300 行**，跟 `clear_tool_results.rs`、
`clear_policy.rs`（299）一样贴着天花板。这一片已经有三个文件没有余量了。

### `PrevSendPlan` 为什么写在 `on_provider_done`

`SendPlan` 只能被独立的 `Session` 命令改（`clear_tool_results` / `advance_boundary`），
它们**结构上不可能在一次 `CallProvider` 在飞期间跑**。所以响应落地那一刻，
`Slot::SendPlan` 里装的正好就是组这次请求时用的那份，直接抄过去即可。

### 独测里最值得留着的一条做法

**反向锁怎么在非压缩轮制造真实漂移**：第一版试过中途换 provider——不行，
三家共用同一套 `wire::tools::build`/`wire::messages`，输出逐字节相同，
**那是红线 12 刻意造成的性质不是 bug**。改用 `Session::activate_skill`
（一条纯状态命令，本 issue 没碰过，`drift.rs` 自己的注释就把它列为未来的
`Intentional` 来源之一）：它真的改 System 段，而 `SendPlan` 一动不动 →
意图必须是 `Reuse` → 必须判 `Unexpected`。实现 agent 和测试 agent 各自独立
撞上同一个诊断。

### ⚠️ 主会话回退了一处越界改动，并补了反向锁

实现 agent 按**本 issue 早期一条写错的验收**改了 `agent-runtime/src/guard.rs`，
让 `DriftVerdict::Expected`（压缩轮）**整轮不进第 3 层的滚动窗口**。已回退。

理由不是「原验收写错了」这么轻——**那个排除会开一个正好落在灾难场景上的盲区**：

> 压缩要是因为 bug 变成每轮都开火（「每轮改中段、每轮全价」，096 决策记录里反复
> 点的形态，DeepSeek 上一次≈120 轮命中的钱），那就是**每一轮都判 `Expected`**
> → 每一轮都被排除 → 窗口里一条观测都没有 → 第 3 层永远不告警。
> **唯一能抓这个形态的一层，恰恰在这个形态下失明。**

而一次性的压缩代价本来就已经被容忍过一次：`DEFAULT_CONSECUTIVE_ALERT` 是 3，
`window.rs` 的文档写着「单轮低命中是正常现象（换前缀、压缩……）。连续三轮说明
不是一次性代价」。再排除一次是重复计算这份容忍。

**回退之后全仓 1699 个测试仍然全绿——说明没有任何测试能区分这两种行为。**
主会话补了 `prefix_intent_compaction_rounds_still_enter_the_window.rs`：
连着三轮压缩 + 全 0% 命中 → **必须**报 `ChronicMiss { streak: 3 }`。
变异检验：把豁免加回去，这条当场红。

它跟 `prefix_intent_single_compaction_round_is_not_chronic.rs` 是一对——
只有后者的话，一个「压缩轮永不进窗口」的实现照样全绿。

### 命令输出

```
$ cargo test --workspace                                   1700 passed; 0 failed
$ cargo test -p agent-server --features ts                 0 failed
$ cargo clippy -p agent-core --all-targets -- -D warnings  干净
$ bash scripts/check-invariants.sh --all                   exit 0
```
