# 102 第 2 档触发与选择

**里程碑** M12 · **依赖** [101](101-clear-tool-results-command.md) · **模型** sonnet · **独立测试 agent** 是 · **状态** 完成

## 目标

落地 [096](096-compaction-trigger.md) 的微观部分：什么水位开火、压到哪、
谁不能碰、先清谁。

**纯函数**：输入是（完整历史、当前 `SendPlan`、上一轮实测 token、`context_window`），
输出是「这次该清哪批 `ToolCallId`」。它自己不写状态，写状态是 101 的 command。

## 做什么

照 096 的一、三、四、五问落地，四个点一个都不能少：

1. **触发信号**：只用上一轮实测的 `TokenUsage.prompt`。`context_window` 为 `None`
   → 不触发。
2. **一次全清，没有目标水位**（096 决策记录第三问的修订）：用量到 **X=85%**
   开火，把保护区之外的工具结果**一次全清**，够得着的清完为止。
   `Y=30%` 不是本档的停止条件，它只被 [108](108-tier-ladder.md) 用来判
   「清完还不够，该上第 3 档了」。X 可配置，默认值写明理由。
3. **保护区**三条：当前轮不清；**最近 3 轮不动**；用户消息永不进本档。
   （原第四条「成对清」已被 099 的换占位做法消解。）
4. **排序**：已清列表按最老优先排——不影响清哪些（全清），只影响序列化字节序，
   而那必须逐字节确定（红线 11）。

## 定死的接口（2026-08-10 主会话定）

落在**新模块** `crates/agent-core/src/compaction/clear_policy.rs`（跟 105 不碰同一批文件）。

```rust
/// 第 2 档的策略：这一轮该清哪些工具结果。
///
/// **纯函数**（红线 1）：零 IO、零时钟、零随机、不读全局。同一份入参算一千次，
/// 输出逐项相同（顺序也相同）。
///
/// 返回空 `Vec` 表示「这轮不清」——不触发和「触发了但没东西可清」对调用方是
/// 同一件事（101 的 `clear_tool_results` 对空输入天然无操作）。
pub fn tool_results_to_clear(
    history: &Vector<Message>,
    plan: &SendPlan,
    /// 上一轮**实测**的 prompt token 数。`None` = 这一轮没有观测（首轮、
    /// 或这家 provider 没报），**不触发**。
    last_prompt_tokens: Option<u32>,
    /// `SessionConfig.context_window`。`None` = 未知/不设限，**不触发**，
    /// 不许 `unwrap`。
    context_window: Option<u32>,
    params: ClearParams,
) -> Vec<ToolCallId>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ClearParams {
    /// 触发线：`last_prompt_tokens * 100 / context_window` **超过**它才开火。
    /// 默认 [`DEFAULT_TRIGGER_PERCENT`] = 85。恰好等于不触发（边界要有确定的一边）。
    pub trigger_percent: u32,
    /// 保护区：最近这么多**轮**的工具结果一个不动。
    /// 默认 [`DEFAULT_PROTECT_RECENT_TURNS`] = 3。
    pub protect_recent_turns: usize,
}

pub const DEFAULT_TRIGGER_PERCENT: u32 = 85;
pub const DEFAULT_PROTECT_RECENT_TURNS: usize = 3;
```

### 「轮」怎么数（定死，别自己发明）

**一条 `Role::User` 消息开启一轮。** 「最近 N 轮」= 从倒数第 N 条 `User` 消息
（含）到历史末尾。历史里 `User` 消息不足 N 条 → **整个历史都在保护区，不清**。

这个定义是纯结构的、不依赖任何计时或 turn_id，所以重放一定得出同样的边界。

### 「一次全清」的精确含义

触发之后，返回**保护区之外、且还没在 `plan.cleared()` 里**的**全部**
`ToolResult` 的 id，按在历史中出现的先后排列（最老在前）。

不设目标水位——够得着的就那些，清完为止（096 决策记录第三问）。

## 验收

- **造一个稳定增长的会话跑 30 轮：第 2 档触发次数 ≤ 2。**
  每次触发后，保护区之外的工具结果**一个不剩**、保护区之内的**一个没动**
- **反向锁：第 2 档不是常开。** 用量在 X 以下时，哪怕有大量够得着的工具结果
  也一次都不触发。漏了这条会变成每轮改中段、每轮全价，而测试全绿
  ——只测「清得对」的话，一个「每轮都清」的实现照样全过
- 投影后 `ToolUse` 与 `ToolResult` 的 id 集合恒等（099 的做法保证配对不破，这里是回归锁）
- `context_window: None`：一次都不触发，且不 panic
- **单调性**：清过一批之后把预算调宽，再算一次 → 输出为空（已清的不回来）
- 最近 3 轮的工具返回从不出现在选中列表里
- 用户消息从不出现在选中列表里
- **同一份输入算 1000 次，选中列表逐字节相同**（顺序也相同，不是集合相等）

## 注意

- **红线 1**——纯函数，按轮不按时间。看了时钟，同一份历史重放两次就会做出不同决定，
  崩溃恢复和审计回放当场分岔
- **红线 12**——只做算术。这里是最容易冒出 `match provider` 的地方
  （「DeepSeek 上应该压得更狠」），**不许**。决策 17 已经把这条路堵死：
  按窗口压力触发，不看折扣比（[ADAPTER.md](../ADAPTER.md) §五个能力位）
- 滞回带那条漏了的症状是**每轮全价**——测试全绿，只在账单上浮出来。
  验收第一条是专门为它写的，别删

## 实做记录（实现 agent + 独立测试 agent 并行，2026-08-10）

与 [105](105-effect-compact.md) 同时开工，文件不重叠。

### 落地的文件

| 文件 | 行数 | 职责 |
|---|---|---|
| `agent-core/src/compaction/clear_policy.rs`（新建） | 299 | `tool_results_to_clear` + `ClearParams` + 两个默认常量 + 9 条内联单测 |
| `agent-core/src/compaction/mod.rs`（新建） | 12 | 挂载 |
| `agent-core/src/lib.rs` | +16/−4 | 类型与常量 re-export（函数本身不提根，同 `cache::reconcile` 的取舍） |
| `agent-core/tests/it/clear_policy_*.rs`（新建 10 个） | 24–131 | 独测 21 条：触发边界 / 轮边界 / 保护区 / 块过滤 / 排序 / 确定性 / 单调 / 30 轮场景 / 与 `project` 的集成锁 |

**⚠️ 299 行，离天花板只剩 1 行**（rustfmt 展开四参数调用撑出来的，没塞额外内容）。
连同 [101](101-clear-tool-results-command.md) 那个 300 行的，这一片现在两个文件都没有余量了。

### 「轮」的边界算法

`protected_region_start`：收集所有 `Role::User` 消息的下标（保持顺序），
用 `user_turn_starts.len().checked_sub(protect_recent_turns)` 取偏移
——`checked_sub` 天然处理「`User` 不足 N 条」→ `None` → 边界 0 → 整个历史都在保护区。
`protect_recent_turns == 0` 单独短路成「无保护区」。**全程只看 `Role`，不看时间戳/
turn_id**，所以重放一定得出同样的边界。

### 设计判断（复核后收）

1. **整数除法 `(prompt*100)/window` 用 `u64` 提升**（`u32::MAX * 100` 溢出 u32）。
   交叉相乘 `a*100 > c*b` 与 floor 除法**不等价**，边界会漂，弃用。
2. **`context_window: Some(0)` 显式短路**——接口只写了 `None` 不触发，
   零窗口会除零 panic，属于「不许 unwrap」精神的延伸。
3. 触发判断与保护区计算拆成两个私有函数，主函数体只剩「触发→定界→过滤」三步。
4. 已清集合用 `BTreeSet` **只做成员查询不迭代**，输出顺序完全来自 `history.iter()`，
   红线 11 不受影响。
5. `protect_recent_turns == 0` 当「无保护区」而不是「全保护」，单独有测试盖住。

### 变异检验（主会话做）

把触发线判断改成常开（每轮都清）：

```
内联  below_or_at_trigger_line_never_fires                    FAILED
独测  below_threshold_returns_empty_despite_reachable_results FAILED
独测  exactly_at_threshold_does_not_trigger                   FAILED
独测  thirty_round_growing_session_triggers_at_most_twice     FAILED
```

那条 30 轮的最有价值：独测 agent 手算了一条递增用量曲线（首次跨线在第 15 轮、
第二次第 27 轮、第三次落在第 39 轮即窗口之外），断言**恰好触发 2 次**。
常开的实现会触发二十几次，当场红——而那正是「每轮改中段、每轮全价」的灾难形态。

### 一个过程教训：实现 agent 栽在后台自旋上

第一次交的不是报告，是「两个后台任务在飞，我暂停等通知」——**正是
[WORKFLOW](../WORKFLOW.md) §四第 -1 条明令禁止的那个循环**（M3/M4 期间栽过五个 agent）。
活其实干完了（文件已在磁盘上），缺的只是前台验证。主会话把它叫回来重跑一遍就好了。

提示词里写了那条禁令仍然没挡住，说明这是个**会复发的失败模式**，成本低但要认得出来。

### 命令输出

```
$ cargo test --workspace        1680 passed; 0 failed
$ cargo clippy -p agent-core --all-targets -- -D warnings   干净
$ bash scripts/check-invariants.sh --all                    exit 0
```
