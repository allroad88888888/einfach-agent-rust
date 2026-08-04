# 066 前端：执行 remote tool 并回传结果

**里程碑** M10 · **依赖** 060（挂死修复） · **模型** sonnet · **独测** —

让前端**真的执行**模型点名的 `web:` 工具，并把结果送回去。这是整条注入链上唯一「让能力
真正可用」的一步。

## 动手前必须先看 [038](038-frontend-tools.md)

038（**另一个会话在做**）是「前端工具闭环：位置透明路由的最后一跳」，**本 issue 很可能
就是它的产物**。现状（勘查确认）：

- `packages/web/src/api.ts` 导出 `createSession`/`eventsUrl`/`fetchAgentTree`/`sendInput`/
  `sendUndo`/`sendRedo`/`sendCancel` —— **没有 `sendToolResult`**。
- `packages/web/src/render/tool.ts` 收到 `tool_executing` **只画一张卡片**，不执行不回传。

**若 038 已交付这一半 → 本 issue 直接关闭并在记录里写明「由 038 覆盖」。**
**若只交付了一部分 → 只补缺口。** 重复实现是最坏的结果。

## 范围（`packages/web/`）

1. **`api.ts` 加 `sendToolResult`**：`POST /sessions/{id}/tool_result`，
   body `{ agent, tool_call_id, result: { content, is_error } }`。
   **没有 epoch 字段**——服务端保管（客户端伪造不了，`tool_result.rs` 的模块文档写死了这条）。
2. **派发**：收到 `tool_executing` 且 `request.location === "Web"` → 按 `request.tool`
   找到本地实现 → 执行 → 回传。**找不到实现** → 回传 `is_error`（**不要沉默**：
   060 之后不回传会等到超时，回传 `is_error` 让模型立刻自纠）。
3. **结果大小**：server 侧上限 1 MiB（`MAX_RESULT_BYTES`），超了会 400。前端**自己先截断**
   并在内容里说明，别让请求被拒。

## 验收（可判定）

- 模型调用一个前端实现了的 `web:` 工具 → 前端执行 → `POST /tool_result` 返回 **202** →
  **同一轮**继续（server 侧 `resolve_remote_tool` → `runner::resume`）→ 结果进下一轮 prompt。
- 模型调用一个前端**没实现**的 `web:` 工具 → 回传 `is_error` → 模型收到错误、loop 继续
  （不挂起、不超时）。
- 超大结果被前端截断后仍然 202（不撞 1 MiB 上限）。
- `pnpm -r typecheck` + `build` 绿。

## 注意

- **先看 038**，别重复实现；有冲突就停下来报告。
- **与 [065](065-frontend-inject.md) 都会碰 `api.ts`**——你只加 `sendToolResult`，
  **不要动** `createSession` 的签名（那是 065 的）。
- **不要碰 Rust 侧**。
- 收工验证前台跑完（WORKFLOW §四 -1）。

---

## 实做记录（完成 · 2026-08-04）

### 先说 038 的核对结论：**没有覆盖，本 issue 照做**

动手第一件事是核 038 交了什么。结论是**一个字都没交**：

- `docs/issues/038-frontend-tools.md` 全文只有 3 行（标题 + 元信息那一行的半句，
  `**里程碑** M5 · **依赖** 034 · **模型** opus · **独`——句子断在这里），没有范围、
  没有验收、没有实做记录。那个会话还没写完 issue 本身。
- `grep -rn "sendToolResult\|tool_result" packages/web/src/` 只捞到**注释和 067 的
  MCP 内部文件**（`mcp/tool_result.ts` 是「`tools/call` result → 文本 + isError」的拍平，
  不是 `POST /tool_result`）：`api.ts` 里没有 `sendToolResult`，`render/tool.ts` 收到
  `tool_executing` 只 `el("div", "tool-card")` 画卡片。

所以本 issue 从零做，**没有和 038 重复的代码**。038 若之后落地，它要接的是同一组接线点
（`findWebTool` / `sendToolResult`），不需要再造一份。

### 改动

| 文件 | 行数 | 干什么 |
|---|---|---|
| `packages/web/src/tool-exec.ts`（新增） | 129 | **执行 + 回传**：`tool_executing` → `findWebTool` → 跑 → `POST /tool_result` |
| `packages/web/src/api.ts`（改，87→111） | 111 | 加 `sendToolResult` + `ToolResultBody`；**`createSession` 一个字没动**（065 的） |
| `packages/web/src/main.ts`（改，56→67） | 67 | 把执行器和渲染器并排接到同一条 SSE 上 |
| `packages/web/scripts/verify-tool-exec.ts`（新增） | 209 | 可判定验收：`pnpm --filter web verify:tool-exec` |
| `packages/web/package.json`（改） | — | 多一条 `verify:tool-exec` 脚本 |

`crates/` 一个字没碰。`packages/web/src/mcp/`（067）、`src/capabilities/`（065）一个字没碰
——**067 的 MCP 工具是经 `registerWebTool` 进的同一张表，所以派发这边一行 MCP 特判都没有**，
本地示例工具和 MCP 转发在 `findWebTool` 之后长得一模一样。

### `sendToolResult` 的签名

```ts
export interface ToolResultBody { content: string; is_error: boolean }

export function sendToolResult(
  id: string, agent: AgentId, toolCallId: ToolCallId, result: ToolResultBody,
): Promise<void>
```

`POST /sessions/:id/tool_result`，body `{ agent, tool_call_id, result: { content, is_error } }`，
走既有的 `postJson`（非 2xx 抛 `describeError` 的统一错误形状）。**没有 epoch 参数**——
server 侧 `RunnerCtx` 保管、必须精确匹配仍在等待的 `(agent, call_id)`，客户端伪造不了，
也就不该出现在签名里（`routes/tool_result.rs` 模块文档）。`is_error` 那边带
`#[serde(default)]`，这边**始终显式发**，不让「没写 = false」变成靠记忆维护的默认值。

### 派发流程

`main.ts` 在同一条 SSE 上并排挂两个消费者，**渲染和执行是两件事**：

```ts
const render = createRenderer(sessionId);        // 画卡片（render/dispatch.ts，没改）
const executeTools = createToolExecutor(sessionId); // 执行（tool-exec.ts，新的）
connect(sessionId, (frame) => { render(frame); executeTools(frame); }, …);
```

**没有**把执行塞进 `render/dispatch.ts` 的 `switch`——那个文件的唯一职责是「一帧 →
该调渲染层哪个函数」，执行工具不是渲染。先渲染后执行，让卡片在工具真跑起来之前就出现。

执行器本体：

1. 不是 `tool_executing` → 不管。
2. `request.location !== "Web"` → 不管。**位置只认 server 推的这个字段**，前端不按
   `web:` 前缀再推一遍（同一件事两处判据迟早分叉）。`Desktop` 不归浏览器管。
3. 同一个 `call_id` 已经处理过 → 不管（一个 call 最多执行一次、最多回传一次）。
4. `findWebTool(request.tool)` → 跑 → `sendToolResult`。

### 找不到实现怎么处理：**回传 `is_error`，并且说人话**

```
本前端没有实现工具 web:xxx。这个会话可用的工具以模型收到的工具表为准，
请换一个已声明的工具，或者换个办法完成这件事。          + is_error: true
```

外加一条 `console.warn`。**沉默是最坏的选项**：060 给远端等待补了截止线之后不回传不再
挂死会话，但要一直等到那条截止线（分钟级）才拿到 `is_error`，中间模型什么也做不了。
同样地——实现**抛异常**（`WebToolImpl` 的约定，MCP 的 `isError: true` 已由
`mcp/register.ts` 对齐成抛异常）→ `is_error` + 异常信息；**回传本身失败**（网络断了/会话
没了）→ `console.error` 明说「这次调用要等到服务端的远端截止线才会拿到 is_error」，
这条日志是排查「模型为什么卡了几分钟」的唯一线索。

### 截断策略

`MAX_RESULT_BYTES = 1 MiB`，跟 `routes/tool_result.rs` 同一个数。三条判断：

1. **按 UTF-8 字节量，不是 `.length`**。Rust 那边是 `String::len()`（字节）；JS 的
   `.length` 是 UTF-16 码元，一个汉字 1 个 `.length`、3 个字节——按 `.length` 判会漏判
   到三倍，照样撞 400。所以用 `TextEncoder`。
2. **说明写进 `content` 本身**，并且**先从预算里扣掉它的字节数**（不扣的话截断后的结果
   正好又超限）。模型只看得到 `content`，没有第二个通道告诉它「后面还有」。
3. **刀口退回字符边界**：UTF-8 续字节形如 `10xxxxxx`，切在多字节字符中间时
   `TextDecoder` 会塞一个 U+FFFD（重新编码 3 字节），反而可能把结果顶回上限之上。
   所以先 `while (keep > 0 && (bytes[keep] & 0xc0) === 0x80) keep -= 1`。

实测 `fitToLimit("汉".repeat(600_000))`（1.8 MB）→ **1048574 字节**（上限 1048576）。

**截断不算失败**（`is_error: false`）：结果是真的，只是不全，模型该拿它继续干活。

### 真实验证（前台跑完，真实输出）

```
$ pnpm -r typecheck
Scope: 3 of 4 workspace projects
packages/protocol typecheck: Done
packages/web typecheck: Done

$ pnpm --filter web build
✓ 18 modules transformed.
dist/index.html                  1.13 kB │ gzip: 0.61 kB
dist/assets/index-Ct4-gWUj.css   3.94 kB │ gzip: 1.41 kB
dist/assets/index-Cqbg7Xt3.js   12.59 kB │ gzip: 5.63 kB
✓ built in 106ms

$ bash scripts/check-invariants.sh <本次改动的四个文件>
红线检查通过
```

**1. `pnpm --filter web verify:tool-exec`（新增，23 条断言全绿）**——mock 端点逐条复刻
`routes/tool_result.rs`（同一个 1 MiB 上限、超了 400、成功 202），所以「截断之后还会不会
被拒」是判出来的不是读出来的：

```
[1] 截断（纯函数，UTF-8 字节口径）      ✓ 没超限原样返回 / ✓ ≤1 MiB / ✓ 内容里说明了 /
                                        ✓ 刀口落在字符边界（无 U+FFFD） / ✓ 保留开头那段
[2] 实现了的工具 → 202，is_error:false  ✓ 打的是 tool_result 端点 / ✓ 202 /
                                        ✓ body 三个字段齐（没有 epoch） / ✓ agent、call_id 原样带回
[3] 没实现 → is_error，不沉默           ✓ 照样 202 / ✓ is_error:true / ✓ content 说得出是哪个工具
[4] 实现抛异常                          ✓ is_error:true / ✓ 带上了异常信息
[5] 超大结果                            ✓ 202 不是 400 / ✓ 字节数在上限内 / ✓ 截断不算失败
[6] 异步实现 + 不该执行的帧             ✓ await 到返回值 / ✓ Server/Desktop/非工具帧/重放 一条都不发
[7] 子 agent                            ✓ agent 是 root/a1，不是 root
=== 23 条通过，0 条失败 ===
```

（`scripts/` 不在 `tsconfig.include` 里，所以它不进 `pnpm -r typecheck`——这是 067 定的
既有形状：仓里没有 `@types/node`，`mock-mcp-server.ts`/`verify-mcp.ts` 同样如此。
断言器是「跑起来红不红」，不是 `tsc`。）

**2. 真 agent-server（`examples/serve`，`AGENT_STATIC_DIR=packages/web/dist` 同源托管）**
——cargo 这次没和别的会话抢到锁，真机跑成了。经真的 `src/tool-exec.ts` 打真的 Rust 路由：

```
201  /sessions                            (body 2 字节)
202  /sessions/sess-76413-0/tool_result   (body 256 字节)       ← 实现了的
202  /sessions/sess-76413-0/tool_result   (body 93 字节)        ← 没实现的（is_error）
202  /sessions/sess-76413-0/tool_result   (body 1048657 字节)   ← 截断后的 1 MiB

对照：不截断直接发 → 被真 server 拒了：
  bad_request: tool result content 不能超过 1048576 bytes
```

最后那一条是关键对照：**上限是真的**，`fitToLimit` 挡住的就是它。

**3. 真浏览器（Playwright）+ 真 server + 真示例工具**。`AGENT_STATIC_DIR` 那条同源路径
`GET /` 返回 200（1130 字节，新构建产物），页面建会话、连上 SSE、`POST /sessions` 的
请求体里两个 `web:demo/*` 声明都在。随后在页面里把真实的 `tool_executing` 帧喂给真实的
`createToolExecutor`，三条 `POST /tool_result` **全部 202**，body 抓的是真实网络请求：

```json
{"agent":"root","tool_call_id":"bt-1","result":{"content":"066 真机验证页","is_error":false}}
{"agent":"root","tool_call_id":"bt-2","result":{"content":"{\"width\":1200,\"height\":773,\"dpr\":1}","is_error":false}}
{"agent":"root","tool_call_id":"bt-3","result":{"content":"本前端没有实现工具 web:demo/没这个工具。…","is_error":true}}
```

`bt-1` 的 `content` 就是页面此刻真实的 `document.title`（验证前改成了「066 真机验证页」）
——**server 无论如何拿不到这个字符串**，它只能来自浏览器里真的跑了一次 `web:demo/page-title`。
控制台除了 `favicon.ico` 404 无任何报错，唯一那条 warning 是第 3 条自己打的
「本前端没有实现 …」（不静默，符合预期）。

### 没能验的一条：**模型自己点名调用**（卡在 062，不是本 issue）

验收第一条的完整链路是「模型调 → 前端执行 → 202 → 同一轮继续」。前半截**现在还走不通**，
和 066 无关：真机让 deepseek 调 `web:demo/page-title`，它的回答是

> 我的工具列表里没有 "web:demo/page-title" 这个工具。我有的工具是：srv_3Afs_2Fread、
> srv_3Afs_2Flist、srv_3Ashell_2Fexec、srv_3Aagent_2Fspawn/status/collect

查下来是 **062（per-session 装配）还没落地**：`routes/sessions.rs` 现在只做
`capabilities::validate(declared)`（061 的校验），**校验完就丢**，没有任何代码把声明装进
这个会话的 `ToolTable`——061 的模块注释也自称「只校验，不装配」，`docs/issues/062-
capabilities-assembly.md` 没有实做记录。于是模型的工具表里根本没有 `web:` 工具，
`dispatch.rs` 那条远端路的 `ctx.tools.declares(&tool)` 闸（060 补的）也就永远不放行。

**062 一落地，这条链路不需要前端再改一行**：前端的这一半（收帧 → 执行 → 回传 → 202）
已经用真 server 逐条验过了，缺的只是「server 把声明装进表、于是真的会推一帧
`tool_executing` 下来」。完整的模型在环验收是 [068](068-host-capabilities-dogfood.md) 的事。

### 一个不归本文件管的已知面：刷新页面会重放

没带 `Last-Event-ID` 的新连接（= 刷新页面）server 会把整个 ring 补发一遍
（`http/hub/ring.rs` 的 `replay(None)`：`effective_last = oldest.id - 1` → 全量），其中包含
**早就回传过**的 `tool_executing`。执行器里那个 `handled` 集合活在一次
`createToolExecutor` 里，挡得住同一页面生命周期内的重复派发，**挡不住刷新**（刷新后是
全新的集合）。真跑第二遍的后果：迟到的回传在 server 侧被安全拒绝（`take_remote_tool`
找不到等待槽 → 既有的 `TransportTrouble` 路，060 的验收条款之一），但**工具本身的副作用
会真的再发生一次**。现在无害（示例工具都是 `"pure"`）；要根治得让协议能区分「补发的历史」
和「等我执行的派发」，那是协议面的判断，不是前端这一层能自己判出来的。`tool-exec.ts`
的头注释里记了同一段。
