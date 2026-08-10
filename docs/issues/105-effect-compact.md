# 105 第 3 档：`Effect::Compact` 变体与 epoch 契约

**里程碑** M12 · **依赖** [104](104-boundary-command.md) · **模型** opus · **独立测试 agent** 是 · **状态** 完成

## 目标

给 `Effect` 加第五个变体。摘要要调模型，那是 IO，core 只能**描述**这件事该发生。

零网络：本 issue 用 mock 走通，真的去生成摘要是 [106](106-summary-via-subagent.md)。

## 为什么现在才加

`engine/effect.rs:16` 那段说得很清楚——M1 刻意只定四个变体，
`SpawnChild`/`Compact`/`Persist` 连空壳都不留，因为「**空壳变体比不定更糟，
它看起来像做完了**」（021 的教训）。

当时列的三个推迟理由：没有 store、没有 `Entry`、阈值没定。前两个 M2 就解决了，
第三个由 [096](096-compaction-trigger.md) 结掉。**三个都不成立了，可以定形状了。**

同时把那张「推迟的」表里 `Compact` 那一行改掉——别留一句已经过期的话在最显眼的地方。

## 做什么

加变体。字段照 `effect.rs` 模块文档的两条线：

1. **不许出现不可序列化的活对象**——带的是快照，不是 `JoinHandle`、不是
   `oneshot::Sender`
2. **必须带 `Epoch`**（红线 6），宿主原样带回结果事件里

要摘要的是哪一段，由边界表达（104 已经有了），effect 里别再抄一份历史——
`CallProvider` 连 payload 都没有（决策 15），`Compact` 更没有理由胖。
**effect 变胖是接缝错位的第一个症状**（`effect.rs:46`）。

## 定死的接口（2026-08-10 主会话定）

三个新增，都在 `crates/agent-core/src/engine/` 下（跟 102 不碰同一批文件）：

```rust
// engine/effect.rs —— Effect 的第五个变体
/// 该去把 `[0, upto)` 这段历史摘要了。
///
/// **不带历史正文**（决策 15 的精神，`CallProvider` 连 payload 都没有）：
/// 要摘要哪一段由 `upto` 表达，正文宿主自己从状态取。effect 变胖是接缝错位的
/// 第一个症状。
Compact {
    agent: AgentId,
    /// 完整历史的前 `upto` 条要被这次摘要盖住。
    upto: usize,
    epoch: Epoch,
},

// engine/event.rs —— 两个新增事件
/// 摘要回来了。
CompactDone {
    agent: AgentId,
    summary: Arc<str>,
    epoch: Epoch,
},
/// 摘要没做成。**这是正常事件不是异常路径**（106 验收）：压缩这一次作废，
/// 边界不动，下一轮照常跑。
CompactFailed {
    agent: AgentId,
    epoch: Epoch,
},
```

### 本条的边界

只定形状 + epoch 契约 + 用 mock 走通。**不做**：真的生成摘要（106）、
把结果写回状态（107）、摘要正文存哪（107）。

`CompactDone` 带 `Arc<str>` 正文是刻意的——它是**进来的事件**不是 primitive，
不受红线 3「必须可序列化」之外的约束；正文往哪存是 107 的判断。

### epoch 契约

带旧 epoch 回来的 `CompactDone` / `CompactFailed` **一律丢弃**：不写状态、
不报错、不重试（红线 6）。这一条在本 issue 就要能被断言，
不能等 107。

## 验收

- 五个变体全部 serde 往返（`roundtrip_all_variants` 那个测试加一条）
- `Compact` 带 `epoch`
- 序列化出来的字段**不含任何历史正文**——跟 `call_provider_carries_no_payload`
  同款的 key 白名单断言
- `CancelInFlight { epoch }` 之后，带旧 epoch 回来的摘要结果**被丢弃**，状态不变
- 全程零网络（mock 拦下这个 effect 直接回结果，照 005 的 `MockProvider` 做法）

## 注意

- **红线 3**——effect 要能跨线程、进日志、进快照
- **红线 6**（在飞 effect 带 epoch，回写前校验）——本 issue 定契约，
  真正的回写校验在 [107](107-summary-writeback.md)。两条都碰红线 6，都派独立测试 agent
- 顺手更新 `agent-runtime/src/lib.rs:31` 那句「`Effect` 现在只有四个变体」，
  它会因为本 issue 过期

## 实做记录（实现 agent + 独立测试 agent 并行，2026-08-10）

与 [102](102-clear-tool-results-policy.md) 同时开工，文件不重叠。

### 落地的内容

- `Effect::Compact { agent, upto, epoch }` —— 第五个变体
- `Event::CompactDone { agent, summary, epoch }` / `Event::CompactFailed { agent, epoch }`
- `Notice::CompactionSummaryReceived` / `Notice::CompactionFailed` —— 见下面的裁决
- `command/transitions/mod.rs`：两个事件**不进那 35 格转移表**，
  匹配 epoch 时各发一条 `Effect::Emit(Notice)`，**状态一个字节不写**（回写是 107）
- 独测 `agent-core/tests/it/effect_compact_{serde,epoch_gate}.rs`（129 / 169 行，9 条）

### ⚠️ 主会话中途改了一次接口：让「接受」可观测

独测先写出来的反向锁 `epoch_matched_compact_done_is_not_silently_dropped`
**一开始是红的**，而且**不是实现写错了，是接口定错了**。

原始设计里：匹配 epoch 的 `CompactDone` 什么都不产出（回写划给 107），
过期 epoch 的也什么都不产出。独测 agent 用 `observe()` 逐字段比对确认——
两种 epoch 下整个 `Session` **完全一样**，effects 也一样。

**这个不可区分本身就是问题**：epoch 闸是个过滤器，两种结果都可观测才测得出来。
那个形状下，一个「不管 epoch 一律丢」的实现和正确实现**在任何外部通道上都相同**
——正是红线 6 要防的静默失败，而反向锁的存在意义就是拦它。

**裁决：匹配 epoch 时发 `Notice`，过期时什么都不发；状态回写仍归 107。**

两点支持这个裁决：

1. 它跟仓库既有惯例一致——102 的独测 agent 独立发现，本仓六个 epoch 相关测试
   （`session_epoch_gate.rs` 等）里，事件真的过闸后落在没有专门处理的组合会产出
   `Notice::ProtocolViolation`（非空），**「空 vs 非空」本来就是这个仓库区分
   「被挡住」和「过了闸」的既有签名**。
2. [109](109-compaction-visibility.md)（被摘要盖住的段要能看见）本来就需要这条
   Notice，不算多做。

`CompactionSummaryReceived` 与 `CompactionFailed` **分两个变体不共用一个带布尔的**
——摘要失败是正常事件不是异常路径，用户该看得见是哪一种。

### 变异检验（主会话做）

把两个摘要事件从 `step.rs` 的 epoch 闸里放行（迟到的摘要照写不误）：

```
stale_epoch_compact_done_after_cancel_in_flight_is_dropped     FAILED
stale_epoch_compact_failed_after_cancel_in_flight_is_dropped   FAILED
epoch_matched_compact_done_is_not_silently_dropped             FAILED
```

三条全红（第三条也红是因为它先钉过期基线再比对，属正常耦合）。闸是实的。

### 顺带更新的两处过期文字

- `engine/effect.rs` 模块文档那张「M1 只定四个变体」的表：`Compact` 那一行列的
  三个推迟理由（没 store、没 `Entry`、阈值没定）**现在三个都不成立了**
- `agent-runtime/src/lib.rs` 那句「`Effect` 现在只有四个变体」

### 命令输出

```
$ cargo test --workspace                                    1680 passed; 0 failed
$ cargo test -p agent-server --features ts                  84 + 109 passed; 0 failed
$ bash scripts/check-invariants.sh --all                    exit 0
$ cargo clippy -p agent-core --all-targets -- -D warnings   干净
```

### 实现 agent 主动做的两件对的事

1. **提前把 `compact_done` / `compact_failed` 加进 `KNOWN_LABELS`**。这一版转移不写
   primitive、落不出带这两个标签的 entry，所以对持久化格式零影响——但等 107 真的写
   状态时，「能产出、认不出」那一版会炸在 `recover` 那头，**离改动点最远**。
   这正是 100 踩过的坑（见那条的实做记录），教训被应用上了。
2. **协议面连带处理干净**：`Effect`/`Event` 不在协议面（没有 ts-rs derive），
   但 `Notice` 在——`Notice.ts` 已重新生成，`Notice` 从纯对象 union 变成混合 union
   之后 `packages/web/src/render/notice.ts` 的 `in` 运算符不能再作用于字符串成员，
   跟着改了，`pnpm -r typecheck` 绿。**动 `Notice` 会连累前端**，下次记得。

### 路过的存量超限（只指出，未重构）

`crates/agent-cli/src/print/events.rs` 在 HEAD 就是 **321 行**（已超 300），
本条被穷举 `match` 逼着加了两条打印 → **332 行**。按规矩「路过存量超限文件（小改）
→ 指出超限，但不擅自顺手大重构」。但它现在只会更难拆，**下一个动这个文件的人应该
先拆它**。
