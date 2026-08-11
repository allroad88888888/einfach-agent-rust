# 119 `agent-mcp` 做成 feature，而不是靠 cfg 摘

**里程碑** M13 收尾 · **依赖** 114 · **模型** sonnet · **独测** ✅（碰构建配置与工具表）

决策 26 说浏览器构建不编 `agent-mcp`。**114c 落地时没做到**，如实记在 114 的报告里：
真要摘掉，得在 `ctx.rs` / `dispatch.rs` / `runner.rs` / `io_task.rs` / `mcp_call.rs` /
`lib.rs` **六处撒 `#[cfg(target_arch = "wasm32")]`**——正是红线 12 与 114 硬约束要避免的形状。

## 当前状态：代码在，但路径不可达

不是"漏了个开关"，是**故意选了两害相权**：

- 工具表里没有任何 `mcp:` 名字（`ToolTable::empty()` 起步，宿主只注入 `web:`）
- `McpRegistry` 是空表
- `dispatch` 的第四路要求 `starts_with("mcp:") && table_declared` **两个条件都不可能成立**
- 所以 `mcp_call::start` 里那句在 wasm 上会 trap 的 `thread::spawn` **永远走不到**

代价是 772K 的 wasm 产物里躺着一坨死代码（`strings` 得到 `tools/call` / `jsonrpc`）。

## 为什么值得单独做，而不是就这么算了

死重量本身不是大问题（几 K）。**真正的问题是「不可达」是一个需要每次改动都重新论证的性质**：
今天 dispatch 那两个条件恰好互斥，明天有人给 `McpRegistry` 加一条默认注册、
或者把 `table_declared` 的判定放宽，那句 `thread::spawn` 就活了——
**而它在 wasm 上是 trap，不是报错**，表现是页面直接崩掉。

feature gate 把「不可达」从**每次都要论证的性质**变成**编译期事实**。

## 做法

`agent-runtime` 加一个 `mcp` feature（native 默认开、wasm 构建不开），
`agent-mcp` 依赖与相关代码路径挂在它下面。

**关键是别把 feature 判断撒进核心逻辑**——那跟撒 cfg 没区别，只是换了个名字。
接缝应该切在「注册与派发」这一层：没有 `mcp` feature 时，
`McpRegistry` 与 dispatch 的第四路整个不存在，而不是存在但走不到。

## 验收

- `cargo build --target wasm32-unknown-unknown -p agent-runtime`（不开 `mcp`）产物里
  **`strings` 搜不到 `tools/call` / `jsonrpc`**
- native 默认构建行为一字不变，`cargo test --workspace` 全绿
- **`rg 'cfg\(target_arch' crates/agent-runtime/src/` 的命中数不增加**——
  这条是本 issue 的自我约束：用 feature 换 cfg，不是两样都加
