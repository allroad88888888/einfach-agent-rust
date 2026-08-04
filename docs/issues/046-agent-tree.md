# 046 可观测性接缝 + `agent_tree()` 派生读

**里程碑** M7 · **依赖** 028（多 agent 图） · **模型** sonnet · **独测** ✅

M7 的脊梁：把「整棵 agent 树此刻什么状态」定型成 core 的一次派生读。接缝定义见
[../OBSERVABILITY.md](../OBSERVABILITY.md)。**不新增任何 primitive**——全是现有槽的投影。

## 范围

1. **`AgentNode` / `AgentTree` 类型**（`agent-core`，`ts` feature 后导出 TS）：
   - `AgentNode { id, parent, depth, task, activity }`——**不含 usage**（per-agent 累计
     token 不是 core 槽，见 OBSERVABILITY.md §「usage 不在 M7」）。
   - `AgentTree { nodes: Vec<AgentNode> }`（含 root，`live_agents()` 的顺序：root 在前、
     字典序，稳定——树渲染不该抖）。
   - `activity`：从现有状态**推**出来的小枚举（`Idle` / `Thinking` /
     `Working{tools}` / `Done{truncated}` / `Failed{reason}`），**不是新 primitive 槽**。
     它是 `TurnStatus` 的呈现投影：`ToolsPending` → `Working`（带在飞工具名，若 core 需要
     一个 `tool_slots_of(agent)` 读口就顺手加，跟 `messages_of`/`status_of` 同款）。
2. **`Session::agent_tree(&self) -> AgentTree`**：`live_agents()` 遍历每个 agent，按逻辑键
   读它的 `status_of` / `messages_of`（第一条 user 消息 = task）/ 在飞工具槽，组装快照。
   - 往下读方向（红线 10 允许）；宿主持 store，读自己拥有的状态。
   - **纯**：不捕获 `AtomId`（红线 4 孪生条款），按逻辑键现查 family；不读时钟 / 随机
     （红线 1）。

## 验收（可判定）

- 单 agent 会话：`agent_tree()` 返回 1 个节点，`parent = None`，`depth = 0`，`status`
  跟 `Session::status()` 一致。
- spawn 两个子 agent 后：返回 3 个节点，两个子的 `parent` = root，`depth = 1`，顺序稳定
  （同一状态两次调用节点顺序逐个相同——树渲染不该抖）。
- 子 agent 在 `Thinking` → 它的 `activity` 是「思考中」；父在等子 agent →
  `WaitingChildren` 带在等的子 id；agent 跑工具 → `RunningTools` 带在飞 call_id + 工具名。
- **undo 一致**：spawn 子 agent 那一轮被 `/undo` 撤掉后，`agent_tree()` **不再**含那个
  子 agent（树跟着状态回退，零专门代码）——这条是红线 1/4 的实检。
- `task` = 该 agent 的第一条 user 消息（子 agent 是 spawn 的任务文本，root 是首轮输入）。

## 注意

- **红线 1**（派生读纯函数）、**红线 4 孪生条款**（不捕获 `AtomId`，按逻辑键现查）：
  违反后 undo / 恢复出来的树是静默错值。**派独立测试 agent**，且必须有「spawn→undo→树
  回退」那条断言（能把静默失败变红，所以 sonnet 够，测试替你红）。
- **红线 10**：只往下读（`read_descendant` 的 status/result/usage 方向）。这个接口是
  宿主 / UI 视角，**不给 agent 之间互看**——别让它变成横读的后门。
- **不新增 primitive**（OBSERVABILITY.md 判据）：`activity` 是 status + 在飞槽的呈现，
  不是新槽。若发现「必须加个字段才能显示 X」，先停下——大概率 X 已经是某个现有槽的投影。
- ts 导出：`AgentNode`/`AgentTree`/`activity` 枚举 `#[cfg_attr(feature="ts", derive(TS))]`，
  048 的协议一致性测试会锁它（web 要在 UI 上渲染）。
- 接口先定（pub 类型 + 签名 + 文档，`todo!()` 体）→ 实现与测试并行 → 合并。

## 实做记录（完成 · 2026-08-03）

接口由主会话钉死（`observe.rs` 的 `AgentNode`/`AgentActivity`/`AgentTree` + `agent_tree()`
签名，用真实 core 读口），实现 agent（sonnet）与独立测试 agent（sonnet）并行分头做。

**产出**：`agent_tree()` = `live_agents()` 遍历 → 逐格 `agent_node()` 组装；`activity_of`
把 `TurnStatus` 五态投影成 `AgentActivity`（`ToolsPending → Working{在飞工具名}`）；
`first_user_text` 取第一条 `Role::User` 消息当 task（缺则 `None`，不用工具名顶替）；
`describe_failure` 把 `Failure` 转可读串。实现 agent 加了 `Session::tool_slots_of(agent)`
（`command/read.rs`，per-agent 取料口，照 `messages_of`/`status_of` 同款——不是第三个跨
agent 读 API）让 `Working` 带上在飞工具名。

**验收兑现**（主会话从磁盘跑）：`cargo test -p agent-core` 全绿（含独立测试
`tests/observe_046.rs` 11 个 + 各源文件内联）；`clippy --all-targets -D warnings` 净；
红线过；`cargo build --workspace` 过。红线 1/4 的实检「spawn→undo→子 agent 从树上消失」
独立测试有断言、真绿。`observe.rs` 291 行 / `read.rs` 244 行，都在红线 9 内。

**独立测试标注**：`AgentActivity` 没单独的 `WaitingChildren` 变体——当前 `spawn_child` 是
直接命令、不走父的 `ToolsPending` 槽（那要等 029 的 `spawn_agent` 工具），「父等子」暂
折进 `Working`。与钉死的枚举一致，不是缺口。

**过程坑（WORKFLOW 铁律 -1 的又一例）**：实现 agent 犯「收尾自旋」——起后台 `cargo
test` + 等监视器，报「完成」却没前台跑完验证，且遗留 orphan cargo 占死 target 锁、把
主会话的验证也卡住。主会话按「代收」处理：`TaskStop` 停 agent、外科手术杀 orphan cargo
（避开另一个 session 的 excel 测试）、从干净状态重跑四道门禁。**代码本身是对的**，spin
纯是进程管理的麻烦。下一个 agent 的派活单里把「验证前台跑完、禁后台+等监视器」写在最前。
