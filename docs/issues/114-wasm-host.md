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
