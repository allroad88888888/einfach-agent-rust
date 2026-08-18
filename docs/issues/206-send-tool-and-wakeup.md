# 206 runtime：`srv:agent/send` + 两个定点排空

**里程碑** M20 · **依赖** [205](205-core-peek-and-inbox.md) · **模型** **opus** · **独测** ✅ · **状态** ✅ 完成（2026-08-18，见文末）

## 目标

决策 204 §二 的运行时半边：**一个工具把消息投出去，两个定点把消息喂进去
（本轮 / 下一轮）。**

**范围切在「送达」这一刀上**：本 issue 只管消息**怎么到、什么时候到**。

`when="next_turn"` 的留言在这里的行为是「等着，下一轮开始时进 prompt」——
而那一轮由谁开起来，本 issue 不管。**「留言自己把下一轮开起来」是
[211](211-auto-driven-turns.md)**（决策 204 §二，用户拍板要自驱动）。

> ## 唤醒拆出去了（2026-08-18，开工核查）
>
> 本 issue 初稿含「turn 内唤醒一个已终态的 agent」。**做不了，也不该在这里做。**
>
> `Effect::CallProvider` 只从 `try_call_provider` 一处发出，而它的四个入口
> （`user_input` / `tool_outcome` 收敛分支 / `provider_failed` / `timeout`）
> 每一个都要求那个 agent 正走在流程里。`on_user_input` 更是**明确拒绝终态**，
> 它的模块文档写着理由：「终态之后开新一轮走 `Session::begin_turn`，不是靠这里对
> 终态网开一面：那会把『一轮从哪开始』这个 turn 边界（`undo_turn` 的分组依据）
> 藏进一格转移里。」
>
> 所以唤醒要**新增一条 core 转移**，而它的语义有三个必须先答的问题（turn 边界算
> 谁的、`TurnsUsed` 怎么算、撞顶了怎么办）——正是 WORKFLOW §一 说的「中间必须停
> 下来看结果再决定下一步」。**单开 [214](214-wake-a-terminal-agent.md)。**
>
> **拆掉唤醒之后本 issue 仍然完整可用**：中途纠偏的目标本来就在跑（`drain_now`
> 在它下一次组装请求之前排空，正好赶上）；下轮留言走 `begin_turn` 之后那个定点，
> 跟唤醒无关。少掉的只有「让一个已经答完的子 agent 接着干」。

## 做什么

### 1. `srv:agent/send`

截获位置照 spawn / status / collect 同款（`dispatch.rs` 的 `Effect::ExecuteTool`
内按工具名截）。

| | |
|---|---|
| 入参 | `{ to: string, text: string, when?: "now" \| "next_turn" }`，`when` 缺省 `"now"` |
| 语义 | 往 `to` 的收件箱投一条（`Session::deliver`），**不等回复**、当场回写 |
| 阻塞 | 否 |
| 可逆性 | `Aftermath::Nothing` → `Undoability::StateOnly`（纯状态，没碰外部世界） |

**`when` 是用户拍的两档**（204 §二），工具描述必须把差别说给模型听，而且要说清
后果不同：

- `"now"` —— 加入本轮 loop，收信人**下一次请求**就带上。`to` 可以是本会话里任意活
  agent：后代、祖先、**兄弟**都行（204 §一 横读开了）。
- `"next_turn"` —— **这一轮结束之后**、下一轮开始时才送达。`to` **只能是 root**：
  子 agent 不跨 turn，投给别人等于投给一个下一轮不存在的收件箱。

拒的情形：不活、是自己、空正文、`next_turn` 且 `to` 不是 root——四种都回 `is_error`
的 tool_result 让模型自己收敛（决策 20 的哲学，跟 spawn / status / collect 一致），
不 panic、不卡这一轮。**第四种的拒绝文本要直说「子 agent 活不到下一轮，要留话就留给
root」**，否则模型只会换个 id 再试一次。

**拒绝文本里给出「你现在能发给谁」**——照 `status_tool::you_can_see` /
`collect_tool::you_can_collect` 的既有写法。清单顺序自己 `sort_by(AgentId)`（红线 11）。

### 2. 两个定点排空，都在组装请求**之前**

| 档 | 定点 | 调什么 |
|---|---|---|
| `Now` | 收信人 `try_call_provider` 之前 | `Session::drain_now(agent)` |
| `NextTurn` | root `begin_turn` 之后、**本轮第一次**组装请求之前 | `Session::drain_next_turn()` |

**不许在别处追加**——对方可能正有一个 provider 请求在飞，那个请求带的是旧消息列表，
回来的 assistant 消息会落在注入的那条**后面**，历史里因此长出一段「答非所问」
（204 §二）。**这不报错。**

收件箱里没有对应那一档的条目 → 什么都不做、不落 entry。

> `NextTurn` 的排空点在 `begin_turn` **之后**是刻意的：那条消息因此属于**新**的 turn，
> `/undo` 掉新这一轮会把它退回收件箱、老那一轮不受影响。放在 `begin_turn` 之前，
> 它会挂在上一轮尾巴上——undo 掉上一轮就把一条还没被读过的消息一起吞了。

### 3. 收信人已经终态怎么办：**不唤醒，条目留着**

`drain_now` 排空的定点在收信人**下一次组装 provider 请求之前**——所以它天然只服务
「还要再说话的那些」。收信人已经落终态（`Done`/`Failed`）时不会再有 `CallProvider`
发给它，**条目就原地留在收件箱里**，按 §4 在 turn 收尾告警。

**不在这里造唤醒**（见文首那段）：那要新增 core 转移，是 [214](214-wake-a-terminal-agent.md)。
214 落地之后本 issue 的断言**一条都不用改**——它加的是一条新边，不改这一条。

### 4. Turn 收尾：两档的命运不同

root 落终态、泵静止时（`runner.rs` 的 B 点）：

- **没被消费的 `Now` 条目 = 异常**。不重新驱动任何人（理由跟 ORCHESTRATION §四.4
  一样：一轮结束就是结束，把没人要的子拉起来跑只是浪费），但**面上给告警**，
  说清「有 N 条消息没被读到」。它们随所属 agent 被 `despawn_child` 一起消失。
- **没被消费的 `NextTurn` 条目 = 正常**。它们**必须原地留着**，等下一轮
  `drain_next_turn` 来收。**别把它们一起告警，更别一起清掉**——那正是这一档存在的
  全部意义。

> 这是本 issue 第二容易写错的地方：孤儿收尾今天是「收件箱非空就告警」的直觉写法，
> 加了 `NextTurn` 之后那个直觉会把正常情况报成异常，接着有人会「顺手清干净」。

## 验收

- **两个兄弟对话**（本 issue 的行为核心）：父 spawn A、B，**两个都还在跑** →
  A `status` 看到 B → `send` 给 B 一个中间结论 → **B 下一次请求的 prompt 里有那条**、
  它用上了 → B 回一条给 A → A 下一次请求里也有。全程没有任何一方被唤醒，
  靠的是「两边本来就还要再说话」——这正是 `drain_now` 那个定点服务的场景。
- **注入顺序**（本 issue 最硬的一条）：B 有一个 provider 请求在飞时给它投一条 →
  断言 B 的 `Messages` 里，**那次在飞请求的 assistant 回复排在被投递的那条之前**。
  写成「直接往 `Messages` 追加」这条必红。
- **投给终态的 agent 不唤醒任何人**：目标已经 `Done` → `send` 仍然成功（它只负责
  投递）、**没有新的 provider 调用发生**、条目留在收件箱里、turn 收尾按 §4 告警。
  这条是 [214](214-wake-a-terminal-agent.md) 落地之后**唯一会需要改的**——那时它变成
  「唤醒了，且 `turns_used` 没被重置」。在此之前它守的是「206 没有偷偷造一条唤醒边」。
- `send` 给自己 / 给不活的 id / 空正文 → `is_error` tool_result，这一轮**继续跑完**。
- **在飞时 `/undo`**（红线 6）：投递之后、收信人回执落地之前 bump epoch →
  那条回执被 `step.rs` 的 epoch 门丢掉，不写进任何人的历史。
- `/undo` 掉包含 `send` 的那一轮 → 收件箱与两边 `Messages` 全部回到投递之前，
  **不产生屏障**（`StateOnly`）。
- turn 收尾有未读的 **`Now`** 条目 → 面上有告警，且**轮次结果仍是 root 的终态**，
  不是 `Failed(Cancelled)`（照 ORCHESTRATION §四.4）。
- **`next_turn` 端到端**：后台子 agent 在轮末 `send(to=root, when="next_turn")` →
  这一轮**照常结束、不被延长**、面上**不告警** → 下一轮用户随便说句话 →
  那条留言**在这一轮的 prompt 里**，模型答得出来。
- **turn 收尾不吞 `NextTurn`**（§4 那条直觉陷阱）：收尾时收件箱里同时有一条 `Now`
  和一条 `NextTurn` → `Now` 那条告警，**`NextTurn` 那条一个字不少地还在**。
- `send(when="next_turn")` 投给一个子 agent → `is_error`，且拒绝文本里含
  「留给 root」这层意思，不是干巴巴一句「不允许」。
- **`/undo` 的归属**（§2 那条脚注）：下一轮 `drain_next_turn` 之后 `/undo` 掉**新**
  这一轮 → 那条留言**退回收件箱**（不是消失、也不是留在 `Messages` 里）。

  > **原文这里还有半句「再 `/undo` 掉上一轮，它照旧在」——写错了**（测试 agent
  > 落地时点出来的）。留言是在**上一轮**被 `deliver` 进收件箱的，那条 entry 就属于
  > 上一轮；undo 掉上一轮必然把 `deliver` 一起退掉，它只能消失。想说的是
  > 「**老那一轮不受影响**」（即 undo 新这一轮不会连带退掉旧的），而那正是上面
  > 那条断言本身。
- **崩溃恢复**：投一条 `next_turn` → `kill -9` → 恢复 → 它还在收件箱里，
  下一轮照常送达。
- `cargo test --workspace` 全绿 + `check-invariants --all` 过 + `build-wasm.sh` 绿。

## 注意

- **别用会话级 cancel 收尾**：它无 agent 字段，且会把轮次判成取消，而 root 明明答成功了
  （ORCHESTRATION §四.4 的既有结论）。
- **别给 `send` 加「等回复」参数**。那是 `send` + 一个阻塞槽，等于把 `collect` 重造一遍，
  而且会引入「A 等 B、B 等 A」的死等——今天没有任何机制能发现它。要等就让模型自己
  `send` 完接着干别的，回信到了它自然会看到。
- **别让 `send` 的截获调 `persist::sync`**：照 `status_tool` / `collect_tool` 的既有理由
  ——真正落日志的是随后经 `Session::step` 的那条，泵自己会转发。
  （`deliver` 是命令，它那条 entry 走的是命令层的常规路。）
- **本 issue 不动泵的停机边界**（唤醒才动，那是 214）。`runner.rs` 模块文档里那段
  「一轮结束 = root 终态且后台子静止」这次一个字不用改——排空是命令，不产生在飞调用。
  别顺手去改它，那会让下一个人以为唤醒已经落地了。

## 实做记录（2026-08-18）

六道门禁全绿：`cargo test --workspace` **2206 passed / 0 failed**；
`check-invariants --all` 退出码 0（红线 9 提示 12，与 207 之后的基线相同）；
`build-wasm.sh` 绿；`pnpm -r typecheck` 过；`clippy --all-targets -D warnings`
零 error；`cargo test -p agent-server --features ts` 全绿。

### 唤醒拆走了（见文首那段），本 issue 的范围因此小一圈

拆的判据不是工作量，是 `Effect::CallProvider` 的四个入口**都要求那个 agent 正走在
流程里**，而 `on_user_input` 明确拒绝终态并把理由写死在模块文档里。单开
[214](214-wake-a-terminal-agent.md)。

### 一条验收写错了，测试 agent 落地时点出来的

原文：「下一轮 `drain_next_turn` 之后 `/undo` 掉**新**这一轮 → 留言退回收件箱；
**再 `/undo` 掉上一轮，它照旧在**」。**后半句不成立**——留言是在上一轮被 `deliver`
进收件箱的，那条 entry 就属于上一轮，undo 掉上一轮必然把 `deliver` 一起退掉，
它只能消失。原意是「老那一轮不受影响」，而那正是前半句本身。已在 §验收 就地改正
并留了说明。

**这正是独立测试 agent 该抓的东西**：它只读规格，所以规格自相矛盾时它会撞上；
实现者写测试会不自觉地按实现的行为去理解那句话。

### 注入验证：头号那条是承重的

把「投递即追加」（`deliver` 之后当场 `drain_now`，绕过定点）注入进 `send_tool` →
**5 条红**，其中就有 `send_indep_injection_order`。那正是 204 §二 点名的、不报错
的坑。测试 agent 自己另做了两次变异验证（改断言方向确认真会红），实测下标
`reply@1 / injected@3`、`note@4 / ask@5`——是真实相邻的位置，不是空跑。

### `UnreadMessages` 一路接到四个壳，三道穷举 match 逐个逼出来

`RunnerEvent` 加一个变体，被三处无通配的 `match` 当场逮住：`agent-cli` 的
`print/events.rs`、`agent-server` 的 `SessionEvent`+`from_runner`+`ts_protocol`
（变体数钉子 19→20，重新生成 `packages/protocol/src/generated`）、`agent-wasm`
的 `events.rs`。

> **第 4 处 TS 没被编译器逮住。** `packages/web` 的 `switch` 没有 `never` 兜底，
> `pnpm typecheck` 照样过——`unread_messages` 会**悄悄什么都不显示**。是手工核出来
> 补的。这条记下来：**typecheck 过 ≠ web 会显示它**，加 `SessionEvent` 变体时
> web 那一处得自己去看，护栏在那儿是缺的。

（`build-wasm.sh` 那一道倒是逮住了 `agent-wasm`——157 那次「别假定 wasm 白拿」的
教训在这次真的兑现了一回。）

### 没能覆盖的四条（测试 agent 报的，都成立）

1. **「B 真的用上了」**——脚本化的假 provider 不会真用收到的信息，只能断「它进了
   B 下一次请求的 prompt」。要证明模型真用上，只有真机 dogfood（[213](213-agent-mesh-docs-and-dogfood.md) §二.1）。
2. **红线 6 用 `Event::Cancel` 推世代，不是 mid-turn `/undo`**——`run_turn` 是同步的，
   从外部拿不到那个时刻。沿用 `spawn_bg_epoch_writeback.rs` 的既有手法并在文档里标注。
3. **崩溃恢复那条**归持久化层，`inbox_indep_undo_restore.rs` 已覆盖「落盘往返带着
   `when` 回来」。
4. 门禁不在测试 agent 的范围里（它只跑 `agent-core` + `agent-runtime`），由实现方收官。
