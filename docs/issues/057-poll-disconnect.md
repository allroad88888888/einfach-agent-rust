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
