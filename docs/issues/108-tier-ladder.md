# 108 阶梯编排：哪一档先上

**里程碑** M12 · **依赖** [103](103-prefix-intent-wiring.md) + [107](107-summary-writeback.md) · **模型** sonnet · **独立测试 agent** 是 · **状态** 完成

## 目标

落地 [096](096-compaction-trigger.md) 的宏观部分：第 2 档清光了才轮到第 3 档，
以及哪几档不参与自动阶梯。

B 支与 C 支在这里合流。

## 做什么

**自动阶梯里只有 2 和 3 两档：**

- 第 2 档：`prompt / context_window` 超过触发线 → 开火（102 已实现）
- 第 3 档：触发条件是「**第 2 档已经清光了还不够**」——状态条件，不是阈值条件

不参与自动阶梯的三档，要在代码里写死，不然以后有人往里加：

- 第 1 档（截断）常开，不看压力
- 第 4 档（清窗口）是用户动作，不受阈值管
- 第 5 档（子 agent）是结构性的，编排时就定了

判读时机：**turn 结束拿到 usage 时判，下一轮出料单时生效**（096 第六问）。

## 定死的接口（2026-08-10 主会话定）

落在 `crates/agent-core/src/compaction/ladder.rs`（跟 102 的 `clear_policy.rs` 同目录、
不同文件）。**纯函数**（红线 1）。

```rust
/// 这一轮该走哪一档。自动阶梯里只有第 2、3 档。
#[derive(Clone, PartialEq, Debug)]
pub enum LadderAction {
    /// 不压。
    Nothing,
    /// 第 2 档：清这批工具结果。
    ClearToolResults(Vec<ToolCallId>),
    /// 第 3 档：把 `[0, upto)` 摘要掉。
    Summarize { upto: usize },
}

pub fn next_action(
    history: &Vector<Message>,
    plan: &SendPlan,
    last_prompt_tokens: Option<u32>,
    context_window: Option<u32>,
    params: ClearParams,
) -> LadderAction;
```

### ⚠️ 从 107 继承的硬契约：摘要回写的 epoch 握手

107 的 `apply_summary` **签名里没有 epoch**——它是一条「表达此刻意图」的命令
（同 `advance_boundary`），红线 6 的闸仍然只住在 `Session::step`（105）。
而 `Event::CompactDone` **不带 `upto`**（105 定死的事件形状），所以 `step` 自己
也没法回写。

于是回写的正确姿势只有一条，**由本 issue 兑现**：

> 持有 `upto` 的一方（106 的 `CompactSlots` 记着）必须**先**把
> `Event::CompactDone` 喂给 `step`；**只有过了闸**——回执里出现
> `Notice::CompactionSummaryReceived`——才调 `apply_summary`。

**「回执非空 = 过闸」这件事目前是隐式的**，而它现在是承重的：判错一次，
一份过期的摘要就会被写进状态，那正是红线 6 要防的静默错值。
本 issue 必须把这个判定写成一处显式、有名字、有测试的东西，
不许散在 runner 里靠「effects 是不是空的」心照不宣。

### 判定顺序（就三步，别加第四步）

1. 压力**没超** `params.trigger_percent` → `Nothing`
2. 超了，且 [`tool_results_to_clear`](102-clear-tool-results-policy.md) 返回非空
   → `ClearToolResults`（**第 2 档优先，永远**）
3. 超了，但第 2 档**已经无可清**（返回空）→ `Summarize { upto: 保护区起点 }`；
   保护区起点为 0（没东西可摘）时 → `Nothing`

### ⚠️ 为什么阶梯是「跨轮」的，不是同一轮里先清再摘

「第 2 档清完还不够」这句话**没法在同一轮里判**：清完之后新的 token 数要等下一轮
实测才知道，而估算 token 需要 tokenizer——那是模型相关知识，写进 core 当场破红线 12
（004 也早就写过「字节数和 token 数关系不稳定」）。

所以阶梯是**时间上的**：这一轮清工具结果，下一轮再测；还超就说明清不动了，
那时第 2 档自然返回空，第 3 档接手。触发线仍是 85%，**一轮的代价换一个不用猜的判据**。

### ⚠️ Y=30% 不是一个可计算的参数

[096](096-compaction-trigger.md) 定的「压到 30%」**没法作为停止条件实现**，同上：
压之前算不出压之后是多少。

两档的实际动作都是「把保护区之外的**全部**处理掉」（096 决策记录第三问的「一次全清」，
以及用户对主动摘要那句「压到多少算多少」）。**30% 因此是预期落点不是输入参数**
——由 [110](110-compaction-dogfood.md) 真机量出来验证，不进代码。

这条是本 issue 对 096 的一处澄清，不是推翻：先清后摘的顺序、触发线、保护区全都不变。

### ⚠️ 摘要子 agent 必须回收（106 落地时发现，并进本条）

106 的实现 agent 报的：**摘要子 agent 收割完从不 `despawn_child`**，而
`AgentLimits.max_children` 默认 **8**。长会话自动压 8 次之后，之后每一次压缩都会
`SpawnRefused::TooManyChildren` → `CompactFailed`——**自动压缩从此永久失效**。

它没红任何一条红线（每次都响亮地报一条 `Notice::CompactionFailed`，不是静默），
所以 106 按契约交付是对的；但**这条正好打在 M12 的要害上**：整条线就是为长会话做的，
而它规定了长会话最多只能压 8 次。

**裁决：收割之后 despawn 摘要子 agent。** 理由：

- 摘要子是**纯粹的一次性工人**，输出已经被复制进父的 `Slot::Summaries`（107），
  它自己的历史之后没有任何人需要
- undo / redo 不受影响——摘要正文住在父那边，不住子那边
- [109](109-compaction-visibility.md) 要展示的是被盖住的**原文**（走完整记录）和
  摘要正文（走 `Slot::Summaries`），都不经过这个子 agent

despawn 的语义与 undo 行为 028/029 已经定死并有测试（`subagent_indep_despawn.rs`、
`subagent_indep_tombstone.rs`），本条只是去调用它。

## 验收

- **造一个「清工具返回够用」的场景：第 3 档一次都没触发，零模型调用。**
  这是整个阶梯的核心断言——顺序反了或者两档并行开火，这条当场红
- 造一个「工具返回全清光仍然超」的场景：第 3 档触发，且**在第 2 档之后**
- **同一份历史重放两次，压缩决定逐字节相同**（含触发了哪一档、清了哪些 id、
  边界推到哪）
- **⚠️ epoch 握手**：摘要在飞时 `/undo` 或取消 → 迟到的 `CompactDone` 被 `step` 挡下
  → **`apply_summary` 一次都不该被调用**，状态一个字节不变。
  反向锁：epoch 对得上时确实调了、边界真的动了
- **⚠️ 连续 10 次自动压缩全部成功**——不因摘要子 agent 占满槽位而从第 9 次开始失败。
  这条是上面那个裁决的度量：`max_children` 默认 8，不回收就必然在第 9 次红
- **⚠️ 从 [106](106-summary-via-subagent.md) 移交过来的四条行为验收**（那条 issue
  落地时 `Effect::Compact` 还对外不可达，见它的「行为验收移交」一节）。本条接上阶梯
  之后 `run_turn` 天然能驱动整条链，这四条在这里第一次可测：
  - 父的 `encode` 不含被摘要原文
  - 父的 `encode` 不含子 agent 的摘要过程（每轮打 `CHILD_STEP_NN` 标记逐一断言，
    照 [097](097-subagent-ingredient-audit.md) 的做法）
  - 摘要失败 / 超时 → `CompactFailed` → **父不卡死、边界不动、下一轮照常跑完**
  - 反向锁：成功路径回执确实是 `CompactDone` 而不是 `CompactFailed`
    （只测失败路径的话，一个「永远失败」的实现照样把上面几条走绿）
- 第 4 档（清窗口）在阈值远未到达时由用户触发：正常生效，说明它不受阈值管
- 阈值到达但用户没动作时：第 4 档**不触发**
- `context_window: None`：两档都不触发

## 注意

- **红线 1**——判读纯函数，按轮不按时间。看了时钟，重放就分岔
- **红线 12**——阶梯顺序是产品判断，跟模型无关。这里不许出现任何 provider 分支
- 判读点放在「拿到 usage 之后」而不是「发请求之前」，是为了让输入全是已落定的
  实测值。挪到发请求前，输入里必然混进本轮的估计值，重放当场不确定

## 实做记录（实现 agent，2026-08-10）

接口按「定死的接口」一节**一字不差**落地（`LadderAction` 三个变体、`next_action`
五个参数的名字/类型/顺序全同），判定顺序就是那三步，**没有第四步**。

### 落地的文件

| 文件 | 行数 | 职责 |
|---|---|---|
| `agent-core/src/compaction/ladder.rs`（新建） | 278 | `next_action` + `LadderAction` + 7 条内联单测 |
| `agent-core/src/compaction/pressure.rs`（新建） | 72 | 窗口压力够不够开火（从 `clear_policy` 拆出，第 2/3 档共用） |
| `agent-core/src/compaction/protected_region.rs`（新建） | 112 | 「最近 N 轮」那条线画在哪（同上，共用） |
| `agent-core/src/compaction/clear_policy.rs` | 299→263 | 只剩第 2 档自己的策略（两个共用原语搬走） |
| `agent-core/src/compaction/mod.rs` | 12→23 | 挂载 + 四个文件各一件事的表 |
| `agent-core/src/lib.rs` | +2/−1 | `LadderAction` 提根（判读函数不提，同 102 的取舍） |
| `agent-runtime/src/compact_ladder.rs`（新建） | 150 | 接线：什么时候问、拿什么当入参、答案怎么执行 |
| `agent-runtime/src/compact_ladder_tests.rs`（新建） | 293 | 9 条：判读时机 / 两档执行形态 / 三条反向锁 |
| `agent-runtime/src/compact_writeback.rs`（新建） | 155 | **epoch 握手**：`passed_epoch_gate` + 回写，含变异检验 |
| `agent-runtime/src/compact_slot.rs` | 178→157 | 加 `upto` 与待过闸队列；收割后 despawn；测试拆出去 |
| `agent-runtime/src/compact_slot_tests.rs`（新建） | 165 | 6 条，含「连续 10 次压缩不撞 `max_children`」 |
| `agent-runtime/src/runner.rs` | 356→369 | 泵里 3 行接线（存量超限，见下） |
| `agent-runtime/src/{compact_spawn,lib}.rs` | 各 +4~18 | `record` 带上 `upto`；crate 文档补一节 |

**行数**：`check-invariants --all` 前后都是 **17 条**存量提示，零新增。
`clear_policy.rs` 从 299 降到 263（那两个共用原语本来就不是它一个人的事），
`compact_slot.rs` 一度到 317，按本仓既有惯例（`compact_spawn_tests.rs` /
`subtree_tests.rs`）把测试拆成 `compact_slot_tests.rs`。

### epoch 握手做成了什么形状

一个**有名字的谓词** `compact_writeback::passed_epoch_gate(&[Effect]) -> bool`
——判的是 `Notice::CompactionSummaryReceived` **那一条具体的通报在不在**，
不是「回执非空」。为什么不能写成非空：今天两者碰巧等价，但哪天 `step` 对
**没过闸**的回执也说一句话（比如加一条「丢弃了一份过期摘要」的通报），
「非空」当场变成恒真，一份属于旧世代的摘要就会被写进当前状态。它带一条
**变异检验**：一批非空但不含那条通报的 effect 必须判 `false`。

链路是三段，`upto` 全程只住在一个地方：

1. `compact_spawn::intercept` 把 `upto` 记进 `CompactSlots`（事件里没有它，105 定死）；
2. `CompactSlots::harvest` 收割时转存成一份 `PendingSummary{agent, epoch, upto, summary}`，
   同时产出 `Event::CompactDone` 喂回泵；
3. 泵每次 `session.step` 之后调一次 `compact_writeback::after_step`：先把世代已经
   推走的意图丢掉（epoch 只增不减，对不上就永远对不上），过闸才 `take_gated_summary`
   （`agent` + `epoch` 双匹配）并调 `apply_summary`。

`apply_summary` 返回 `Err`（边界语义拒绝，例如第 4 档刚清过窗口）时**不静默**：
发一条既有的 `Notice::CompactionFailed`，不新造变体（054 的教训：新变体要连锁改
`SessionEvent` → 生成的 TS → fixtures）。

### despawn 接在哪

`CompactSlots::harvest` 里，**读完终答、造完事件之后当场调
`Session::despawn_child`**（`compact_slot::reap`）。成功和失败两条路都回收。

- 为什么在 harvest 而不是轮末：`orphan::reap` 只看 `Subtree` 的 detached 名单，
  摘要子从来不在那张表上（它不是一次 spawn 工具调用），轮末清算根本看不见它。
- 为什么不在这里 `persist::sync`：这个函数每划掉一格必产出一个事件，泵下一圈处理
  它时无条件 sync 一次，中间没有任何 IO。
- 拒绝路径结构上不可达（没有子孙、没有跨 agent 读者、不是 root），真撞上也只是少
  回收一格，`let _ =` 掉并在函数文档里写明理由。

内联单测 `ten_consecutive_compactions_never_run_out_of_child_slots` 是这条裁决的
度量：连跑 10 轮 spawn→收割→回收，`max_children` 默认 8 一次都没撞到。

### 「跨轮阶梯」怎么实现的

**一次 `resume` 只判一次**（`compact_ladder::Ladder` 这个闩），判读点在泵的 A 段、
`session.step` 之后：

```rust
ladder.note(&event);                       // step 之前：这条是不是 root 的实测
let mut effects = session.step(event);
persist::sync(ctx, session);
compact_writeback::after_step(...);        // 过闸才回写
effects.extend(ladder.fire_once(session, ctx));   // 第 3 档并进这一批 effect
```

第 3 档产出的 `Effect::Compact` 跟 core 自己产出的 effect 走**同一条**派发路
（`dispatch::run_effect` → `compact_spawn::intercept`），所以泵里不需要第二套
effect 处理，`runner.rs` 只多了 3 行。

**闩就是「跨轮」本身**：没有它，第 2 档清完之后泵下一次静止时再判一次，
`tool_results_to_clear` 因为都进了 `plan.cleared()` 而返回空，第 3 档当场在同一轮
里接上——「清完还不够」就从**下一轮实测**退化成**同一轮的推断**，而那个推断要
tokenizer 才做得准（红线 12）。验收第一条正是冲着这个失效模式写的。

三道开火前提，每一道都有反向锁测试：

1. `measured`——这一轮真的有一条属于 root 的 `ProviderDone`。光看「到终态了没」
   不够：`Done` 还有两条没有新观测的到达路径（终态上又收到 `UserInput` 的协议
   违规；轮预算在开轮那一刻就用尽，一次请求都没发），那两条上 `prev_prefix` 装的
   是上一轮的实测值，而它已经被上一轮判过了——拿它再判一次会凭空多开一次第 3 档。
2. 状态是 `Done{..}`——`Failed(Cancelled)` 收尾的一轮不压（用户刚按下取消，
   紧接着起一个摘要子 agent 是最不该发生的事）。
3. 闩没落下。

判读的两个入参都取自**已经落定的事实**：`last_prompt_tokens` 来自
`Session::prev_prefix_of(root).prompt_tokens`（`provider_done` 那一格用真实 usage
回填进状态的，所以跟着 undo 一起回退）；`context_window` 来自这个 agent 起飞时会
用的那条 `ExecutionBinding` 的 `SessionConfig`——缺 binding / `None` / `Some(0)`
一律不触发，不 `unwrap`。**只判 root**（`run_turn` 的契约是 root 中心的；子 agent
不跨 turn，摘要子更不该被压缩——那会递归）。

### 一处对 096/102 的连带整理（不是顺手重构，是本次改动的一部分）

`window_pressure_triggers` 与 `protected_region_start` 原来是 `clear_policy.rs` 的
私有函数，而第 3 档要用**同一条**触发线和**同一条**保护区线。留在原地靠
`pub(crate)` 借出去也能编译，但那会让「触发线是多少」「最近 N 轮从哪算」变成两个
文件共同持有的知识；真正的风险是第二种——两处各画一条线的那天，会出现「摘要盖住
了一段第 2 档还认为在保护区里」的错位，而错位不报错。所以按职责拆成两个各自
只回答一个问题的文件。`clear_policy.rs` 因此从 299 降到 263（它本来就顶着天花板）。

### 一个可以少花半小时的坑（留给下一个人）

跑真实 `run_turn` 的多轮集成测试**必须在第二轮起显式 `session.begin_turn()`**
（`run_turn` 的文档写了，026 判断 13：turn 边界是会话层面的概念，不藏进转移表）。
漏了不会报错也不会 panic：`Done` 上再来一条 `UserInput` 是协议违规，状态原样停在
`Done{truncated:false}`，于是 `assert_eq!(status, Done{truncated:false})` 照样绿，
但**那一轮根本没发过请求**——阶梯自然一次都不开火，症状表现为「第 2 档没清东西」。

### 命令输出

```
$ cargo test --workspace          实现侧全绿；独测 agent 当时在飞的 5 条
                                  ladder_* 集成用例红在同一个原因上（见上一节：
                                  漏 begin_turn），把那一行补进去当场转绿——
                                  用它们的副本验证过，不是推断
$ cargo clippy -p agent-core --all-targets -- -D warnings      干净
$ cargo clippy -p agent-runtime --all-targets -- -D warnings   5 个存量错误，零新增
$ bash scripts/check-invariants.sh --all                       exit 0；17 条行数提示，与改动前逐条相同
$ cargo test -p agent-server --features ts                     84 + 109 passed
```

## ⚠️ 独测抓到的一个真 bug：第 3 档整条路是哑的

**症状**：第 3 档压缩之后 `SendPlan` 状态全对（`boundary=4`、`summary=Some`、
`summary_text` 读得回来），但下一轮**实际发出去的请求体里 11 条原文一条不少**，
摘要正文一个字都没进去。

**根因**在 `agent-runtime/src/provider_call.rs` 的取料处——`project` 的第三个参数
`summary_text` 一直传的 `None`：

```rust
let durable_messages = project(&history, &plan, None);   // ← 100 留下的
```

而 099 的投影规定「**有摘要引用但拿不到正文 → 边界作废、整份历史照发**」
（宁可多发，不可发一段引用不到正文的空洞）。于是第 3 档**完全哑火**：
`apply_summary` 照常写、状态全对、undo 和崩溃恢复都正常，
**只有真正发出去的字节一个都没压**。

**这是一根三方都没盖住的线**：

| | 当时做了什么 | 为什么没接上 |
|---|---|---|
| [100](100-projection-into-ingredients.md) | 传 `None` | **当时是对的**——摘要还不存在，issue 里明确写了「别自己发明摘要仓库」 |
| [107](107-summary-writeback.md) | 做出 `summary_text` | 范围是**写**那一侧 |
| 108 | 接阶梯 | 默认读那一侧已经通了 |

**不报错、状态测不出异常、只在账单上浮出来**——正是本仓最贵的那一类。

**怎么被抓到的**：独测 `ladder_parent_excludes_summarized_material.rs` 断言的是
**真实请求体**（录制服务器收到的字节），不是状态。任何只查 `SendPlan` 的测试都会全绿。

**修复**（主会话）：取料处把正文一起取出来喂给投影。

```rust
let summary_text = plan.summary().and_then(|id| session.summary_text(&agent, id));
let durable_messages = project(&history, &plan, summary_text.as_ref());
```

改完那条测试当场转绿，全仓 1783 passed / 0 failed。**那条测试就是这个 bug 的反向
变异检验**——把参数改回 `None`，它必红。

**留下的规矩**：M12 之后凡是「状态写对了」和「发出去的字节对了」不是同一件事的地方，
**验收必须断言字节，不能只断言状态**。这是继 [100](100-projection-into-ingredients.md)
那个 `KNOWN_LABELS` 之后，同一族的第二个坑了。
