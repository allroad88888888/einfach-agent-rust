# 首发帖文案（191）

两个渠道两套文案，**不是同一篇复制**。HN 关心「这是什么、为什么反直觉」，
r/rust 关心「Rust 侧你怎么做的」。

---

## A. Show HN

### 标题

```
Show HN: An agent runtime where undo actually removes the turn from the model's memory
```

**为什么是这句**：不含形容词，说清是什么，且包含一个可验证的反直觉断言。
读者的第一反应应该是「等等，别人的 undo 不是这样吗？」——那个疑问就是点进来的理由。

备选（如果觉得上面太长）：
```
Show HN: Einfach-agent – undo that actually removes the turn from the model's memory
```

### 正文

```
I kept hitting the same thing while building agent products: undo is fake. You delete a
bubble from the UI, the model still has the turn in its context, and the next reply
contradicts what the user just "removed."

So I built the runtime the other way around. All agent state lives in one atomic
dependency graph with a command log, and the prompt is reconstructed from that log. Which
means undo, redo, crash recovery and audit replay stop being four features that drift
apart — they're four views of the same mechanism. Recovery is literally the redo loop, the
same function.

There's a browser demo with no backend at all — the runtime is compiled to wasm and runs in
the tab, your key goes straight to the provider:
https://allroad88888888.github.io/einfach-agent-rust/

Thirty seconds to check the claim: tell it a passphrase, ask for it back, hit "Undo one
turn" twice, then ask again *without using the word "undo"* (say the word and you feed the
model the answer — I got a false negative that way while testing). It says it never knew.
It stays gone after a refresh.

Two other things that fell out of the same design:

- kill -9 mid-conversation and continue. Not a special code path; loading a session IS the
  redo loop.
- Reversibility barriers: undo stops at an irreversible tool call and tells you which one,
  instead of silently rolling past it.

Written in Rust. The same core runs as a CLI, a standalone HTTP/SSE server, embedded in a
desktop app or a Java gateway, and in the browser with no server.

Honest about what it isn't: single-tenant, single-replica, API not stable, first commit was
ten days ago. Multi-replica and multi-tenancy are unbuilt and marked as unbuilt everywhere
they come up — including in the architecture doc, which has a section that opens with
"there is not one line of code behind this."

Repo: https://github.com/allroad88888888/einfach-agent-rust
```

**长度**：HN 正文短为好，这个约 280 词，接近上限，别再加。

**「Honest about what it isn't」那段必须留**：HN 上藏短处会被扒出来，主动说反而加分。
而且那句「there is not one line of code behind this」是真的引用，不是姿态。

---

## B. r/rust

### 标题

```
einfach-agent: an agent runtime built on an atomic dependency graph, so undo/redo/crash-recovery/audit-replay are one mechanism
```

r/rust 允许长标题，且这里的关键词（atomic dependency graph）就是 Rust 侧读者的钩子。

### 正文

```
I've been building an embeddable agent runtime in Rust and the central bet is unusual
enough that I think it's worth writing up.

**The bet**: all agent state lives in one atomic dependency graph (a fork of a
signals/jotai-style engine) with a command log in front of every write. Source state is
primitive atoms; everything derived — prompt ingredients, pending aggregation, UI
projections — is recomputed by the engine. So "the complete state" is a thing you can
actually name: the values of all primitive atoms.

That buys undo, redo, crash recovery and audit replay from one mechanism rather than four.
Recovery is loading a snapshot and pushing the log forward, which is the redo loop calling
the same function.

**Rust-specific things that came out of it, in case they're useful to someone:**

- **The store is deliberately not Send/Sync.** It's Rc<RefCell<_>> with Rc listeners,
  because that's what synchronous re-entrant, glitch-free propagation costs. Wrapping it in
  Arc<Mutex> would turn re-entrancy into deadlock risk for a benefit that isn't real. So
  each session owns a thread; the outside world talks to it over mpsc/broadcast. Only the
  server crate knows tokio exists.
- **On-disk keys are logical, never AtomId.** AtomId is an incrementing u64, so a snapshot
  keyed by it silently misaligns the moment someone inserts a create_atom into the middle
  of a graph-construction function. Logical keys also give schema evolution for free — a
  new slot just isn't in old snapshots and takes its default.
- **Message history is imbl::Vector, not Arc<Vec<_>>.** store.get() returns owned values,
  so every read clones; append needs to be O(log n) with structural sharing or a
  thousand-message session re-copies history several times per turn.
- **Twelve invariants, six of which produce no error when violated.** They only surface as
  a wrong value during undo or crash recovery — the two paths that get tested least. The
  ones a grep can decide are wired into a hook that runs on every file save; the rest live
  in a design doc, because a checker that cries wolf gets disabled.
- **No provider branching in the core, and no capability flags either.** Capability flags
  are the same branching with vendor names filed off: N flags is 2^N combinations, most
  never executed, and adding a fourth vendor still means editing the core. Instead the core
  states intent and the adapter reports what it had to change, attached to the turn.

Also compiles to wasm and runs entirely in a browser tab, no server:
https://allroad88888888.github.io/einfach-agent-rust/

Repo (Apache-2.0 / MIT): https://github.com/allroad88888888/einfach-agent-rust

Young — first commit ten days ago, API not stable, single-replica only. Happy to answer
anything, especially if you think the not-Send/Sync store is a mistake; that's the decision
I'd most like to be argued with about.
```

**最后那句是有意的**：邀请针对最贵的那个决策的反驳。r/rust 上主动请人挑最硬的地方，
比防守姿态好得多，也确实是我最想听到反馈的一条。

---

## 发之前的检查（[191](../issues/191-launch-post.md) 的清单）

| | 状态 |
|---|---|
| demo 链接可用且**当天验过** | ✅ HTTP 200（发之前再验一次） |
| README 第一屏有 demo + GIF | ✅ |
| 一条陌生人能真的走通的路径 | ✅ demo，自带 key，公网 CORS 已验 |
| LICENSE 在 | ✅ 双许可 |
| **至少一篇文章已发并有反响** | ❌ **五篇都是初稿，一篇没发** |
| **你有一整天能守评论区** | ❓ 只有你知道 |

**后两条不满足就别发。** 尤其倒数第二条：Show HN 只有一次机会，
带着一篇没人看过的文章去发，等于把最能证明「这人认真」的材料浪费掉。

建议顺序：先发 [183](../issues/183-post-providers.md)（三家实测那篇，
独立价值最高、最可能被转），看反响，再 Show HN 并在正文里引用它。

---

## 评论区常见问题预案

**「跟 rig / langchain-rust 有什么区别？」**
不同品类。那些是拼 LLM 应用的库；这个是嵌进产品的运行时，卖点是状态账本。
真要类比，近的是 LangGraph 的 time travel 和 Temporal 的 durable execution。

**「为什么是 Rust？」**
嵌入形态需要一个能被 CLI / 服务端 / 桌面 / 浏览器 wasm 同时装进去的核心，
而且状态引擎是热路径。顺带：wasm 那条形态基本是免费拿到的。

**「为什么只有中国模型？」**
最早接的三家（DeepSeek/Kimi/GLM）是手上有 key 的，而且它们的缓存语义差异大，
恰好逼出了正确的 adapter 接缝。**现在有通用 OpenAI 兼容 adapter**，
任何 OpenAI 兼容端点填个 base_url 就能用。

**「生产用了吗？」**
没有。诚实说：每个里程碑都以真 provider（不是 mock）的实跑收官，
但没有第三方在生产里用它。**别把 dogfood 说成生产验证。**

**「不 Send/Sync 不是限制吗？」**
是，而且是有意的——代价写在 ARCHITECTURE 里。每个 session 独占一个线程，
跨 session 天然并行；限制在于单个 session 的状态操作不能多线程并发，
而那正是我们不想要的东西（状态并发 ≠ IO 并发，后者照旧是并发 future）。

**「这不就是 event sourcing 吗？」**
是它的一个具体应用，加上「derived 全部由引擎重算」这一半。
关键差别在后半：不是所有 event sourcing 系统都能说清「完整状态是什么」。
