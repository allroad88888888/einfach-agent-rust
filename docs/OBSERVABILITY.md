# 子 agent 可观测：一次派生读，不是新机制

核心命题的直接推论。子 agent 的全部状态早就活在同一张 atom 图里（整棵 agent 树共用
一个 store）——所以「看子 agent 在干啥」不是要造一套监控系统，是**对现有状态的一次
派生读**。子 agent 不该是黑盒，正因为它本来就不是——它的状态一直在 store 里，只是还
没有一个把它摆出来给人看的接口。

## 一句话与判据

> 可观测性把「已经在 store 里的状态」摆成「人能看的一棵树」。

要显示的每样东西，先问一个问题：

> **它是不是 store 里某个 atom 的投影？**
> 是 → 派生读，不新增 primitive。（几乎全部：status / task / usage / 在飞工具都是既有槽）
> 不是 → 才考虑加 primitive。（M7 里一个都不需要）

**为可观测性新增 primitive atom，是接缝错位的第一个症状。** 它意味着你在 store 外面又
存了一份「agent 在干啥」——第二真值源，undo 一致性当场破：撤一轮，真状态回退了，你那份
「当前动作」没回退，树显示的和世界对不上。

## snapshot，不是 reconstruct

树由 core 权威计算（`Session::agent_tree()`），UI 是**哑渲染器**。

**不让 UI 从事件流自己重建一个状态机。** 那样：脆（漏一个事件树就错）、reconnect 断
（中途接入的客户端拿不到完整历史，重建不出当前树）。快照是真值，UI 只画。CLI `/agents`
和 web 树面板因此**共用同一个 `agent_tree()`**，不是两套各自维护的重建逻辑——两套一定
会在某个状态转移上分叉。

## 读方向：宿主持有 store，读的是自己的状态

宿主读整棵树 = 读每个 agent 的 `status` / `result` / `usage`。宿主**持有 store**，
读的是自己拥有的状态——这一条跟红线 10 没关系，那条红线管的是 **agent 之间**。

> **M20 的修正**：红线 10 已经不是「禁横读」了（决策 35：不限方向，判据换成
> 「跨 agent 的边只许指向 primitive」），所以这一节原先那句「agent 之间不经这个接口互看
> ——那还是横读禁令」的**理由**过期了，**结论没变**，只是理由换成了分层：
>
> **可观测性是宿主 / UI 的视角，不是 agent 的视角**——是「外面的人看这棵树」，不是
> 「树里的节点互相看」。模型侧要看别人有 `srv:agent/status`（207 起也是全树），
> 那是**工具层**的口，走 dispatch 截获、有自己的边界（只给 activity + task，不给正文）。
> 两条路分开是刻意的：混成一条之后，「给人看的面板加一个字段」就会顺手变成
> 「模型也能读到它」，而那两件事的成本完全不同（一个是像素，一个是每轮 token）。

### 211 之后面板要多说两件事

会话能自己往下跑了（`srv:agent/send` 的 `when="next_turn"` + 自驱动预算），所以
**用户失去的第一样东西是「我知道现在在干什么」**。三个壳（CLI / web / 桌面）都要能看出：

- **这一轮是不是自驱动开的**（`RunnerEvent::AutoTurnStarted`，不是只进日志）；
- **还剩几格自驱动预算**（同一条事件的 `remaining`）；
- 以及**为什么停了**（`AutoTurnHeld`：预算见底 / 用户喊停 / 刚恢复）——三种都不是错误，
  但都必须说出来，因为「留言原地留着、不丢弃」是三条共有的承诺，而一个不说话的
  「什么都没发生」跟「留言被吞了」在外面长得一模一样。

**浏览器里这三条比在 CLI 上更要紧**：那儿没有 Ctrl-C，页面的停止按钮是唯一的出口。

## undo / 崩溃恢复 / 回放自动一致

`agent_tree()` 是 primitive 的投影（派生读，红线 1 纯函数、红线 4 孪生条款：不捕获
`AtomId`，按逻辑键现查）。于是：

- 撤一轮 → 被撤 agent 的槽回退 → 树跟着回退，**零专门代码**。
- 崩溃恢复 → 快照 + redo 重建 primitive → 树是重算出来的，不是从某个缓存复活的。

这正是「undo / 恢复 / 审计回放是同一套机制的四个投影」在可观测性上的第五个投影——
它白拿了前四个的一致性，前提是它必须是**纯派生读**，不偷存状态。

## AgentNode 暴露什么

```
id / parent / depth   —— 树的形状（AgentId 路径语义，028）
task                  —— spawn 时那句（agent 的第一条 user 消息）
activity              —— 此刻在干啥：Idle / Thinking / Working{在飞工具} / Done / Failed
                         （TurnStatus + 在飞槽的呈现投影，不是新 primitive）
```

### activity 不新增字段，从状态推

`activity` 是 `TurnStatus` 的呈现层投影，不是新状态：

- `Thinking` → 思考中（有一次 provider 调用在飞）
- `ToolsPending` → `Working`，带在飞的工具名 / 在等的子 agent（`toolcall.<id>.result`
  为 `Pending` 的槽 + spawn 槽等的子）
- `Done{truncated}` / `Failed` → 终态原样带过来

全部是现有槽的读法。**不往 store 里加一个「当前动作」字段**——那会是第二真值源
（见上「判据」）。

### usage 不在 M7（为什么）

per-agent 累计 token **不是 core 槽**——M1 起它只在 runtime 的缓存 guard 滚动窗口里过，
core 图上没有这个值。放进 `AgentNode` 只有两条路，都不取：新增一个累计槽（第二真值源，
本文档判据禁），或让 runtime 层给 core 算出的树 augment 一份（把树的组装拆到两层，
CLI/web 两个壳又得各接一次）。等真需要显示 token 再单开——加一个**走 command 层、
journaled** 的 per-agent usage 槽（那样它才跟 undo 一致），不是现在猜。

## 活树，不是时间线（M7 范围）

只给「此刻整棵树什么状态」的快照 + 变化推送。**可回放**（任意 epoch 的快照，把树拖回
历史某刻看当时的样子）延后——它是活树的**超集**（活树 = 当前 epoch 的快照），等真需要
回溯审计再加，不提前猜形态。跟决策 17「事后可见 > 事前猜」一致：先把当前状态摆出来
用，真出现「想看三轮前那棵树长啥样」的需求再上时间线。

## 三样东西过接缝（core → 壳）

1. **`AgentTree`**（一批 `AgentNode`）—— `agent_tree()` 的返回，`ts` feature 后导出 TS（web 要）。
2. **`agent_tree` 变化事件** —— 树的形状 / 状态变了就推一帧（复用 M3 的 SSE + Last-Event-ID 补发）。
3. **GET 端点** —— reconnect / 开页时拿当前快照做种，之后靠变化事件增量更新。

## 落到哪几个 issue

| issue | 定这一层的哪部分 |
|---|---|
| [046](issues/046-agent-tree.md) | 接缝 + `agent_tree()` 派生读 + `AgentTree`/`AgentNode` 类型（core，ts-export） |
| [047](issues/047-cli-agents.md) | CLI `/agents` 文本树 ← 最小「能用」刹车片（只需 core，先证明快照对） |
| [048](issues/048-tree-sse.md) | SSE 快照变化事件 + GET 端点（server，协议一致性 + Last-Event-ID） |
| [049](issues/049-web-tree.md) | web / 桌面活树面板 ← M7 终点 |

## 自查：放错地方的症状

| 症状 | 说明什么 | 怎么办 |
|---|---|---|
| UI 从事件流重建 agent 状态机 | 该用快照 | core 算 `agent_tree()`，UI 哑渲染 |
| 为「当前动作」加了个 primitive atom | 第二真值源，undo 一致性破 | 从现有槽推 `activity`，不新增 |
| agent 之间经这个接口互看 | 横读（红线 10） | 可观测性是宿主 / UI 视角，不是 agent 的 |
| 树 undo 之后不回退 | `agent_tree()` 不纯或捕获了 `AtomId`（红线 1 / 4 孪生） | 派生读按逻辑键现查，别捕获 id |
| CLI 和 web 的树在某个状态上不一致 | 两套重建逻辑分叉了 | 两个壳共用同一个 `agent_tree()` |
