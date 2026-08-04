# @agent/web —— M3 最小客户端 + M7 活树面板

vite + vanilla TS（零框架——组件化是 `packages/ui` 的事，未排期，M3 要的是
验收面不是 UI 资产）。连 `agent-server` 的 HTTP/SSE 面看流、看子 agent、
undo。协议类型只从 `@agent/protocol` 导入，不手写（决策 2，
`docs/issues/033-web-client.md`）。

## 三步启动

**1. 起 server**（`crates/agent-server/examples/serve.rs`，**不是** M4 的
`agent-server-bin`——example 不违背「bin 是 M4」）：

```sh
cargo run -p agent-server --example serve
```

读 `providers.toml`（跟 `agent-cli` 同一份查找顺序：`$AGENT_PROVIDERS_CONFIG`
→ `./providers.toml` → `~/.config/agent/providers.toml`），默认绑
`127.0.0.1:0`（操作系统挑一个空闲端口，红线 8：默认 loopback，不设
`AGENT_BIND` 连不上 `0.0.0.0`）。stderr 会打印实际监听地址，例如：

```
agent-server 监听 http://127.0.0.1:51234（provider=deepseek model=deepseek-v4-pro tools=builtin+shell+spawn，开满档）
```

记下这个地址；也可以用 `AGENT_SERVER_PORT=4000` 固定端口，跳过第 2 步的
环境变量。

**2. 起 vite dev server**，告诉它上一步的地址：

```sh
AGENT_SERVER=http://127.0.0.1:51234 pnpm --filter web dev
```

不设 `AGENT_SERVER` 时默认代理到 `http://127.0.0.1:4000`——如果第 1 步用了
`AGENT_SERVER_PORT=4000` 可以省略这个环境变量。`/sessions` 开头的所有请求
（六个端点 + 会话创建/查询）全部走 vite 的 dev-only 反向代理转发到 server。
**这不是给 server 加 CORS**——vite 代理模拟的是企业网关同源的形状（issue
033「注意」原文），`crates/agent-server` 的源码不因为这个包的存在而改动。

**3. 浏览器开 vite 打出来的地址**（通常 `http://localhost:5173`）。

### 持久化（`--session` 提示）

`POST /sessions` 的请求体是 `{ "session_path": 可选 }`——给了路径就落盘
（`SessionTemplate::open_spec`，跟 `agent-cli --session <path>` 同款语义），
不给就是内存会话（进程 / 页面刷新即丢，但注意：内存会话是绑在 server
进程上的，不是绑在浏览器标签页上——只要 server 没重启，刷新页面靠
`Last-Event-ID` 补发照样能接上历史）。**这一版最小 UI 没有暴露这个字段**——
`src/api.ts` 的 `createSession()` 只发 `capabilities`（见下一节），不发
`session_path`。要接落盘，改那个函数多带一个 `{ session_path: "..." }`，
server 那边不用动。

### 能力声明（065，`src/capabilities/`）

`POST /sessions` 还带一段 `capabilities`——**本前端把「我有哪些工具」交给
模型用**（接缝 `docs/HOST-CAPABILITIES.md` §四；只对这个会话生效、只在建会话
那一次生效）。声明源是 `src/capabilities/`：`demo-tools.ts` 是两个只有浏览器
干得了的示例工具（`web:demo/page-title` 读 `document.title`、`web:demo/viewport`
读视口尺寸，都是 `"pure"`），`index.ts` 汇总成 `webCapabilities()` 给 `main.ts`
在建会话那一行传进去。

- 类型是 **061 生成的**（`@agent/protocol` 的 `Capabilities`/`CapabilityTool`），
  前端不手写协议形状。
- 名字必须 `web:` 前缀（位置从前缀推 → `Location::Web` → 走既有的 remote 工具
  通道），不合规 server 一律 400、不改写。
- **不传 `capabilities` 时行为一字不变**：`createSession()` 不带参数就还是发 `{}`。
- 声明**不等于**会执行——把模型点名的 `web:` 工具真跑起来并回传是 066 的事。

## 设计判断

- **帧解析**：`src/connection.ts` 里 `EventSource.onmessage` 拿到的
  `event.data` 就是 `JSON.parse(...) as Frame`（034 起的 agent 归属信封，
  `{ agent, event }`）——不手写协议接口，真正的「判别联合收窄」发生在
  `src/render/dispatch.ts` 的 `switch (frame.event.type)`，每个 `case` 里
  TS 把 `frame.event` 收窄成对应的变体。
- **去重**：帧 id（`hub/ring.rs` 分配的单调 u64）只需要记住「见过的最大
  id」（`src/dedupe.ts` 的 `FrameWatermark`）——同一 session 生命周期内严格
  递增，重连补发里 id 不大于水位线的帧直接跳过，不需要一个会无限增长的
  Set。
- **agent 归属**：`frame.agent` 是真实字段（034 补的信封，见
  `crates/agent-server/src/event/frame.rs`），不再是 033 那份「数在飞的
  `srv:agent/spawn` 调用」近似——root 不挂标记，非 root 挂缩进/变色
  （`style.css` 的 `.sub-agent`）并带一个短标签（`root/a1/a2` → `a1/a2`，
  `src/dom.ts` 的 `shortAgentLabel`），多个并行子 agent 因此在时间线上分得
  开。`src/render/stream.ts` 的 `StreamCursor` 按「kind + agent」两个维度
  判断要不要另起一个气泡——两个子 agent 的增量哪怕紧挨着到达也不会被粘进
  同一段文字。
- **流式文本**：`src/render/stream.ts` 的 `StreamCursor` 让连续到达的
  同一个 agent 的 `text_delta`/`thinking_delta` 复用同一个气泡（增量追加），
  任何其它类型的帧、或者换了 agent，都会打断这个连续段——下一次 delta 另起
  一个气泡。
- **undo_blocked**：原生 `confirm()`（`src/render/undo.ts`），文案带工具名
  /call_id（`UndoOutcome::Blocked` 的 `tool`/`call_id`/`label` 字段，034
  补的富化——`agent-server` 现查 `Session::barrier_info`），确认即
  `POST .../undo { granularity: "turn", force: true }`——027 的 `/undo!`
  语义搬到点击；由收到的 `SessionEvent::Undo` 帧触发，不是按钮点击本身
  （031 的四个命令端点是 fire-and-forget，按钮点下去那一刻还不知道会不会
  撞屏障）。
- **spawn**：`examples/serve.rs` 用 `ToolTableSpec::Full`（034 补的第三档）
  开满档，模型拿得到 `srv:agent/spawn`，子 agent 真的会经 HTTP 跑起来。
- **活树面板（049，M7 终点）**：跟归属分栏帧流并存的一块独立面板
  （`index.html` 的 `<aside class="tree-panel">`），答「谁在干啥、树长啥样」
  而不是「说了什么」。**哑渲染器**——`src/render/agent_tree.ts` 收到
  `SessionEvent::agent_tree` 帧（标 `AgentId::root()`，不写进时间线）或
  `GET /sessions/:id/agents`（`src/api.ts` 的 `fetchAgentTree`）的返回，
  整棵清空重画，不维护自己的 agent 状态机、不从零散事件推断父子关系——跟
  CLI `/agents`（047）共用同一份 `agent_tree()` 数据，两个壳的树不该在任何
  状态上分叉。`src/main.ts` 在连接状态变 `"open"`（含每次自动重连）时补一次
  GET 做种，之后全靠 SSE 帧增量；`renderAgentTree` 幂等，重复调用无副作用。

## 已知限制（033 上报的三条缺口，034 已全部补上）

033 曾经上报过三条协议缺口（SSE 帧不带 agent 归属、`srv:agent/spawn` 经
HTTP 连不上、`undo_blocked` 拿不到工具名/call_id），`docs/issues/
034-server-multiagent.md` 已经补齐——上面「设计判断」一节记的就是补完之后
的形状。历史记录见 034 的实做记录，这里不重复保留过时的「协议现状」描述。
