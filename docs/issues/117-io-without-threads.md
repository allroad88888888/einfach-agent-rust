# 117 IO 载体换掉：`thread::spawn` → 并发 future

**里程碑** M13 · **依赖** 116 · **模型** **opus** · **独测** ✅（碰红线 6，且失败方式静默）

116 之后泵已经是 async 的，但 IO 仍然靠 `std::thread`。本 issue 把载体换掉。
**仍然在 native 上验证**——wasm 到 114 才进场。

模型定 opus 的理由：本 issue 要动的四样东西里有三样**违反了不报错**
（背压、并行、幽灵增量），符合 WORKFLOW §二 的第三档。

## `io_thread::spawn` 那一下同时扛着四件事，逐条都要有替代

| 它扛的 | 替代方案 | 缺了会怎样 |
|---|---|---|
| 发请求并回喂流 | 换成 future，用 113 已做好的 `post_stream_async` | —— |
| **029 的并行载体** | 同一事件循环上的并发 future | **退化成串行，不报错只变慢** |
| **`sync_channel(0)` 的会合背压** | `futures::channel::mpsc::channel(0)` | 见下，115 已定接受缓冲 1 条 |
| **超时后「放弃而不 join」** | future 直接 drop | `DoneDebt` 的欠债—还债语义要重新落实 |

## 115 已定的两条，直接执行，不要重新讨论

**① 零容量会合买不到，接受「每个发送端至少缓冲 1 条」。**
已验源码：tokio 的 `mpsc::channel(0)` 直接 `assert!(buffer > 0)` panic；
futures 的容量公式是 `buffer + num-senders`，**每个 sender 保证一个槽位**，
所以 `channel(0)` 也不是会合。

论证（115 给的，实现时要验证而不是引用）：在飞调用数本来就有上限
（决策 20 的深度 ≤3 / 子数 ≤8），所以缓冲总量有界，不是无限堆积。

**② `DoneDebt` 的语义要保住。** 原实现用 `Drop` 保证「线程 panic 了泵也不会干等」。
future 被 drop 时同样要还上这条终态消息，否则泵会为一个已经没了的调用永远等下去
——而且**不报错，表现为对话永久转圈**（这正是 A1 里记的那个 `isTurnFinished` 事故的同款症状）。

## 验收（可判定，三条都是「不做就静默坏掉」的）

1. **并行没退化**：起两个子 agent，**断言它们的 provider 调用时间窗重叠**
   （115 定的方式，沿用 M7 真机验收的思路）。不许用「看起来同时」「日志里挨着」这类观感判据。
2. **取消后的幽灵增量被挡住**——**这是本 issue 最重要的一条**：
   泵划掉一个在飞调用之后，那个发送端手里可能还攥着一条已写进 channel 的增量
   （旧的会合语义下不存在这个窗口，是本次引入的）。
   要有一条**对抗测试**：制造「取消 → 晚到增量」的时序，断言它按 `(agent, attempt)`
   找不到凭据而被丢弃，**且没有写进消息历史**。
   参照现成的 `tests/mcp_epoch_writeback.rs`——那条就是同款对抗断言（结果确实回来了 +
   历史里没有它）。
3. **未还的债**：future 被 drop（超时/取消路径）时泵仍收到终态消息，
   turn 能正常收工，不会挂住。

外加：`cargo test --workspace --no-fail-fast` 除既有失败外全绿；CLI 真机跑一轮含子 agent
的对话，`Ctrl-C` 取消后进程还活着。

## 注意

- **碰红线 6**（epoch 回写）与**红线 3**（store 外的活句柄）。验收第 2 条就是冲着红线 6 来的。
- **不要顺手动 `mcp_call.rs`**。它也用 `thread::spawn`，但浏览器形态下 `agent-mcp` 整个不编
  （决策 26），不在本里程碑范围内。**native 上它照旧用线程，两种载体并存是可以的。**
- 116 在「同步 channel」与「async 泵」之间搭的那座临时桥，**本 issue 要把它拆掉**。

## 实做记录（2026-08-11）

**状态：完成。** `cargo test --workspace --no-fail-fast` 除既有的 `agent-server`
`http_image_input::text_stays_on_old_wire_shape_and_attachment_reference_survives_recovery`
之外全绿（那一条在 `a960951` 的干净 worktree 上单独跑同样 FAILED，panic 在同一行
「等待第 1 个模型请求超时」，确认是既有失败）。

### 临时桥拆在哪

116 的桥只有一句：`runner::receive` 里那句真阻塞的 `rx.recv_timeout(POLL_INTERVAL)`。
拆掉它之后 `runner.rs` 从 376 行降到 364 行，D 点变成
`bus.receive(POLL_INTERVAL).await`——第一个会真的让出线程的 await 点。原来那一句
同时扛着的三件事被拆成三个东西：

| 116 的一句 | 117 之后 | 文件 |
|---|---|---|
| 「等一条消息」 | `futures` 的 `mpsc::channel(0)` | `io_bus.rs` |
| 「等的时候顺便让别的 IO 有进展」（原先靠线程各跑各的） | `FuturesUnordered`，每次 poll 推一遍全部在飞 future | `io_bus.rs` |
| 「最多等 20ms 就回去看时钟和取消标志」 | 显式的心跳 | `heartbeat.rs` |

### 四件事各自的替代品

| `io_thread::spawn` 扛的 | 现在归谁 |
|---|---|
| 发请求并回喂流 | `io_stream.rs`（平台接缝）。**native 底下仍有一条只读字节的工作线程**——ureq 是阻塞的，物理上必须有人扛这份阻塞；不扛在工作线程上就得扛在泵的线程上，那当场就是 029 并行的死亡。接缝切在「行」这一层：累积器、`(agent, attempt)` 信封、背压、欠债全部搬回泵所在的单线程。wasm 上只换这一个文件（`fetch` 的 `ReadableStream` 本身就是异步行源，不需要线程），上面的代码一字不动 |
| 029 的并行载体 | `io_bus.rs` 的 `FuturesUnordered` |
| `sync_channel(0)` 的会合背压 | `io_task.rs` 里 `delta_tx.send(..).await`，channel 是 `mpsc::channel(0)` |
| 超时后放弃而不 join | future 直接丢掉；`io_task::DoneDebt` 的 `Drop` 还终态债 |

**`DoneDebt` 现在必须独占一份 sender**，这是 115 决策 3 换 channel 之后新长出来的约束：
`Drop` 里没有 `.await` 只能 `try_send`，而 `futures` 的容量公式是 `buffer + sender 数`
（槽位按 sender 记）。只要这份 clone 一辈子只发一条消息，`try_send` 就只可能因为
「接收端没了」失败，不会因为「channel 满了」失败——后者正是「泵为一个已经没了的调用
永远等下去」的入口。这条不靠注释保证，`io_task_tests.rs::
a_fresh_sender_always_has_one_slot_even_when_the_channel_is_full` 焊着它。

**`mcp_call.rs` 只改了一处接线**：`SyncSender` → `IoSender`、`send` → `try_send`
（同一条保底槽位论证，它一辈子也只发一条）。载体照旧是线程，符合本 issue 的注意事项。

### 验收结果

1. **并行没退化** —— `spawn_parallel_futures_interleave::
   two_children_stream_concurrently_instead_of_one_after_another`：既断言服务端两条服务
   区间重叠（老判据，`subagent_parallel.rs` 那条也照旧绿），又断言**客户端事件流里两个
   子 agent 的增量交替出现**。后者才是本 issue 的判据：请求是一起发出去的、底下各有一条
   工作线程，所以哪怕泵退化成「一次推一个 future 推到完」，服务端看到的重叠照样成立。
   实测形状 `A B A B A B A B A B A B`（12 条增量 12 段），阈值是 3；退化写法结构上只能
   产出 2 段。
2. **取消后的幽灵增量被挡住** —— 两条，都做过「拆掉闸就变红」的验证：
   - `provider_call::attempt_correlation_tests::
     a_delta_already_in_the_channel_is_dropped_once_its_credential_is_gone`（确定性，不碰
     HTTP）：喂一行 → **只推 IO future 不收消息**（增量此刻真的躺在 channel 的槽位里，
     这就是 117 新引入的那个窗口，旧的会合语义下不存在）→ 划掉旧凭据并补一张**同 agent
     同 epoch 的重试凭据** → 收：消息确实回来了（`receive` 拿到了它），而 `land` 认不出
     `(agent, attempt)` → 丢弃，无事件、无待办、重试凭据原封不动。
   - `late_provider_reply_after_timeout`（端到端，走真 socket）：一条**对照** + 一条
     **对抗**共用同一份服务器脚本。对照组（超时预算 5s）证明那些字节确实会到达泵、会进
     历史；对抗组（预算 500ms）只改这一个数，幽灵回来时凭据已被 `deadline::sweep` 划掉、
     重试在飞，断言幽灵没变成事件、没进历史，且整轮仍靠重试落 `Done`（被放弃那次的终态
     一旦冒领重试的凭据，这一轮会变成 `Failed(Cancelled)`）。
   - 两条都验过：把 `provider_message::land` 的认领条件从 `agent && attempt` 削成 `agent`，
     两条立刻红。
3. **未还的债** —— `io_task_tests::dropping_the_task_before_its_terminal_item_still_pays_the_debt`
   （丢掉在飞的 future → 泵收到 `gone`）、`a_settled_task_pays_exactly_one_terminal_message`
   （正常路径只还一次，`Drop` 不补第二条）。行源半路断掉（工作线程 panic）落到同一条
   `Drop` 路径，泵翻成一次可重试失败，不挂住。
   顺带 `the_task_never_runs_more_than_one_delta_ahead_of_the_pump` 把 115 接受的那条代价
   （「每个发送端最多缓冲 1 条」）的上界焊死。

**CLI 真机**：仍然没有可用的 provider key（`DEEPSEEK_API_KEY` 返回 401），所以改成把
`providers.toml` 的 `base_url` 指向一个本地假 SSE server 跑真二进制。四条依次为
yes：正常一轮流式答完 → 流到一半发真 `SIGINT` 判成 `[本轮失败: Cancelled]` 且进程还活着
→ 取消之后新一轮照常答完 → `/quit` 1 秒内干净退出（服务端共收到 3 个请求，与三轮对得
上）。**没有验证的是真实模型对话与前缀缓存命中率**——假 server 的 usage 是编的。

### 依赖：`futures-core` 删了，`futures-channel` 加了

116 留的两个依赖里只有一个真被用上：

- `futures-util` ✅ —— `StreamExt`/`FuturesUnordered`/`SinkExt`，**要开 `sink` feature**
  （不是默认）。
- `futures-core` ❌ —— 从头到尾没有一行代码用到它（它只提供 trait 定义，我们用的是
  `futures-util` 的扩展 trait），按「用不上就删」删掉；它仍是传递依赖。
- `futures-channel` ➕ —— 115 决策 3 点名的 `futures::channel::mpsc` 住在这个 crate 里，
  **`futures-util` 没有转发它**（`futures_util::channel` 不存在，只有全量门面 `futures`
  才有 `futures::channel`，实测确认）。所以「引 futures 的 mpsc」在最小子集下就等于直接
  依赖 `futures-channel`（同样要开 `sink`）。
- `agent-cli` 那一行 `futures-util` 删了——116 的注释自己写了「如果 117 之后证实 CLI
  永远不需要直接碰 futures-util，这一行可以删」，答案是不需要。

`rg 'futures' crates/*/Cargo.toml` 现在只命中 `agent-runtime` 的
`futures-channel`/`futures-util`（外加 `agent-transport` 里 113 就有的
`wasm-bindgen-futures`，子串误命中）。

### 遗留问题

- **native 上还剩两处线程**，都在平台接缝之下、都不是「IO 载体」本身：`io_stream` 的
  阻塞 socket 工作线程（ureq 的物理约束，wasm 上换成 `fetch` 就没有了）和 `heartbeat`
  的 20ms 心跳（native 的 async 世界没有现成定时器，本仓不引 tokio；wasm 上是
  `setInterval`）。114 接 wasm 时这两个文件各换一份实现即可，泵与 `io_task` 不用动。
- `runner.rs` 364 行，仍然超 300 的普通上限（本 issue 让它**少了 12 行**，没有变大）。
  它是事件泵本体、单一状态机，够得上「复杂文件」候选（上限 500），但仍然没有正式走一遍
  认定——跟 116 的遗留记录同一条，原样传给下一次触碰它的 issue。
- 本 issue 的 `io_thread.rs` 删除被并行进行的另一个 agent 的 docs 提交
  （`cc72bb2`）意外裹了进去，导致那一个提交单独 checkout 出来编不过（`lib.rs` 还声明着
  `mod io_thread;`）。本 issue 的提交把两边补齐，`feat/wasm-target-m13` 的 tip 是好的；
  历史里那一个中间提交不可编译，已知，没有回改（分支是共享的）。


## 实做之后补的两件事（2026-08-11）

### 一、我给的验收条 1 判据不够，实现者当场纠正了

原文写「断言两个子 agent 的 provider 调用**时间窗重叠**」。实现者指出这条**盯不住
本次的退化**：请求是一起发的、底下各有工作线程，**泵退化成串行时服务端看到的时间窗
照样重叠**。已改成同时断言**客户端事件流交替**（实测 `A B A B A B…` 12 个增量 12 段，
阈值 3；串行写法结构上只能产 2 段）。

**教训记在这儿**：定「并行没退化」的判据时，要问「测的是发出去那一端，还是收回来
那一端」。并行的证据在收端，不在发端。

### 二、真模型上的取消验证（实现者只有假 SSE server）

在 `3786757` 上用真 DeepSeek 补跑，四条依次成立：

1. 流式写散文写到一半，发**真 `SIGINT`** → `[本轮失败: Cancelled]`，正好断在
   「我正在等一个」这个字上；
2. **进程存活**；
3. `[已撤销] 取消的第 1 轮留下的 2 条痕迹已经擦除，没有计入历史`
   —— `DoneDebt` 与幽灵增量的丢弃在**真 provider 上**成立，不只是假 server 上；
4. 取消之后紧接着的下一轮：`prompt=6403 cached=6272`（**0.98**）。
   这条比看上去重要：**取消轮如果在前缀里留下任何残渣，缓存会当场掉到 0**。
   命中率没掉，说明「擦除」是真的擦干净了。
5. `/quit` 干净退出，没动用 `kill -9`。
