# 模型侧编排：异步子 agent（spawn background / status / collect）

接缝定义文档。管「模型如何**中途观测子 agent 并据此改变编排**」这一件事。与
[OBSERVABILITY.md](OBSERVABILITY.md)（**给人看**的活树面板）成对：那个是人类视角，这个是
**模型视角**。落地里程碑 **M8**，issue 见 051-054。

## 一、这不是「加并行」——并行早就有

先钉死一个容易搞错的前提：**并行 spawn 现在就能做**。`srv:agent/spawn` 是普通 tool call，
模型在一条 assistant 消息里发多个 spawn，它们并发跑（STATE-MODEL §并发：「子 agent 的并发
是 IO 并发」——provider 调用泵在同一条线程上的并发 future 里并行，回写串行）。

所以本里程碑的价值**不是「能并行」**，而是唯一多出来的那件事：

> **模型能在子 agent 还在跑的时候看它们在干啥、并据此改变后续编排** ——
> 而不是「发 N 个然后干等全部一起回来」。

当前（决策 20，阻塞 spawn）模型发 N 个 spawn 后**结构性地卡在 `ToolsPending`**，全部收敛
才拿到全部结果，中途看不到、反应不了。本里程碑补的就是「看得到、反应得了」：先 collect
快的、据此再 spawn、避开挂住的慢的。这正是用户要的「模型去获取子 agent 相关」。

## 二、关键决策：子 agent **不跨 turn**（turn 内异步）

**背景**（Explore 勘查钉死，见 runner.rs 模块文档 §20-27）：现在这套干净——`turn_id` 由
root 分配、子 agent 继承 spawn 那轮的 turn_id、不产生 turn 边界、`undo(turn)` 连带整棵子树
——**全靠「子 agent 在父的同一个 turn 内生死」**。阻塞 spawn 就是这样：spawn→跑→结果回写
全在父这一次 `run_turn` 里。

pump 的静止条件是 `calls.is_empty()`（043 后加上 `&& mcp_calls.is_empty()`），模块文档
（`runner.rs:20-27`）把「root 已终态、子树还在跑」称作无定义状态并拒绝它。

> **后来的修正**（主会话 opus 读 043 后的 `runner.rs`，2026-08-04）：那句「无定义」是**过虑**。
> 后台子自己的 provider 调用就住同一张 `calls` 表里，所以 root 终态时泵**自然继续**驱动子、
> 直到全静止才返回——语义天然是「一轮结束 = root 终态 **且** 后台子静止」，不会卡死。
> 真问题只是**浪费**（把没人要的子跑到底）。这不动摇下面的决策（跨 turn 的难点在别处：
> 跨 `run_turn` 的绑定映射、`turn_id`/undo 语义、per-child 取消），但把「唯一动 pump 不变量
> 的地方」降级成了「B 点加一道定点 despawn」。详见 §四.4。

**决策：把大版本收敛到「turn 内异步编排」——子 agent 仍不跨 turn。**

- 后台 spawn 返回句柄、可并发 observe/react，但**父这一 turn（一次 `run_turn`）结束前，所有
  后台子 agent 必须已 collect 或被拆掉**。没 collect 的孤儿在 turn 收尾由
  `despawn_child` 定点拆（迟到结果撞 `is_live` 闸被丢），面上给告警——**不走会话级 cancel**
  （它会把轮次判成 `Failed(Cancelled)`，而 root 明明答成功了）。
- 于是子 agent 依旧在一次 `run_turn` 内生死：`turn_id` 继承、undo 连带子树、`Subtree` 局部
  绑定（`runner.rs:95` 每次 `resume` 重建）**全部一行不改**。
- 换来的能力完整：spawn N 个 → `status` 看谁快谁慢谁挂 → 先 collect 快的 → 据此再 spawn /
  收尾。**中途可观测可反应**，且 undo 不破。

**代价（诚实标注）**：真·跨 turn 后台 agent（turn N spawn、turn N+2 才 collect，像 shell 的
`run_in_background`）**不做**——它要 store 落地的 pending-slot 映射跨 `run_turn` 重挂
（Explore 问题 3）、要重写 `turn_id`/undo 语义、要 per-child 取消（Explore 问题 5）。等真实
使用反馈证明「turn 内」不够再开，别提前造。

## 三、三个工具（都 `Server` 位置，dispatch 截获）

截获位置照 spawn/skill 同款（`dispatch.rs:70` 的 `Effect::ExecuteTool` 内按工具名截）。

| 工具 | 语义 | 阻塞? | 可逆性 |
|---|---|---|---|
| `srv:agent/spawn` 加 `background: bool` | `false`（缺省）= 现状：槽位 `Pending` 到子收敛、结果回父（决策 20 **不变**）。`true` = 建子 + **立刻回写 `{agent_id}` 到 spawn 槽**（父不被挡）+ 记进 detached 名单 | 否（bg） | `Reversible`（同现状，补偿 = despawn_child） |
| `srv:agent/status(id?)` | **非阻塞**下读：`agent_tree()` 收窄到调用者子树，返回每个后代的 `AgentActivity`（Idle/Thinking/Working{tools}/Done/Failed）+ task + depth。不含子的消息正文 | 否 | `Pure`（纯读，无屏障） |
| `srv:agent/collect(id)` | 下读子 agent 的最终结果。子已终态 → 立刻回写；仍在跑 → 绑一个 `ChildSlot` 到本 collect 槽，槽 `Pending`、父 `ToolsPending`，pump 驱动子、harvest 回写 —— **就是现有阻塞 spawn 的 harvest，只是 key 在 collect 而非 spawn** | 是（子未完时） | `Irreversible`? 否——纯读子结果，无副作用，`Pure`；子自己的不可逆操作带自己的屏障 |

**漂亮之处：前台 spawn ≡ spawn(bg) + 紧跟 collect 融进一个槽。** 后台把这俩拆开，中间塞进
observe/react。决策 20 是这个模型的一个特例，不是被推翻。

> **那两道闸的数字从哪来**（决策 32，M18）：`AgentLimits { max_depth, max_children }`
> 默认 3/8，但**是部署方可配的参数**，不是硬编码——`agent-server`/`agent-cli` 的
> `--max-agent-depth` / `--max-children`（`AGENT_MAX_*` 兜底）。写进工具描述给模型看的
> 那份和 `Session::spawn_child` 真正拦人的那份**必须是同一组数**：新建会话由
> `ToolTableSpec::spawn_limits()` 对齐，恢复由 `recover` 的 `limits` 入参对齐
> （[issues/160](issues/160-recover-limits-param.md)——它不进原子图也不进日志，
> 恢复不出来，只能由宿主再说一遍）。所以本文提到「上限」时指的都是这组**参数的当前值**，
> 不是 3 和 8 这两个字面量。

## 四、映射到现有机械（Explore 勘查，file:line 为证）

1. **bg spawn**（改 `spawn_tool.rs` schema + `dispatch.rs::spawn`）：`background=true` 时，除
   现有的 `session.spawn_child` + `subtree.record` 外，**立刻**往父 spawn 槽发一条
   `Event::ToolResult{agent:parent, call_id, content:"{agent_id}"}`（槽当场收敛），并把子记进
   `Subtree` 的 **detached 集**。detached 子的 harvest 行为不同：终态时**把结果转存到「已完成
   未领取」stash，不回写父**（父那槽早收敛了）。
2. **collect**（`dispatch.rs` 新截获 + 复用 `subtree.rs`）：`collect(id)` → 若 id 在 stash →
   立刻回写结果；若 id 仍在 detached 跑 → `subtree.record(id, parent, collect_call_id, epoch)`
   绑定，走**现有 `harvest`→`ToolResult` 回写路**（`subtree.rs:65-93`）。子结果正文用现有
   `subtree::final_text`（`messages_of(child)` 末条 assistant，`subtree.rs:134`）——**运行时侧
   读，非 core 跨读**，和阻塞 spawn 今天读子正文同一条路。
3. **status**（`dispatch.rs` 新截获 + 复用 `observe.rs`）：`status(id?)` → `session.agent_tree()`
   收窄到「调用者的后代」（`is_descendant_of` 过滤 `live_agents`）→ 序列化成 tool_result 正文。
   纯读、无 Pending、当场回写。
4. **孤儿收尾**（`runner.rs` 的 B 点，`:140`）——**不是**「修静止条件」。043 之后 B 点是
   `calls.is_empty() && mcp_calls.is_empty()`，而后台子自己的 provider 调用就住 `calls` 里，
   所以 root 落终态时泵**自然继续**把子驱动到静止再返回（「一轮结束 = root 终态 **且** 后台子
   静止」），**不会卡死**——原先怕的「无定义状态」过虑了。真问题是**浪费**（把没人要的子跑
   到底烧 token）。做法：root 终态且 detached 集里还有活子（且无 collect 绑定）→ 逐个
   `session.despawn_child`（既有 spawn 补偿，自叶向根、一个 undo 步）→ 子变非活 → 它迟到的
   在飞结果撞 `Session::step` 的 **`is_live` 闸**（`step.rs:75`）被丢 → 凭据照常落地、表排空
   → B 正常返回 **root 的终态（不是 `Failed(Cancelled)`）**。**不用会话级 cancel**——它无
   agent 字段、且会把轮次判成取消，而 root 明明答成功了。完整推导 + 验收断言见
   [issue 052](issues/052-spawn-background.md) §「孤儿收尾的机制」（主会话 opus 定）。

## 五、红线账（逐条过）

- **红线 6（在飞 effect 带 epoch、回写前校验）**：detached 子的结果带 spawn 时的
  epoch（`ChildSlot.epoch`，`subtree.rs:45`）；collect 回写经 `step.rs:69` 的同一道 epoch 门
  （`event.epoch() != self.epoch` 丢弃）。undo/cancel bump epoch → 幽灵子结果被丢。**不新造
  一套**，复用异步路已有的门。这是 052/053 里 opus 要盯死并写「在飞时 bump epoch、结果被丢弃」
  断言的那条。
- **红线 10（只上下读、禁横读）**：`status` 读的是 `Status` 槽派生（`AgentActivity`）——
  `Status` 是 **Downward-visible**（`visibility.rs:77`），下读合法。`collect` 读子正文走**运行时
  harvest**（不是 core 的 `read_descendant`——`Messages` 是 Upward-only，core 跨读拿不到子正文，
  Explore 问题 4）：harvest 是宿主给自己写回 `ToolResult` 的既有合法路，不经 core 跨读 API，
  不违反可见性。**status 只暴露 activity，不暴露子正文**；只有 collect 暴露正文，且经既有回写路。
- **红线 11（进 prompt 的东西逐字节确定）**：`status` 的 tool_result 正文进下一轮 prompt →
  `AgentTree` 序列化必须逐字节确定：`nodes` 按 `AgentId` 路径排序，禁 `HashMap`/`HashSet`。
  （`collect` 回写子正文，和阻塞 spawn 今天一样，已确定。）
- **红线 3（活句柄住 store 外）**：不新增活句柄——子 agent 本就在 store 里（整棵树共用一个
  store），detached 名单存的是 `AgentId`（可序列化），不是句柄。
- **红线 12（core 无模型判断）**：按工具名截 spawn/status/collect 在**宿主侧**（dispatch），
  和现有 spawn/skill 截获同款合法性，core 不碰。
- **决策 20 兼容**：前台 spawn 一行不改；bg = spawn+collect 拆开。A 不封死本扩展，本扩展是
  「又一个会调 spawn 的调用方」，反向不成立——正是决策 20 当初留的口。

## 六、不做（延后，等真实反馈）

- **跨 turn 后台 agent**（子活过一次 `run_turn`）：见 §二代价。
- **per-child cancel 工具**（`cancel(id)` 单独杀一个后台子）：现在 cancel 是 session 级、无
  agent 字段（Explore 问题 5）。turn 收尾的孤儿取消用 session 级够了（turn 反正要结束）；
  mid-turn 单杀一个要 per-child epoch，延后。
- **http / resources / prompts**：与本里程碑无关（那是 MCP 的延后项）。

## 七、issue 分解

- **051** `status` 工具（独立、可先发、sonnet）：纯观测半边，不碰状态模型，自己就能落
  「模型看得到子 agent」。红线 11（tool_result 逐字节确定）。
- **052** `spawn(background)` + detached 名单 + 静止条件/孤儿取消（opus）：碰 pump 不变量 +
  红线 6。这是核心增量。
- **053** `collect` 工具（opus）：复用 harvest 回写，红线 6 回写校验。052+053 合起来 = 「发
  后台子 → observe → collect」闭环，本里程碑的「能用」终点。
- **054** CLI/web 面板呈现 bg/collect + 真机 dogfood（sonnet，照 049）：deepseek 真实上游，
  模型发 bg 子 → status 观测 → collect —— 真机现形「测试绿、世界不对」的老规矩。
