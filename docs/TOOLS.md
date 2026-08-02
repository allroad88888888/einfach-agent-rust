# 工具、Skills、MCP

## 模型看到的是一张扁平表

AI 只管挑名字。执行在哪、可不可逆、从哪注册进来的，全是 descriptor 上的元数据，
由 router 和 undo 逻辑各自读各自的字段。

```rust
struct ToolDescriptor {
    name: String,        // "srv:fs/read" | "web:selection/read" | "desk:shell/exec"
    schema: JsonSchema,
    location: Location,           // Server | Web | Desktop —— router 派发
    reversibility: Reversibility, // Pure | Reversible | Irreversible —— undo / 崩溃恢复
    source: Source,               // Builtin | Mcp(ServerId) | Skill(SkillId)
}
```

叫 `Reversibility` 不叫 `Effect`——`Effect` 留给 loop 的「该发生什么」
（[issue 001](issues/001-loop-contract.md)），撞名是上一版真踩过的坑。

`location` 和 `reversibility` 是**正交的两个维度**。一个前端 tool 可以是不可逆的
（`web:clipboard/write`），一个服务端 tool 可以是纯的（`srv:fs/read`）。别合并。

### 命名空间

`<location-prefix>:<namespace>/<tool>`。MCP 来的工具再多一层 server id：
`mcp:<server>/<tool>`。

企业级多来源一定会撞名——两个 MCP server 各有一个 `search`，前端和后端各有一个
`read_file`。冲突策略拖到后面改就是破坏性变更，现在定死。

## 位置透明路由

`agent-core` 只发 `ToolCall`，**不认识「前端 / 后端」这个概念**。router 看 `location`：

- `Server` —— 本地执行，await 结果
- `Web` / `Desktop` —— 往 SSE 上扔 `tool_call` 事件，把
  `toolcall.<id>.result` 置 `Pending`，等客户端 POST `/tool_result` 回来结算

**对 core 而言两条路径完全同构**：发出去、置 `Pending`、等回写。这正是上游
`#BUSY!` 机制的现成落点，见 [STATE-MODEL.md](STATE-MODEL.md) §「Pending 的来历」。

所以 SSE 单向下行 + 普通 POST 上行就够了，不需要 WebSocket——服务端「反向调用客户端」
只是在流上推一个事件，客户端自己发一个请求回来。

### 回写必须带 epoch

`POST /tool_result` 的 body 里带上发出时的 epoch。用户在结果回来之前按了 undo，
epoch 已经 bump，这个回写直接丢弃。见红线 6。

### reversibility 等级怎么定

这个字段决定 undo 能不能越过它，以及崩溃恢复时能不能重发。**定错了是数据事故**，
不是体验问题。

| 等级 | 判据 | 例 |
|---|---|---|
| `Pure` | 重复执行任意次，外部世界不变 | 读文件、查询、搜索 |
| `Reversible` | 有明确的补偿动作，且补偿本身可靠 | 创建资源（补偿=删除）、写入有版本的记录 |
| `Irreversible` | 其余全部 | 发邮件、支付、删数据、跑 shell |

**拿不准就是 `Irreversible`。** 判错成 Pure 的代价是重复发邮件；判错成 Irreversible
的代价只是多问用户一次。

`undo` 往回走时撞上 `Irreversible` 的 entry → 停下，推 `undo_blocked` 事件，让用户
确认「继续（副作用不回滚）」还是取消。

## Skills

本质是「按需注入 context 的资产」——一段指令 + 若干文件，触发时进 prompt。

天然 atom 形状：

```
skills.active (primitive)  →  prompt.system (derived)  →  prompt.payload (derived)
```

换一个 skill 只重算 system prompt，不碰消息序列化。

Skill 可以携带 tool（`source: Skill(id)`），激活时进工具表，停用时移出，
`tools.registry_version` bump 一次，`prompt.system` 自动重算。

### 多来源与合并

内置 / 项目 / 用户 / 远端四个来源，用**和 tool 同一套** merge + 冲突策略。
不要为 skill 另造一套解析规则。

## MCP

**当成一个 adapter，不是核心抽象。** `agent-mcp` 的职责就是把 MCP server 暴露的
tools / resources / prompts 翻译成本仓的 `ToolDescriptor` 和 skill 资产，喂进同一张表。

### 服务端工具不是第四种 Location

有些 provider 能自己执行工具（检索、联网搜索），而**我们看不到**：响应里没有
`tool_calls`，也没有任何调用痕迹。它在模型内部发生，router 不参与也无从观测。

所以它不进 command log、undo 回滚不了、副作用等级无从判断、审计链路有洞。正确的建模
不是加一个 `Location` 变体，而是**一个会话级开关，开了就等于放弃这部分的可审计性**
—— 要显式承认并让用户知情，不是默认开着的便利功能。

另一类服务端工具是可见的：声明成普通 `function`，我们收到 `tool_calls` 后自己去调
provider 的执行端点 —— 那种正常走 `Location` 与 reversibility 判定。两者别混。
哪家属于哪类见 [probes/PROVIDERS.md](../probes/PROVIDERS.md)。

### host 能力差异

stdio 传输只有 server 和桌面侧有，浏览器只能 http。所以 registry 要能表达
**「这个源在这个 host 上不可用」**，而不是假装它存在然后调用时才失败。

### reversibility 等级从哪来

MCP 协议不提供副作用等级。所以：

- 有 `annotations.readOnlyHint` 的，映射成 `Pure`
- 其余**一律 `Irreversible`**，除非本地配置里显式标注

不要猜。一个未知来源的 MCP 工具默认可重放，是把数据事故的开关交给第三方。
