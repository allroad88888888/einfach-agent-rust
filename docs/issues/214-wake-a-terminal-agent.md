# 214 唤醒一个已终态的 agent（**含 core 转移**）

**里程碑** M20 · **依赖** [206](206-send-tool-and-wakeup.md) · **模型** **opus** · **独测** ✅ · **状态** ✅ 完成（2026-08-18）

## 目标

让一个已经落终态的 agent **在同一个 turn 内**被重新拉回泵里——收到一条 `Now` 消息
就接着干，而不是让那条消息躺在收件箱里等到轮末告警。

这条是 [206](206-send-tool-and-wakeup.md) 开工时拆出来的：初稿把它写在 206 里，
落地前才查清**它做不了纯运行时的改动**。

## 缘起：为什么运行时一个人办不到

`Effect::CallProvider` 全系统只从 `try_call_provider` 一处发出
（`transitions/mod.rs:139`），而它的四个入口——`user_input`、`tool_outcome` 的收敛
分支、`provider_failed`、`timeout` 的 provider 超时分支——**每一个都要求那个 agent
正走在流程里**。

`on_user_input` 更是明确拒绝终态，而且它的模块文档把理由写死了：

> 别的状态收到 `UserInput` 都是非法——没有「排队等下一轮」……**终态之后开新一轮走
> `Session::begin_turn`**，不是靠这里对终态网开一面：那会把「一轮从哪开始」这个
> turn 边界（`undo_turn` 的分组依据）**藏进一格转移里**。

所以唤醒必须是**一条新的、名字叫得出自己在干什么的 core 转移**，不是把
`on_user_input` 的闸放宽——后者正是那段文档点名不要的形状。

## 三个必须先答的问题

**答错都不报错**，这也是它值得单开一个 issue 的原因。

### 一、这一步属于哪个 turn？

**属于当前 turn，不开新 turn。** 决策 204 §二 拍的是「turn 内唤醒」：子 agent 依旧
在父的同一次 `run_turn` 内生死，于是 `turn_id` 继承、undo 连带子树、`Subtree` 局部
绑定**全部一行不改**（ORCHESTRATION §二 的既有结论）。

开新 turn 就是 214 变成「跨 turn 复活已死的子 agent」——决策 204 §五 明确不做。

### 二、`TurnsUsed` 怎么算？

**照常计数，绝不重置**（`txn.rs:274` 那条重置路是 `begin_turn` 的，唤醒不走它）。

这是决策 204 §二 点名的、这一波唯一会**静默出错**的地方：写成重置，两个 agent
互相喊话就是真无界——不报错、测试也不红，只把 token 烧到见底。

唤醒后那次 provider 调用**照常过 `try_call_provider` 的 `max_turns` 闸**
（`txn.rs:242`），跟别的调用一视同仁。

### 三、撞顶了怎么办？

`used >= max_turns` → **不唤醒，条目留在收件箱里**，落回 206 §3 的行为（轮末告警）。
**不是**落 `Done{truncated:true}`——它已经是终态了，再落一次终态没有意义，而且会
把「因为预算耗尽而没被叫醒」和「自己正常答完了」两件事在状态上抹平。

## 做什么

### 1. core：一条新转移

形状待定（这是本 issue 要拍的第一件事），但要满足：

- 入口是**终态**（`Done` / `Failed`），别的状态一律 `protocol_violation`
  ——跟 `on_user_input` 只认 `Idle` 是同一种严格；
- **不 `push_message`**：消息已经由 `drain_now` 进了 `Messages`（206 的定点），
  这条转移只负责「再动起来」。**两处都写就是同一句话进两次历史**；
- 复用 `try_call_provider`，不新造一条发 `CallProvider` 的路（那条闸散着写就是多
  一个漏判的机会——`transitions/mod.rs:139` 的注释原话）。

### 2. runtime：在排空之后接上

`drain_now` 返回搬了几条（205 已经给了）。搬到了 **且** 目标是终态 → 发这条转移。
搬到了但目标还在跑 → 什么都不做，它下一次请求本来就会带上。

### 3. 泵的停机论证要重写

**这是本 issue 唯一动 `runner.rs` 不变量的地方。**

今天泵停得下来靠的是「没有任何 agent 能把别人重新拉起来」：工作量只随 spawn 往下长，
树被 `max_depth`/`max_children` 封顶，所以有限。加了唤醒边之后 A 能叫醒 B、B 能叫醒 A，
兜底只剩每个 agent 自己的 `MaxTurns`——**边界从「树的大小」变成「树的大小 × 每人的
轮次预算」**。

`runner.rs` 的模块文档里那段「一轮结束 = root 终态且后台子静止」要按这个重写，
并写清新的上界。那段文档是下一个人读懂泵为什么会停的唯一入口。

## 验收

- **唤醒真的发生**：目标 `Done` → 投一条 `Now` → 它**真的又发了一次 provider 调用**、
  prompt 里有那条消息、它答了。
- **`TurnsUsed` 不重置**（本 issue 最硬的一条）：`turns_used = 2` 的终态 agent 被唤醒
  → 断言唤醒后是 **3**，不是 1。写成重置这条必红。
- **撞顶不唤醒**：`max_turns = 3` 跑满 3 轮的 agent → 投一条 → **没有新的 provider
  调用**、条目留在收件箱里、状态还是原来那个终态（不是被改写成 `Done{truncated:true}`）。
- **互相喊话会停**：A 和 B 各自 `max_turns = 2`，一对无脑互发的 fake provider →
  断言这一轮**会结束**，且 provider 调用总数 ≤ 活 agent 数 × `max_turns`。
- **不重复进历史**：唤醒那条转移不 `push_message` —— 断言被投递的正文在目标的
  `Messages` 里**恰好出现一次**。
- **turn 边界没变**：唤醒前后 `turn_id` 相同；`/undo` 那一轮连带把唤醒后干的活一起退。
- **非终态收到这条转移 → `protocol_violation`**，不是悄悄放行。
- `cargo test --workspace` 全绿 + `check-invariants --all` 过 + `build-wasm.sh` 绿。

## 注意

- **别放宽 `on_user_input` 的闸**。那是这条 issue 存在的全部理由（见§缘起）。
- **别在唤醒里 `push_message`**。消息由 206 的 `drain_now` 进历史，两处都写就是重复。
- **206 的断言只该改一条**：「投给终态的 agent 不唤醒任何人」变成「唤醒了，
  且 `turns_used` 没被重置」。别顺手动别的。

## 实做记录（2026-08-18）

### 形状拍板：`Event::Wake { agent, epoch }`

「形状待定」那一条落成一个**新事件变体**，转移住 `command/transitions/wake.rs`。
三个选择及理由：

- **是事件不是命令**：`Effect::CallProvider` 只能由转移产出，而转移只由
  `Session::step(event)` 驱动。写成一条 `Session::wake()` 命令的话，它要么自己
  造 effect（绕开转移表，红线 2 的精神破了），要么返回一批 effect 给调用方
  （命令层从此有两种返回形状）。
- **带 `epoch`，过闸**：泵决定唤醒和真的 `step` 之间，用户可能已经 Ctrl-C 或
  `/undo`。唤醒一个已经被埋掉的世代 = 在没人要的世界里重新起一次 provider 调用，
  花钱且不报错。`Event::agent()` / `Event::epoch()` 那两个穷举 match 当场逼着
  回答了这两个问题——它们存在的全部理由就是这个。
- **不进 5 态 × 7 的网格**：它只有一格合法（终态），其余全是 `protocol_violation`，
  摊进网格只会得到五行里四行同样的答案。跟 105 的两条压缩事件同一条理由，
  转移表模块文档里照那两条的先例记了一句。

### 撞顶那一格没走 `try_call_provider`

`try_call_provider` 撞顶时落 `Done{truncated:true}`。被唤醒的 agent **本来就是
终态**，再落一次终态会把「因为预算耗尽而没被叫醒」和「自己正常答完了」在状态上
抹平。所以 `on_wake` 在调用它**之前**自己问一次新加的 `Txn::turns_exhausted()`
（`record_turn_attempt` 也改成复用它，两处判据从此是同一行代码），撞顶就
`Vec::new()` ——不写 primitive ⇒ 空 batch ⇒ `History::append` 拒绝空步 ⇒ 不留 entry。

### 唤醒点在 `send_tool`，不在泵里扫

`send_tool::wake_needed`：`Deliver::Now` + 目标已终态 → 额外产出一条
`Event::Wake`（`Dispatched::Events`，复用后台 spawn 那条既有的「一次 effect 两件事」
的路）。**顺序是先 `ToolResult` 再 `Wake`**：反过来收信人的活会排在发送方那条
result 之前，而那条 result 才是发送方接着往下走的凭据。

**这里不 `drain_now`。** 唤醒转移发出 `Effect::CallProvider`，而 206 的排空定点
就在它的派发处——在投递处再排一次就是同一句话搬进 `Messages` 两遍。验收里
「被投递的正文恰好出现一次」守的就是这个。

### 泵的停机论证重写了（`runner.rs` 模块文档）

052 之前不用问：工作量只随 spawn 往下长、树被两道闸封顶，**没有任何 agent 能把
别人重新拉起来**。214 加了唤醒边之后那条结构性论证没了，新的上界是
**活 agent 数 × 各自的 `max_turns`**——「树的大小」变成「树的大小 × 每人的轮次
预算」。写错的形状只有一个而且不报错：唤醒时重置 `TurnsUsed`。

### 顺带修一处 206 留下的漏

`unread_reported` 那个「只报一次」的闩要能**重新上膛**：唤醒会把 root 从终态拉回来
接着跑，之后它会再落一次终态，而那一次可能有新的漏读。一路 `true` 到底的话第二次
终态一条都报不出来——不报错。

### 206 的两条测试都动了，不止一条

issue §注意 说「只该改一条」。实际是两条，第二条是**规格没预见到的连带**：
`turn_end_warns_about_the_now_item_and_keeps_the_next_turn_one_untouched` 靠「`Now`
那条剩在收件箱里」跟 `NextTurn` 做对比，而 214 之后 `Now` 不会再剩下。改法是把它
重新聚焦到它真正守的东西（`NextTurn` 原地不动、且从不被当成未读），并在文件头
写明「对比那一半移交给 214 自己的『撞顶不唤醒』用例」——那时 `Now` 才会真的剩。

### 红线 9：`runner.rs` 顶破 500 硬上限 → 拆出 `runner_entry.rs`

模块文档加了停机论证那一段之后 503 行。拆的位置是**三个入口**
（`run_turn_async` / `run_turn` / `resume_async`）：它们只在「进泵之前允许重置
什么」上不同（取消标志、`NextTurn` 收件箱），而那正是一格写错了不报错的判断
——`resume_async` 清了取消标志，用户在等待期间按的 Ctrl-C 就被抹掉了。
泵本体留在 `runner.rs`（417 行，仍在复杂文件的 500 之内）。
