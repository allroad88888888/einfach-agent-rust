# 052 `spawn(background)` + detached 名单 + 静止条件/孤儿取消

**里程碑** M8 · **依赖** 042 无关 / 与 043 同碰 dispatch（排 043 之后做，避冲突） · **模型** opus · **独测** ✅（碰红线 6 + pump 不变量）

异步核心的**发射半边**。碰两处易静默失败的地方：pump 的静止不变量、红线 6 的 epoch。是本
里程碑的 opus。接缝见 [ORCHESTRATION.md](../ORCHESTRATION.md) §二/四/五。

## 背景（动手前钉死，Explore 勘查产）

现在阻塞 spawn：父 spawn 槽 `Pending`，`subtree.harvest`（`subtree.rs:65`）在子终态时把子正文
（`final_text`，`:134`）回写父 spawn 槽。pump 静止 = `calls.is_empty()`（`runner.rs:133`），
模块文档（`runner.rs:20-27`）**明确拒绝「root 终态 + 子树还跑」的世界**——它靠「父卡在
`ToolsPending` 直到子收敛」使得 root 终态 ⟺ calls 空。后台 spawn 破坏的正是这条，所以要在
本 issue 里把不变量**显式补回**（孤儿取消）。

## 范围

1. **`spawn` 加 `background: bool`**（`spawn_tool.rs` schema + `SpawnRequest`，缺省 `false`）。
   `false` = 现状一行不改。`true` 时在 `dispatch.rs::spawn`（`:115`）：除现有
   `session.spawn_child` + `subtree.record` 外，**立刻**发一条
   `Event::ToolResult{agent:parent, call_id, content:"{\"agent_id\":\"...\"}"}` 让父 spawn 槽
   **当场收敛**（父不被挡、继续下一轮），并把子记进 `Subtree` 的**新 detached 集**。
2. **detached 子的 harvest 行为不同**（`subtree.rs`）：detached 子终态时**不回写父**（父那槽早
   收敛了），而是把结果（`final_text` + is_error）转存到「已完成未领取」stash（key = child
   `AgentId`）。stash 供 053 的 collect 领取。detached 集 + stash 都是 `Subtree` 局部字段
   （`runner.rs:95` 每次 resume 重建）——**turn 内生死，不跨 `run_turn`**（决策见 §二）。
3. **静止条件 / 孤儿收尾**（`runner.rs` 的 B 点，`:140`）——**设计已由主会话（opus）定死，
   见下「孤儿收尾的机制」**。照那个做，别再自行发明。

## 孤儿收尾的机制（主会话 opus 定，2026-08-04）

动手前主会话读了 043 之后的 `runner.rs`，把这块从「开放判断」变成「照着做」。**两个纠正**：

**纠正一：泵不会卡死,原判「无定义状态」过虑了。** 043 之后 B 点的静止条件是
`calls.is_empty() && mcp_calls.is_empty()`（`runner.rs:140`），而**后台子自己的 provider 调用
就住在同一张 `calls` 表里**。所以 root 落终态、后台子还在飞时，`calls` 非空 → B 不返回 →
泵继续 A/D 循环把子驱动下去 → 子终态后表排空 → B 返回 root 的终态。**语义天然成立**：
「一轮结束 = root 终态 **且** 后台子都静止」。不需要为「别卡死」写任何代码。

真问题不是卡死，是**浪费**：root 已经答完了，还把没人要的子跑到底（烧 token）。

**纠正二：不能用 session 级 cancel。** 原文写「发取消 bump epoch」是错的——既有取消是
**会话级**（`Effect::CancelInFlight` 无 agent 字段）、且会把这一轮判成
`Failed(Cancelled)`。root 明明答成功了，把轮次标成取消是**错的状态**。

**定下的做法：`despawn_child` 定点拆，不碰会话级取消。**

在 B 点返回前加一道：`session.status().is_terminal()`（root 已终态）**且** detached 集里还有
`is_live` 的子 → 对每个这样的子调 `session.despawn_child(child)`
（`agent-core/src/command/despawn.rs:108`，既有的 spawn 补偿：自叶向根逐出、整棵子树
**一次 `store.batch` = 一个 undo 步**、`prev` 记进 entry 所以 undo 拿得回来）。然后
**继续循环**（别在这里直接 return）：

- 被 despawn 的子 `is_live` 变 false → 它在飞的结果回来时撞 `Session::step` 的
  **`is_live` 闸**（`step.rs:75`，`if !self.is_live(&agent) { return Vec::new(); }`）→ 丢弃，
  不写进已经收尾的世界。**复用既有闸，不新造**（和红线 6 的 epoch 闸并列的第二道闸）。
- 那些在飞凭据照常经 D 落地、从 `calls`/`mcp_calls` 里移除 → 表排空 → 下一次 B 正常返回
  root 的终态。**有界，不空转**。
- 效果 = **砍尾**：子当前这一轮 provider 调用还是会回来（已经在飞、砍不掉），但它的**下一轮
  不会再起**（死 agent 的事件被闸丢，不产生新 effect）。10 轮的子被砍在第 1 轮末，不是跑满 10 轮。

**告警**：被 despawn 的孤儿要在最终输出里留一条可见通报（模型 spawn 了后台子却没 collect
就收尾了）——走 `ctx.emit` 的既有通报路，别静默。

**与 053 的接口**：绑了 collect 的子**不是孤儿**（父正等它，`collect` 槽 `Pending` 会让 root
非终态，本条根本不触发）。052 单独落地时没有 collect，但实现时把判据写成「detached 且
`is_live` 且**没有 collect 绑定**」，053 接上就不用再改这里。

## 验收（可判定）

- 父发 `spawn(background=true, task=...)` → **立刻**拿到含 `agent_id` 的 tool_result（不阻塞），
  父能在同一 turn 继续发下一个工具调用；子在后台被 pump 驱动（有在飞 provider call）。
- 父发两个后台 spawn → 两个子并发跑；父随后（同 turn）能观测/收尾。
- detached 子终态 → 结果进 stash，**没有**回写父的幽灵 tool_result（断言父的消息里没多出一条）。
- **孤儿收尾**：父发后台 spawn 后**不 collect**、直接产出最终答案想收尾 → 活孤儿被
  `despawn_child` 拆掉，`run_turn` 干净返回（不永久空转、不 panic），最终输出带告警。
  断言四条：①`run_turn` 真的返回了；②返回的是 **root 的正常终态，不是
  `Failed(Cancelled)`**（这条专门钉「没走会话级取消」——走了就会红）；③树里孤儿已非活
  （`is_live` false / `agent_tree()` 里没了）；④孤儿那条迟到的在飞结果**没写进消息历史**
  （被 `is_live` 闸挡掉）。
- **砍尾有效**：让后台子是个多轮的（会连着起第 2、3 轮 provider 调用）→ root 终态触发
  despawn 后，断言它**没有**再起新一轮（死 agent 事件被闸丢、不产生新 effect），
  `run_turn` 在有界时间内返回。
- **红线 6 对抗测试**（必须有）：后台子在飞时 bump epoch（模拟 undo）→ 子结果回来 epoch 不符
  → 被 `step.rs` 门丢弃、不进 stash、不进状态。断言结果真被丢。
- undo：后台子在 turn N 内生死 → `undo(turn N)` 连带回滚整棵子树（`turn_id` 继承、
  `ToolsAllowed→Null`，`observe.rs` 既有机制）——和阻塞 spawn 的 undo 一致，无新代码。

## 注意

- **红线 6**（静默失败）：后台子的在飞结果必须带 spawn 时 epoch（`ChildSlot.epoch`,
  `subtree.rs:45`），回写/入 stash 前经 `step.rs:69` 同一道门。**不新造 epoch 机制**。派独测，
  且必须有「在飞时 bump epoch、结果被丢弃」的断言（本仓最贵的一类 bug）。
- **pump 不变量**：本 issue 是全里程碑唯一动 `runner.rs` 静止条件的地方。孤儿取消的触发点
  错了 = 要么 root 终态后子树被 pump 无谓驱动到底（浪费），要么永久空转（`calls` 不空但
  root 终态、无人再 collect）。opus 想清楚这两种坏结局怎么都不发生。
- **决策 20 兼容**：`background=false` 路径**逐字节不改**，跑既有 spawn 测试全绿证明没回归。
- **不做**：跨 turn 后台子（子活过一次 `run_turn`）——见 ORCHESTRATION §二/六。本 issue 的 detached
  集/stash 都是 turn 内局部，别做成 store 落地的跨 turn 映射。
- **不碰 `agent-tools/`**（并发会话 WIP）；**不碰 043 正改的 MCP 第四路**——本 issue 排 043
  之后做，两者都动 `dispatch.rs` 的 `Effect::ExecuteTool` 臂，避免同文件冲突。
- 收工验证前台跑完（WORKFLOW §四 -1）。

## 实做记录（完成 · 2026-08-04）

发射半边落地。**一个 atom / primitive / `Effect` / `Notice` 都没加**，`agent-core`
一行没改——后台 spawn 整个就是「同一条 spawn 路的最后一步分岔」加一张运行时局部表，
孤儿收尾复用的是 028 就有的 `despawn_child` 和 `step.rs` 那两道闸。

### 建了什么

| 文件 | 行 | 干什么 |
|---|---|---|
| `agent-runtime/src/spawn_tool.rs` | 229（+39） | 改：schema/描述加 `background`，`SpawnRequest.background`，`parse` 认 `true/false`（缺省 `false`，写错类型 = 给模型看的错误文本） |
| `agent-runtime/src/subtree.rs` | 277（+124） | 改：detached 名单 + stash 两张表、`detach`/`take_orphans`/`take_stash`、`harvest` 拆成 `harvest_slots`（原样）+ `harvest_detached`（**红线 6 的回写校验点**） |
| `agent-runtime/src/subtree_tests.rs` | 145 | 新：stash / 孤儿判据 / **epoch 闸的确定性差分**（`#[path]` 子模块，红线 9 同 043/051 处置） |
| `agent-runtime/src/dispatch.rs` | 293（+57） | 改：`Dispatched::Events(Vec<Event>)` 新变体、`detach()`（槽当场收敛 + 记 detached）；前台路一字未动 |
| `agent-runtime/src/orphan.rs` | 111 | 新：轮末清算——活孤儿 `despawn_child` + 告警，stash 里没人领的告警丢掉，`persist::sync` 落 teardown entry |
| `agent-runtime/src/runner.rs` | 299（+18） | 改：B 之前加 B0（`orphan::reap` + 补一次树快照）、`Dispatched::Events` 那一臂、模块文档改掉「那个世界没有答案」那句 |
| `agent-runtime/src/lib.rs` | 90（+10） | 改：`mod orphan`、模块文档补「后台子 agent（052）」一节 |
| `agent-runtime/tests/spawn_bg_support/mod.rs` | 76 | 新：`#[path]` 复用 029 的假服务器/装配夹具 + 四个断言助手 |
| `agent-runtime/tests/spawn_bg_two_children_no_block.rs` | 131 | 新：两个后台子并发 + 父不被挡（服务器侧时间为证）+ **不回写父** |
| `agent-runtime/tests/spawn_bg_orphan_reaped.rs` | 110 | 新：孤儿收尾四条断言 + 告警 + undo 连带整棵子树 |
| `agent-runtime/tests/spawn_bg_tail_cut.rs` | 109 | 新：砍尾（多轮子被砍在第一轮末）+ **孪生对照组**（同一份脚本没被拆时跑满两轮） |
| `agent-runtime/tests/spawn_bg_epoch_writeback.rs` | 152 | 新：**红线 6 对抗测试** + 孪生对照 |

### 接口决策一：`Dispatched::Events`，不是把 `Event` 改成 `Vec`

后台 spawn 是唯一一处**一个 effect 要产出两件事**的地方：给父的 `ToolResult`（槽当场
收敛）和给子的 `UserInput`（子开工），而这两件事分属两个 agent，塞不进一个事件里。
把既有的 `Dispatched::Event(Event)` 改成 `Vec` 的话，tool / skill / status / refuse
那四条「一个 effect 一个结果」的路每一处都要为一个恒定长度 1 的 `vec![]` 付一次注意力，
所以新加一个变体。父的 `ToolResult` **排在前面**：它先解开阻塞，父的下一跳请求和子的
第一跳请求这才真的同时在飞（e2e 里用服务器侧的到达时刻钉死了这一点）。

### 接口决策二：tool_result 正文只有 `agent_id`

`{"agent_id":"root/a1"}`，一个字段。它会原样躺在父的历史里进**以后每一次**请求
（红线 11 要求逐字节确定），所以不放任何「此刻的状态」——`"status":"running"` 下一秒
就可能是假话，而它会假一辈子。要看子在干啥有 `srv:agent/status`（051），那是一次现读。

工具描述里明写了两件模型必须知道的事：`background=true` 的回答**不会自己回来**，而且
**父这一轮结束时它会被拆掉**。决策 20 的兜底哲学（把约束告诉模型，让它自己收敛）在这里
的形状就是这两句话——不写的话模型会 spawn 完就等，等一个永远不来的结果。

### 孤儿收尾的落点：`runner.rs` 的 **B0**（B 之前），不是修静止条件

```rust
// B0. 轮末清算（052）
if orphan::reap(session, ctx, &mut subtree) { maybe_emit_tree(...); }
// B. 两张在飞表都空 → 收工
if calls.is_empty() && mcp_calls.is_empty() { ... return status; }
```

- **放在 B 之前**而不是「B 判空之后、return 之前」：这一圈可能就是收工的那一圈（后台子
  已经静止但还活着），拆干净了再返回，不把一棵没人要的子树留给下一轮。
- **静止条件一个字没改**。后台子自己的 provider 调用就住在同一张 `calls` 表里，所以
  root 落终态、子还在飞时 B 本来就不返回，泵照旧驱动到静止——「一轮结束 = root 终态
  **且** 后台子静止」是天然成立的，不需要新条件。两种坏结局各自被什么挡住：
  - **永久空转**：不可能。`reap` 只拆不等，被拆的子在飞的凭据照常经 D 落地、从 `calls`
    里移除（它的回执撞活性闸被丢，但凭据该摘还是摘），表必然排空。
  - **无谓跑到底**：正是 `reap` 消掉的那件事。被拆的子当前这一轮回执还会回来（已经在飞，
    砍不掉），但它的**下一轮不会再起**——死 agent 的事件被闸丢，不产生新 effect。
    `spawn_bg_tail_cut.rs` 用「子的第二跳请求**没有**到达假服务器」把这条钉死，并配了
    一个孪生对照组（同一份脚本，root 还忙着的时候子跑满两轮）证明这条断言不是空的。
- **不碰会话级取消**。`despawn_child` 是 spawn 自己的补偿命令（自叶向根、整棵子树一次
  `store.batch` = 一个 undo 步、活值记进 `prev`）。验收里那条「返回的是 `Done`，不是
  `Failed(Cancelled)`」就是钉这个的：走了 `Effect::CancelInFlight` 立刻红。
- **判据**：`detached && is_live && 没有 collect 绑定`（`Subtree::take_orphans`）。第三条
  现在恒真（还没有 `collect`），写死在代码里是为了 053 接上时这里一行不用改。
- **告警**：两类都报，不静默。①活孤儿被拆；②stash 里跑完没人领的。走 `ctx.emit` 的
  `RunnerEvent::TransportTrouble`——既有变体里唯一「一句话文本、只进日志/CLI、不参与任何
  判断」的通报口。**诚实标注**：这个变体的名字对不上这件事。给它开一个专属
  `RunnerEvent` 变体会连锁改 `SessionEvent`（跨 SSE 的协议枚举）→ 生成的 TS → fixtures，
  那是 **054**（面板呈现 bg/collect）的范围；到 054 一次做完，接住的地方也齐了。
- **持久化**：`reap` 里 `persist::sync` —— `despawn_child` 落了一条 teardown `Entry`，
  不转发的话恢复出来的会话里会有一个「已经被拆掉、日志里却还活着」的子 agent。
- **树快照**：`reap` 之后补一次 `maybe_emit_tree`。A 那段的变化检测只跟着 `session.step`
  走，而这条路不经过 step（048 真机逮到过同一类漏投影：撤了子 agent 面板不动）。

### 红线 6：回写校验点在哪两处

| 路径 | 谁把门 | 什么时候 |
|---|---|---|
| 后台子自己的在飞 provider / MCP 回执 | `Session::step` 入口的 epoch 闸（`step.rs:71`） | 既有的门，**没新造** |
| 子落终态 → 结果**进 stash** | `Subtree::harvest_detached` 里 `entry.epoch != session.epoch()` → 丢 | 新的一处，因为**它不经过 `step`** |

第二处必须自己比一次：`harvest_detached` 不产出任何事件（父那个槽早收敛了），所以没有
任何别的地方替它把门。判据跟 `step.rs` 那道**逐字一致**（`!=` 而不是 `<`，世代只增不减）。
不挡的后果不是「消息历史脏了」，而是**幽灵结果的落地点从消息历史挪到了另一张表**——
一份已经被回滚掉的世界里的答案躺在 stash 里等 053 的 `collect` 来领。

**053 的接力点**：`collect` 把 stash 里的东西写回父，走的是 `Event::ToolResult` 经
`Session::step`——那条路上 epoch 闸自动生效（用的是那次 `ExecuteTool` 的当前世代），
所以「stash 里躺久了变陈旧」这件事在 collect 那一刻会被自然挡住，不需要给 `Stashed`
再加一个世代字段。

### 红线 6 的对抗测试长什么样（两条，一条确定性一条端到端）

**端到端**（`spawn_bg_epoch_writeback.rs`）照 043 的骨架，但**去掉了时序**——不靠 sleep
赌毫秒：

```
1. root 一跳吐两个调用：spawn(background=true) + 一个远端工具（Location::Web）
   → 后台子当场开工；root 那个远端槽 Pending，于是 root 停在 ToolsPending 非终态
     ——轮末清算因此不触发，这个子在整个过程里一直活着
2. 子自己也吐一个远端工具调用 → 凭据记下起飞那一刻的 epoch=0
   → 两张在飞表都空，run_turn 返回 ToolsPending
3. 测试在这里 bump epoch：一次真的 Cancel（undo 走的是同一个 bump）→ epoch=1
4. 测试回传子那次远端调用的结果：resolve_remote_tool 照常发一条 ToolExecuted
   （**证明结果真的回来了**），组出 ToolResult{epoch:0} 喂回泵
5. step 入口的 epoch 闸 0 != 1 → 丢弃、不写消息历史
```

**为什么第 1 步非要让 root 停在非终态**：这样子在整个过程里一直 `is_live`，于是「结果
没落地」就**只可能是 epoch 闸干的**——活性闸（epoch 闸后面那一道）被 `assert!(session.
is_live(&child))` 显式排除掉。孪生对照组同一份脚本、同一次回传，只是不 bump，结果就
老老实实落进子的历史：一个进一个不进，闸的存在才是被测出来的。

**确定性单测**（`subtree_tests.rs`）钉 stash 那一侧：同一份 fixture（子已 `Done`、
detached 记着 epoch=0），一条在 harvest 前 `Cancel` 推走世代 → stash 空，孪生不推 →
stash 有一条。

**三条闸都做过突变验证**（改回原样后重跑全绿）：

| 把这行改成永假 | 哪条测试变红 |
|---|---|
| `step.rs:71` 的 epoch 闸 | `a_background_childs_late_result_is_dropped_by_the_epoch_gate`（幽灵被写进了子的历史） |
| `subtree.rs` 的 `entry.epoch != now` | `a_stale_epoch_keeps_the_background_result_out_of_the_stash` |
| `runner.rs` 的 `orphan::reap` | `an_uncollected_background_child_is_despawned_and_the_turn_still_ends_normally` |

### 决策 20 兼容

`background=false` 的分岔点在 `dispatch::spawn` 的**最后一步**：解析、子集校验、
`spawn_child`、`persist::sync` 四步逐字节共用，`if parsed.background` 之后才分开。
九个 `spawn_indep_*` 独测（029 的验收）全绿，是「没回归」的操作证据。

### 红线 9 的余量（留给 053 的话）

`runner.rs` 299 / `dispatch.rs` 293 —— 都在 300 以内，但**只剩个位数**。053 的 `collect`
要在 `dispatch.rs` 加第五处截获，**先拆再加**：按 051 的先例，把 spawn 那三个函数
（`spawn`/`detach`/`refuse`）挪进 `spawn_tool.rs`（它的 `#[cfg(test)] mod tests` 同时挪进
`#[path]` 子文件），`dispatch.rs` 就回到纯分派器的形状，跟 `status_tool::intercept` /
`skill::tool::intercept` 的既有摆法也一致。

### 验证（前台跑完，真实输出）

```
$ cargo test -p agent-runtime -p agent-core
exit=0
binaries=97 passed=528 failed=0 ignored=0

     Running tests/spawn_bg_epoch_writeback.rs
running 5 tests
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
     Running tests/spawn_bg_orphan_reaped.rs
running 4 tests
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.96s
     Running tests/spawn_bg_tail_cut.rs
running 5 tests
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.54s
     Running tests/spawn_bg_two_children_no_block.rs
running 4 tests
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.73s

$ cargo clippy -p agent-runtime --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.49s
exit=0

$ bash scripts/check-invariants.sh --all
红线检查通过
规则与理由：docs/INVARIANTS.md
exit=0
```

### 没做（照 issue §不做）

`collect`（053）、跨 turn 后台子（detached 名单和 stash 都是 `Subtree` 的局部字段，
每次 `resume` 重建，没有任何 store 落地的跨 turn 映射）、per-child cancel。
