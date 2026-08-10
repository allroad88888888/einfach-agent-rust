# 107 摘要回写与 epoch 校验

**里程碑** M12 · **依赖** [106](106-summary-via-subagent.md) · **模型** opus · **独立测试 agent** 是 · **状态** 完成

## 目标

摘要回来了，把它写进状态：边界推进 + 摘要引用填上，两件事**一条 entry**。

红线 6 的正面战场——摘要是在飞的异步回写，回来的时候世界可能已经变了。

## 为什么是 opus + 独测

红线 6 违反后不报错。摘要在飞期间用户 `/undo` 了一轮，回来的摘要如果照写不误，
状态就是一份「摘要盖住的范围和实际历史对不上」的静默错值——
下一轮 prompt 里少一段或多一段，模型照答不误，人发现不了。

这正是 [INVARIANTS](../INVARIANTS.md) 说的「在 undo 或崩溃恢复时以静默错值的形式浮出来」。

## 做什么

1. 回写前**校验 epoch**：对不上直接丢，不写、不报错、不重试
2. 对得上：一条 command 同时改边界和摘要引用（两个字段一条 entry，
   不能拆成两条——中间态是「边界推了但没有摘要」，那一瞬间的 prompt 会缺一段）
3. 摘要正文以 `Arc` 落在 `SendPlan` 之外，引用进 `SendPlan`

## 定死的接口（2026-08-10 主会话定）

```rust
impl Session {
    /// 摘要回来了：**一条 entry 同时做三件事**——存正文、推边界、填引用。
    ///
    /// 三件事必须原子。拆开会出现「边界推了但还没有摘要」的一瞬间，
    /// 那时的 prompt 缺一整段。
    ///
    /// `SummaryId` 由 `upto` 派生（见下），不需要调用方给。
    pub fn apply_summary(
        &mut self,
        agent: &AgentId,
        upto: usize,
        summary: Arc<str>,
    ) -> Result<SummaryId, BoundaryRejected>;

    /// 取某个摘要的正文。投影（099 的 `project`）要用它。
    /// 找不到 → `None`，投影那边会把边界作废（宁可多发，不可发空洞）。
    pub fn summary_text(&self, agent: &AgentId, id: &SummaryId) -> Option<Arc<str>>;
}
```

### `SummaryId` 从 `upto` 派生，不用计数器

**一个边界值最多对应一个摘要**——104 已经把「同边界换摘要」定成拒绝
（`BoundaryRejected::SameBoundaryDifferentSummary`），边界又只增不减。
所以 `upto` 本身就是唯一键，`SummaryId` 直接从它派生即可。

好处：**不需要一个计数器槽位，也不需要任何随机或时钟**（红线 1）——
同一份历史重放两次，摘要 id 逐字节相同。

### 摘要正文住哪

**新增槽位 `Slot::Summaries`**，存 `Vec<(SummaryId, Arc<str>)>`。

- **禁 `HashMap`/`HashSet`**（红线 11）：正文会进 prompt，容器迭代顺序必须确定
- **大值 `Arc`**（红线 5）
- **只增不删**：连续两次压缩时，摘要 1 仍要留着——回收了 redo 就拿不回来
- 可见性 `Private`；槽位计数跟着 +1（103 若已把 16→17，本条 17→18）

## 验收

- 摘要在飞时 `/undo` 一次 → 回来的摘要**不写入**；`SendPlan` 与 undo 之后的状态一致
- 摘要在飞时 `CancelInFlight` → 同上
- epoch 对得上：边界与摘要引用**同一条 entry** 生效；`/undo` 一次两个字段一起退回
- 该 entry 的 `prev` **不含任何被摘要的历史正文**，且大小**与被摘要的历史长度无关**
  ——摘要 100 条消息和摘要 10000 条，`prev` 一样大。这是
  [095](095-compaction-tiers.md) 整个形状决策的最终兑现点
- **⚠️ 一处刻意的放宽**：`Slot::Summaries` 是一份只增的列表，所以第 N 次压缩那条
  entry 的 `prev` 会含**前 N−1 份摘要正文**。这是可接受的，理由：摘要按构造远小于
  它替代的那段历史（不然就白压了），而且大小只跟**摘要条数**线性相关，跟历史长度
  无关。原文写的「`prev` < 1 KB」在第二次压缩起就不成立，**那是我把 095 对
  第 2/4 档的度量照抄过来写错了**，已改成上面这条真不变量
- 连续两次压缩：第二次摘要的是「摘要 1 + 之后的消息」，边界继续前进，
  摘要 1 **留在库里不回收**（回收了 redo 拿不回来）
- 摘要正文的长度不影响 entry 大小——摘要 100 字节和 10 KB，`prev` 序列化后一样大

## 注意

- **红线 6**（在飞 effect 带 epoch，回写前校验）——主线
- **红线 2**——走 command 层
- **红线 5**（大值 `Arc`，`PartialEq` 走 `ptr_eq` 快路）——摘要正文是大值
- **压缩不是 undo 屏障**（095 第 5 点）：`shell/exec` 那种动了外部世界的才需要屏障，
  压缩连内部状态都没动。别顺手给它加一个

## 实做记录（实现 agent + 独立测试 agent 并行，2026-08-10）

接口按「定死的接口」一节**一字不差**落地（`apply_summary` / `summary_text` 的签名、
参数名、返回类型全同）。

### 落地的文件

| 文件 | 行数 | 职责 |
|---|---|---|
| `agent-core/src/command/apply_summary.rs`（新建） | 298 | `apply_summary` / `summary_text`：三件事一条 entry + `SummaryId` 派生 + epoch 契约 |
| `agent-core/src/value/summaries.rs`（新建） | 100 | 摘要库 ↔ `AgentValue::Json` 的一处编解码（形状三条理由写在这） |
| `agent-core/src/graph/slot_default.rs`（新建） | 139 | 从 `slot.rs` 拆出来的「槽位没有值时是什么」（见下） |
| `agent-core/src/graph/{slot,visibility,mod}.rs` | 各 +2~20 | `Slot::Summaries`、`Private`、`ALL` 17→18 |
| `agent-core/src/command/{mod,meta,transitions/mod}.rs` | 各 +2~10 | 挂载、`KNOWN_LABELS` 新增一项、`CompactDone` 那一格的注释改成现状 |
| `agent-runtime/tests/it/jsonl_restart_after_compaction_command.rs` | +45 | 第三条重启回归（新 label 专用，见下） |
| 六个 `agent-core/tests/it/apply_summary_*.rs` | 19 条 | 独测 agent 写的，实现侧一条没碰 |

另有 7 个存量测试文件的槽位计数 17→18（`session_indep_snapshot_shape.rs` 的
`EXPECTED_SLOT_COUNT`、`subagent_indep_despawn` 的墓碑计数等），
`session_indep_accounting.rs` 的穷举 `match` 补一个分支——那个 `match` 正是为了
「新增槽位不站队就编译不过」而存在的，它按设计红了一次。

### 三件事怎么落成一条 entry

**不调 104 的 `advance_boundary`**：那条命令自带一次 `replace_send_plan`，
也就自带一条 `Entry`，两条命令 = 两个 undo 步。分诊表跟它同一张（照抄语义，
不是照抄调用），写入这一步自己做——一次 `commit_as("apply_summary", …)` 里
`set_key` 两次（`Slot::Summaries` + `Slot::SendPlan`），整批落成一条 `Entry`。
断言写在独测 `apply_summary_writes_boundary_reference_and_text_in_one_atomic_entry`
与实现侧内联单测里：`entry.changes` 恰好两条，键恰好是那两个槽位。

### `SummaryId` 怎么派生

`SummaryId::new(format!("summary@{upto}"))`——**没有计数器、没有时钟、没有随机**
（红线 1）。一处由此产生的措辞变化值得记下来：id 从 `upto` 派生之后，
104 定的「同边界换摘要」**不再表现为 id 不同**（id 必然相同），而是表现为
**正文不同**；判定点从比 id 挪到比正文，拒绝的语义一个字没变
（`BoundaryRejected::SameBoundaryDifferentSummary` 原样复用，没有新错误变体）。

顺带一条：边界没动时，只有「引用已经指向这个 id **且** 库里存着逐字相同的正文」
才算幂等。第 4 档刚清过窗口（引用是 `None`）而一份正好盖到这个边界的摘要迟到，
是**拒绝**——「重新摘要同一段」是一条新决策，不在这里顺手放开。

### label 用了新的，`KNOWN_LABELS` 与重启回归都补了

**没有复用 `replace_send_plan`。** 104 复用它的理由是「这条命令在状态层做的事
就是整体换掉那一个槽位的值」——`apply_summary` 不是：它在一条 entry 里同时写两个
槽位，而 label 要回答的是「当时发生了什么」（`EntryMeta.label`）。挂着「换了个
发送计划」的名字去审计一条同时存进摘要正文的 entry，时间线上就少了一件真的发生
过的事（109 要展示的正是它）。

所以照 100 的教训办：`"apply_summary"` 同时进 `KNOWN_LABELS`，并在
`jsonl_restart_after_compaction_command.rs` 加第三条重启回归（压缩命令 → 落盘 →
新进程 → `recover` 成功，且**边界、引用、正文三件一起回来**）。

**变异检验**（做了，不是自评）：把 `"apply_summary"` 从 `KNOWN_LABELS` 里删掉 →
`a_session_that_applied_a_summary_still_recovers_after_a_restart` **FAILED**，
另两条重启回归照绿。这正是 100 踩过、1604 个测试全绿也没抓到的那个坑。

### epoch 校验在哪（红线 6，本条的主线）

**在 `Session::step` 的闸上，不在 `apply_summary` 里**，而这不是偷懒：

- `apply_summary` 是一条**命令**，跟 `advance_boundary` / `clear_tool_results` 一样
  表达「此刻的意图」（同 `UserInput` / `Cancel` 不带 epoch 的理由）。它的返回类型
  也说明了这件事——`BoundaryRejected` 两个变体都是边界语义上的拒绝，**没有、也不该
  有「这份摘要过期了」那一种**（规格里「不写、不报错、不重试」的「不报错」）。
- 在飞的回执是 `Event::CompactDone`，它带 `epoch`，105 已经把闸装在 `step` 入口。
  闸只有一处是刻意的（`step` 的文档：转移表有几十格，漏一格就是漏一条回写路径）。

**`CompactDone` 那一格仍然不写状态**，因为回写要知道 `upto`，而 105 定死了事件里
不带它（effect 不带历史正文，事件也没有理由胖）。于是本条留给 108 的**硬契约**：

> 持有 `upto` 的那一方必须先把 `Event::CompactDone` 喂给 `Session::step`，
> **只有过了闸**（回执里有 `Notice::CompactionSummaryReceived`——105 专门为了让
> 「接受」可观测才加的那条通报）才调 `apply_summary`。

契约守不进类型系统，只能由 108 的接线兑现，所以它同时写进了
`command/apply_summary.rs` 的模块文档和 `transitions/mod.rs` 那一格的注释
（105 留在那里的「回写是 107」已经过期，改成了现状）。

**两个 agent 独立收敛到同一个结论**：独测 agent 的
`matching_epoch_lets_the_pipeline_observe_acceptance_and_then_apply_summary_writes`
正是按这条链写的（过闸 → 看到通报 → 调用方拿自己记的 `upto` 写入），
并且它顺手钉住了「`step` 这一步本身不写 `SendPlan`」。

### `prev` 实测

第一次压缩那条 entry 的 `prev` 合计 **62 字节**（`Summaries` 的 `{"Json":[]}` 11 字节
+ `SendPlan` 的 pristine 编码 51 字节），**与摘要正文长度无关**（100 字节和 10 KB
两份摘要，`prev` 逐字节一样大），也与被摘要的历史长度无关。第 N 次压缩的
`prev` 含前 N−1 份摘要正文——那是本 issue 验收里刻意放宽的那一条，独测
`the_second_compactions_prev_may_carry_the_first_summary_but_never_raw_history`
把「可以含摘要、绝不含原文」钉住了。

### 顺带做的一次拆分（不是顺手重构，是本次改动的一部分）

`graph/slot.rs` 加 `Slot::Summaries` 之后 **317 行**，顶破 300。它的模块文档自己
写着「只回答两个问题：一个槽位怎么称呼、它没有值的时候是什么」——两个问题就是
两件事，于是按那条线拆开：名字（枚举 / 逻辑键 / `ALL`）留在 `slot.rs`（242 行），
缺席值搬进新的 `slot_default.rs`（139 行，含 3 条内联单测）。`graph/mod.rs` 的
文件表跟着加一行。

### 命令输出

```
$ cargo test --workspace                                        全绿（agent-core it 375 + 19 条 apply_summary 独测）
$ cargo clippy -p agent-core --all-targets -- -D warnings        干净
$ cargo clippy -p agent-runtime --all-targets -- -D warnings     5 个存量错误，零新增
$ bash scripts/check-invariants.sh --all                         exit 0；17 条行数提示全是存量文件
$ cargo test -p agent-server --features ts                       84 + 109 passed
```
