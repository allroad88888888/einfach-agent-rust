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

---

## 114c 实做记录（2026-08-11）

浏览器里真跑通了。产物 `crates/agent-wasm`（独立 workspace，理由同 `probes/api`），
构建 `scripts/build-wasm.sh`，页面 `crates/agent-wasm/www/index.html`。

### 接缝落在哪（`agent-core` 里仍然零 `cfg`，红线 12）

117 之后 native 只剩两处线程，两处都在平台接缝之下，这次各拆成一个目录：

| | 契约 | native | wasm32 |
|---|---|---|---|
| `agent-runtime/src/io_stream/` | `open() -> Receiver<StreamItem>`，同步返回、请求当场起飞 | 工作线程 + `block_on` 发送 | 两个 `spawn_local`：生产侧跑 `post_stream_async`，转发侧把行会合式交给泵 |
| `agent-runtime/src/heartbeat/` | `start`/`register`/`Drop` | 只睡觉只叫人的线程 | `setInterval`/`clearInterval` |

`io_task.rs`/`io_bus.rs`/`runner.rs` 一行没改——这是接缝位置正确的判据。

**wasm 行源为什么中间多一条 unbounded channel**：`post_stream_async` 的 `on_line`
是同步回调（与 native 逐字同签名），而浏览器单线程模型下没有「阻塞等一个
Promise」这回事。所以同步回调只做不阻塞的 `unbounded_send`，另一个任务把它转成
会合式 `send().await` 交给泵——**并且它是泵那条 channel 唯一的写入方**，否则
`Done` 会插到还排队的行前面。代价：`fetch` 与转发任务之间没有背压（浏览器本来
就在自己那层缓冲响应体，我们没有手段把背压传回 `ReadableStream`）。

### 会话持久化：`persist/idb/web_store.rs`

`SessionStore::load()` 是同步的、IndexedDB 不是，所以真正的重放挪到一个 `async`
构造器 `WebIdbStore::open()`（宿主开会话时 `await` 一次），此后 `load()` 读它自己
连续维护的 mirror。这不是「缓存了一份可能过期的数据」——`worker.rs` 的整套记账
本来就建立在「mirror 与 journal 重放结果恒等」上，这里只是把同一条不变量用在读的
一侧。写入走一条内存队列 + 同一时刻至多一个 drain 任务（journal key 是递增计数器，
每个写各 `spawn_local` 一次会让事务完成顺序决定编号顺序）。

### 真机验收结果

1. **跑完一轮** ✅ Chrome + DeepSeek `deepseek-v4-pro`，流式回复。托管只有
   `python3 -m http.server`（只发 html/js/wasm 三种字节，不参与任何模型请求）。
2. **模型调宿主工具** ✅ 声明 `web:page/title` / `web:page/url`（`crate::tools`），
   模型调 `web:page/title` 拿到 42 字节标题并据此作答。走的是 M10 远端等待槽那条
   既有路，同进程只是把 HTTP 往返换成一次函数调用。
3. **刷新接着聊 + 红线 11** ✅ 刷新四次、同一 id 重开，历史从 IndexedDB 重放
   （最后一次 12 条，含 tool_use/tool_result）。抓真实请求体比对：重开后第一轮的
   `tools` 与关闭前最后一轮**逐字节相同**（416 字节，字符串全等）。这是 114a
   `web_kv.rs` 第一次真跑，`put`/`scan_prefix` 都对。
4. **取消** ✅ 流到一半点取消 → 65ms 内 `AbortController.abort()` 被调用（100ms
   取消轮询节奏），`performance` 里那条请求的 duration 正好停在点击那一刻；
   `Failed(Cancelled)` + `undo_turn` 丢弃半轮（`Applied { entries: 3 }`），下一轮
   正常作答。
5. **`srv:` 不出现** ✅ 工具表从 `ToolTable::empty()` 起步，只 `with_host_tools`。
   真实请求体里没有 `srv:`；连 wasm 产物里都搜不到 `shell/exec`/`srv:fs/read`
   ——那些 spec 构造器从没被调用，被 DCE 整个删掉了。
6. `cargo test --workspace --no-fail-fast` ✅ 唯一失败是既有的
   `agent-server` `http_image_input::text_stays_on_old_wire_shape_...`。

**没验成的两条，如实记：**

- **三家 provider 各跑通一轮 —— 只跑通了 DeepSeek。** 这台机器的
  `~/.config/agent/providers.toml` 里 kimi/glm 两段 `api_key` 是空的，环境变量也
  没有。用占位 key 各发了一轮，两家的请求都**穿过 CORS 拿到真实 401**并被 adapter
  正确分类成 `Failed(Provider(Auth))`——说明 transport 与 adapter 在 wasm 下无差异，
  差的只是一把能用的 key。
- **`agent-mcp` 仍然编进了浏览器产物。** 111 决策表第一行要求「不编」，但
  `agent-runtime` 对它是无条件依赖，摘掉要在 `ctx.rs`/`dispatch.rs`/`runner.rs`/
  `io_task.rs`/`mcp_call.rs`/`lib.rs` 六处撒 cfg——那正是本 issue「别往业务逻辑里
  撒 cfg」要避免的。当前状态是**代码在、路径不可达**：工具表里没有任何 `mcp:` 名字，
  `McpRegistry` 是空表，`dispatch` 的第四路要求 `tool.starts_with("mcp:") &&
  table_declared`，两个条件都不可能成立，所以 `mcp_call::start` 里那句
  `thread::spawn`（wasm 上会 trap）永远走不到。产物里搜得到 `tools/call`/`jsonrpc`
  字符串，是死重量不是活代码。真要摘干净，该另开一个 issue 把 MCP 做成
  `agent-runtime` 的 feature，而不是在这里顺手加六处条件编译。
