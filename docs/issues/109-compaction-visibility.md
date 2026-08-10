# 109 被摘要盖住的段在时间线上可见

**里程碑** M12 · **依赖** [107](107-summary-writeback.md) · **模型** sonnet · **独立测试 agent** 否 · **状态** 完成

## 目标

让人看得见压缩发生了什么：哪一段被摘要盖住了、盖住的原文长什么样、清了哪些工具返回。

## 为什么这条不能砍

五档里第 3 档是唯一「**丢了你不知道**」的——清工具返回你知道清了哪几个，
要用重跑一次就行；摘要看起来永远是完整的，模型会照着摘要里那句话往下编细节，
**不报错**（[IMAGES.md](../IMAGES.md) §四第三条踩过同一个坑：
模型答了句「这是一张发票」不等于信息都提出来了）。

完整记录本来就在库里（095 的分界），能看见是白捡的——不做才是浪费。
这也是 [OBSERVABILITY.md](../OBSERVABILITY.md) 那条线的自然延伸：子 agent 状态给人看，
压缩状态同理。

## 做什么

时间线上标出压缩点，可展开看被盖住的原始轮次；被清的工具返回给一个可展开的占位，
不是凭空消失。

## 接线约束（2026-08-10 主会话定）

本条不预先钉死协议形状——**先设计再报告**，但下面五条是硬的：

1. **展开原文走完整记录那条链，不经过 `SendPlan`。**
   `SendPlan` 回答的是「发什么」，这里要的是「有什么」。走错了会看到压缩后的视图，
   那等于没做（issue「注意」一节原文）。
2. **`/undo` 一次，压缩标记跟着还原。** 这是 [090](090-image-undo-timeline.md) 的教训：
   当时 server 侧历史已经恢复，浏览器时间线却在 undo 后仍留着图。**别再犯一次。**
3. **动了任何进协议面的 Rust 类型就要跑** `cargo test -p agent-server --features ts`，
   红了用 `cargo run -p agent-server --features ts --example gen_protocol_ts` 重新生成
   （WORKFLOW §四第 4 步）。`Notice` 已经因为 105 变成混合 union 了，
   动它会连累 `packages/web/src/render/notice.ts`——那次的教训见 105 实做记录。
4. **前端要能区分两种压缩痕迹**：被摘要盖住的一段（有摘要正文可看）
   和被清掉的工具结果（占位文本 `CLEARED_TOOL_RESULT`，点开能看原文）。
   它们是两档，不该长成一个样子。
5. **摘要正文从 `Slot::Summaries` 取**（107 落地的），不要从子 agent 那边找
   ——那个子 agent 已经被 108 回收了。

## 验收

- 时间线上能看到压缩发生在哪一轮
- 点开任一摘要，看到它盖住的**原始轮次**（从完整记录读，不是从摘要反推）
- 被清的工具返回显示为占位而非凭空消失，点开能看到原文
- **`/undo` 一次，压缩标记跟着还原**——这是 [090](090-image-undo-timeline.md) 的教训：
  当时 server 侧历史已经恢复，浏览器时间线却在 undo 后仍留着图。别再犯一次
- 连续两次压缩：两个压缩点都在，第一次的摘要仍然能展开（107 定了摘要正文不回收）

## 注意

- 不碰红线，不派独立测试 agent
- 展开原文走的是完整记录那条链，**不经过 `SendPlan`**——`SendPlan` 是「发什么」，
  这里要的是「有什么」。走错了会看到压缩后的视图，那就等于没做


## 实做记录（2026-08-10）

### 协议加了什么、为什么是这个形状

**两个离散 SSE 事件**（不是重播快照、不是 `Notice`）：

- `SessionEvent::CompactionApplied { turn_id, upto, summary_id }`
  —— `apply_summary` 成功后从 `compact_writeback::after_step` 发
- `SessionEvent::ToolResultsCleared { turn_id, call_ids }`
  —— `clear_tool_results` 真的清到东西时从 `compact_ladder::fire_once` 发

core 的 `Notice::CompactionSummaryReceived` / `CompactionFailed` 刻意不带 `upto`
（105 的裁决：状态不是通报），所以这两个事实只能住在上面一层——那里才知道
`upto` / `call_ids`。

**否掉的更大方案**：照 048 的 `AgentTree` 做「每次相关状态变化都重播一份快照」。
压缩事件是**稀疏离散**的（一轮最多一次），不像树的活动那样连续变化，重播是杀鸡用牛刀。
改成事件里带 `turn_id`，前端拿它跟 undo/redo 帧里**已有的** `turn_id` 对上就行，
不用新开一套 diff 快照管道。

**一个新端点** `GET /sessions/{id}/compaction_record` → `{ messages, summaries }`：
`messages` 是 `Session::messages_of` **原样**（永不经 `project`，接线约束第 1 条），
`summaries` 是 `Slot::Summaries` 原样（第 5 条）。前端点开标记时**才**懒加载，
一次取回同时服务两种痕迹（摘要盖住的轮次 = `messages[0..upto]`；
被清工具结果的原文 = 按 `call_id` 查）。走新的 `ActorMessage::ReadCompactionRecord`
邮箱查询而不是「每次 step 都重算+比较+推送」的共享格——这是用户点击才触发的偶发读。

**顺带**：工具结果原文以前**从没送到过浏览器**（`ToolExecuted` 只带 `output_len`），
这个端点是它第一次可见。

### undo 一致性怎么保证的（090 的教训）

`apply_summary` / `clear_tool_results` 落在**触发它们的那一轮的同一个 `turn_id`** 里
（读 `runner.rs` 的泵循环确认：它会把整棵树、包括摘要子 agent 驱动到静止才返回）。
前端给每个标记打上创建时的 `turn_id`，undo/redo 帧带着同一个 `turn_id`，
于是 `compactionTimeline.undo(turnId)` 精确摘掉那一轮的标记。

**没有用「一次 undo 弹一个」的 LIFO 假设**——那对稀疏的压缩标记本来就是错的
（`user_input.ts` 能用 LIFO 是因为它 1:1 对应轮次）。

### 两档痕迹前端怎么区分

🗜「压缩：生成了一份摘要，覆盖前 N 条消息」（展开看摘要正文 + 被盖住的原始轮次）
vs 🗑「已清除 N 个工具结果」（展开看每个 call 的**原文**，不是 `CLEARED_TOOL_RESULT`）。
共用 `buildMarker` 但各有内容渲染器，不会塌成一个样子。

### 三个层次各做了变异检验

- `agent-core`：`undo_takes_the_summary_back_out_of_the_library`
- `agent-runtime`：把 `ctx.emit(...)` 掐掉 → 断言 `turn_id`/`upto`/`summary_id`/`call_ids`
  的那两条红
- `agent-server`：把 actor handler 里的 `messages` 写死成空 → 端点那三条红

### 路过的三次强制拆分

`agent-server/src/event/mod.rs`、`ts_protocol/fixtures.rs`、`agent-cli/src/print/events.rs`
（那个 332 行的存量超限文件）都被行数 hook 拦下，各自**按文件自陈的职责边界**拆开，
不是按行数硬切。四个预先点名的贴顶文件一个没碰。

### 命令输出

```
$ cargo test --workspace                      30/30 harness ok，0 failed
$ cargo test -p agent-server --features ts    85 + 112 passed
$ pnpm -r typecheck                           protocol + web 均 Done
$ pnpm --filter web build                     clean
$ bash scripts/check-invariants.sh --all      exit 0（存量从 17 降到 16——拆掉了一个）
```
