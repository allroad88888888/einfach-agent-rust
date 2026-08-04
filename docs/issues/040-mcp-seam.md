# 040 MCP 接缝定义（决策）

**里程碑** M6 · **依赖** — · **模型** opus · **决策类**（无代码产出，产出是决策 + 理由）

## 要定什么

MCP 接入的形状，动手写代码之前必须先定——它改变 041–045 所有代码的形状。
完整论证见 [../MCP.md](../MCP.md)，这份只记「定了什么」。

## 决策

1. **crate 边界**：新 crate `agent-mcp`，是 adapter（和 `agent-providers` 同类），依赖方向
   `agent-mcp → agent-core`。它**做 IO**（spawn 子进程、JSON-RPC 往返），所以不在红线 7
   的两个 crate 里；协议 + 翻译那层写成纯函数可对录制帧回归。

2. **可逆性元数据流**（枢纽）：`ToolTable::snapshot()` 现在按名字前缀机械判可逆性，MCP
   打破这个假设（同 `mcp:` 前缀、`readOnlyHint` 不同）。工具表**携带一份 `mcp:` 工具的
   可逆性映射**，`snapshot` 对 `mcp:` 查映射，查不到落保守 `Irreversible`。翻译规则
   TOOLS.md 钉死：`readOnlyHint==true` → `Pure`，其余一律 `Irreversible`。

3. **执行模型**：MCP `tools/call` 走**异步在飞路**（`provider_call` 同款），不走同步的
   `tool_exec`。理由：MCP 慢无上限、阻塞 actor 冻住全树；红线 6 的 epoch 回写天然在异步
   路上；复用在飞凭据不新发明记账。

4. **MVP 范围**：**stdio + tools only**。http/sse 远端传输、resources、prompts、OAuth
   全部延后（接口留位，等真实反馈）。照 022「先打通一家 provider」的先例——最小「能用」。

5. **配置格式**：`.mcp.json` + `mcpServers` 对象，跟 Claude Code 对齐。stdio 用
   `{command,args,env}`；远端形状 `{type,url,headers}` 文档占位、M6 不实现。server id 撞名
   装载报错。

## 验收

- `docs/MCP.md` 写定接缝（三样过接缝、可逆性枢纽、活句柄住 store 外、异步执行、`.mcp.json`）。
- ROADMAP §一 加一条决策（编号顺延），记 crate 边界 + 可逆性元数据 + 异步执行 + MVP 范围。
- **人拍板**（决策类必须）。

## 注意

- 红线 3（活句柄住 store 外）、红线 6（在飞 epoch）、红线 11（工具表逐字节确定）、红线 12
  （MCP 不是 provider，不碰红线 12，但按工具名分派要写清「宿主侧合法」的理由）。
- 不派测试 agent（还没有可测的东西）。
