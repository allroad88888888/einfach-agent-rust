# MCP 怎么接：一个 adapter，不是核心抽象

`agent-mcp` 把「一个外部工具服务器提供的能力」翻译成本仓那张**扁平工具表**里的项。
它和 `agent-providers` 是同一类东西——架构里「外部差异合法存在」的地方——判据也同源：

> **它是外部来源相关的判断吗？** 是 → `agent-mcp`。不是 → 别处。

接缝定歪了差异会往两边漏：漏进 core 就是 `match server` 满天飞、`mcp` 字样出现在
`agent-core`；漏进 runtime 就是 dispatch 开始懂 JSON-RPC。

## 跟 provider adapter 的一同一异

| | provider adapter | mcp adapter |
|---|---|---|
| 同 | 唯一一处外部差异合法存在；依赖方向 → `agent-core`；零 core 侵入 | 同左 |
| **异** | **纯函数、零 IO**（`agent-transport` 发网络） | **必须做 IO**（spawn 子进程、JSON-RPC 往返） |

因为要做 IO，`agent-mcp` **不在红线 7 的两个 crate 里**（core / store），它是独立 crate，
自带传输。红线 7 的精神仍在：协议与翻译那一层写成**纯函数**，可对录制帧回归（041）；
IO 只在 stdio 传输那一层（042）。

## 三样东西过接缝（MCP server → 本仓）

```
        外部 MCP server            agent-mcp                本仓
        ────────────────         ─────────              ──────
 tools/list 响应   ──────→   翻译   ──────→   Vec<ToolSpec>（喂模型）
 annotations       ──────→   翻译   ──────→   每工具的 Reversibility
 连接/握手/退出     ──────→   健康   ──────→   server 可用性（连上没/host 支不支持）
```

只有这三样。**没有第四样**——MCP 的 wire 类型（JSON-RPC envelope、`initialize` 的
capabilities 协商、protocol 版本）**烂在 `agent-mcp` 里**，就像 provider 的 wire 字段名
烂在 `agent-providers` 里。`agent-core` grep 不到 `mcp` / `jsonrpc`。

## 枢纽：可逆性不能再从名字推

现状 `ToolTable::snapshot()` 按**名字前缀机械判**可逆性（`srv:fs/read` → `Pure` 写死在
`reversibility_of`）。这对内置工具成立：名字是本仓自己起的，可逆性是常量。

**MCP 打破它。** `mcp:everything/echo` 和 `mcp:everything/sendEmail` 同前缀，
一个 readOnly 一个不是。**可逆性是每个工具的属性，来自 server 声明的
`annotations.readOnlyHint`，不是名字。** 所以工具表要**携带一份 `mcp:` 工具的可逆性
映射**，`snapshot()` 撞上 `mcp:` 前缀就查这份映射，查不到落保守 `Irreversible`。

这是整个 M6 对现有代码**唯一的结构性冲击**，落在 043。

### 翻译规则（TOOLS.md 钉死，不猜）

- `annotations.readOnlyHint == true` → `Reversibility::Pure`
- 其余**一律 `Irreversible`**（无 annotations、字段缺失、为 false），除非本地配置显式标注

理由是代价不对称：判错成 `Pure` 的代价是重复发邮件 / 扣款；判错成 `Irreversible`
只是多问用户一次。**一个未知来源的 MCP 工具默认可重放，等于把数据事故的开关交给
第三方。** 默认必须落在保守那边。

### M19 之后：这份映射只决定显示，不决定 undo 挡不挡

决策 34（[199](issues/199-reversibility-as-delivery-decision.md)）把可逆性从「声明的
枚举」改成「工具执行完交回的还原函数」——三态 `Aftermath`（`Nothing` 没碰外部世界 /
`Undo(f)` 碰了且给出还原函数 / `Irreversible` 碰了还不回去），对应落盘的三态
`Undoability`（`StateOnly` / `Hooked` / `Blocked`）。MCP 协议里**没有「撤销」这个
概念**——`tools/call` 只是又发一次 RPC，server 在结构上交不出一个我们能调用的还原
函数（[202](issues/202-host-mcp-undo-none.md)）。所以上面这份 `readOnlyHint` 映射从
M19 起**只影响显示**（CLI/Web 打印这一行时用什么字样），不再是「`/undo` 撞上它要不要
停下来问」的依据。

undo 挡不挡看的是另一条判据：**声明的是事实还是承诺**。`readOnlyHint: true` 声明的是
「这次调用没碰外部世界」——一个**事实断言**，不需要函数来兑现，照单全收，落
`StateOnly`，**不挡**。宿主声明的 `reversible` 则相反：声明的是「有补偿动作」这个
**承诺**，兑现承诺就得交出那个结构上交不出来的函数，落 `Blocked`，**挡**。MCP 的
`annotations` 里没有第二个字段能声明这种承诺（没有等价于宿主 `reversibility:
"reversible"` 的档位），所以「承诺挡」这条判据在 MCP 这边**没有对应的落点**——
`readOnlyHint` 缺失 / `false` / 无 annotations 一律落 `Blocked`，理由从「可逆性等级
不够」变成「没有事实断言，默认不采信」，结果不变。

**上面那条翻译规则（决策 22）因此不被反转**：`true → 不挡`、其余 `→ 挡`，字面行为
逐字不变；变的只是「不挡」背后站着的理由。

## 活句柄住 store 外（红线 3）

stdio server 是一个子进程。它的句柄（stdin/stdout pipe、`Child`、reader 线程）
**不可序列化**，塞不进 atom。红线 3 早就点名了这个场景：「MCP 子进程句柄放 store
外面的 runtime registry，atom 里只放可序列化句柄」。

所以：

- **atom / 快照里**只有 server 的**配置与逻辑标识**（server id、命令行、可用性位）——全部可序列化。
- **活句柄住一个进程内的 `McpRegistry`**（runtime 持有，类比在飞 provider 调用的凭据表）。
- **崩溃恢复**：从配置**重连**，不从快照复活句柄。恢复出来的会话历史里有 MCP 调用记录，
  但那些子进程是新起的——这正确，因为句柄本来就是进程局部的。

## 执行模型：异步在飞，不同步阻塞（决策）

`tools/call` 是对子进程的 JSON-RPC 往返，**可以任意慢**（网络型工具，如联网搜索 server）。
本仓有两条现成的工具执行路：

| | 谁在用 | 代价 |
|---|---|---|
| 同步 | `fs/read`、`shell/exec`（`tool_exec::execute`，actor 线程上跑完） | **冻住所有 agent**，undo/cancel 一起卡。shell 靠内部超时兜，但它的慢有上限 |
| 异步 | provider 调用（`provider_call::start/finish`，起一个 IO future（同线程 `FuturesUnordered` 泵进度）+ 在飞凭据 + 泵管落地） | 多 agent 并行不被掐死；**epoch 校验天然在这条路上** |

**MCP 走异步。** 三个理由：

1. **MCP 的慢没有上限**——阻塞 actor 线程不可接受（一个 agent 等 server，全树停摆）。
2. **红线 6 的 epoch 回写天然在这条路上**：调用在飞时用户 undo，结果回来 epoch 不符
   直接丢弃——异步路的泵已经在替 provider 调用做这件事，MCP 复用它。
3. **不新发明记账**：复用 `provider_call` 的在飞凭据机制，不为 MCP 造第二套。

代价：MCP 执行要一套自己的在飞凭据（比同步路复杂）。这是 043 的 opus 判断，碰红线 6。

## dispatch 怎么分第四路

现状 `run_effect` 的 `Effect::ExecuteTool` 分三路：spawn 截获 / skill 截获 /
其余走 `tool_exec::execute(ctx.fs)`。**MCP 加第四路**：

```
tool 名以 "mcp:" 开头 且 工具表声明了它
    → 起一次异步 MCP 调用（发给 McpRegistry 里那个 server 的 client）
    → 返回在飞凭据，泵落地（epoch 校验后回写）
```

`ctx.fs`（`ToolExecutor`）够不着 MCP client，就像它够不着 `Session`（spawn/skill 走截获同理）。
按前缀 match 在**宿主侧**合法：宿主本来就持有工具表，这里没有任何模型相关判断——
红线 12 管的是 **core** 里的 **provider** 分支，不管宿主按工具名分派。

## host 能力差异（为延后的 http 留位）

stdio 只有 server / 桌面 host 有，浏览器 host 只能 http。**M6 只做 stdio**，所以浏览器
host 上没有任何 MCP server——registry 要能表达**「这个源在这个 host 上不可用」**，
而不是假装它存在、到调用时才失败（TOOLS.md §「host 能力差异」）。

这条**现在就把接口形状写死**（registry 带可用性位），等 http 传输来了（延后 issue），
浏览器 host 才长出远端 server，那时不用改破坏性接口。

## 配置：`.mcp.json`，跟 Claude Code 对齐

```json
{
  "mcpServers": {
    "everything": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-everything"],
      "env": {}
    }
  }
}
```

- **key = server id**，进 `mcp:<id>/<tool>` 命名（对应 Claude Code 的 `mcp__<server>__<tool>`）。
- **stdio**：`command` + `args` + `env`。
- **（延后）远端**：`{ "type": "http" | "sse", "url", "headers" }`——形状先在文档占位，M6 不实现。

多来源与冲突：两个 server 各有 `search` → `mcp:a/search` vs `mcp:b/search`，**server id
天然消歧**。同一个 server id 在配置里撞名是配置错误，装载时报出来，不静默取后者。

## M6 明确不做（等真实反馈，别提前猜）

- **http / sse 远端传输**（浏览器 host 的 MCP）——接口留位，实现延后。
- **resources → skill 资产、prompts → skills**——TOOLS.md 提过方向，但形态没有真实使用
  反馈，先定死等于猜。
- **OAuth**（远端 server 鉴权）——随 http 一起延后。
- **provider 内部执行的隐形工具**（TOOLS.md §「服务端工具不是第四种 Location」）——那是
  provider adapter 的会话级审计开关，**跟 MCP 客户端是两回事**，别混进来。

## 落到哪几个 issue

| issue | 定这一层的哪部分 |
|---|---|
| [040](issues/040-mcp-seam.md) | 本文档 + ROADMAP 决策（crate 边界 / 可逆性元数据流 / 异步执行 / MVP 范围） |
| [041](issues/041-mcp-protocol.md) | 协议类型 + JSON-RPC 帧 + 翻译，零 IO 对录制帧全绿 |
| [042](issues/042-mcp-stdio.md) | stdio 传输 + 握手，真子进程；句柄住 store 外 |
| [043](issues/043-mcp-execution.md) | 执行路由 + 可逆性元数据 + epoch 回写（红线 6） |
| [044](issues/044-mcp-config.md) | `.mcp.json` 装载 + 多 server + 失败隔离 + host 可用性门 |
| [045](issues/045-mcp-cli.md) | CLI 接入 + `/mcp` 状态，M6 全链验收 |

## 自查：放错地方的症状

| 症状 | 说明什么 | 怎么办 |
|---|---|---|
| `agent-core` 里出现 `mcp` / `jsonrpc` / `match server` | 差异漏上来了 | 吞回 `agent-mcp` |
| 可逆性从 `mcp:` 名字推 | 错，MCP 可逆性是 per-tool 元数据 | 工具表携带映射，从 `readOnlyHint` 翻译 |
| 活句柄（`Child`、pipe、线程）进了 atom / 快照 | 违反红线 3（编译期挡不住，review 挡） | 句柄住 `McpRegistry`，atom 只放 server 配置 |
| MCP 调用同步阻塞 actor 线程 | 多 agent 并行被掐、undo 卡住 | 走异步在飞路（provider_call 同款） |
| 一个 server 起不来，整个会话失败 | 失败没隔离 | 标 `Unavailable`，其余照常，会话能起 |
| 未知 `readOnlyHint` 被当成 `Pure` | 把数据事故开关交给第三方 | 缺失/false/无 annotations 一律 `Irreversible` |
| 以为 MCP server 能交回还原函数（「让 server 多加一个补偿工具就不用挡了」） | 还原函数是本进程的闭包，跨 JSON-RPC 交不出来；MCP 协议里也没有「撤销」这个概念 | 只有「没碰外部世界」这个**事实**能被采信（`readOnlyHint: true` → `StateOnly`，不挡）；任何「我能补偿」的**承诺**一律落 `Blocked`，挡 |
