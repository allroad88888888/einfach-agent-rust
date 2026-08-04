# 078 server 形态下 MCP 是休眠的——`with_mcp` 一次都没被调用过

**里程碑** M10 收尾时捞到 · **依赖** — · **模型** sonnet · **独测** ✅（碰红线 11 与可逆性查表）

068 第四条真机验收时撞上的。**跟 064 发现的「server 形态下 skill 是休眠的」是同一个形状**，
只是这次轮到 MCP。

## 现象

```
$ grep -rn "with_mcp\|McpRegistry" crates/agent-server/src/ | grep -v tests
crates/agent-server/src/registry/spec.rs:61:   /// （§五，形状照 `ToolTable::with_mcp` 的既有先例）
crates/agent-server/src/actor/capabilities.rs:37: //! 排在 `with_skills`/`with_mcp`/…
```

**两处都只是注释。** `ToolTable::with_mcp` 在 `agent-server` 里**一次都没被真正调用**，
`agent-server-bin` 也不读 `.mcp.json`——只有 `agent-cli` 装 MCP（`crates/agent-cli/src/mcp.rs`
的 `bootstrap`）。

所以**经 HTTP 起的会话里没有任何 `mcp:` 工具**：M6 做的整套服务端 MCP，在 server / 桌面 /
网关这三种形态下等于不存在。

## 它挡住了什么

068 第四条的后半句**结构上跑不出来**：

> **同一会话里同时有** `mcp:everything/echo`（`.mcp.json` 配的，服务端连）**和**
> `web:mcp-<x>/<t>`（前端连的）→ **两条路都调得通、互不干扰**。

前半句（形态 B，前端自己连）**已经真机验过**（068 §真机记录第四条），后半句缺的就是本 issue。

## 范围

1. **server 形态装载 `.mcp.json`**：照 `agent-cli::mcp::bootstrap` 的既有形状——
   **别新发明**。它已经处理好了「缺失/坏配置不致命」「server 之间按 id 排（红线 11）」
   「失败隔离」「告警经 `warn` 出口」（074 补的最后一跳）。
2. **配置从哪来要拍**：CLI 用 `--mcp-config` + 默认 `./.mcp.json`。server 形态的对应物
   是什么？`ServerConfig` 的一个字段？环境变量？**这是部署配置不是会话状态**
   （跟 073/076 那一类相反——MCP server 列表是运维定的，不是客户端声明的），
   所以**不进 store**，跟 `remote_tool_timeout`（060）同一类。拍完写进记录。
3. **句柄住 store 外**（红线 3）：`Arc<McpRegistry>` 进 `RunnerCtx`，跟 CLI 同一条路。
   MCP client 从不进任何 command / atom——043 已经把这条钉死，本 issue 只是复用。
4. **跟 076 的开关对齐**：`mcp:` 工具算不算「默认那批」、能不能被 `disable_builtin` 关掉？
   **倾向不能**——076 的天花板明确只含五档，理由是「宿主控制不了的那批」；
   而 MCP server 列表是运维配的，运维不想给就别配。拍完写进记录，别默默决定。

## 验收（可判定）

- **server 形态真的有 MCP 工具**：配一个 `.mcp.json`（stdio `server-everything`）→
  `POST /sessions` → 那个会话的工具表里有 `mcp:everything/echo`，且
  `snapshot()` 的可逆性从 `readOnlyHint` 翻译出来（`Pure`，043 的既有机制）。
- **两条 MCP 路共存不打架**（**这条就是 068 第四条的后半句**）：同一个会话里既有
  `.mcp.json` 来的 `mcp:everything/echo`，又有前端注入的 `web:mcp-<x>/<t>` →
  **两个都调得通**，各自走各自的执行路径（前者服务端 stdio、后者远端回传）。
- **红线 11**：MCP 工具进表之后，第 2 轮缓存对账 `预测 == 实际`
  （M6 CLI 上验过 `7040`，server 形态要重新确认一次）。
- **没配 `.mcp.json` 时逐字节不变**：不配的会话工具表与今天完全相同（这条要有断言）。
- **失败隔离**：配一个起不来的 server → 会话照常建、照常能用，只是没有那些工具。

## 注意

- **别顺手把 MCP 做成会话状态**：它是部署配置（见 §范围 2）。073/076 那条「进 store、
  恢复原模原样复刻」的规矩**不适用**——搞混了会给运维配置开一个 undo 的口子。
- **别碰** `crates/agent-mcp/` 的既有形状（041/042/043/074 都有测试钉着）。
- 070 的两层锁、074 的同名去重都在 `agent-mcp` 里，本 issue 白拿，别重做。
- 红线 9：≤300 行。
- 收工验证前台跑完（WORKFLOW §四 -1），含 `--features ts`。
