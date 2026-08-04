# 068 宿主能力注入真机 dogfood ← M10 终点

**里程碑** M10 · **依赖** 064 + 066 + 067 + **072**（重放不重复执行）+ **073**（恢复复刻） · **模型** 主会话真机 · **独测** 真机

M10 的「能用」终点。照 M6/M8/M9 的规矩：**验收靠一次真实运行，不靠形容词**。

## 真机验收（可判定）

**一、宿主声明的工具，模型真的用上了**
前端声明一个只有它干得了的工具（如 `web:demo/page-title` 读 `document.title`）→ 建会话 →
给模型一个**只能靠它完成**的任务 → 模型自己发现并调用 → 前端执行 → `POST /tool_result` →
结果进下一轮 → 模型用它回答。
断言：事件流里有 `tool_executing`（`location: "Web"`）+ 对应的 `tool_executed`，
最终回答含那个工具返回的内容。

**二、作用域隔离**
同一个 server 上另起一个**不带声明**的会话 → 那个工具**不在它的表里**，模型调不到。

**三、skill 的延迟加载**
声明两个 skill → 第一轮 prompt 只有**索引两行**、没有 `body` → 模型
`srv:skill/activate` 一个 → 那一轮才出现 `body` 和它自带的工具 → `/undo` 撤掉激活 →
`body` 消失。

**四、MCP（形态 B）与服务端 MCP 共存**
前端连一个真 MCP server → 工具以 `web:mcp-<server>/<tool>` 出现 → 模型调用 → 前端转发 →
结果回来。**同一会话里同时有** `mcp:everything/echo`（`.mcp.json` 配的，服务端连）
**和** `web:mcp-<x>/<t>`（前端连的）→ **两条路都调得通、互不干扰**。

**五、红线 11 的真机体现**
第 2 轮起缓存对账 `预测 == 实际`——注入的工具进了表但**没有破坏前缀稳定性**。
M6/M8 都验过这个形式，注入是新变量，要重新确认一次。

**六、恢复原模原样复刻（073）**
同一个 chatid + `session_path` 建会话并注入 → 对话一轮 → 让会话落盘并关掉 →
**同 chatid 重开、请求体里一个 `capabilities` 字段都不带** → 那些工具**仍然在表里**、
模型仍然调得到；且**恢复后第一轮的工具表字节与关闭前最后一轮相同**（看缓存对账，
不是看「有这个工具」）。
再断反面：重开时**带上** `capabilities` → **400 `session_has_history`**（不是通用
`bad_request`，客户端要能区分「我名字写错了」和「这会话已有历史」）。

**七、重连不重复执行（072）**
模型调一个有副作用的注入工具（用一个**会计数**的假工具，比如往文件追加一行）→
宿主执行并回传 → **新开一条无游标的连接**（不带 `Last-Event-ID` 的
`GET /events/poll`，网关背后的浏览器刷新就是这个形状）→ 历史里那帧 `tool_executing`
会照旧补发 → 断言**副作用计数仍然是 1**。
对照组同样要断：派了活**还没回传**就换新连接 → 那条活**必须**被执行（漏活比重复执行
更隐蔽）。

## 这两条为什么必须走 chatid，而不是浏览器 demo

`packages/web` 的 `createSession` **没有 chatid 参数**、每次开页建全新会话
（`api.ts:51` / `main.ts:18`），所以浏览器 demo 里**既恢复不了历史、也重放不出历史帧**——
六、七两条在它上面**结构上不可复现**。

好消息是**不需要动网关**：`POST /sessions` 的请求体同时收 `id`（chatid，055）、
`session_path`（落盘）、`capabilities`（061），**curl 自己就是一个合法宿主**——
它声明能力、执行工具、回传结果，跟浏览器走的是同一条路。
参考网关（`examples/java-gateway`）当前**只发 `{id}`、不带 `capabilities`**，
所以它跑不出注入的工具；「网关要不要透传 `capabilities`」是**部署策略**，
接缝 §九（安全，暂缓）里的倾向是**默认不透传、要用得显式打开**——**不在本 issue 射程**。

## 怎么跑（本机的既有坑，别再踩一遍）

- **`curl` 必须加 `--noproxy '*'`**：本机有 `http_proxy=http://127.0.0.1:7897`，
  不加会假 502（M9 dogfood 踩过，还一度被误判成「vite 代理坏了」）。
- **浏览器验收用 `AGENT_STATIC_DIR`**：`examples/serve` 认它，由 server 同源托管
  `packages/web/dist`，**不经 vite dev 代理**——省掉一个变量（M8 dogfood 的做法）。
- 一、二、三、五**用 curl 就能覆盖**（`capabilities` 是 `POST /sessions` 的字段，
  `tool_result` 是普通 POST）；第四条需要前端跑起来。
- **providers.toml 只读不印不提交**，任何输出只出长度/状态。

## 注意

- 真机若捞到新问题 → **单列新 issue**，不塞进本 issue 硬修（049/050/060 的先例）。
- 真机步骤前台跑完、如实报（WORKFLOW §四 -1）。

## 真机记录（主会话，2026-08-04）· 七条里六条兑现，第四条待做

**跑法**：`agent-server`（真 deepseek，`tools=builtin+shell+spawn` 开满档）+ 一个
**Node 驱动脚本当宿主**（scratchpad 里的 `dogfood-068.mjs`）——它声明能力、执行工具、
回传结果，跟浏览器走同一条 HTTP 路，区别只在没有 DOM。**24 条断言全绿**，分四段：

| 段 | 条数 | 兑现的验收 |
|---|---|---|
| 管道（不烧 token） | 7 | 带 chatid + `session_path` + 声明建会话；`GET /pending_tools` 通；`GET /sessions/{id}` 三态；**400 `session_has_history`** 且报文含「先 GET」的指引 |
| 真模型 | 6 | 一、二、五 |
| 恢复 | 5 | 六（含 072 在恢复路径上再验一次） |
| skill | 6 | 三 |

**几条决定性的证据**（不是「跑通了」这种形容词）：

- **三、skill 延迟加载**：正文里埋口令 `<NONCE>-REFUND`，索引行里没有。第一轮模型能列出
  两个 skill 的**名字和描述**，但**说不出口令**；第二轮它自发调 `srv:skill/activate`，
  然后一字不差念出 `ZK8UPZE5-REFUND`；**另一个 skill 的口令仍然不知道**。
  ——「body 到底进没进 prompt」从客户端唯一可判定的形式（prompt 在服务端组装，外面看不见）。
- **七、重放不重复执行**：收场之后新开**无游标**连接，历史里那条 `tool_executing`
  **确实又来了一遍**（`重放 tool_executing×1`），但副作用计数**没动**（`1 → 1`）。
  同一条断言既证明了 bug 的前提真实存在、也证明了修复挡住了。
- **六、恢复复刻**：同 chatid、**请求体里一个 `capabilities` 都不带**重开 → 模型这一轮
  **新调**了 `web:demo/lookup_order`，副作用 `1 → 2`，且无前缀漂移告警。
- **一**：最终回答里含只有工具才知道的校验码；**二**：不带声明的会话里那个工具泄漏×0。

### 第四条（前端 MCP 形态 B）· 真浏览器跑通，**后半句另开 078**

**跑法**：自写一个最小 **Streamable HTTP** MCP server（`scratchpad/mcp-http-server.mjs`，
60 行，2025-03-26 单端点）——`packages/web/src/mcp/transport.ts` 明确只说 Streamable HTTP，
而本机的 `server-everything` 是 stdio 形态，所以造一个可控的对手，并在返回值里埋一个
**只有它知道的编号**，「模型是不是真的走通了这条路」才判定得了。
前端由 `crates/agent-server/examples/serve.rs` 的 `AGENT_STATIC_DIR` **同源托管**
（不经 vite 代理，省掉一个变量），playwright 驱动真浏览器。

**067 留下的接线由本 issue 补上**（067 §七原话：「配置从哪来本模块不管……那是 068 真机
接入时的判断」）：新增 `packages/web/src/mcp-config.ts`（72 行），MCP server 列表从
**地址栏**来——`?mcp=<id>=<url>`。取向与理由写在那个文件的头注释里：形态 B 的要点就是
**浏览器自己连**，配置放在浏览器自己的地址栏最短；写死要重新 build、localStorage 看不见
清不掉。**不带参数一条都不连**，`webCapabilities()` 与 067 之前逐字节相同。
`main.ts` 加了「先连 MCP、再建会话」的 11 行（顺序不是风格问题：注入只有建会话那一次机会）。

**真机证据**（浏览器里逐条可见）：

```
⋯ 准备调用 web:mcp-stamp/stamp_document
⚙ web:mcp-stamp/stamp_document
Web · Irreversible · {"doc_id":"HT-2024-001"}
✓ web:mcp-stamp/stamp_document · 输出 56 字节
骑缝章编号：**MCPBC3A685-2**
usage prompt=2280 completion=39 cached=2048 · drift=Clean
       · reconcile=Match{"predicted":2048,"actual":2048} · window=Healthy{turns:2,hit_percent:87}
```

对应的 MCP server 端日志：`initialize → ok` / `tools/list → ok` / `tools/call → ok`。

逐条对上验收：**名字**是接缝定的 `web:mcp-<server>/<tool>`；**位置与可逆性**推对了
（`Web · Irreversible`——server 声明 `readOnlyHint: false`）；**结果真的回来了**并被模型
用进回答（`MCPBC3A685-2` 只有那个 MCP server 知道）；**红线 11 白拿**——第 2 轮
`predicted == actual == 2048`，注入的 MCP 工具进了表也没炸前缀（第五条在浏览器里又验了一次）。

顺带证明了**两类注入共存**：模型的思考里先想 `web:demo/page-title`（065 的演示工具）
再改用 MCP 那个——宿主自己声明的和 MCP 翻译进来的在同一张表里，互不干扰。

### 第四条的后半句跑不出来 → **078**

验收原文还要求「同一会话里同时有 `mcp:everything/echo`（`.mcp.json` 配的，**服务端连**）
和 `web:mcp-<x>/<t>`（前端连的）」。**做不到，而且不是环境问题**：

`ToolTable::with_mcp` 在 `crates/agent-server/` 里**一次都没被调用过**（只剩两处注释提到它），
`agent-server-bin` 也不读 `.mcp.json`——**只有 `agent-cli` 装 MCP**。所以经 HTTP 起的会话里
根本没有任何 `mcp:` 工具。**跟 064 发现的「server 形态下 skill 是休眠的」是同一个形状。**

单列 [078](078-server-form-mcp-is-dormant.md)（照本 issue §注意「真机若捞到新问题 → 单列新
issue，不塞进本 issue 硬修」）。**M10 因此是「六条兑现 + 第四条前半句兑现」**，
后半句随 078 补。

### 驱动脚本自己错了两次，记一笔（都不是产品问题）

1. 帧的真实形状是 `{id, event:{agent, event:{type,data}}}`——**领域事件比第一版脚本假设的
   多嵌一层**，结果整轮全空。
2. 更阴的一条：第二轮的回答跟第一轮**一字不差**。原因是从空游标起 poll 把历史整段重放
   回来，而重放里有上一轮的**终态通知**，等待循环当场 break——第二轮压根没等，还拿上一轮
   的回答当结果。recover 和 skills 两处各踩一次，已收进脚本的 `drainToTip()`。

第 2 条的形状**恰恰就是 072 说的那件事**：历史帧和新帧长得一样，消费者分不出。
写驱动的人（我）就是那个分不出的消费者——这条比任何论证都说明 072 不是纸上问题。
