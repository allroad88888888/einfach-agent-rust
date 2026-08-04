# 057 拉取式的断开检测：每次 poll 持一个 `SubscriberGuard`

**里程碑** M9 · **依赖** 056 · **模型** opus · **独测** ✅

补上拉取式唯一缺的那块：客户端跑了要能取消在飞轮次。**碰「不白烧 token」这条正确性保证**
（ARCHITECTURE §取消传播原文：「这不是运维功能，是正确性」），且是时序相关的静默失败
——宽限没生效 = 客户端早走了模型还在烧钱，功能测试全绿也看不出来。接缝见
[INTEGRATION.md](../INTEGRATION.md) §四。

## 背景：为什么拉取式缺这块

SSE 的断开是**免费可知**的：TCP 断 → hyper 丢弃响应体 `Stream` → `SubscriberGuard` drop
→ 计数归零 → 宽限倒计时 → `SessionHandle::cancel()`。031 的独测**专门为此踩过坑**：
guard 必须活在 axum 会 drop 的那个 `Stream` 对象里，而不是活在只通过 mpsc 弱关联的后台
任务里，否则「上游挂住 + 客户端断开」的组合下永远发现不了。

拉取式没有这个信号——「客户端跑路」和「它只是还没来拉下一次」在服务端看来一模一样。

## 范围

**方案已定：整套复用 `hub/guard.rs`，零新取消逻辑。** 照做，别自己发明时间戳方案。

1. **poll handler 全程持有一个 `SubscriberGuard`**：请求进来 `SubscriberGuard::attach(hub)`，
   响应发出即 drop。现有语义恰好就是要的：
   - `attach`：`subscribers += 1` + **`task.abort()` 掉在飞的倒计时**（「是不是重连」不需要
     任何判断，任何新连接天然满足——这是现有实现的写法，别加判断）
   - `drop`：`subscribers -= 1`；**归零才**起 `sleep(grace)` → 到点**二次确认** `== 0`
     → `handle.cancel()`
2. **长轮询期间 guard 必须一直在**（等待窗口内计数非零），否则挂住的那 25s 会被误判成断开。
3. **确认 SSE 与拉取式共用同一个计数器**：同一 session 上一个 SSE 观众 + 一个拉取网关，
   走掉一个**不会**误杀另一个。这是复用而非另起炉灶的红利，要有测试钉住。

## 验收（可判定，全部要真时序不要 sleep 猜）

- **短轮询超时取消**：poll 一次（`wait=0`）→ 之后不再拉 → 宽限（测试里调小，照
  `http_disconnecting_all_subscribers_cancels_after_grace` 用 `GRACE=200ms` +
  `PROVIDER_NEVER_TIMES_OUT=60s` 的先例）到点 → **在飞轮次被取消**。
  断言取消来自**宽限计时器**而非 provider 自然超时（那条测试就是这么设计的，照抄它的构造）。
- **宽限内再拉不取消**：poll → 宽限内再 poll → **倒计时被 abort，轮次没被取消**，
  且第二次 poll 正常拿到帧。
- **长轮询期间不误杀**：`X-Poll-Wait-Ms` 远大于宽限（如 wait=2s、grace=200ms）→ 挂住期间
  **不触发取消**（这条是本 issue 最容易写错的地方：guard 若只在响应发出时才 attach，
  这条立刻红）。
- **混合订阅者**：一个 SSE 连着 + 一个 poll 走掉 → **不取消**（计数还有 SSE 那一个）；
  再把 SSE 也断开 → 宽限后取消。
- 既有 SSE 的宽限测试全绿不回归（`http_disconnecting_all_subscribers_cancels_after_grace`、
  `http_indep_grace_cancel`、`guard.rs` 的三条单测）。

## 注意

- **这是本里程碑唯一的静默失败点**：宽限没生效不会报错、不会测红，只会在账单上出现。
  所以**派独立测试 agent**，且必须有「客户端不再拉 → 在飞轮次真的被取消」这条断言。
- **别改宽限的默认值**（`DEFAULT_CANCEL_GRACE = 5s`）——它同时管着 SSE，动它是另一回事。
- **网关侧的约束写进 058 的文档**：短轮询时轮询间隔必须 < 宽限，否则会被判成断开。
  **推荐网关一律长轮询**（wait 20–25s），guard 全程持有，既无这个约束又少空转请求。
- **不要**新增「last-poll 时间戳」之类的并行状态——那是第二真值源，跟 guard 的计数会不一致。
- 收工验证前台跑完（WORKFLOW §四 -1），别后台自旋。

---

## 实做记录（完成 · 2026-08-04 · 由并发线实现，主会话回填）

跟 056 同一个提交（`d0c60fe`，2026-08-04）落地——**没有任何 diff 能把 057 的增量单独切
出来**：`routes/poll.rs` 第一次出现在仓库里时，那行 guard 就已经在里面了。所以下面按
**职责**归属，不是按提交归属。这份记录是主会话读落地代码补的，读不出来的地方标了
「未考证」。

### 落地形状：一行 attach，生产代码没有别的

`crates/agent-server/src/http/routes/poll.rs`：

```rust
// 从请求进入到响应交还给 axum 都持有 guard。特别是长轮询的 await 期间，
// 不能因为没有 SSE 连接就误触发取消宽限。
let _subscriber = SubscriberGuard::attach(Arc::clone(&hub));
```

`hub/guard.rs`：**一行没改**（`git diff 0f7c003 d0c60fe` 里没有它）。方案原文说「整套复用、
零新取消逻辑」，落地兑现得比字面还干净——`agent-server/src` 里没有出现任何「last-poll
时间戳」「poll 专用超时」之类的并行状态，第二真值源一个都没建。

三个细节值得写下来：

1. **绑定成 `_subscriber` 而不是 `_`**。`let _ = SubscriberGuard::attach(..)` 会**当场**
   drop，等于「attach 完立刻断开」，长轮询挂住的那 25s 会被判成没人；`_subscriber` 这个
   名字让它活到函数返回。**差一个下划线就是静默失败，而且功能测试全绿**——
   `a_waiting_poll_keeps_a_hanging_turn_alive` 守的就是这一个字符。
2. **attach 排在 `replay_and_subscribe` 之前**。于是倒计时的 abort 发生在任何等待之前：
   哪怕这次 poll 空手而归、要挂满 `wait`，在飞的倒计时也已经在请求进门的一瞬间停掉了。
3. **drop 点是「响应交还给 axum」**，不是「客户端收到」。短轮询因此带一条约束：网关的
   轮询间隔必须小于宽限——这条已经写进 058 的网关文档与 `INTEGRATION.md` §四（**推荐
   一律长轮询**）。

### 为什么「零新逻辑」真的够：三种情形逐一对上现有语义

| 情形 | 现有 guard 的动作 | 结果 |
|---|---|---|
| 长轮询挂住 | 计数在 attach 时 `+1`，整个 `await` 期间不归零 | 不误杀 |
| 宽限内又来拉 | `attach` 里那句 `task.abort()` | 倒计时停掉；**「是不是重连」不需要任何判断**，任何新连接天然满足 |
| 客户端跑路 | 没有下一次 attach，倒计时到点二次确认 `== 0` → `canceller.cancel()` | 走跟 SSE 断开**完全同一条**取消路 |

第二行是这套复用最值钱的地方。拉取式没有「连接」这个概念，最容易在这里发明一个「上次
poll 的时间戳 + 判断是不是同一个客户端」的新机制；而 `attach` 的写法把这个问题**消解**
掉了——它压根不问「是不是重连」，只要有人来就不该继续倒数。

### SSE 与拉取共用同一个计数器：红利在哪

计数器是 `SseHub.subscribers`（`AtomicUsize`，**一个 session 一个 hub 一个计数**）。两条
传输拿的是同一个 `Arc<SseHub>`（`AppState::hub_for` 按 session id 懒建、复用），attach /
drop 走的是同一段代码，于是：

- 一个浏览器 SSE + 一个网关短轮询，网关那次 poll 结束**不会**把浏览器正在看的轮次干掉；
- 两边都走了，倒计时照样起——不需要「谁是最后一个」的额外判断，`fetch_sub` 的返回值
  （`previous != 1`）就是。

**这是复用而非另起炉灶的直接红利**：拉取式若自己记一份计数，上面两句都要重写一遍，而且
两份计数不一致时的症状是「浏览器还开着，轮次被网关的一次 poll 干掉了」——用户侧看起来
像模型自己停了。

### 三条属于 057 的测试

| 测试 | 钉的是 |
|---|---|
| `a_completed_poll_starts_the_shared_grace_cancellation` | **短轮询走掉 → 宽限到点取消**：`Script::Hang` 的上游 + `grace = 150ms`；poll 一次（建 hub、attach 又 drop）→ `POST /input` 起一轮挂住的轮次 → 等 `grace * 4` 再拉 → 帧里出现 `Cancelled` |
| `a_waiting_poll_keeps_a_hanging_turn_alive` | **长轮询期间不误杀**：`wait = 1000ms` 远大于 `grace = 150ms`，等 `grace * 3` 后断言 `!waiting.is_finished()`（还挂着 = 没被取消）；再 `POST /cancel` 显式取消，长轮询**被唤醒**返回带 `Cancelled` 的批次 |
| `polling_and_sse_share_one_subscriber_count` | **混合订阅者**：SSE 全程连着 + 一次 poll 走掉 → 等 `grace * 4` 仍**没有** `Cancelled`；再 `drop(sse)` → 等 `grace * 4` → `Cancelled` 出现 |

**「取消来自宽限计时器而非 provider 自然超时」怎么被区分开的**——落地用了两道，比 issue
原稿要求的一道更强：

- **内容**：断言的是响应里出现 `Cancelled`。取消的终态是
  `TurnStatus::Failed(Failure::Cancelled)` → `{"Failed":"Cancelled"}`，而 provider 超时是
  `{"Failed":{"Provider":…}}`——**根本不含 `Cancelled` 这个词**，混不了。
- **时序**：独测 harness 的 `provider_timeout` 是 5s，断言发生在 `grace * 4 = 600ms`。

**与 issue 原稿的一处出入**：原稿点名照抄 `http_disconnecting_all_subscribers_cancels_after_grace`
的 `GRACE=200ms + PROVIDER_NEVER_TIMES_OUT=60s` 构造，落地实际走的是
`http_indep_grace_cancel.rs` 那一路——直接用独测共享 harness（`provider_timeout` 5s）配
`grace = 150ms`。**这个选择更省事，而且仍然判得开**（600ms vs 5s，再加上面那条内容判据），
只是时序余量从 300 倍缩到 8 倍；将来 CI 机器抖得厉害的话，这三条是首先要看的。

### 混合订阅者那条：变异验证说明共享计数由**两道**防线护着

`polling_and_sse_share_one_subscriber_count` 是主会话后补的（并发线交付时没有这条）。补的
时候做过变异验证，结论值得留下来：

- 只破坏 `Drop` 里那句 `if previous != 1 { return; }`（改成任何 drop 都起倒计时）→
  **不红**。因为倒计时到点还有第二道：`if hub.subscribers.load() == 0`，而此刻 SSE 还占
  着 1，取消不会发生。
- 只破坏到点那句 `== 0` 二次确认 → **也不红**。因为第一道根本没让倒计时起来。
- **两道一起破才红。**

所以「共享计数」这件事**由两道彼此独立的防线保证**，任何一道单独失守都不会有任何测试
变红。这既是这条测试的价值（没有它，共享计数退化成两份独立计数时**其余每一条测试都照样
绿**——测试自己的文档注释里写了这句），也是给后来人的一条警告：**改 `guard.rs` 的这两行
里的任何一行，现有测试都不会拦你**。

（变异验证是补这条测试当时跑的；本次回填是纯读代码，**没有重跑**——但上面两条推理都能
直接从 `hub/guard.rs` 那十几行里复核出来。）

### 没有被直接钉住的（如实列出）

- ~~**「宽限内再拉一次 → 倒计时被 abort、轮次没被取消」在 HTTP 拉取这一层没有专门的测试**。
  这条验收目前由两处间接覆盖：`guard.rs` 的单测 `reconnecting_within_the_grace_period_avoids_the_cancel`
  （机制层）、`http_indep_grace_cancel.rs::reconnecting_within_grace_keeps_the_turn_alive`
  （SSE 那一层）。poll 走的就是同一个 `attach`，机制上覆盖到了，但**没有一条测试真的
  「poll → 等半个宽限 → 再 poll → 断言没取消」**。~~
  **2026-08-04 补上**：`re_polling_within_the_grace_period_aborts_the_countdown`，
  见下面 §补测（2026-08-04）。
- 「既有 SSE 宽限测试全绿不回归」这一条，**回填当时没有重跑**（有并发线在跑构建，跑
  cargo 会抢锁）：`d0c60fe` 落地时通过了验收。**2026-08-04 的补测顺手复验了**——
  `http_indep_grace_cancel`（2 条）、`http_disconnecting_all_subscribers_cancels_after_grace`
  （2 条）、`guard.rs` 的 3 条单测全绿，输出见下节。

---

## 补测（2026-08-04）

主会话补的三条集成测试，全部落在 `crates/agent-server/tests/http_indep_poll.rs`（**只改了
这一个文件**，271 行，红线 9 通过）。补的就是 056 / 057 两份记录末尾「没有被直接钉住的」
里列的三条。

| 新测试 | 钉的是 | 原缺口 |
|---|---|---|
| `re_polling_within_the_grace_period_aborts_the_countdown` | **宽限内再拉 → 倒计时被 abort、轮次没被取消**（拉取这一层） | 057 第一条 |
| `a_long_poll_with_no_traffic_returns_an_empty_batch_at_its_deadline` | `wait` 到点、期间始终无帧 → 约 `wait` 后返回 `frames: []`，`next` 原地回显游标 | 056 第一条 |
| `a_malformed_poll_wait_header_degrades_to_an_immediate_poll` | 垃圾 `X-Poll-Wait-Ms` 的**端到端**降级：真发 `"not-a-number"` / `"-1"` / `"1.5"` / `""` 上线，仍是 200 且立刻返回 | 056 第二条 |

### 第一条为什么不能写成「poll 一次 → 长轮询挂住」

**最初就是这么写的，突变验证不红。** 原因值得记下来，它是上一节「两道防线」那段发现的
直接续集：

长轮询挂着的时候订阅计数是 **1**。旧倒计时（第一次 poll 的 guard drop 起的那一轮）到点
时撞上 `Drop` 里那条二次确认 `if hub.subscribers.load() == 0`——**这第二道防线自己就拦住了
取消**。于是 `attach` 里那句 `task.abort()` 注释掉之后，轮次照样活着，测试照样绿。
换句话说：**「长轮询期间不误杀」那条路径根本不需要 abort**，它由二次确认单独兜住
（`a_waiting_poll_keeps_a_hanging_turn_alive` 也因此对这个突变免疫）。

真正只有 abort 能兜住的，是**网关按比宽限更快的节奏短轮询**：旧倒计时是在两次 poll 的
**间隙**到点的，那一刻计数确实是 0，二次确认拦不住，只有「有人回来了 → 把在飞的倒计时
掐掉」能拦。所以最终形状是：`grace = 300ms`，每 `0.4 * grace` 短轮询一次、连拉 6 次
（共 2.4 个宽限），每次都断言帧里没有 `Cancelled`。轮询节奏刻意不取宽限的整数分之一，
好让「没被 abort 的倒计时」的到点时刻落在两次 poll 的正中间，判据不靠时序运气。

这也补上了上一节那句警告的一个缺口：`guard.rs` 的 `attach` 那行现在**有测试拦你了**
（`Drop` 里那两行仍然是各自单独破坏都不红）。

### 突变验证（真实输出）

把 `SubscriberGuard::attach` 里的 `task.abort()` 注释掉：

```
running 12 tests
...
test re_polling_within_the_grace_period_aborts_the_countdown ... FAILED
...
test result: FAILED. 11 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.24s

---- re_polling_within_the_grace_period_aborts_the_countdown stdout ----
thread 're_polling_within_the_grace_period_aborts_the_countdown' panicked at
crates/agent-server/tests/http_indep_poll.rs:230:9:
宽限内又来拉了，倒计时该被 abort：{"frames":[... {"TurnStatusChanged":{"status":{"Failed":"Cancelled"}}} ...],"next":10}
```

**只有新增这一条红**——其余 11 条（含 `a_waiting_poll_keeps_a_hanging_turn_alive` 和
`polling_and_sse_share_one_subscriber_count`）在突变下全绿，这正是上面那段分析的实证。

改回 `task.abort()` 之后：

```
running 12 tests
test a_malformed_poll_wait_header_degrades_to_an_immediate_poll ... ok
test poll_replays_the_same_frames_as_sse_without_duplicates ... ok
test poll_synthesizes_the_same_gap_envelope_as_sse ... ok
test poll_long_wait_returns_when_a_new_event_arrives ... ok
test a_long_poll_with_no_traffic_returns_an_empty_batch_at_its_deadline ... ok
test a_waiting_poll_keeps_a_hanging_turn_alive ... ok
test a_completed_poll_starts_the_shared_grace_cancellation ... ok
test re_polling_within_the_grace_period_aborts_the_countdown ... ok
test polling_and_sse_share_one_subscriber_count ... ok
（+ 夹具自带的 3 条 chunked 单测）

test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.24s
```

连跑 5 次全绿（`1.23~1.24s`），不抖。`guard.rs` 已按字节改回（`shasum` 与突变前的备份一致，
`grep -rn MUTATION crates/` 无残留）。

### 不回归

- `cargo test -p agent-server --lib http::hub` → 12 passed（含 `guard.rs` 三条单测）
- `cargo test -p agent-server --test http_indep_grace_cancel` → 5 passed
- `cargo test -p agent-server --test http_disconnecting_all_subscribers_cancels_after_grace` → 2 passed
