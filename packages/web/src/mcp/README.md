# `src/mcp/` —— 浏览器自己连 MCP（issue 067）

**形态 B**（`docs/HOST-CAPABILITIES.md` §七）：浏览器连 MCP server，把
`tools/list` 翻成注入声明报给 agent-server；模型调用时路由回前端，前端转成
`tools/call`。**server 全程不碰 MCP 协议、不 spawn 任何东西**——它眼里那就是
一批普通的 `web:` 注入工具。

```
浏览器 ──连──> MCP server（Streamable HTTP）
   └── tools/list → web:mcp-<server>/<tool> → capabilities.tools ──> agent-server
模型调用 ──路由回前端（Location::Web）──> tools/call ──> POST /tool_result
```

## 被否决的形态 A（别不小心实现成它）

前端交 `{"command":"npx","args":[…]}` 之类的配置、server 去 spawn，
= **让客户端在服务器上执行任意命令（RCE）**；远端形态（server 侧发请求）
= **SSRF 打内网**。这不是「安全策略问题」，它在任何策略下都不该存在。
服务端连哪些 MCP 由**部署方**用 `.mcp.json` 决定（M6 已做）。

**本模块的配置里因此只有 `url`，没有 `command`——那是形态的定义，不是遗漏。**

## 文件

| 文件 | 一件事 |
|---|---|
| `errors.ts` | 三类错误：协议畸形 / 传输失败 / server 回的 JSON-RPC error |
| `jsonrpc.ts` | JSON-RPC 2.0 信封的构造与解析 |
| `sse.ts` | `text/event-stream` 响应体 → 一条条 `data:` 载荷 |
| `protocol.ts` | `initialize`/`tools/list`/`tools/call` 的 params 与 result |
| `tool_result.ts` | `tools/call` result → 「文本 + isError」 |
| `transport.ts` | Streamable HTTP 一次往返 + 应答匹配 + 超时 |
| `client.ts` | 一个已握手的连接：握手 / `tools/list` / `tools/call` |
| `translate.ts` | MCP 工具 → 注入声明；名字的拼与拆 |
| `status.ts` | 每个 server 的可用性（connected / unavailable + 原因） |
| `connect.ts` | 编排：一批配置 → 工具 + 状态 + 按名字路由的 `call` |
| `register.ts` | 与 065 的接线：逐个 `registerWebTool` |
| `index.ts` | 导出面 |

协议这一层逐条对着 `crates/agent-mcp/src/` 搬（041 的 `jsonrpc`/`protocol`/
`translate`、042 的握手、044 的失败隔离），**Rust 侧一行没改**。

## 两条翻译规则

1. **名字 `web:mcp-<server>/<tool>`。** location 从 `web:` 前缀白拿
   （`location_of` 判成 `Location::Web` → 路由回前端），`location_of` 一个字
   没动。中间 `mcp-` 是给人看的来源标记。于是服务端 MCP
   （`mcp:everything/echo`）与前端 MCP（`web:mcp-figma/get_file`）**同会话
   共存不冲突**。
2. **可逆性：`annotations.readOnlyHint === true → "pure"`，其余一律
   `"irreversible"`。** 代价不对称：判错成 pure 的代价是重放副作用（重复发
   邮件/扣款），判错成 irreversible 只是多问用户一次。未知来源的第三方工具
   默认必须落保守边。

## 失败隔离

`connectMcpServers` **不会 reject**。一个 server 连不上 / 握手失败 / 超时 /
`tools/list` 形状不对 → 它标 `unavailable`（带原因），**其余照常、会话照常
起**。每条状态都进 `servers[]`（UI 可渲染）**并且**打一条 `console.warn`
——「别静默」是验收条款：一个源悄悄没了，模型只会表现为「它突然不会用那个
工具了」，那种 bug 最难查。

同一条原则贯彻到更细的粒度：

- **工具名过不了 061 的白名单** → 跳过那一个工具并告警，**绝不 sanitize**
  （悄悄改写会让两个不同声明撞成一个）。不跳的话整个 `POST /sessions` 会被
  400，那才是真的拖垮会话。
- **server id 重复 / 含非法字符** → 那一条不装载，其余照常。
- **`tools/call` 失败**（传输错、JSON-RPC error）→ 落成
  `{ isError: true }` 而不是抛，066 直接回传给模型自纠。

## 代价：连接管理、超时、重连都在前端

这是本设计**把复杂度从 server 推到浏览器**的代价，如实记在这里：

- **超时**：服务端 MCP 有 tokio 和进程树兜底，浏览器什么都没有——一次
  `fetch` 挂住就是永远挂住。所以每次往返自带 `AbortController` + 计时器
  （握手 20s、普通请求 30s，可按 server 配）。
- **重连**：**本模块不做自动重连。** 声明只在建会话那一次发出去（接缝 §三：
  不做运行时增删，理由是红线 11 的前缀缓存），会话中途某个 MCP server 掉线，
  再重连也没法把它的工具补进这个会话的工具表。掉线表现为调用时
  `isError`，模型能看到并自纠。要「换一批能力」就得建新会话。
- **CORS**：浏览器直连意味着 MCP server 必须允许这个源跨域，且要能接受
  `Mcp-Session-Id` / `MCP-Protocol-Version` 这两个自定义头。够不着的 server
  （只监听 localhost stdio 的那类）**本来就不该由 server 代劳**——那就是形态 A。
- **传输**：只做 Streamable HTTP（2025-03-26 起的单端点形态），不做
  2024-11-05 那版 `GET /sse` + 分离 POST 端点的老传输。

## 接线点（留给 065 / 066）

本 issue **只提供模块**，不改 `main.ts` / `api.ts` 的既有函数。

**065（声明）**——`main.ts` 里在 `createSession` **之前**加两行：

```ts
const mcp = await connectMcpServers([{ id: "figma", url: "https://…/mcp" }]);
registerMcpTools(mcp);              // 混进 065 的 webCapabilities()
const sessionId = await createSession(webCapabilities());
```

`registerMcpTools` 走的就是 065 `capabilities/index.ts` 里写明留给 067 的那个
口子（`registerWebTool(tool, impl)`，`impl` 转发一次 `tools/call`）。
配置从哪来是接入方的事（本模块不读任何配置文件——读配置就得决定配置放哪，
那是 068 真机接入时的判断）。

**066（执行）**——两条路二选一：

- 走 065 的 `findWebTool(name)`：`registerMcpTools` 已经把实现登记进去了，
  066 **一行都不用为 MCP 特判**（推荐）。
- 或者自己路由：`mcp.handles(name)` → `mcp.call(name, input)` →
  `{ text, isError }` 直接组 `POST /tool_result` 的 body。

**UI**——`mcp.servers` 逐条 `describeStatus()` 就是一行人话，挂状态栏即可
（`console.warn` 已经保底，UI 是加分项）。

## 验证

```sh
pnpm --filter web verify:mcp
```

`scripts/verify-mcp.ts` 对着 `scripts/mock-mcp-server.ts`（最小 mock，
Streamable HTTP）跑 46 条断言：翻译形状、版本协商不断言、SSE 里插播的通知与
错号响应被跳过、失败隔离、调用路由、与 065 的接线。为什么不是 vitest：本仓
没有任何 TS 测试框架，Node 24 自带类型擦除能直接跑 `.ts`，只差一个 12 行的
解析钩子——细节见 `docs/issues/067-frontend-mcp-client.md` 的实做记录。
