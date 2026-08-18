# 212 `srv:agent/await`：真订阅 + 等待图 + 建立时查环

**里程碑** M20 · **依赖** [205](205-core-peek-and-inbox.md) · **模型** **opus** · **独测** ✅ · **状态** ✅ 完成（2026-08-18，独测在飞）

## 目标

决策 204 §一 的「互相订阅」落地：**一个 agent 能挂起等另一个 agent 到达某个状态，
含兄弟。**

**本 issue 建出这个系统历史上第一条跨 agent 的依赖边。** 开工时查实（204 §一 末节）：
`args.get` 在生产代码里只有 `build.rs:103` 一处，读的是自己 agent 的 `ToolSlots`；
`read_ancestor`/`read_descendant` 走的是命令层的非追踪读，从来不建边。**所以在此之前，
全系统一条跨 agent 的边都没有。**

于是新红线 10（**边只许指向 primitive**）的落地与测试**都在这里**，不在 205——
205 那个口不建边，它证不了这条。

**依赖环在这里仍然不可能**，但理由要在本 issue 里由断言兑现：`Slot` 全是 primitive
（类型上的事实，`build.rs:47`/`:53`），所以这条新边是一条**长度 1 的悬边**，
绕不回来。**死锁才是真危险**，而它不报错——所以本 issue 的一半是那张等待图。

## 做什么

### 1. `srv:agent/await`

| | |
|---|---|
| 入参 | `{ id, until? }`。`until` 缺省 = 任一终态（`Done` / `Failed`） |
| 语义 | 调用者这个工具槽保持 `Pending`，直到 `id` 到达 `until` |
| 目标 | 本轮任意活 agent，**含兄弟**（204 §一）。不能是自己 |
| 内部 | 一个新 derived，读目标的 `Status`（primitive，叶子） |
| 可逆性 | `Aftermath::Nothing` → `Undoability::StateOnly`（纯读，不碰外部世界） |

`await` 与 `collect` 的分工要在工具描述里说清，否则模型会拿 `await` 当 `collect` 用：
**`await` 只告诉你「它到了」，不给正文；正文要 `collect`**（而 `collect` 本身就会等，
所以「等一个自己 spawn 的后台子」直接 `collect` 就行，用不着先 `await`）。
`await` 的用武之地是**等一个不归你领的**——兄弟，或者别人开的。

> 这是全系统第二种 derived。`build.rs:71` 那个 `let DerivedKey::ToolsConverged(agent) = key;`
> 的不可反驳 let 会**编译不过**，必须改成 `match`。这是好事：编译器逼着下一个人
> 在这里回答「新 derived 读了什么」。改的时候把 204 §一 那条判据抄进模块文档。

### 2. 等待图：是**状态**，不是内存里的表

新 primitive 槽 `Slot::AwaitingOn`（`Private`）：这个 agent 此刻在等谁。

- 必须是 journaled 状态：**恢复之后还得查得了环**。放内存里，一次崩溃恢复就把
  查环能力丢了，而丢了不报错。
- 有序容器（红线 11：它进 `await` 的拒绝文本）。
- undo 连带回滚——撤掉建立 `await` 的那一轮，等待边跟着消失。

### 3. 建立时查环，**不是卡住之后再救**

`await(id)` 建立等待边**之前**：从 `id` 出发顺着等待边走，**走得回调用者就是环** →
当场拒，回 `is_error` 的 tool_result。

拒绝文本要**说清是谁在等谁**（把环上那条链原样列出来），模型才知道怎么绕开——
照 `status_tool::not_a_descendant` / `collect_tool::not_collectable` 的既有写法：
拒绝要给出下一步，不是只说「不行」。

**为什么必须在建立那一刻挡**：卡住之后没有人有能力发现它。泵的静止条件是
`calls.is_empty() && mcp_calls.is_empty()`（`runner.rs:291`），两个互等的 agent
都在等 derived、都没有 provider 调用在飞——**泵会安静地返回**，留下两个永远
`ToolsPending` 的槽。没有 panic、没有超时、没有告警。

### 4. 等的对象死了怎么办

目标被 `despawn` / 撤销 / 随 turn 收尾拆掉 → 等待方的槽必须**收敛成 `is_error`**，
不能永远 `Pending`。这条跟 `collect_tool.rs:140` 的 `is_live` 闸是同一类防死等，
写法照抄。

## 验收

- **兄弟互等**：A `await` B（B 是兄弟）→ B 干完 → A 的槽收敛、A 继续跑。
  **这是「互相订阅」的行为证据。**
- **环被挡在门口**（本 issue 最硬的一条）：A `await` B 成功 → B `await` A →
  **B 那次当场拿到 `is_error`**，两个 agent 都没卡住，这一轮**正常结束**。
  断言拒绝文本里含 A 和 B 两个 id。
- **三角环**：A→B、B→C、C→A，第三条被拒。**两条边的直接互等不是唯一形状**，
  只查「目标是不是直接在等我」会漏掉它。
- **恢复之后仍然查得了环**：建一条 `await` → `kill -9` → 恢复 → 反向 `await`
  **仍然被拒**。等待图放内存里这条必红。
- **等的对象死了**：A `await` B → B 被撤销 → A 的槽收敛成 `is_error`，
  **不是永远 `Pending`**；这一轮正常结束。
- **`/undo` 掉建立 `await` 的那一轮** → 等待边消失，反向 `await` 从此放行。
- `await` 自己 / 不活的 id → `is_error`，这一轮继续跑完。
- **红线 6**：`await` 挂着时 `/undo` bump epoch → 目标后到的状态变化不会把一个
  已经作废的槽写活。
- **新 derived 的纯函数性**（红线 1）：read fn 里没有时钟、没有随机数、没有 IO；
  `check_invariants` 的 `check_derived_purity` 覆盖新文件。
- **边只许指向 primitive**（新红线 10 的落点，本 issue 独有的一条）：遍历
  `Slot::ALL`，对每一个构造 `AtomKey::Agent(id, slot)`，断言它落在 **source** family
  上、`derived` family 里没有对应项。这是「这条新的跨 agent 边是长度 1 的悬边」的
  直接证据。**哪天有人加了一个跨 agent 读 derived 的 derived，这条会红——那正是要它
  红的时刻**（本 issue 加的是新 `DerivedKey` + 一条指向 primitive 的边，不该红）。
- **`args.get` 的跨 agent 目标清单**：断言新 derived 的 read fn 只 `args.get` 了
  目标的 `Status` 这一个 primitive（不是「读了一堆恰好都是 primitive」）。
  形式可以是把「这个 derived 读哪些键」抽成一个可测的纯函数。
- `cargo test --workspace` 全绿 + `check-invariants --all` 过 + `build-wasm.sh` 绿。

## 注意

- **别造「检测死锁并挑一个牺牲者」的机制**（204 §五）。建立时挡得住的事，不该留到
  运行期再用更复杂的机械去救。
- **别让 `await` 顺手把目标的正文带回来**。那是 `collect` 的事，而且会把「任何 agent
  读得到任何 agent 的完整 transcript」这条 204 §一 明确不给模型开的路从侧门放进来。
- **别把等待图做成 derived**。它是「谁在等谁」的账，要被查环算法遍历；做成 derived
  就是让查环这件事本身去建边——那才真会造出环。
- `until` 别做成通配的条件表达式。枚举几个终态够用了，一个能表达任意谓词的参数
  等于让模型往 derived 的 read fn 里塞代码，红线 1 当场破。
