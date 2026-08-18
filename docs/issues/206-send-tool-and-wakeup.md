# 206 runtime：`srv:agent/send` + 定点排空 + turn 内唤醒

**里程碑** M20 · **依赖** [205](205-core-peek-and-inbox.md) · **模型** **opus** · **独测** ✅ · **状态** 待做

## 目标

决策 204 §二 的运行时半边：**一个工具把消息投出去，两个定点把消息喂进去
（本轮 / 下一轮），一条唤醒边把收信人拉回泵里。** 这是 M20 的核心增量，
也是唯一碰泵不变量的一条。

**范围切在「送达」这一刀上**：本 issue 只管消息**怎么到、什么时候到**。
`when="next_turn"` 的留言在这里的行为是「等着，下一轮开始时进 prompt」——
而那一轮由谁开起来，本 issue 不管。

**「留言自己把下一轮开起来」是 [211](211-auto-driven-turns.md)**（决策 204 §二，
用户拍板要自驱动）。这么切是因为 211 要动的是泵的停机边界和一道新预算闸，
而本 issue 要动的是消息进 prompt 的时机——两件事的失败模式完全不同，
混在一条 issue 里做，出了问题分不清是哪半边。

所以本 issue 单独验收时，`next_turn` 的留言**要等用户下一次开口**。
211 落地之后它才会自己跑起来，而**本 issue 的断言到那时一条都不用改**。

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

### 3. 唤醒：turn 内，且**不重置 `TurnsUsed`**（**只对 `Now`**）

`NextTurn` 不唤醒任何人——它等的就是下一轮。以下只说 `Now`。

收信人已经落终态（`Done` / `Idle`）时，`deliver` 之后要把它重新拉回泵：给它起一次
新的 provider 调用，那次调用照常过 `try_call_provider` 的 `max_turns` 闸
（`txn.rs:242`）。

- **子仍在父的同一次 `run_turn` 内生死**——`turn_id` 继承、undo 连带子树、`Subtree`
  局部绑定全部一行不改（ORCHESTRATION §二）。
- **`TurnsUsed` 绝不重置**（`txn.rs:274` 那条路是 `begin_turn` 的，唤醒不走它）。
  这是 204 §二 点名的、本里程碑唯一会静默出错的地方：写成重置，两个 agent 互相
  喊话就是真无界，不报错、测试也不红，只把 token 烧到见底。
- 撞顶（`used >= max_turns`）→ **收信人不被唤醒，消息留在收件箱里**，
  发送方那次 `send` 仍然算成功（它只负责投递）。孤儿收件箱在 turn 收尾按 §4 处理。

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

- **两个兄弟对话**：父 spawn A、B → A `send` 给 B → B 被唤醒、读到那条、回一条给 A →
  A 被唤醒、读到。**这是「横读开了」在行为面的证据。**
- **注入顺序**（本 issue 最硬的一条）：B 有一个 provider 请求在飞时给它投一条 →
  断言 B 的 `Messages` 里，**那次在飞请求的 assistant 回复排在被投递的那条之前**。
  写成「直接往 `Messages` 追加」这条必红。
- **唤醒不重置轮次预算**：`max_turns = 3` 的 agent 跑满 3 轮 → 给它投一条 →
  断言 `turns_used` 仍然是 3、**没有新的 provider 调用发生**、消息留在收件箱里。
- **互相喊话会停**：A 和 B 各自 `max_turns = 2`，写一对无脑互发的 fake provider →
  断言这一轮**会结束**，且 provider 调用总数 ≤ 活 agent 数 × `max_turns`。
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
  这一轮 → 那条留言**退回收件箱**（不是消失、也不是留在 `Messages` 里）；
  再 `/undo` 掉上一轮，它照旧在。
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
- 唤醒是本仓第一条「不由自己的状态驱动」的边。**在 `runner.rs` 模块文档里写清楚它**，
  连同停机边界从「树的大小」变成「树的大小 × 每人的轮次预算」这句话——
  那段文档是下一个人读懂泵为什么会停的唯一入口。
