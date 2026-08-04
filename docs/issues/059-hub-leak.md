# 059 hub 表永不回收：drain 任务自持有导致 `remove` 执行不到

**里程碑** M9（**排最前**，先于 055） · **依赖** — · **模型** opus · **独测** ✅

勘查传输面时静态分析捞到的一条既存缺陷，跟 M9 无依赖关系但**被 chatid 方案放大**。
接缝上下文见 [INTEGRATION.md](../INTEGRATION.md) §四末尾。

## 现象（静态分析，**尚未实测**）

`SseHub::spawn`（`agent-server/src/http/hub/mod.rs`）里：

```rust
let hub = Arc::new(SseHub { handle: handle.clone(), .. });   // hub 自持有 SessionHandle
let mut sub = handle.subscribe();
let drain_hub = Arc::clone(&hub);                            // drain 任务持有 Arc<SseHub>
tokio::spawn(async move {
    while let Some(envelope) = sub.recv().await { .. }
    hubs.lock().unwrap().remove(&id);                        // ← 全 crate 唯一的 hub 清理点
});
```

引用链成环：

```
drain task ──> Arc<SseHub> ──> SessionHandle ──> events: broadcast::Sender<Frame>
     ▲                                                        │
     └──── 等 sub.recv() 返回 None（需要所有 Sender 都 drop）◀─┘
```

`Subscription::recv` 返回 `None` 的**唯一**条件是 `RecvError::Closed`，即所有
`broadcast::Sender<Frame>` 被 drop。而 drain 任务自己持有的那条链上就有一个 Sender 克隆
——**它在等的条件被它自己拿着的东西挡住**。于是 session actor 线程退出后 drain 任务照样
不退出，`remove(&id)` 永远执行不到。

**后果**：每个死会话泄漏一个 `SseHub`（256 帧 ring + broadcast channel）+ 一个永久挂起的
tokio 任务；`AppState.hubs` 这张表只增不减。

**为什么现在要紧**：单副本 + 会话数有限时只是慢性累积；[055](055-chatid-session.md) 之后
**每个业务 chat 都是一个 session**，量级完全不同。

## 范围

1. **先写一条会红的测试**（这一步不许跳）：起 session → 关掉它（既有的 close/shutdown 路）
   → 断言 `AppState` 的 hub 表里那一项**消失**。现有 `tests/` 下没有任何一条断言 hub 被摘掉，
   所以这条测试本身就是净增的护栏。**跑起来确认它真的红**，再动手改。
2. **断掉自持有**。方向由 opus 判，两条候选（**不要两条都做**）：
   - **hub 不持有 `SessionHandle` 的强引用**——改持弱引用，或只存 drain 真正需要的那部分
     （注意 `SseHub.handle` 还被 `guard.rs` 的 `handle.cancel()` 用着，不能简单删）。
   - **drain 任务不持有 `Arc<SseHub>` 强引用**——改 `Weak<SseHub>`，每次 upgrade；upgrade
     失败即所有外部引用已走，退出并清表。
3. **确认 `guard.rs` 的取消路不受影响**：宽限倒计时里的 `hub.handle.cancel()` 必须仍然可达
   （改成 Weak 的话，倒计时任务里 upgrade 失败要能安全跳过而不是 panic）。

## 验收（可判定）

- 步骤 1 那条测试：修之前**红**，修之后**绿**（在 issue 记录里如实写出「先红后绿」，不是
  只贴一个绿）。
- 关掉 N 个 session 之后 hub 表长度回到 0（不是「变小」，是精确归零）。
- 既有 SSE 全套测试不回归：`http_indep_frame_format` / `http_reconnect_*` /
  `http_two_sse_clients_are_independent` / `http_disconnecting_all_subscribers_cancels_after_grace`
  / `http_indep_grace_cancel` 全绿——**尤其宽限取消那两条**（改引用关系最容易伤到它们）。
- 没有引入新的 `unwrap` panic 路径：Weak upgrade 失败是正常终止路径，不是 panic。

## 注意

- **这是静态分析的结论，没有实测**。所以范围第 1 步是「先让它红」——万一它其实不泄漏
  （比如某处还有别的 drop 触发），测试会立刻告诉你，那就把这个 issue 关掉并把结论如实
  写进记录（**「查明不存在」也是有效产出**，别为了交差硬改）。
- **红线 3 邻近**：hub / handle / channel 都是**活句柄**，本来就住 store 外。本 issue 只动
  它们之间的引用关系，不把任何句柄塞进 store。
- **不要顺手改 hub 的其它行为**（ring 容量、宽限值、forward 语义）——那些都有独测钉着，
  本 issue 只解决「死了要能回收」。
- 收工验证前台跑完（WORKFLOW §四 -1），别后台自旋。

---

## 实做记录（2026-08-04）

### 结论：真的漏，静态分析没冤枉它

第一步没跳，先写会红的测试：`crates/agent-server/src/http/state.rs` 的模块内独测
`closing_every_session_empties_the_hub_table`（hub 表就住在这个文件里）——开三个
session、各自 `hub_for`（跟 `POST /sessions` 同一条路：造完 hub 就把那份 `Arc` 丢掉）、
再逐个 `SessionRegistry::close`（`SessionsHandle::close_all` 和
`tests/close_then_reopen_recovers.rs` 走的都是它），然后**最长等五秒**看表空没空。

**修之前（红）**：

```
running 1 test
test http::state::tests::closing_every_session_empties_the_hub_table ... FAILED

---- http::state::tests::closing_every_session_empties_the_hub_table stdout ----
thread '...' panicked at crates/agent-server/src/http/state.rs:185:9:
assertion `left == right` failed: 全部 close 之后 hub 表该精确归零（issue 059）
  left: [SessionId("sess-3481-0"), SessionId("sess-3481-1"), SessionId("sess-3481-2")]
 right: []

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 40 filtered out; finished in 5.02s
```

三个 close 完的 session，五秒后一个都没被摘掉——**不是「变慢了」，是根本不回收**。

**修之后（绿）**：

```
running 1 test
test http::state::tests::closing_every_session_empties_the_hub_table ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 40 filtered out; finished in 0.03s
```

`5.02s`（等满超时才失败）→ `0.03s`（等待循环第一次检查就已经空了）。表长度**精确归零**，
不是「变小」。

### 选了哪条修法：hub 只留 `CancelHandle`（候选 1），并且候选 2 单独做**根本修不好**

候选 2（drain 改持 `Weak<SseHub>`）看起来对称，其实断不掉那条边：**hub 表自己攥着一个强
`Arc<SseHub>`**，drain 每次 upgrade 都会成功 → hub 活着 → 它存的 `SessionHandle` 活着 →
`events` 发送端活着 → `sub.recv()` 永远不返回 `None` → drain 永远走不到那句 `remove` →
表项永远不摘 → 强 `Arc` 永远在。自锁死一圈原样回来，只是换了个说法。要让 drain 等得到
`None`，唯一的办法是**这条引用链上不再有任何 `broadcast::Sender<Frame>`**。

于是走候选 1 的「只存 drain/guard 真正需要的那部分」：`SessionHandle` 拆出一个
`CancelHandle`（`mpsc::Sender<Command>` + `Arc<AtomicBool>` 取消标志，**不含 `events`**），
`SseHub` 存它而不是整个 `SessionHandle`；`SseHub::spawn` 收下的那份 `SessionHandle` 在
`subscribe()` 之后当场走出作用域被 drop。session 一死（`close` 发 `Shutdown`，或者 actor
panic），actor 线程退出、最后一个 `Sender` 落地，drain 立刻收到 `None`、摘表、退出。

**顺带的好处**：全程没有 `Weak`，也就没有「upgrade 失败」这条新分支要处理——验收里
「没有新增 `unwrap` panic 路径」不是靠小心翼翼绕开，是压根不存在这条路径。

### 取消路怎么确认没伤到

- `CancelHandle::cancel()` 就是原先 `SessionHandle::send(Command::Cancel)` 的那两步（先翻
  共享标志打断在飞的 provider 调用，再入队唤醒消息），而且现在**只有这一份实现**：
  `SessionHandle::send` 遇到 `Command::Cancel` 直接转调它，不存在两份会各自漂移的取消协议。
- `guard.rs` 宽限到点那行只从 `hub.handle.cancel()` 变成 `hub.canceller.cancel()`，逻辑一
  步没改；三条 guard 单测（提前不取消 / 宽限期内重连不取消 / 还有另一个订阅者时压根不起
  计时）逐条跑过，见下面验证输出。
- 端到端那两条宽限取消（`http_disconnecting_all_subscribers_cancels_after_grace` 的两个用例、
  `http_indep_grace_cancel` 的两个用例）也逐条跑过、全绿。

### 诚实标注：hub 仍持有一个 `mpsc::Sender<Command>`

取消要靠它唤醒「正等 Web 工具回传」的空闲 actor，不能删（删了就是改取消语义，超范围）。
它的影响面跟 059 之前**完全一样**（那时 hub 存的整个 `SessionHandle` 里就含这个 `tx`）：
只要 `close`/`Shutdown` 或 actor panic 二者之一发生，actor 线程就退出、`events` 端跟着落地，
drain 照常回收；只有「谁都不 close、直接把整个 `AppState` 丢掉」那条路上，actor 线程要靠
命令队列关闭端来收尾，这时 hub 的这份 `tx` 克隆仍会拖住它。**本 issue 不动这条既有取舍**，
在此记一笔备查。

### 改动文件（行数为改后，红线 9 全部 ≤300）

| 文件 | 改了什么 | 行数 |
|---|---|---|
| `crates/agent-server/src/handle.rs` | 新增 `CancelHandle`（含 `cancel` 的唯一实现 + `#[cfg(test)]` 的 `is_cancelled`）；`SessionHandle` 改为持有它；模块文档补「为什么『能取消』被单拎出来」 | 221 |
| `crates/agent-server/src/http/hub/mod.rs` | `SseHub.handle: SessionHandle` → `canceller: CancelHandle`；`spawn` 先 `subscribe` 再造 hub；模块文档新增一节「hub 为什么只留 `CancelHandle`」（含引用环图与「别再塞回去」的警告） | 235 |
| `crates/agent-server/src/http/hub/guard.rs` | 取消调用换成 `hub.canceller.cancel()`；三条单测断言换成 `is_cancelled()`；`fake_hub` 文档改写（现在 drain 会当场退出，说明为什么对这三条断言无害） | 113 |
| `crates/agent-server/src/http/state.rs` | **新增本 issue 那条独测**（护栏，`tests/` 下此前没有任何一条断言 hub 被摘掉） | 187 |
| `crates/agent-server/src/actor/mod.rs` | `SessionHandle` 构造处跟着改一行 | 177 |

观察口没有新增任何公开 API：那条测试住在 crate 内部（`http::state` 的子模块），直接读
`AppState` 私有的 hub 表——比为测试开一个 `pub` 更小。

### 收工验证（前台跑完，真实输出）

```
### TEST: cargo test -p agent-server ###
    Finished `test` profile [unoptimized + debuginfo] target(s) in 7.84s
test result: ok. 42 passed; ...   ← lib（含新增那条 + guard 三条）
（其余 35 个测试二进制逐条 test result 全部 ok，0 failed；合计 0 个 failed）

### 逐条点名：宽限取消 + SSE 全套（`--offline` 跑，避开并发会话的 cargo 锁）###
cargo test -p agent-server --lib -- http::hub
test http::hub::guard::tests::last_subscriber_leaving_cancels_after_the_grace_period ... ok
test http::hub::guard::tests::reconnecting_within_the_grace_period_avoids_the_cancel ... ok
test http::hub::guard::tests::a_second_concurrent_subscriber_prevents_the_countdown_from_even_starting ... ok
test http::hub::ring::tests::（六条，含 gap/eviction/backlog）... ok
test result: ok. 12 passed; 0 failed; 30 filtered out

cargo test -p agent-server --test http_disconnecting_all_subscribers_cancels_after_grace \
  --test http_indep_grace_cancel --test http_indep_frame_format \
  --test http_reconnect_with_last_event_id_replays_missed_frames \
  --test http_reconnect_past_buffer_gets_a_gap_frame --test http_two_sse_clients_are_independent
test disconnecting_the_only_subscriber_cancels_the_flying_turn_after_the_grace_period ... ok
test reconnecting_within_the_grace_period_keeps_the_turn_alive ... ok        （2 passed）
test disconnecting_all_subscribers_cancels_the_in_flight_turn_after_grace ... ok
test reconnecting_within_grace_keeps_the_turn_alive ... ok                   （5 passed）
test text_delta_data_is_a_plain_string_and_frame_ids_are_monotonic ... ok    （5 passed）
test reconnecting_with_last_event_id_replays_exactly_the_missed_frames_byte_for_byte ... ok
test a_last_event_id_older_than_the_ring_buffer_yields_an_explicit_gap_frame ... ok
test two_sse_clients_of_the_same_session_see_the_same_sequence ... ok
test one_client_disconnecting_does_not_affect_the_other ... ok
（六个二进制全部 test result: ok，0 failed）

### CLIPPY: cargo clippy -p agent-server --all-targets -- -D warnings ###
    Checking agent-server v0.1.0 (/Volumes/work/self/einfach-agent-rust/crates/agent-server)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 13.35s
（0 warning。第一次跑 test 时 `is_cancelled` 报了 dead_code——它只被 cfg(test) 用到，
 补 `#[cfg(test)]` 后这道门一次过；不是靠 allow 压下去的）

### INVARIANTS: bash scripts/check-invariants.sh --all ###
红线检查通过
规则与理由：docs/INVARIANTS.md
```

**过程如实记**：本仓当时有并行会话在跑 `cargo test -p agent-runtime`/`-p agent-server-bin`，
共享 `target/` 的构建锁被长期占着，前台 `cargo test` 两次干等满 600s 一行输出都没有。
**没有转成后台自旋**——改成给本 issue 单开一个 `CARGO_TARGET_DIR`（scratchpad 下），前台
冷编译 71s、跑完全套，逐条点名那两次再加 `--offline` 连包缓存锁也绕开，全部前台拿到真实
输出。收工 `ps` 确认没留下属于本 issue 的 cargo/rustc 孤儿进程。

**同一文件的并发**：验证期间另一个会话（056 拉取式端点）把 `hub/mod.rs` 的补发逻辑拆去了
`hub/replay.rs`。上面的输出是在**合并后的树**上跑的，两边改动互不重叠（它动补发/转发，本
issue 动引用关系），全绿。
