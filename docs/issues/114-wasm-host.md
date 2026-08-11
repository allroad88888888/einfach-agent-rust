# 114 wasm 宿主打通：浏览器里跑完一轮真实对话 ← M13 终点

**里程碑** M13 · **依赖** 112 + 113 · **模型** sonnet · **独测** —（终点 issue，靠真机验收）

把 112 的接缝和 113 的 transport 接起来，产出一个能在浏览器里独立跑的 agent 核心。

## 范围

1. **wasm 编译目标**：`wasm32-unknown-unknown` + wasm-bindgen。浏览器构建里不编
   `agent-mcp`，不声明 `agent-tools` 的 `srv:` shell/fs specs（111 定的裁剪）。
2. **`Instant` / `SystemTime` 垫片**：`PendingRemoteTool.deadline` 存的是绝对时刻，
   wasm 上垫 `web-time`。
3. **配置来源**：浏览器没有 `providers.toml`（113 明确不移植 `config.rs`）。
   provider 配置与 key 由宿主页面传入。**key 一律来自使用者自己**（111 的契约），
   不得内置任何默认 key，也不得把 key 写进任何受版本控制的文件。
4. **会话持久化走 IndexedDB**：`agent-store` 只认识 atom / 依赖图 / command log，
   本身零 IO，所以这是一个新的落盘后端，不是改 store。
   - **恢复必须仍是「从自己的 journal 忠实重放」**（决策 6）。`CreateSessionRequest` 没有、
     也不要加任何「客户端灌历史」的入口——那会同时破坏红线 11 的前缀缓存和审计一致性。
   - 会话 id 由宿主给，仍走 055 的白名单（`[A-Za-z0-9_-]`、≤128、**拒绝不 sanitize**）。
     宿主若想按 URL 分会话，**归一化和取摘要是宿主的事**，URL 本身不是合法 id。
5. **能力注入照旧**：`web:` 工具的声明与执行走 M10 那条既有路，同进程只是让它更快。
   `tool_claim` 的 CAS / epoch / `sweep()` **保留不动**（server 形态还要用，见 111）。

## 验收（可判定）

- **真机跑完一轮**：浏览器打开页面 → 用自己的 key → 跟模型说一句话 → 拿到流式回复。
  过程中**没有任何服务端进程**。
- **模型能调宿主工具**：声明一个 `web:` 工具（读页面标题那种只有前端拿得到的），
  模型调用它并用结果回答。这条证明能力注入链路在同进程下仍然成立。
- **刷新页面能接着聊**：同一个会话 id 重开，历史从 IndexedDB 的 journal 回放出来，
  且**第一轮的工具表与关闭前最后一轮逐字节相同**（红线 11）。
- **取消有效**：回复流到一半点取消，请求真的中断（DevTools 网络面板可见），进程还活着，
  下一轮能继续。
- **`srv:` 工具不出现在 prompt 里**：wasm 产物的工具表里没有 `shell/exec`、`fs/read` 这些，
  模型压根不知道有它们。
- 三家 provider 各跑通一轮（CORS 已在 111 实测过，这里验的是 adapter 在 wasm 下无差异）。

## 注意

- **别为了跑通就往 core 里加 `#[cfg(target_arch)]` 分支。** 红线 12 禁止 core 里有平台/模型
  相关判断，平台差异归 transport 与宿主装配。core 里出现 cfg 分支说明接缝切错了位置，
  回 112/113 改。
- 本 issue 不做 Tauri 侧的任何变更；决策 12「`agent-server` 是库」与既有两种形态一行不动。
- 三种形态并存之后，**CI 要能同时构建 native 与 wasm**，否则 wasm 目标会在几周内悄悄烂掉
  ——这正是决策 10 当初想避免的成本，既然选择付，就要付在能发现的地方。

---

## 拆法（2026-08-11 加）：四块，其中一块现在就能并行

114 原本是一个大 issue，串在 117 后面。但它五件事里**只有一部分真的依赖 117**
（117 换的是 IO 载体，动的是 `runner.rs` / `io_task.rs` / `ctx.rs`）。
按「跟 117 抢不抢文件」重排如下：

| | 内容 | 能不能现在开工 |
|---|---|---|
| **114a** | **IndexedDB 的 `SessionStore` 实现** —— 落在 `agent-runtime/src/persist/`（红线 7：只有运行时层能做 IO），复用 `agent-store` 的 `SessionLog`，不碰 `agent-store` | **能**。只新增 `persist/` 下的文件 + 一行 `mod`，跟 117 零重叠 |
| 114b | `Instant`/`SystemTime` 垫 `web-time` | 不能。`deadline.rs` / `ctx_remote_tools.rs` 在 117 手里 |
| 114c | wasm 编译目标 + wasm-bindgen 宿主入口，裁掉 `agent-mcp` 与 `srv:` specs | 不能。要等 117 换完载体才可能真的编过 |
| 114d | provider 配置与 key 从宿主页面注入 | 不能。碰 `ctx.rs` |

**114a 的关键设计（决定了它能不能现在验）**：`SessionStore` 是**同步、fire-and-forget**
的端口（见 `agent-store/src/persist/mod.rs` 的模块文档），所以 `append` 天然适配
——把写扔给 IndexedDB 就返回，不等。真正有风险的是**回放**：红线 11 要求
「重开会话后第一轮的工具表与关闭前最后一轮逐字节相同」。

所以 114a 要**把回放语义与 IndexedDB 绑定分开**：
- 回放/游标/压实那套逻辑复用 `SessionLog`，**在 native 上用假的 KV 后端就能测**，
  不需要浏览器；
- `web_sys::IdbDatabase` 那层做薄，薄到「看一眼就知道对不对」。

这样 114a 里唯一必须等浏览器才能验的东西，就只剩那层薄绑定。
