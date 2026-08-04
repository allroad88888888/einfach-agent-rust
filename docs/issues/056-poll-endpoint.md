# 056 拉取式端点 `GET /events/poll`：ring 的第二个投影

**里程碑** M9 · **依赖** 059 · **模型** sonnet · **独测** ✅（拉取与 SSE 必须给出同一序列）

给同一个环形缓冲加第二个消费者，让 Java 网关不用代理 SSE。接缝见
[INTEGRATION.md](../INTEGRATION.md) §四。

## 范围

1. **新端点** `GET /sessions/{id}/events/poll`（`agent-server/src/http/routes/`，
   一个新文件，照 `sse.rs`/`sessions.rs` 的 handler 风格）：

   ```
   Last-Event-ID: 41          ← 跟 SSE 完全同一个游标 header
   X-Poll-Wait-Ms: 25000      ← 可选；缺省/0/解析失败 = 立刻返回
   → 200 {"frames":[{"id":42,"event":{...}}], "next":42}
   ```

2. **游标走 header，不走 query**：`agent-server` 的 axum 是 `default-features = false`
   （features 只有 `http1`/`json`/`tokio`）——**没有 `query` feature**，Cargo.toml 注释明确
   写「这个仓库没有查询参数协议」。**不要为这个端点加 feature**。header 读法照抄
   `routes/sse.rs` 的 `Last-Event-ID`（`get` → `to_str` → `parse::<u64>`，失败静默降级 `None`）。
3. **复用 `RingState::replay`**：`hub/ring.rs` 的三个变体原样用——`Live` / `Backlog(vec)` /
   `Gap{skipped, gap_frame_id, tail}`。**Gap 合成帧照 `hub/mod.rs` 现有那段**（标
   `AgentId::root()`、id 用 `gap_frame_id`、后面接 `tail`），客户端语义跟 SSE 一致。
   无 `Last-Event-ID` → `replay(None)` → 必然 Backlog、永不 Gap（031 分歧 1 的既定裁决）。
4. **`next` 由服务端算**：最后一帧的 id；**空批时也要给出正确的 `next`**（等于传进来的
   游标，首拉无游标时为 `0`）。这是下次应传回的 `Last-Event-ID`：ring 的语义是只回
   `id > Last-Event-ID`，因此不能再加一，否则会跳过下一帧。
5. **长轮询**：`X-Poll-Wait-Ms` 给了正数时，若 replay 结果为空则等到有新帧或超时。
   实现用 `tokio::time::timeout` 包住 live 订阅的 `recv()`。
   **注意：`agent-server/src` 至今一次都没用过 `tokio::time::timeout`**（唯一的定时器是
   `guard.rs` 的 `sleep`），这是第一次；测试里的用法（掐订阅上限）不是这个场景，别照抄。
   等待期间必须已经订阅了 `live`（否则等待窗口里到达的帧会漏），订阅与读 ring 的**同一次
   持锁**约束照 `spawn_forwarder` 那段（`hub/mod.rs` 有完整论证）。
6. **老的 `GET /events`（SSE）一行不改**——拉取式是新增不是替换。

## 验收（可判定）

- **推拉同源（本 issue 最重要的一条）**：同一个 session 跑一轮，一路用 SSE 收、一路用 poll
  拉 → **两边拿到的帧序列（id + 内容）完全相同**。这条钉死「同一个 ring 的两个投影」。
- 游标语义：`Last-Event-ID: N` → 只回 id > N 的帧；不带 header → 回缓冲区现有全部（
  **不是空**，031 分歧 1）；`next` 拿去当下次的 `Last-Event-ID` 能接上不重不漏。
- **空批**：没有新帧时返回 `frames: []` + **正确的 `next`**（等于传入游标），客户端拿它
  接着拉不会倒退或跳帧。
- **Gap**：把 ring 容量调小（`ServerConfig::with_ring_capacity`，测试里已有先例）撑爆缓冲
  → poll 拿到 gap 帧 + tail，且**拿 gap 帧的 id 当游标再拉一次不会二次 Gap**（ring 已保证
  `gap_frame_id = oldest-1` 的自洽，这里只需验证拉取式没破坏它）。
- **长轮询**：`X-Poll-Wait-Ms=2000` 且当前无新帧 → 请求挂住；期间产生一帧 → **立刻返回**
  （不是等满 2s）；始终无帧 → 约 2s 后返回空批。
- `X-Poll-Wait-Ms` 缺省/0/垃圾值 → 立刻返回（静默降级，跟 `Last-Event-ID` 同款）。

## 注意

- **不新增第二真值源**：读的必须是**同一个 ring**。**不要**为拉取式另建缓冲——那就是两份
  事实，reconnect 时对不上（OBSERVABILITY §「snapshot 不是 reconstruct」同精神）。
- **本 issue 不做断开检测**，[057](057-poll-disconnect.md) 做。056 单独落地时拉取式没有
  断开保护（客户端跑了不会取消在飞轮次）——这是**已知的中间态**，不是漏了；但也因此
  056 落地后**别急着让网关切过去**，等 057。
- **可见性**：`BufferedFrame` 是 `pub(in crate::http)`、`RingState::replay` 是 `pub(super)`。
  新端点在 `crate::http::routes` 下，可达；**不要**为了写它把这些放宽到 `pub`。
- **红线 11 不适用**（走协议面不进 prompt），但协议一致性仍由 032 的 ts-rs 链路 + 一致性
  测试锁——响应体类型要进 `packages/protocol` 生成。
- **红线 8**：端点在 `agent-server` 下，默认 loopback，不硬编码 `0.0.0.0`。
- 收工验证前台跑完（WORKFLOW §四 -1）。

---

## 实做记录（完成 · 2026-08-04 · 由并发线实现，主会话回填）

代码、测试、TS 生成随提交 `d0c60fe`（2026-08-04）一起落地并通过验收，但那条线没回填
记录。这份是**主会话读落地代码补的**：每条结论都能在下表的文件里逐行对上；读不出来的
地方标了「未考证」，没有为了圆场编。

### 建了什么

| 文件 | 行 | 干什么 |
|---|---|---|
| `agent-server/src/http/routes/poll.rs` | 98 | 新：handler + 两个 header 的解析 + `await_new_frames` + `next`——整个端点就这一个文件 |
| `agent-server/src/http/poll_protocol.rs` | 23 | 新：`PollFrame`/`PollResponse`（`ts` feature 下带 `ts_rs::TS`） |
| `agent-server/src/http/hub/replay.rs` | 37 | 新（前置重构）：`Replay` 三变体 → 有序帧序列，**Gap 合成从 `hub/mod.rs` 搬到这里** |
| `agent-server/src/http/hub/mod.rs` | 214→198 | 改：加 `replay_and_subscribe`/`replay_frames` 两个读口；`send_replay` 连同它内联的 Gap 合成一起删掉 |
| `agent-server/src/http/mod.rs` | +4 | 改：`mod poll_protocol` + `#[cfg(feature = "ts")] pub(crate) use poll_protocol::PollResponse` |
| `agent-server/src/http/routes/mod.rs` | +2 | 改：`.route("/sessions/{id}/events/poll", get(poll::events))` |
| `agent-server/src/ts_protocol/export.rs` | +2 | 改：`PollResponse::export_all(&cfg)` 接在 `Command`/`Frame` 之后 |
| `packages/protocol/src/generated/PollFrame.ts` / `PollResponse.ts` | 6 / 9 | 新（生成物） |
| `packages/protocol/src/index.ts` | +2 | 改：两个类型的再导出 |
| `agent-server/tests/http_indep_poll.rs` | 202 | 新：六条集成独测（其中三条属 057） |

`routes/sse.rs`、`hub/ring.rs`、`hub/guard.rs`：**一行没改**（`git diff 0f7c003 d0c60fe`
里没有它们）。「拉取式是新增不是替换」是由 diff 兑现的，不只是嘴上说的。

### 一处前置重构：Gap 合成搬进 `replay.rs`

原先 Gap 帧是在 `hub/mod.rs::send_replay` 里就地合成、就地 `tx.send` 的——那是 SSE 那条
mpsc 管道专用的形状，而拉取式要的是一个 `Vec`。**没有把那段抄第二遍**，而是把「`Replay`
→ 有序帧序列」整个提成 `replay::frames`，两条传输都调它：

- SSE：`replay_and_subscribe` → `send_frames`（逐帧灌 mpsc）
- 拉取：`replay_and_subscribe` → 直接当响应体

于是「Gap 帧标 `AgentId::root()`、id 用 `gap_frame_id`、后面原样接 `tail`」这条语义**只写
了一遍**。抄一遍也能过全部验收，但两份实现会各自漂移，而漂移的症状恰好就是本 issue 最
重要那条验收要防的东西：同一段历史的两个投影给出不同的帧序列。

### 游标走 header：勘查结论落地，一个 feature 都没加

`Cargo.toml` 里 axum 仍是 `default-features = false, features = ["http1","json","tokio"]`，
注释「这个仓库没有文件上传、没有查询参数协议」原样还在——**没有为这个端点开 `query`**。
`poll.rs::cursor()` 跟 `sse.rs` 那一行逐字同构（`get` → `to_str().ok()` → `parse().ok()`，
失败静默降级 `None`）；`wait_duration()` 同款，降级到 `Duration::ZERO`。

**复用 `Last-Event-ID` 的价值在这里兑现**：两条传输**同一个游标 header、同一套解析、同一
个 ring**，客户端在 SSE 与拉取之间切换时游标逻辑零改动。这不是设计文档里的一句漂亮话，
第一条测试把它钉成了断言——同一轮里 SSE 的帧 id 序列与 poll 的 id 序列、以及事件正文，
**逐一相等**。

### `next` 是最后一帧的 id，**不是 +1**

```rust
let next = frames.last().map_or(last_event_id.unwrap_or(0), |frame| frame.id);
```

两条路各有理由：

- **有帧**：`next = frames.last().id`。ring 的 `replay` 只回 `id > effective_last`
  （`ring.rs` 那句 `filter(|f| f.id > effective_last)`），所以「最后一个**已交付**帧的 id」
  正好是下次该传的游标。**加一会跳帧**——下次变成只回 `id > last + 1`，恰好漏掉 `last + 1`
  那一帧，而且不报错、不重复，只是那一帧永远不出现。
- **空批**：`next = 传进来的游标`，首拉无游标时为 `0`。`0` 不是随手挑的哨兵：`ring.rs` 的
  帧 id 从 1 起，**0 专门留给「客户端从没见过任何帧」**，所以拿 `0` 回来接着拉天然落进
  「从头补」而不是被误判成一个真实存在过的帧。

**这一点最初的设计稿写反了（写成 +1），实现是对的**。更正现在在四处都已落地：本 issue
§范围 4「因此不能再加一」、`INTEGRATION.md` §四「`next` 不能加一，否则会跳过下一帧」、
058 记录 §「`next` 不加一」、`examples/java-gateway/README.md`「without adding one」。
**「+1」这个说法在当前仓库里已经一处不剩**；`d0c60fe` 提交时本 issue 正文就已是更正后的
版本，所以更正发生在提交之前，**具体在哪一稿改的未考证**（无更早的 git 历史可查）。

钉住它的方式是「拿 `next` 再拉一次」：`poll_replays_the_same_frames_as_sse_without_duplicates`
先断言 `next == 最后一个 SSE 帧 id`（实现若 +1 了这里当场红），再用它当 `Last-Event-ID`
拉一次，要求 `frames: []` 且 `next` **原样返回**。

### 长轮询：被唤醒之后**重读 ring**，不是交付那一条

```rust
let _ = tokio::time::timeout(wait, live_rx.recv()).await;
hub.replay_frames(last_event_id)
```

只在 `initial.is_empty() && !wait.is_zero()` 时才走这条路（有帧就立刻返回；`wait` 缺省 /
0 / 垃圾值也立刻返回）。三个判断值得写下来：

1. **醒来后重读 ring，而不是把 `recv()` 到的那一帧直接当响应**。被唤醒的那一小段里完全
   可能已经连着来了好几帧（模型的 delta 就是这种形状），重读让响应始终是一个**完整、
   同源的 replay 批次**——跟短轮询、跟 SSE 走的是同一个 `replay.rs`。只交付被唤醒那一条
   也不会丢帧（`next` 仍自洽，下次能补上），但每次只回一帧，长轮询就白长了。
2. **`recv()` 的返回值被整个丢掉（`let _ =`）**，四种结局（有帧 / 超时 / `Lagged` /
   `Closed`）走同一条出路。这不是偷懒：**响应的唯一真值源是那次重读**，所以 `Lagged`
   （这条 receiver 自己落后了）正好被重读兜住，`Closed`（session 死了）也只是让它提前
   返回当前 ring 快照，而不是白挂满一个 `wait`。
3. `tokio::time::timeout` 确实是 `agent-server/src` 里**第一次也是目前唯一一次**使用
   （全 src grep 只有 `poll.rs:75` 一处；`actor/body.rs` 用的是 `recv_timeout`，`guard.rs`
   用的是 `sleep`）——issue 原稿的那句提醒到现在仍然成立。

### 等待窗口漏不了帧：靠 `replay_and_subscribe` 的同一次持锁

`poll.rs` 拿 `initial` 和 `live_rx` 用的是**同一个** `replay_and_subscribe`，也就是 SSE
`spawn_forwarder` 用的那一个：在**一把 ring 锁**里既拍快照又 `live.subscribe()`。drain
任务往 ring 追加需要同一把锁，于是「快照已读、订阅还没建」这个窗口里它不可能已经把新帧
广播出去——`hub/mod.rs` 模块文档「补发和直播的接缝为什么不会漏一帧」有完整论证，拉取式
**继承**了这份论证，没有另写一套。

**这也是长轮询正确性的全部依赖**：`await_new_frames` 里那次 `recv()` 用的 receiver 是在
持锁时刻建立的，所以「快照为空 → 开始等」这段时间里到达的帧一定进得了它。若换成先
`replay_frames` 再单独 `live.subscribe()`，两次操作之间到达的帧会**既不在快照里、也不在
订阅里**，长轮询就会白等满一个 `wait`——而且不报错，正是最难查的那类。

### 可见性：一处都没放宽

`BufferedFrame` 仍是 `pub(in crate::http)`、`RingState::replay` 仍是 `pub(super)`，新加的
`replay_and_subscribe`/`replay_frames` 也是 `pub(super)`（= 对 `crate::http` 可见）。
`poll.rs` 住在 `crate::http::routes` 下，本来就够得着，**一个 `pub` 都没有加**。唯一对外
扩散的是 `PollResponse` 那行 `pub(crate) use`，而它**挂在 `#[cfg(feature = "ts")]` 后面**
——正常 `cargo build`/`cargo test` 里这行根本不存在，纯粹是给 `ts_protocol::export` 用的。

### 协议一致性：走 032 的既有链路，没另开

`export.rs` 里多了一句 `PollResponse::export_all(&cfg)`（`PollFrame` 作为依赖被递归带出）。
032 的一致性检查是「本进程现导出一份到临时目录 → 跟 `packages/protocol/` 逐字节比」，
所以这两个新类型**自动**进了那道门禁，不需要为它们写第二套检查；`packages/protocol/src/
index.ts` 也补了两行再导出，前端拿到的是生成类型而不是手写镜像。

### 六条独测各钉什么（其中三条属 057）

| 测试 | 钉的是 |
|---|---|
| `poll_replays_the_same_frames_as_sse_without_duplicates` | **本 issue 最重要那条**：同一轮里 SSE 与 poll 的 id 序列 + 事件正文逐一相等；外加 `next` 语义与空批（拿 `next` 再拉 → `frames: []` 且 `next` 原样） |
| `poll_synthesizes_the_same_gap_envelope_as_sse` | `ring_capacity: 2` 撑爆缓冲 → 首帧 `agent: "root"` + `type: "gap"`，且 `frames.len() > 1`（**tail 没被一并放弃**，031 分歧 2）；再拿 gap 帧自己的 id 当游标拉一次 → **不再是 gap** |
| `poll_long_wait_returns_when_a_new_event_arrives` | 先拉一次把 hub 造出来（`hub_for` 是懒建的），再挂一个 `wait=1000ms` 的长轮询，80ms 后 `POST /input` → **900ms 内**返回且非空：是被唤醒的，不是等满超时的 |
| `a_completed_poll_starts_the_shared_grace_cancellation` | 057 |
| `a_waiting_poll_keeps_a_hanging_turn_alive` | 057 |
| `polling_and_sse_share_one_subscriber_count` | 057 |

`poll.rs` 自己还有一条模块内单测 `malformed_headers_degrade_to_an_immediate_first_poll`：
两个 header 都喂垃圾字符串 → `cursor == None` 且 `wait` 归零。**加上它，056 + 057 相关的
测试一共 7 条**（6 条集成 + 1 条单测）——若别处写着「九条」，那是记错了，
`grep 'async fn\|#\[test\]' crates/agent-server/tests/http_indep_poll.rs` 可查。

### 没有被直接钉住的（如实列出，不是漏写）

**这两条已于 2026-08-04 补上**（详见 [057 §补测（2026-08-04）](057-poll-disconnect.md#补测2026-08-04)），
原文保留在下面只为记录当时的缺口：

- ~~**「`wait` 到点、始终无帧 → 约 `wait` 之后返回空批」没有专门的测试**。这条路由
  `timeout` 的超时分支覆盖，跟「被唤醒」共用同一句重读，但没有任何一条断言量过它的耗时
  与空批形状。~~ → 现由
  `a_long_poll_with_no_traffic_returns_an_empty_batch_at_its_deadline` 钉住（耗时、空批、
  游标原地不动三条都断言了）。
- ~~**垃圾 `X-Poll-Wait-Ms` 的端到端行为**只由单测覆盖（解析层），没有一条集成测试真的往
  线上发一个垃圾 header。~~ → 现由
  `a_malformed_poll_wait_header_degrades_to_an_immediate_poll` 钉住（四种垃圾值真发到线上，
  200 + 立刻返回）。

于是 056 + 057 相关的测试从 7 条变成 **10 条**（9 条集成 + 1 条单测）。
