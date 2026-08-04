# 049 web / 桌面活树面板 ← M7 终点

**里程碑** M7 · **依赖** 048 · **模型** sonnet · **独测** —（终点靠真浏览器验收）

扛 M7 验收：浏览器里一个**活的 agent 树面板**，子 agent 在干啥实时可见——不再是「只有
交错的文字帧」，而是一棵带状态的树。桌面版同一套前端白拿（M4：逐文件同哈希）。

## 范围

1. **树面板组件**（`packages/web`）：开页 `GET /agents` 做种，之后每收到 `agent_tree`
   帧就更新。每个节点显示：状态灯（Idle/Thinking/ToolsPending/终态）、activity（思考中 /
   在跑哪个工具 / 等哪个子 agent）、task、usage。缩进/连线体现父子。
2. 跟现有的**归属分栏帧流并存**（M3 已有）：树面板回答「谁在干啥、树长啥样」，帧流回答
   「它具体说了什么」。两者互补，不是替代。
3. 桌面版不额外做——同一份 `packages/web` 构建产物内嵌（M4 的逐文件同哈希，别动那个不变式）。

## 验收（M7 验收，可判定）

- 真浏览器（Playwright 驱动，真实上游）：给模型一个会 spawn 子 agent 的任务 → 树面板
  **实时**长出子 agent 节点、状态灯随 `Thinking`→`ToolsPending`→终态变化、activity 显示
  当前动作。
- 子 agent 跑工具（含 043 之后的 MCP 调用）时，树面板显示它「在跑 `mcp:.../x`」——
  MCP 在飞调用在树上可见（这也是把可观测性插在 M6 中间的理由兑现）。
- `/undo` 撤一轮 → 树面板里被撤的子 agent 消失（快照回退，UI 哑跟随）。
- 断开重连 → 树面板恢复成当前正确的树（GET 做种 + Last-Event-ID 补帧）。

## 注意

- **UI 是哑渲染器**（OBSERVABILITY.md）：面板不维护自己的 agent 状态机，只画收到的快照。
  收到新 `agent_tree` 帧就用它替换/更新，不从零散事件推断树。
- 跟 CLI `/agents`（047）**共用同一份 `agent_tree()` 数据**，只是渲染成面板而非文本——
  两个壳的树不该在任何状态上分叉。
- 桌面：不碰 M4 的「web 一套不变、逐文件同哈希」不变式。
- 回填 issue + 更新 ROADMAP §二 加 M7 完成段、README 状态行、issues/README M7 段。
- 收工验证：真浏览器四点（长出/状态变/undo 回退/reconnect 恢复）前台跑过再交（WORKFLOW §四 -1）。

## 实做记录（前端件完成，M7 真浏览器终验待主会话 · 2026-08-03）

**产出**（`packages/web`，哑渲染器，跟 CLI `/agents`（047）共用同一份 `agent_tree()`
数据，只是渲成 DOM 而非文本；格式化规则——短 id / activity 呈现 / task 折叠+截断——
逐条照抄 `agent_tree.rs` 的判据，两个壳不分叉）：

- **`render/agent_tree.ts`**（新增）：`renderAgentTree(tree: AgentTree)` 整棵重画
  `#agent-tree` 容器（`replaceChildren`，不做增量 DOM patch）。每行：状态灯
  （`.tree-dot--{idle|thinking|working|done|failed}`，四档映射 `AgentActivity` 判别
  标签）+ 短 id（复用 `dom.ts` 导出的 `shortAgentLabel`，`root/a1` → `a1`，root 本身
  仍是 `root`）+ activity 文本 + task（折叠空白 + 60 字符截断，`(无任务文本)` 占位）。
  缩进按 `node.depth` 内联算 `padding-left`，不靠 CSS 选择器猜层级。
- **`render/dispatch.ts`**：加 `case "agent_tree"` → 直接 `renderAgentTree(event.data)`，
  不经 `frame.agent`/`appendToTimeline`（树帧标 `AgentId::root()`，是会话级快照不是
  某个具体 agent 的活动，不写进时间线）、也不打断 `StreamCursor`（树面板跟时间线是
  两块独立 DOM，互不打断）。
- **`api.ts`**：加 `fetchAgentTree(id): Promise<AgentTree>` → `GET /sessions/:id/agents`。
- **`main.ts`**：`connect()` 的 `onStatus` 回调里，状态变 `"open"`（覆盖首次连接 **和**
  每次 `EventSource` 自动重连——两者在这个回调里同一个值）就补一次
  `fetchAgentTree(...).then(renderAgentTree)`——GET 做种 + 之后靠 SSE `agent_tree` 帧
  增量，双保险覆盖「Last-Event-ID 补发跟不上（`gap`）」的情形；`renderAgentTree` 幂等
  （整棵重画），多调一次无副作用。
- **`dom.ts`**：加 `agentTreeEl`（`#agent-tree` 的 `requireEl`）；把原本模块私有的
  `shortAgentLabel` 导出，树面板复用同一份短 id 规则，不重新发明。
- **`index.html`**：`#timeline` 和新的 `<aside class="tree-panel">`（内含静态
  `<h2>` 标题 + `#agent-tree` 挂载点）包进 `.workspace` 左右两栏，跟 `#composer`/
  `header` 同级——面板跟归属分栏帧流并存，不替代。
- **`style.css`** / **`tree-panel.css`**（新增）：树面板自己的样式（`.tree-panel`/
  `.tree-row`/`.tree-dot*`/状态灯呼吸动画/窄屏 ≤720px 折叠为上下堆叠）独立成
  `tree-panel.css`，`style.css` 顶部 `@import` 引入（Vite 构建期内联，不额外发请求，
  `index.html` 不需要第二个 `<link>`）——这次改动会把 `style.css` 顶到 300 行硬顶，
  按 `one-file-one-thing` 规则当场拆，不留到下次。
- **`packages/protocol/src/index.ts`**：补 `AgentTree`/`AgentNode`/`AgentActivity`
  三个类型的导出（此前只有 046/048 生成了 `generated/` 下的文件，没人从入口收拢）。

**验收兑现**（前台实跑，真实输出）：
- `pnpm --filter @agent/protocol typecheck` → `tsc --noEmit` 净。
- `pnpm --filter web typecheck` → `tsc --noEmit` 净。
- `pnpm -r typecheck`（3 of 4 workspace projects——`@agent/desktop` 没有
  `typecheck` 脚本，M4 范围不动）→ protocol/web 均 `Done`。
- `pnpm --filter web build` → `vite build` 15 modules transformed，产出
  `dist/index.html` + `dist/assets/index-*.{css,js}`，125ms 内完成；抽查产物 css，
  确认 `@import` 已被内联成同一个文件（不是运行时二次请求）。
- 前端无运行时测试设施（`pnpm -r typecheck` 是唯一断言器，`packages/protocol/src/
  fixtures.test.ts` 头注释记的先例）；`fixtures/events.json`/`events.ts` 已含
  `agent_tree` 样本（048 留下的），本次不需要新增 fixture。

**待办（M7 真浏览器终验，主会话做，本次不做）**：Playwright 驱动真实上游，验四点——
spawn 子 agent 后树面板实时长出节点、状态灯随 `Thinking`→`ToolsPending`→终态变化、
`/undo` 撤一轮后被撤子 agent 从树上消失、断开重连树恢复正确。

**接线点里没把握的地方（供主会话复核）**：
1. `main.ts` 用连接状态 `"open"` 作为「(re)connected，补一次 GET」的触发点——
   `connection.ts` 目前不区分「首次 open」和「断线后重新 open」，两者共享同一个
   回调分支。语义上够用（GET 幂等、多调不错），但如果主会话认为「只在真正重连后
   才 GET」更精确，需要 `connection.ts` 加一个显式的 reconnect 信号才能分开。
2. `AgentActivity::Working` 目前只携带 `tools: Array<string>`（工具名，含 043 后的
   MCP 全名如 `mcp:xxx/yyy`），树面板直接把工具名列表拼进 activity 文本
   （`Working(mcp:xxx/yyy)`）——没有单独渲染「等哪个子 agent」这一态（协议里没有
   区分「在等子 agent」vs「在跑工具」，`Working.tools` 是唯一信号源）；若真浏览器
   验收发现这一态需要单独视觉区分，需要先在 core/协议侧补，前端这层目前是如实
   呈现协议给的東西。
3. issue 范围条款 1 原文提到节点要显示 `usage`，但 `AgentNode`（046 生成）没有这个
   字段——`OBSERVABILITY.md` §「usage 不在 M7」已经明确记了这是刻意搁置（per-agent
   token 不是 core 槽，加会造第二真值源），本次因此没有渲染 usage，这是遵照已拍板
   的决策而非遗漏。
