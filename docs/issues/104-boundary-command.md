# 104 第 4 档：边界推进 command（含清窗口）

**里程碑** M12 · **依赖** [100](100-projection-into-ingredients.md) · **模型** sonnet · **独立测试 agent** 是 · **状态** 完成

## 目标

把 `SendPlan` 的**边界**做成一个正规的状态变更。第 4 档（清窗口）就是它的一个特例：
边界推到最新、不生成摘要。

C 支的第一条。与 B 支（101→102→103）完全并行，不碰同一批文件。

## 为什么第 4 档不单开机制

「清窗口」= 边界推到底 + 摘要引用留空。跟第 3 档共用同一个字段、同一条 command，
少一套机制。区别只在第 3 档还会填摘要引用，第 4 档不填。

跟 Claude Code 的 `/clear` 不同的是：**记录还在库里，所以这一步能 undo**
（095 的分界）。这是这套架构白捡的好处，验收里要锁死。

## 两个用户按钮，别合成一个

096 第八问定的：用户侧有**两个**按钮，语义分开，不做成一个带参数的。

| 按钮 | 边界推到 | 摘要 | 归谁实现 |
|---|---|---|---|
| **清窗口** | 最新（**不留最近 3 轮**） | 无 | 本 issue |
| **主动摘要** | 最近 3 轮之前（**留**） | 有，压到多长算多长 | 本 issue 的 command + [105](105-effect-compact.md)–[107](107-summary-writeback.md) 的摘要机制 |

两个都**不受 X / Y 水位约束**——水位是自动档用来判「够不够」的，
用户按下去就是执行一次。

## 做什么

一个 command，输入是新边界。约束一条：**边界只能前进，不能后退**。
（后退了 History 段会来回漂，每轮全价——跟 102 的滞回带是同一类坑。）

undo 当然要能把边界退回去——那是 undo，不是「后退」。区别在于 undo 走的是 undo log，
不是再发一次这个 command。

## 定死的接口（2026-08-10 主会话定）

```rust
impl Session {
    /// 第 3、4 档共用：把边界推到 `next`，同时设定 / 清除摘要引用。
    ///
    /// 三种情况，别合并：
    /// - `next > 当前边界` → 生效，产生一条 entry
    /// - `next == 当前边界` 且摘要引用相同 → **幂等无操作，不产生 entry**
    /// - 其余 → `Err`，**状态不变、不留痕**（先校验再写）
    ///
    /// 第 4 档「清窗口」= `next` 取历史长度、`summary` 传 `None`。
    /// 用户主动摘要 = `next` 取「最近 3 轮之前」、`summary` 传 `Some`。
    pub fn advance_boundary(
        &mut self,
        agent: &AgentId,
        next: usize,
        summary: Option<SummaryId>,
    ) -> Result<(), BoundaryRejected>;
}

#[derive(Clone, PartialEq, Debug)]
pub enum BoundaryRejected {
    /// `next` 比当前边界小——边界只能前进（回退会让 History 段来回漂，每轮全价）。
    NotAdvancing { current: usize, requested: usize },
    /// 边界没动但摘要引用不同。**重新摘要同一段不在本条支持范围内**——
    /// 若 107 之后需要「摘要重生成」，那是一条新决策，不是这里顺手放开。
    SameBoundaryDifferentSummary,
}
```

`next` **不跟历史长度校验**（沿用 099 的判断：`SendPlan` 不知道历史多长，
越界边界在投影里退化成「一条正文都不发」，不 panic）。

底层复用 100 的 `replace_send_plan`。

## 验收

- 清窗口后，`encode` 的输出只剩边界之后的消息
- `/undo` 一次，边界退回，被盖住的消息全部重新出现
- 清窗口后 `History.entries()` 的**长度不变**——记录一条没少
- 该 entry 的 `prev` 序列化 **< 1 KB**（它装的是一个数）
- 传一个比当前更小的边界：**拒绝**，状态不变，且不是静默忽略
- 边界推到底之后再推一次：幂等，不产生新 entry

## 注意

- **红线 2**——走 command 层
- **红线 3**——边界是 primitive 的一部分，要能进快照
- 别把这条跟 `SessionStore::drop_oldest` 搞混：那个是**空间管理**（cap 溢出，
  记录真的从库里丢了），这个是**发送侧**（记录全在，只是不发）。
  两件事，两套机制，095 的分界就是为了把它们分开

## 实做记录（实现 agent + 独立测试 agent 并行，2026-08-10）

与 [101](101-clear-tool-results-command.md) 同时开工，两支零冲突。

### 落地的文件

| 文件 | 行数 | 职责 |
|---|---|---|
| `agent-core/src/command/advance_boundary.rs`（新建） | 247 | `advance_boundary` 命令 + `BoundaryRejected` |
| `agent-core/src/command/{mod,meta}.rs`、`lib.rs` | 各 +1~2 | 挂载、根导出、`KNOWN_LABELS` 补一项（见下） |
| `agent-core/tests/it/advance_boundary_{command,window_clear}.rs`（新建） | 118 / 234 | 独测：按「校验契约」与「观测效果」拆开，12 个测试 |

### 三种情况怎么落地的

在调值层方法**之前**先分诊——099 的 `SendPlan::advance_boundary` 对
`next <= boundary` 一律 `Err`，命令层要把其中「原地不动且摘要没变」改判成幂等：

- `next < current` → `Err(NotAdvancing)`，**不碰** `replace_send_plan`
- `next == current` 且摘要引用相同 → `Ok(())`，**不碰** `replace_send_plan`
- `next == current` 且摘要不同 → `Err(SameBoundaryDifferentSummary)`
- `next > current` → 值层方法（此时保证不会再拒绝）+ `replace_send_plan`

entry 的 label 复用 `"replace_send_plan"`，不新开一格——这条命令在状态层做的事
跟 100/101 调同一个底层 setter 是同一种事件。

`prev` 实测 **51 字节**（要求 < 1 KB）。

### 独测里两条断言的写法值得留着

- **「拒绝不留痕」**：不只断言返回 `Err`，还在调用前后各取一次 `send_plan_of` 与
  历史长度，断言两者逐值相等——状态和日志物理上都没被摸过。
- **「两个字段同一条 entry」**：不满足于行为层的 undo 验证，直接从 `last_entry()`
  的 changes 里过滤出 `AtomKey::Agent(root, Slot::SendPlan)`，断言 `len() == 1`。

### 顺带发现并修掉的 100 遗留 bug

见 [100 的实做记录](100-projection-into-ingredients.md) §「事后发现的一个 bug」。
一句话：`replace_send_plan` 的 label 没进 `KNOWN_LABELS`，任何用过压缩的会话
重启就恢复不了。本 issue 的实现 agent 在复核 100 时发现并补上了那一项。

### 变异检验（主会话做）

把「同边界换摘要」改成静默当幂等（摘要变更被吞掉）：
`advance_boundary_command` 两条专测该场景的测试红，其余 15 条绿。定位精准。

### 命令输出

```
$ cargo test --workspace
1640 passed; 0 failed

$ cargo clippy -p agent-core --all-targets -- -D warnings
干净

$ bash scripts/check-invariants.sh --all
exit 0；17 条行数提示全是存量文件
```
