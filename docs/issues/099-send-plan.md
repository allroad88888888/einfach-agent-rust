# 099 `SendPlan` 与投影纯函数

**里程碑** M12 · **依赖** [095](095-compaction-tiers.md) + [096](096-compaction-trigger.md) · **模型** opus · **独立测试 agent** 是 · **状态** 完成

## 目标

M12 主干的地基：一个描述「这一轮发什么」的纯值，和一个把它作用到完整历史上的纯函数。

## 为什么类型和函数不拆开

[WORKFLOW](../WORKFLOW.md) §一明写「写数据结构 → 写方法」是坏的拆法，第一个验证不了
任何东西。`SendPlan` 单独落地能断言的只有 serde 往返；加上投影才有一个可判定的中间态：
**给一份完整历史和一份 `SendPlan`，投影出的历史逐字节等于期望**。

## 做什么

三字段（形状见 095，别在这里重新讨论）：

| 字段 | 装什么 | 谁改 |
|---|---|---|
| 已清列表 | 被清掉的 `ToolCallId` | 第 2 档 |
| 边界 | 从第几条开始发 | 第 3、4 档 |
| 摘要引用 | 边界之前那段的摘要在哪 | 第 3 档 |

投影签名形如 `(完整历史, &SendPlan) -> 要发的历史`。**零 IO、零时钟、零随机。**

## 定死的接口（2026-08-10 主会话定，实现与测试都照这个来）

读过 `value/message.rs` 之后对 095 的一处修正：**`ToolUse` / `ToolResult` 是
`Message.blocks` 里的块，不是独立消息**。所以「清工具返回」清的是块，不是消息。
由此得到一个比 095 原设想更安全的做法——见下面 `CLEARED_TOOL_RESULT`。

```rust
// crates/agent-core/src/value/send_plan.rs（文件位置可按 one-file-one-thing 调整）

/// 被清除的工具结果在 prompt 里的占位文本。
///
/// **逐字节确定**（红线 11）：只有固定文本，无时间戳、无 id、无大小数字。
/// 跟 004 的截断标记同一套纪律。
pub const CLEARED_TOOL_RESULT: &str = "（工具结果已清除以腾出上下文；需要请重新调用）";

/// 这一轮实际要发给 provider 的历史长什么样。完整历史永远不变，变的只有它。
///
/// 字段全私有：三个不变量（已清列表去重且保序、边界只增、摘要与边界同进同退）
/// 由方法维护，不让外部直接摆弄内部坐标系。同 `persist::SessionLog` 的做法。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)]
pub struct SendPlan { /* 私有 */ }

impl SendPlan {
    /// 恒等元：不清任何东西、边界 0、无摘要。投影它等于完整历史。
    pub fn new() -> Self;

    /// 从没压过。`encode` 用它走「逐字节不变」的快路。
    pub fn is_pristine(&self) -> bool;

    pub fn cleared(&self) -> &[ToolCallId];
    pub fn boundary(&self) -> usize;
    pub fn summary(&self) -> Option<&SummaryId>;

    /// 第 2 档。**幂等**：已在列表里的不重复加入；保持**首次加入的顺序**
    /// （红线 11——顺序变了序列化就变了）。
    pub fn clear_tool_results(&mut self, ids: impl IntoIterator<Item = ToolCallId>);

    /// 第 3、4 档。**边界只能前进**：`next <= self.boundary()` 返回 `Err`，
    /// 不静默忽略。摘要与边界同进——一次调用改两个字段，不给中间态留缝。
    pub fn advance_boundary(
        &mut self,
        next: usize,
        summary: Option<SummaryId>,
    ) -> Result<(), BoundaryNotAdvancing>;
}

/// 投影。**纯函数**（红线 1）：零 IO、零时钟、零随机、不读全局。
///
/// - `summary_text` 由调用方从别处取好再传进来——摘要正文是大值，不住 `SendPlan`
///   里（红线 5）。传 `None` 而 `plan.summary()` 是 `Some` 时，视为摘要还没到，
///   **边界不生效**（宁可多发，不可发一段引用不到正文的空洞）。
/// - 清除工具结果 = 把 `ContentBlock::ToolResult.content` 换成
///   [`CLEARED_TOOL_RESULT`]，**`ToolUse` 与 `ToolResult` 块都留在原地**。
///   这样配对天然不破（有的 provider 见到落单的 `ToolUse` 直接 400），
///   而 `ToolUse.input` 通常远小于结果正文，省下的还是大头。
/// - 投影后为空的消息（所有块都没了）整条丢弃——空消息发出去也是 400。
pub fn project(
    history: &Vector<Message>,
    plan: &SendPlan,
    summary_text: Option<&Arc<str>>,
) -> Vec<Message>;
```

**「换成占位」而不是「删掉块」是本 issue 唯一的形状判断**，理由三条：
配对天然不破（095「注意」那条硬约束直接消失）；模型知道自己调过这个工具、
结果没了、要用得重调，比假装没调过更诚实；省下的字节仍然是大头
（`ToolUse.input` 是几十字节量级，结果上限 32 KiB）。

## 验收

- **同一份 `(历史, SendPlan)` 投影 1000 次，输出逐字节相同**
- 空 `SendPlan`（无已清、边界 0、无摘要）投影出的历史**等于**完整历史——恒等元存在，
  这条保证「不压缩」不是一条特殊路径
- 已清列表里的 `ToolCallId`：对应的 `ToolResult.content` 变成
  `CLEARED_TOOL_RESULT`，**`ToolUse` 块原样保留**——投影结果里
  `ToolUse` 与 `ToolResult` 的 id 集合恒等，任何输入下都不出现落单的一半
- 投影后所有块都没了的消息**整条不出现**（空消息发出去是 400）
- `plan.summary()` 是 `Some` 但 `summary_text` 传 `None`：**边界不生效**，
  投影出完整历史而不是一段引用不到正文的空洞
- 边界之前的消息不出现；摘要引用非空时，摘要作为一条消息出现在最前面
- `SendPlan` serde 往返；序列化结果里**不含** `HashMap`/`HashSet`
  （容器类型用 `Vec` 或 `BTreeSet`）
- 摘要正文**不在** `SendPlan` 里——`SendPlan` 序列化后的大小不随摘要长度增长

## 注意

四条红线一起踩，这是本 issue 用 opus 且必派独立测试 agent 的原因：

- **红线 1**（纯函数）——投影不纯，它就不能当 derived，整个「压缩只在发送侧」的方案塌掉
- **红线 3**（primitive 可序列化）——`SendPlan` 要进快照
- **红线 5**（大值 `Arc`）——摘要正文放别处，这里只有 id
- **红线 11**（逐字节确定）——投影结果进 prompt

`tool_use`/`tool_result` 成对那条不在红线里但同级：只清一半，有的 provider 直接 400。
已清列表存 `ToolCallId` 而不是消息下标，就是为了配得成对。

## 实做记录（实现 agent + 独立测试 agent 并行，2026-08-10）

接口按上面「定死的接口」一节**一字不差**落地，两个 agent 拿同一份签名，
测试 agent 不读实现体（[WORKFLOW](../WORKFLOW.md) §三）。

### 落地的文件

| 文件 | 行数 | 职责 |
|---|---|---|
| `agent-core/src/value/send_plan.rs`（新建） | 233 | `SendPlan` 纯值：三个私有字段 + 维护三条不变量的方法 + `BoundaryNotAdvancing` |
| `agent-core/src/value/send_plan/project.rs`（新建） | 287 | 投影纯函数 `project` + `CLEARED_TOOL_RESULT` |
| `agent-core/src/ids.rs` | 175（+33） | 新增 `SummaryId` |
| `agent-core/src/value/mod.rs` | +4 | 挂 `pub mod send_plan` |
| `agent-core/src/lib.rs` | +7 | 根导出 |
| `agent-core/tests/it/send_plan.rs`（新建） | 189 | 类型级不变量（独测） |
| `agent-core/tests/it/send_plan_project_clearing.rs`（新建） | 228 | 第 2 档投影行为（独测） |
| `agent-core/tests/it/send_plan_project_boundary.rs`（新建） | 142 | 边界/摘要投影行为（独测） |

`project` **拆进子模块**：合成一个文件 ~500 行，而且状态（要进快照）与纯函数
（要能当 derived）本来就是两件事。测试 agent 那份原本 335 行，被行数 hook 拦下，
按「清除档 / 边界档」两个场景拆成两个文件——两次拆分都是被规则逼出来的，
不是事后补的。

### 设计判断（实现 agent 裁决，主会话复核后收）

1. **`Vec` 而不是 `BTreeSet`**：`cleared() -> &[ToolCallId]` 只能由连续内存给出，
   且「首次加入顺序」是定死的语义。去重用线性 `contains` + 追加。
2. **`advance_boundary` 的摘要是赋值不是合并**，`None` 清掉——第 4 档清窗口正是
   走 `None` 这一支：窗口一清，旧摘要描述的那段已经不是新边界之前的全部，
   留着就是一句对不上号的话。拒绝路径**不留痕**（先校验再写）。
3. **`is_error` 清结果时保持原样**：结果没了不代表当时没出错，翻成 `false`
   等于替模型改了一次历史判断。
4. **边界不跟历史长度校验**：`SendPlan` 不知道历史多长。越界边界在投影里退化成
   「一条正文都不发」，不 panic。
5. **`project` 不提 crate 根**（类型和常量提了）：沿用 `lib.rs` 里既有裁决
   「函数名说不清就不提根」——裸的 `agent_core::project(...)` 说不出在投什么。

### 主会话复核修正的一处

`SUMMARY_MESSAGE_ID = MessageId(0)` 这个**结论是对的，实现 agent 给的理由是错的**。
它写的是「现铸『比最大还大 1』会破坏逐字节确定性」——两处都错：现铸方案对**固定
入参**照样确定，1000 次那条测试根本抓不到它；`MessageId` 也不进 wire
（`wire/messages.rs` 里只有测试模块引用它），跟前缀缓存无关。

正确的理由是：**投影的输出只该取决于 `(历史内容, plan)`，不该取决于历史的 id
编号**——内容相同、编号不同的两份历史必须投出相等结果，否则 `PartialEq` 判等和
任何 golden 断言都会随编号漂。注释已改，并留了一句「这个理由写错过一次」。

文档注释里一个错的理由比没有理由更糟，下一个人会照着它推。

### 变异检验（主会话做，不是 agent 自评）

注入 095 原设想的做法——清除时**删掉** `ToolResult` 块（会留下落单的 `ToolUse`）：

```
send_plan_project_clearing::project_clears_tool_result_content_but_keeps_tool_use  FAILED
send_plan_project_clearing::project_tool_use_and_tool_result_ids_never_go_orphan   FAILED
```

实现侧的内联单测 `clearing_swaps_the_result_and_keeps_the_tool_use` 同时红。
两层都抓得住。已还原。

### 给测试 agent 设的陷阱题（答对了）

「加入顺序不同、id 集合相同的两个 `SendPlan`，序列化结果该相等还是不等」——
接口说的是「保持首次加入的顺序」，正确答案是**不等**。它写的是
`assert_ne!(plan_ab, plan_ba)` + `assert_ne!(s_ab, s_ba)`，没有默认集合语义。
写成相等就说明红线 11 那条没吃透。

### 命令输出

```
$ cargo test --workspace
1587 passed; 0 failed

$ cargo test -p agent-core
106 lib（新增 14 内联单测）+ 316 it（新增 23 独测）+ 6 doctest，全过

$ cargo clippy -p agent-core --all-targets -- -D warnings
干净

$ bash scripts/check-invariants.sh --all
新增/改动的文件零违规；17 条行数提示全是存量文件
```
