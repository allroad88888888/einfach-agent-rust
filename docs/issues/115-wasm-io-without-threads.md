# 115 wasm 上没有线程：provider IO 路径怎么办（**决策**）

**里程碑** M13 · **依赖** 113 · **模型** opus · **独测** 决策类

113 实做时撞出来的，**111/113/114 都没预料到**。这条不定，114 做不下去。

## 事实（113 实测，不是推断）

`Client::post_stream` 是同步签名（为了「上层零改动」）。但在
`wasm32-unknown-unknown` 上**没有线程就没法阻塞等 `fetch`**：
`std::thread::spawn` **能编译过，运行时直接 trap**（该目标没有 `+atomics` /
`SharedArrayBuffer`，本仓工具链也没有开）。

113 的处置是两个都发：`post_stream` 保持签名但立刻返回一条解释性
`TransportError`（**不死锁**），真实实现走 `post_stream_async`。于是
`agent-providers` / `agent-runtime` **编译不受影响——但那只是编译过，不是能跑**。

## 挡在哪：`io_thread` 不只是「起个线程发请求」

`crates/agent-runtime/src/io_thread.rs::spawn` 里 `thread::spawn` 那一下，同时扛着四件事。
**逐条都要有替代，缺一件就是静默坏掉**：

| 它扛的 | 为什么不能简单换掉 |
|---|---|
| **发请求并回喂流** | 唯一一件「换成 async 就完了」的 |
| **029 的并行载体** | 「每个 agent 一个 IO 线程，子 agent 的 provider 调用因此真的同时在飞」。子 agent 的并发**就是** IO 并发（STATE-MODEL §并发）。线程没了，并行要由单事件循环上的并发 future 顶上 |
| **`sync_channel(0)` 的会合式背压** | 容量 0 是刻意的：「一个线程发一条增量就等泵收走」。**单线程上，往零容量 channel 发而接收端没在跑 = 死锁**，不是变慢 |
| **超时后「放弃而不 join」** | 模块文档写明不返回 `JoinHandle`，就是为了让泵能走开。`DoneDebt` 的 `Drop` 保证泵永远不会为一个死掉的线程干等——这套「欠债—还债」在没有线程的世界里对应什么，要重新想 |

## 还牵动一条已拍板的决策

**决策 16**：「`ProviderRequest` 存在的理由是线程边界，不是组装」——
`store` 是 `Rc<RefCell>` 不 `Send`，HTTP 在别的线程，所以必须有一份「在 actor 线程上
提取、能带走」的东西。

**wasm 上没有那条线程边界。** 决策 16 的**理由**在这个目标下不成立了。
但 `ProviderRequest` 作为「已提取、可带走的料」本身仍然有用（它同时是 `check_drift`
的快照）。**所以：不要因为理由不成立就删掉它**——要么给决策 16 补一条「wasm 上理由
不同但结论保留」，要么明确它是双理由的。这条必须在本 issue 里表态，不能留给实现者临场判断。

## 要定的四件事

> **四条全部拍板**（2026-08-10 / 08-11）。实现拆成 116、117 两个 issue，见文末。

1. ✅ **泵怎么在不阻塞的情况下等 —— 已定**：泵 async 化。其余三条路
   （泵同步+IO async / Worker+Atomics / Worker+同步 XHR）分别因
   **物理上不成立 / 需要我们控制不了的 COOP-COEP 头 / 丢掉流式**被排除。
   并已拍板**一套路径、两边都 async**（native 也 async，不做 `#[cfg]` 双份泵）。
   **未决的连带账：`agent-runtime` 目前零 async 依赖，引入哪一种 async 原语要单独定**，
   见「调研 · 子问题一」。
2. ✅ **async 原语 —— 已定：引 `futures`，且只引最小子集**（`futures-core` + `futures-util`）。
   否掉 tokio（只开 `sync` 给不了 executor，等于还要再引第二样；且把 tokio 拉进 runtime
   会诱使后来人用它在 wasm32 上不完整的 `time`/`rt`）。否掉全部手写——
   **理由是这两样东西的风险不对等**：`block_on`（约 30 行）自己写完全没问题，错了当场暴露；
   但会合 channel 自己写要处理 waker 注册/取消/多发送端，**错的方式恰好是本仓最忌讳的那种
   ——静默、偶发、难复现**。为了省一个依赖去自维护一根会偶发卡死的管道，不划算。
   限定最小子集是为了不给「核心路径可以随便引 async 库」开口子。
3. ✅ **零容量会合 —— 已定：接受「每个发送端至少缓冲 1 条」，但必须补对抗测试**。
   两个现成 channel 都买不到真会合（已验源码，见「调研」）。选 `futures` 的 `mpsc::channel(0)`。
   论证见「调研 · 子问题三」：在飞调用数本来就有上限（决策 20 的深度 ≤3 / 子数 ≤8），
   缓冲总量有界；真正的风险窗口在**取消之后的幽灵增量**，那正好撞红线 6，
   **必须有一条对抗测试证明晚到消息确实被 `(agent, attempt)` 挡掉**，不许假设。
4. ✅ **决策 16 的表态 —— 已定：结论保留，理由改成双理由**。
   `ProviderRequest` 在 native 上的理由是线程边界（store 是 `Rc<RefCell>` 不 `Send`）；
   wasm 上没有那条边界，但它**同时是 `check_drift` 的快照**——这第二个理由与目标无关。
   已在 `ROADMAP.md` 决策 16 补注。**不要因为第一个理由不成立就删掉它。**
5. ✅ **029 并行的验收方式 —— 已定**：沿用 M7 那条思路，
   **断言两个子 agent 的 provider 调用时间窗重叠**。退化成串行不报错、只变慢，
   必须有断言，不许靠观感或「看起来同时」。

## 调研（2026-08-10，主 agent 备料。子问题一已拍板，二、三仍待定）

### 答案空间比看起来窄：同步函数无法让出 JS 事件循环

泵的 D 步是 `runner.rs::receive()` → `rx.recv_timeout(POLL_INTERVAL)`，**同步阻塞**。
单线程 wasm 上这一句阻塞，整个 JS 事件循环就停了——**包括那些本该送来消息的 fetch
promise**。不是变慢，是**必死锁**。

而 Rust 的同步函数**没有任何办法**让出 JS 事件循环（那需要 await 点）。所以「泵保持同步、
只让 IO 那段 async」这条路**在物理上不成立**，不用再讨论。

四条路，两条当场排除：

| | 做法 | 判断 |
|---|---|---|
| **A** | 泵 async 化，`receive` 改成 await | **可行**。代价是传染：`run_turn` 变 async，一路传到 `agent-server`/`agent-cli` |
| **B** | 泵同步、IO async | ❌ **物理上不成立**（同步函数无法 await） |
| **C** | 整个核心塞进 Web Worker，用 `Atomics.wait` 真阻塞 | ❌ 需要 `SharedArrayBuffer` → 宿主页面必须发 COOP/COEP 头。**宿主是 `cloud.deepfos.com`，不归我们控制**。且 worker 里要 fetch 仍需另一个线程送数据回来，等于把 native 的双线程模型重造一遍 |
| **C'** | Worker + 同步 XHR（`async:false`，worker 里合法） | ❌ **不流式**。SSE 增量是这个产品的地基，丢了等于没做 |

**所以实际上只有 A。** 本 issue 的价值不在「选哪条」，而在**选 A 之后的三个子问题**。

### 子问题一：async 化到哪一层 —— **已拍板：一套路径，两边都 async**（2026-08-10）

被否的是「两套路径」（native 同步 + wasm async，`#[cfg]` 交换整个泵）。
113 对 transport 那么干是对的——transport 是边缘、已隔离；**泵在 `agent-runtime` 核心
执行路径上，性质不同**：复制一份意味着 029 的并行、epoch 闸、`DoneDebt` 的欠债—还债、
截止线扫描**全都两份，而且漏改一边不会报错**。

取向与决策 17 一致：宁可一次性付改动面，不留「两份实现要同步」的长期税
（那次是把事前分支的 2^N 种组合掉回 1）。

**这条拍板牵出的连带账，实现前必须一并解决：**

`agent-runtime` **目前零 async 依赖**——只有 path deps + serde，靠 `std::thread` +
`std::sync::mpsc`。全仓只有 `agent-server` 有 tokio（`sync`/`net`/`time`/`rt`，为 axum 与 SSE）。
所以「两边都 async」等于**要给核心执行路径引入 async 原语**，三选一：

| | 代价 |
|---|---|
| 引 `futures`（或 `futures-core`/`futures-util`） | 核心路径多一个依赖。相对克制，生态标准 |
| 引 `tokio`（只开 `sync`） | `sync` 在 wasm32 上可用，但把 tokio 拉进 runtime 会诱使后来人用它的 `time`/`rt`（那两个在 wasm32-unknown-unknown 上不完整） |
| 手写最小 executor + channel | `block_on` 约 30 行（`thread::park`/`unpark`），零容量 async channel 约 80 行。**零依赖，但两样都得自己维护且自己测** |

**顺带**：native 侧 `run_turn` 变 async 之后，`agent-cli` 需要一个 `block_on`
（它现在没有 tokio），`agent-server` 已有 tokio 不受影响。

### ⚠️ 子问题二有个坑：零容量会合在常见 async channel 里**买不到**

`sync_channel(0)` 的语义是**会合**——发送方阻塞直到接收方取走，一条增量都不缓冲。
迁到 async 时不能想当然地「换个 async channel 就行」：

- `tokio::sync::mpsc::channel(n)` **要求 `n ≥ 1`**（传 0 会 panic）
- `futures::channel::mpsc::channel(0)` 的 buffer 0 **不等于会合**——它额外给每个 sender
  保证一个槽位，实际仍能缓冲

也就是说**照搬任何一个现成 channel 都会悄悄改变背压语义**：增量开始在内存里堆积，
而这**不会报错**，只在长回复 / 慢消费时表现为内存涨和「取消后还在收增量」。

实现者必须二选一并写明理由：**手写一个真会合的 async channel**，或者**明确接受
「至少缓冲 1 条」并论证为什么无害**（后者要给出论证，不是一句「应该没事」）。
这两条我都没验到底，**动手前先实测确认上面两个 channel 的真实行为**，别照我这段写。

### 子问题三：029 的并行怎么证明没退化

线程换成同一事件循环上的并发 future 之后，「子 agent 真的同时在飞」要有**可判定的验收**。
现成的参照：M7 真机验收用的是「活树面板从 1 节点长到 3、状态灯随 `Thinking→Working→Done` 变」。
建议沿用同一条：**wasm 侧跑同样的 spawn 场景，断言两个子 agent 的 provider 调用时间窗重叠**
——退化成串行不会报错，只会变慢，必须有断言而不是靠观感。

## 拆出的实现 issue

按「每一步都有可独立验证的中间态」切（WORKFLOW §一），**wasm 到 117 之后才进场**：

| # | 做什么 | 验证方式 | 依赖 |
|---|---|---|---|
| [116](116-async-pump.md) | 引 `futures` 最小子集；泵与 `run_turn` async 化；`agent-cli` 接 `block_on` | **纯 native**：`cargo test --workspace` 全绿，行为不变。还没碰 wasm | 115 |
| [117](117-io-without-threads.md) | `io_thread` 的 `thread::spawn` 换成并发 future；`sync_channel(0)` 换 `futures` mpsc；029 并行保全 + 取消时序对抗测试 | **仍在 native 上验**：并行不退化、幽灵增量被挡 | 116 |
| [114](114-wasm-host.md) | 真正编到 wasm 跑起来 | 真机 | 117 |

## 范围

**本 issue 只产出决策与接缝定义，不写实现。**

理由跟 004 / 040 / 095 一样：这四件事任何一件定错，后面的代码形状全要重来。
而且它碰的是 `agent-runtime` 的核心执行路径，不是边缘适配。

## 验收（可判定）

- 上面四件事各有明确结论**与理由**，不是「视情况而定」。
- 决策 16 的表态落到 `ROADMAP.md`（补注或新决策条目），不只写在本文件里。
- 029 并行能力的保全方式写清楚，并给出**可判定的验收方式**
  （怎么证明子 agent 真的同时在飞，而不是看起来像）。
- 拆出的后续 issue 有编号、依赖关系和各自的验收标准。

## 注意

- **碰红线 6**（epoch 回写）与**红线 3**（store 外的活句柄）。在飞调用的凭据、
  `(agent, attempt)` 关联、晚到消息的丢弃，这套在改 IO 载体时极易破——
  破了不报错，只在「幽灵结果写进历史」时浮出来。
- **不要在本 issue 里顺手动 `mcp_call.rs`**。它也用 `thread::spawn`，但浏览器形态下
  `agent-mcp` 整个不编（决策 26），不在本里程碑范围内。
- 111 的代价清单里写的「唯一的结构性改动是 `RunnerCtx.fs`」**已被证伪**——
  那是 112 的范围内唯一，整个 M13 的结构性改动至少还有这一条。
  111 的实做记录要补一笔，别让后来人以为只有一处。
