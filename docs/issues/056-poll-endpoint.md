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
