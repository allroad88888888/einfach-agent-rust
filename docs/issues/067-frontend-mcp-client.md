# 067 前端 MCP 客户端：浏览器自己连，注入的是工具

**里程碑** M10 · **依赖** 065（注入通道） · **模型** sonnet · **独测** —

MCP 从前端注入的**唯一正确形态**。接缝见
[HOST-CAPABILITIES.md](../HOST-CAPABILITIES.md) §七。

## 形态（先记住哪条被否了）

**否决的形态 A**：前端交 `{"command":"npx","args":[...]}` 之类的配置、server 去 spawn ——
那是**让客户端在服务器上执行任意命令**（RCE），远端形态则是 SSRF 打内网。
**这不是「安全策略」问题，它在任何策略下都不该存在。** 服务端连哪些 MCP 由部署方用
`.mcp.json` 决定（M6 已做）。

**采用的形态 B**：

```
浏览器 ──连──> MCP server（http/SSE，浏览器够得着的那些）
   └── tools/list → 翻成 capabilities.tools 注入 ──> agent-server
模型调用 ──路由回前端（Location::Web）──> 前端调 MCP tools/call ──> POST /tool_result
```

**server 完全不碰 MCP 协议、不 spawn 任何东西**——它眼里就是一批普通的注入工具。
这恰好补上 M6 明确延后的「**http/sse 远端传输（浏览器 host 的 MCP）**」。

## 范围（`packages/web/`，以**新模块**为主）

1. **MCP 客户端**（新目录 `src/mcp/`）：连一个 http/SSE 传输的 MCP server、
   `initialize` 握手、`tools/list`、`tools/call`。
   **可以参考 `crates/agent-mcp/` 的协议实现**（041 做的，JSON-RPC 帧、版本协商
   **记录不断言**——真 server 会回显客户端提的版本，M6 实测过）。
2. **翻译成注入声明**：
   - 名字 **`web:mcp-<server>/<tool>`** —— location 从 `web:` 白拿，
     **不动 `location_of`**（050 刚在那块落地，别撞）。于是服务端 MCP
     （`mcp:everything/echo`）与前端 MCP 在同一会话**共存不冲突**。
   - **可逆性前端翻译**：`readOnlyHint: true → "pure"`，否则 `"irreversible"`
     （041 的 `translate` 逻辑搬到 TS）。**server 不重新解释、也不需要懂 MCP**。
3. **接进 065 的声明源**：翻译出来的工具混进 `capabilities.tools`。
4. **执行接进 066**：模型调 `web:mcp-*` → 前端转成 `tools/call` → 结果回传。
5. **失败隔离**：一个 MCP server 连不上**不能拖垮会话**——不注入它的工具，其余照常
   （044 在服务端解决过同一问题，这次在前端重演）。

## 验收（可判定）

- 连一个真 http MCP server（或本地 mock）→ `tools/list` 的工具出现在注入声明里、
  名字形如 `web:mcp-<server>/<tool>`、`readOnlyHint: true` 的翻成 `"pure"`。
- 模型调用其中一个 → 前端转发到 MCP → 结果经 `POST /tool_result` 回来。
- **失败隔离**：把 MCP server 地址改成连不上的 → 其余工具照常注入、会话正常建起来、
  界面上能看出这个源不可用（别静默）。
- `pnpm -r typecheck` + `build` 绿。

## 注意

- **不要碰 Rust 侧**——本 issue 是纯前端；`crates/agent-mcp/` 只读参考。
- **不要碰** `api.ts` 的既有函数（065/066 的地盘）；本 issue 以新建 `src/mcp/` 为主。
- 连接管理/重连/超时**都在前端**，这是本设计把复杂度推过去的代价，**如实写进 README**。
- 收工验证前台跑完（WORKFLOW §四 -1）。

## 实做记录（实现 agent，2026-08-04）

**纯前端，新建 `packages/web/src/mcp/`（12 个文件 + README）+ `packages/web/scripts/`
（验证用，不进 tsconfig 的 `include`、不进 vite 产物）。`crates/` 一行没改。**

### 一、形态：采用 B，A 连形状都表达不出来

`McpServerConfig` 只有 `{ id, url, headers?, timeouts? }`——**没有 `command`/`args`
字段，这不是遗漏，是形态的定义**。前端交 spawn 配置（形态 A）在本模块里连类型都写不出来。
server 全程不碰 MCP 协议：它收到的只是一批普通的 `web:` 注入工具声明（走 061/062 那条路）。

传输只做 **Streamable HTTP**（2025-03-26 起的单端点形态）：`POST` 一条 JSON-RPC，响应体
要么 `application/json`、要么 `text/event-stream`。不做 2024-11-05 那版
`GET /sse` + 分离 POST 端点的老传输（两条通道分离、状态更多，浏览器侧没有理由背它）。
`EventSource` 用不上——它只会发 GET、不能带自定义头、拿不到 POST 响应体里的流，
所以 SSE 分帧自己写（`sse.ts`，用 `getReader()` 而不是 `for await`，后者浏览器至今没有普遍支持）。

### 二、模块结构（一个文件一件事，最大 200 行）

| 文件 | 行 | 一件事 |
|---|---|---|
| `errors.ts` | 58 | 三类错误：协议畸形 / 传输失败 / server 回的 JSON-RPC error |
| `jsonrpc.ts` | 89 | JSON-RPC 2.0 信封的构造与解析 |
| `sse.ts` | 83 | `text/event-stream` 响应体 → 一条条 `data:` 载荷 |
| `protocol.ts` | 140 | `initialize`/`tools/list`/`tools/call` 的 params 与 result |
| `tool_result.ts` | 49 | `tools/call` result → 「文本 + isError」 |
| `transport.ts` | 200 | 一次往返 + 应答匹配 + 超时/取消 |
| `client.ts` | 108 | 一个已握手的连接 |
| `translate.ts` | 77 | MCP 工具 → 注入声明；名字的拼与拆 |
| `status.ts` | 48 | 每个 server 的可用性 |
| `connect.ts` | 193 | 编排：配置 → 工具 + 状态 + 按名字路由的 `call` |
| `register.ts` | 38 | 与 065 的接线：逐个 `registerWebTool` |
| `index.ts` | 32 | 导出面 |

协议层逐条对着 `crates/agent-mcp/src/` 搬（041 的 `jsonrpc`/`protocol`/`translate`、
042 的握手与应答匹配、044 的失败隔离）。**一处刻意的分层差异**：Rust 侧应答匹配在
`client.rs`（stdio 是一条长连接管道，所有响应混在一起），这里在 `transport.ts`
——响应就在本次 POST 的响应体里，「一次往返」是传输自己的语义。判断本身一字不改。

导出的接口签名：

```ts
connectMcpServers(configs: McpServerConfig[], options?: ConnectMcpOptions): Promise<McpToolSource>
registerMcpTools(source: McpToolSource): number

interface McpServerConfig { id: string; url: string; headers?: Record<string,string>;
                            handshakeTimeoutMs?: number; callTimeoutMs?: number }
interface McpToolSource {
  tools: CapabilityTool[];                                   // 直接混进 capabilities.tools
  servers: McpServerStatus[];                                // 谁连上了、谁没有、为什么
  handles(name: string): boolean;                            // 066 路由用
  call(name: string, args: unknown): Promise<ToolCallOutput>; // { text, isError }
  close(): Promise<void>;
}
```

另导出低层件（`McpClient` / `parseInjectedToolName` / `flattenToolResult` /
`describeStatus` / 三个错误类），正常接线用不到。

### 三、翻译规则

1. **名字 `web:mcp-<server>/<tool>`**。location 从 `web:` 白拿（`location_of` 判成
   `Location::Web` → 路由回前端），**`location_of` 一个字没动**（050 那块没碰）。
   中间 `mcp-` 是给人看的来源标记，跟 065 自己的 `web:demo/*` 一眼分得开。
   服务端 MCP（`mcp:everything/echo`）与前端 MCP（`web:mcp-figma/get_file`）同会话共存不冲突。
2. **可逆性**：`annotations.readOnlyHint === true → "pure"`，其余（`false` / 没有
   `annotations` / `annotations` 在但没有 `readOnlyHint`）**一律 `"irreversible"`**
   ——照 041 `translate.rs` 的规矩，代价不对称，未知来源的第三方工具必须落保守边。
   四种取值在验证里穷举了。
3. **描述缺失落空串**，不拿工具名顶替（不替 server 编话）；**schema 原样搬**。
4. 名字**只允许 `[A-Za-z0-9_-]`**（比 061 的白名单 `[A-Za-z0-9_/-]` 再收紧一格：段内
   不许有 `/`，否则 `web:mcp-a/b/c` 拆不回唯一的 `(server, tool)`）。

**类型来源**：`CapabilityTool` 从 `@agent/protocol` 导入（061 生成的）。动手时 065 还在
用它自己的 `src/capabilities/wire.ts` 本地定义，做到一半 061 落地、065 换成了生成物，
本模块跟着换掉——**全程没有第二份手写镜像**（决策 2）。`McpTool.inputSchema` 的静态类型
直接借 `CapabilityTool["schema"]`，因为它就是被原样搬过去的那个字段。

### 四、失败隔离怎么做的

`connectMcpServers` **不会 reject**。四种失败各自落成一条结构化状态 + 一条 `console.warn`：

- server 连不上 / 握手失败 / 超时 / `tools/list` 形状不对 → 该源标 `unavailable`（带原因），
  其余照常连、工具照常产出；
- **server id 重复** → 后来的那条不装载（不做「后来居上」，那会产出撞名工具让 061 400 掉整个会话）；
- **server id 含非法字符** → 该条不装载；
- **单个工具名过不了白名单** → 跳过那一个工具，同 server 的其余照常。**绝不 sanitize**
  （同 055 的 chatid、061 的名字校验：悄悄改写会让两个不同声明撞成一个）。

「别静默」落到两处：`servers[]` 里一定有它那一条（UI 可渲染，`describeStatus()` 给一行人话）
**并且**一定打 `console.warn`（`onStatus` 回调是加分项，不传也不会静默）。
`fetch` 的 `cause` 剥了一层——不剥的话每个连不上的源报出来都是同一句 `TypeError: fetch failed`，
等于没报。

**运行期的失败**（`tools/call` 传输错、JSON-RPC error）不抛，落成 `{ isError: true }`
——066 直接回传给模型自纠（对齐 066「找不到实现也要回传 is_error，别沉默」）。
只有「这名字根本不归我管」才抛，那是调用方的路由 bug，不该悄悄变成一次工具失败。

### 五、验证：为什么是 `node` 脚本而不是 vitest

**本仓没有任何 TS 测试框架**——`packages/protocol/src/fixtures.test.ts` 那份「测试」的
断言器就是 `tsc` 自己（它的头注释写死了这条）。为跑几十条断言往仓里装 vitest + 一套配置，
是给所有人加一个要维护的依赖。Node 24 自带类型擦除能直接跑 `.ts`，唯一的坎是它**不改 ESM
解析规则**（`import "./client"` 找不到，必须带 `.ts`），而本包 `src/` 一律无扩展名 +
`moduleResolution: "Bundler"`。所以加了 `scripts/ts-resolve.mjs`（31 行的 `registerHooks`
解析钩子，解析不到就试 `.ts` / `/index.ts`）——**改源码去迁就测试是本末倒置**，那会让整个包
的 import 风格分裂，还要给 tsconfig 开 `allowImportingTsExtensions`。

`scripts/mock-mcp-server.ts` 是最小 mock（`node:http`，Streamable HTTP），刻意做了三件
真 server 会干、客户端必须扛住的事：① `initialize` 回 `"2099-01-01"`（跟客户端提议的
`"2025-06-18"` **不同**）；② `tools/list` 走 SSE，真响应之前插播一条注释、一条
`notifications/tools/list_changed` 通知、一条 **id=9999 不对号**的响应，真响应本身还拆成
**两行 `data:`**；③ 带 `Mcp-Session-Id`，后续请求必须回带。跑起来是真 HTTP、真 `fetch`、
真流——不是 fake fetch。

```
$ pnpm --filter web verify:mcp

[1] 翻译规则（纯函数，对应 crates/agent-mcp/src/translate.rs）
  ✓ readOnlyHint: true → pure
  ✓ readOnlyHint: false → irreversible
  ✓ 没有 annotations → irreversible
  ✓ annotations 里没有 readOnlyHint → irreversible
  ✓ 名字形如 web:mcp-<server>/<tool>
  ✓ 描述缺失落空串，不拿名字顶替
  ✓ schema 原样搬

[2] 名字拆解（066 路由用）
  ✓ 拆回 (server, tool)
  ✓ 服务端 MCP 的名字不归本模块管
  ✓ 前端自有工具不归本模块管
  ✓ 缺斜杠 → null
  ✓ 非法字符 → null（不 sanitize）

[3] tools/call result 拍平（066 直接拿去组 tool_result）
  ✓ 多个 text 块拼接
  ✓ isError 读出来
  ✓ 没有 text 块时不喂空串

[4] 版本协商：记录，不断言相等
  ✓ 采用 server 回的版本
  ✓ 跟客户端提议的版本确实不同（这一步没抛，就是不断言）
  ✓ serverInfo.name 读出来

[5] 失败隔离：一个连不上，另一个照常
[mcp] demo 的工具 "bad name!" 名字含不允许的字符，跳过（不改写）
[mcp] MCP demo：已连接，注入 5 个工具
[mcp] MCP broken：不可用（连不上 http://127.0.0.1:59144/mcp（TypeError: fetch failed：connect ECONNREFUSED 127.0.0.1:59144））——该源的工具未注入，其余照常
[mcp] MCP demo：不可用（server id 重复——后来的这个不装载，避免撞名）——该源的工具未注入，其余照常
[mcp] MCP bad id!：不可用（server id 含不允许的字符（只允许 A-Za-z0-9_-））——该源的工具未注入，其余照常
  ✓ 四条配置四条状态
  ✓ 好的那个连上了
  ✓ 坏的那个标 unavailable
  ✓ unavailable 带得出原因（不静默）
  ✓ 重复 id 不装载
  ✓ 非法 id 不装载

[6] tools/list：SSE 里的通知/错 id 被跳过，工具翻成注入声明
  ✓ 六个工具里跳掉名字非法的那个，剩五个
  ✓ 名字全部形如 web:mcp-demo/<tool>
  ✓ readOnlyHint: true → pure
  ✓ readOnlyHint: false → irreversible
  ✓ 无 annotations → irreversible
  ✓ 无 readOnlyHint → irreversible
  ✓ 无描述 → 空串
  ✓ 名字非法的工具没混进来
  ✓ 后续请求回带 Mcp-Session-Id
  ✓ 后续请求带协商后的 MCP-Protocol-Version
  ✓ Accept 两种都声明

[7] 路由与执行
  ✓ handles 认自己的
  ✓ handles 不认服务端 MCP
  ✓ handles 不认没连上的源
  ✓ 调用结果拍平成文本
  ✓ MCP 报 isError → 原样带回
[mcp] 调用 web:mcp-demo/no_annotations 失败：McpRpcError: server 报错 [-32602]: 未知工具 no_annotations
  ✓ JSON-RPC error 落成 isError，不抛
  ✓ 不归自己管的名字要抛（那是调用方路由 bug，不该变成一次工具失败）

[8] 与 065 的接线：registerWebTool
  ✓ 五个都登记进去了
  ✓ MCP 工具进了 webCapabilities()
  ✓ 065 自己的示例工具还在
  ✓ close() 不抛

=== 46 条通过，0 条失败 ===
```

```
$ pnpm -r typecheck
Scope: 3 of 4 workspace projects
packages/protocol typecheck$ tsc --noEmit
packages/protocol typecheck: Done
packages/web typecheck$ tsc --noEmit
packages/web typecheck: Done

$ pnpm --filter web build
vite v7.3.6 building client environment for production...
✓ 17 modules transformed.
dist/index.html                  1.13 kB │ gzip: 0.61 kB
dist/assets/index-Ct4-gWUj.css   3.94 kB │ gzip: 1.41 kB
dist/assets/index-C-Z8ALT5.js   10.94 kB │ gzip: 4.92 kB
✓ built in 111ms
```

**诚实标注一条**：`build` 绿但**没有覆盖到 `src/mcp/`**——`main.ts` 还没有引用它，
vite 只打包从入口可达的模块（17 modules 里没有本模块）。真正覆盖它的是 `typecheck`
（`tsc` 按 `src/**/*.ts` 全量检查）和上面那 46 条断言。接线一进 `main.ts`，build 自然覆盖。

### 六、代价：连接管理 / 超时 / 重连都在前端

本设计把复杂度从 server 推到浏览器，如实记（README 里有同一份）：

- **超时**：服务端有 tokio 和进程树兜底，浏览器什么都没有——一次 `fetch` 挂住就是永远挂住。
  每次往返自带 `AbortController` + 计时器（握手 20s、普通请求 30s，可按 server 配）。
  超时和连不上在错误消息里分得开（排查方向完全不同）。
- **重连：本模块不做。** 声明只在建会话那一次发出去（接缝 §三，理由是红线 11 的前缀缓存），
  会话中途某个 server 掉线，再重连也没法把工具补进这个会话的工具表。掉线表现为调用时
  `isError`，模型看得到并自纠；要换一批能力就得建新会话。**这是接缝的性质，不是偷懒**。
- **CORS**：浏览器直连意味着 MCP server 必须允许这个源跨域，且接受 `Mcp-Session-Id` /
  `MCP-Protocol-Version` 两个自定义头。够不着的 server（只监听本机 stdio 的那类）
  **本来就不该由 server 代劳**——那就是形态 A。

### 七、留给 065 / 066 的接线点

本 issue **只提供模块**，`main.ts` / `api.ts` 的既有函数一行没改（那是 065/066 的地盘）。

**065（声明）** —— `main.ts` 里 `createSession` 之前两行：

```ts
const mcp = await connectMcpServers([{ id: "figma", url: "https://…/mcp" }]);
registerMcpTools(mcp);                      // 混进 065 的 webCapabilities()
const sessionId = await createSession(webCapabilities());
```

`registerMcpTools` 走的正是 065 `capabilities/index.ts` 写明留给 067 的口子
（`registerWebTool(tool, impl)`，`impl` 转发一次 `tools/call`；MCP 的 `isError: true`
翻成 `throw`，对齐 065 定的「抛异常 = 这次调用失败」）。**配置从哪来本模块不管**
——读配置就得决定配置放哪，那是 068 真机接入时的判断。

**066（执行）** —— 二选一：

- 走 065 的 `findWebTool(name)`：`registerMcpTools` 已经把实现登记进去了，
  066 **一行都不用为 MCP 特判**（推荐）；
- 或者自己路由：`mcp.handles(name)` → `mcp.call(name, input)` → `{ text, isError }`
  直接组 `POST /tool_result` 的 body。

**UI** —— `mcp.servers` 逐条 `describeStatus()` 就是一行人话，挂状态栏即可
（`console.warn` 已经保底）。

### 八、没做的 / 不该顺手做的

- **不读任何配置文件**：MCP server 列表从哪来是接入方（068）的判断，本模块只收数组。
- **不做自动重连**（理由见 §六）、不做 `resources`/`prompts`（本仓工具表只吃 tool）。
- **不碰 Rust 侧、不碰 `location_of`、不碰 `api.ts`/`main.ts` 的既有函数。**
- `packages/web/package.json` 加了一个 `verify:mcp` 脚本（纯新增，不动既有三条）。
