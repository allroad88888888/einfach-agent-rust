# 041 MCP 协议类型 + JSON-RPC 帧 + 翻译

**里程碑** M6 · **依赖** 040 · **模型** sonnet · **独测** ✅

025 的 MCP 版：把接缝的**纯函数部分**定型，对录制帧全绿，**零 IO**。传输（真起子进程）
是 042——这个 issue 一行网络代码都没有，所有测试喂的是录制好的字节。

## 范围

新 crate `crates/agent-mcp`。这个 issue 只交付协议 + 翻译层：

1. **JSON-RPC 2.0 envelope** 的编解码：request（`id`/`method`/`params`）、response
   （`id`/`result` | `error`）、notification（无 `id`）。`initialize`、
   `notifications/initialized`、`tools/list`、`tools/call` 四个方法的 params/result 形状。
2. **`McpTool`**（server 声明的工具：`name`/`description`/`inputSchema`/`annotations`）。
3. **翻译**：`McpTool` + server id → `(ToolSpec, Reversibility)`。
   - `ToolSpec.name` = `mcp:<server>/<tool>`；`description`/`schema` 原样搬。
   - `Reversibility`：`annotations.readOnlyHint == true` → `Pure`；其余一律 `Irreversible`。

Cargo.toml 只依赖 `agent-core`（拿 `ToolSpec`/`Reversibility`）+ `serde`/`serde_json`。
**无 `tokio`/`reqwest`/`std::process`**（那些进 042）。

## 验收（可判定）

- 录制的 `initialize` 响应 → decode 出 protocol 版本 + server capabilities；未知字段不报错、
  不丢弃到猜。
- 录制的 `tools/list` 响应（含 ≥2 个工具）→ `Vec<ToolSpec>`，name 全是 `mcp:everything/<t>`
  形状，**顺序原样保留**。
- **红线 11**：同一份 `tools/list` 翻译两次，`ToolSpec` 序列化逐字节相同；`inputSchema`
  的 key 集合相同但 JSON 里插入顺序不同的两份，翻译出的 `schema` 字节相同。
- **可逆性翻译穷举**：`readOnlyHint:true` → `Pure`；`readOnlyHint:false` → `Irreversible`；
  `annotations` 缺失 → `Irreversible`；`annotations` 在但无 `readOnlyHint` → `Irreversible`。
- 序列化一个 `tools/call` request（`name` + `arguments`）= 录制的字节。
- 畸形 JSON-RPC（`id` 缺失、`result` 与 `error` 同时在、`result` 与 `error` 都不在）→
  明确的 `Err`，**不猜成成功**。
- 整个 crate 零 IO（Cargo.toml grep 不到 IO 依赖）。

## 注意

- **红线 11**（工具表逐字节确定）：翻译产物会进 prompt 最前面。`inputSchema` 是
  `serde_json::Value`，`Map` 后端是 `BTreeMap`（顶层 `serde_json` 不开 `preserve_order`）——
  依赖这个行为，测试要钉死它。见 `agent-core/src/value/tool.rs` 的既有红线 11 测试。
- **可逆性翻译错了是数据事故**（`Pure` 当 `Irreversible` → 重放副作用）——所以派独立测试
  agent，且穷举 `readOnlyHint` 的四种取值。这条判据可被断言变红（WORKFLOW §三两步判据），
  所以 sonnet 够，测试替你红。
- decode 未知 `finish`/字段走保守（`agent-providers` 的 `StopReason::Other` 同精神）：
  未知不猜成成功。
- 接口先定（pub 类型 + 签名 + 文档注释，`todo!()` 体）→ 实现与测试并行 → 合并。

## 实做记录（完成 · 2026-08-03）

接口由主会话先钉死（`src/` 的 pub 签名 + `todo!()` 体，能编译），实现 agent（sonnet）
与独立测试 agent（sonnet）**并行**分头做，主会话从磁盘合并验证。

**产出**：`jsonrpc.rs`（信封编解码）/ `protocol.rs`（initialize/tools/list/tools/call 的
params 构造与 result 解析）/ `translate.rs`（`McpTool → (ToolSpec, Reversibility)`）。测试
按 `one-file-one-thing` 拆成 9 个 `tests/*_041.rs`（全 ≤106 行）+ 各源文件贴身单测。

**验收兑现**：`cargo test -p agent-mcp` = 34 单元 + 28 集成全绿；`clippy --all-targets
-D warnings` 净；红线检查过。红线 11 逐字节两条（翻译两次 / schema 插入顺序无关）双验
（实现自测 + 独立测试各一份，独立写、结论一致）。可逆性四态穷举双验。

**坑（合并时收的）**：`tests/common/mod.rs` 的共享 helper 触发 `dead_code`——每个
`tests/*.rs` 独立编译成 crate、只 `use` 自己需要的 helper，没用到的那个二进制里就报死
代码，是 Rust 集成测试共享模块的结构性假阳性。修法 `#![allow(dead_code)]`（不合并测试
文件，那会破坏按场景的拆分）。独立测试 agent 只跑 `--no-run` 碰不到 `-D warnings`，这个
口归合并步。

**留给 042 的实测发现**：真 `@modelcontextprotocol/server-everything`（探针实抓）协商的
`protocolVersion` 是 `"2025-11-25"`，与本仓 `CLIENT_PROTOCOL_VERSION = "2025-06-18"` 不符。
041 按范围未改握手——042 的握手要**接受 server 回的版本**（MCP 是协商：client 提议、
server 定），别在版本不等时硬失败。
