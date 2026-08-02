# 012 把 loop 接到真实 transport 上

**里程碑** M1 · **依赖** 003 · **模型** sonnet · **独立测试 agent** 否 · **状态** 完成

## 目标

loop 产出的 `Effect::CallProvider` 真的打到模型上，响应真的变回事件喂进 loop。

## 为什么单列

001–003 全是纯函数，对着 mock 跑；022 的 CLI 壳是直连的、没有 loop。
**这个 issue 是第一次让 loop 碰真东西**——两头都在了，缺的只是接起来。

接线本身不难，难的是这一步会暴露 001–003 所有假设里错的那些。所以它单列，不混进 003。

## 做什么

一个 runner：从 loop 拿 `Vec<Effect>`，逐个执行，把结果转成 `Event` 喂回去，
直到 loop 说停。

- `CallProvider { agent, epoch }` → **在 actor 线程上**让 adapter 从状态取料组装
  （决策 15），产出的请求发给 transport。**流式增量在 runner 里直接打印，
  不进 loop**——001 实做时裁决掉了 `ProviderChunk` 事件（理由与翻案条件见
  [001](001-loop-contract.md) 实做记录第 1 条），整轮完成后以一个 `ProviderDone` 喂回
- `Emit` → 先打到 stdout（M3 有 SSE 之后改成推流）
- `Persist` → M1 阶段丢弃（还没有 store）
- `CancelInFlight` / `SpawnChild` / `Compact` → M1 阶段返回「未实现」而不是静默忽略

**未实现的 effect 要显式报错**，不能默默吞掉——静默忽略会让 loop 以为事情做了。

**计时器活在 runner。** 超时不是 core 的概念（001 的验收：core 里没有
`Instant::now()`），runner 到点往 loop 里注入 `Timeout` 事件。

## 验收

- 一轮真实对话跑通：用户输入 → 模型响应 → 打印
- 三家 provider 都能跑（用 `providers.toml` 里的 `[default]` 切换）
- 流式增量实时打印，不是等整轮结束才出
- 024 的三层兜底告警时打印出来——第一次在真实 loop 里看它工作

## 注意

runner 是 IO 层，不进 `agent-core`（红线 7）。放 `agent-transport` 或新建
`agent-runtime`，取决于它是否需要知道 loop 的状态——**如果需要，说明 loop 的接口漏了**。

## 实做记录

新建 `crates/agent-runtime`（库 crate，`Cargo.toml` 加进 workspace members）：
`ctx.rs`（90）/`event.rs`（58）/`guard.rs`（29）/`io_thread.rs`（82）/`lib.rs`
（56）/`provider_call.rs`（144）/`runner.rs`（76）/`tool_exec.rs`（48）/
`tool_table.rs`（94），全部 ≤300 行，一文件一件事。`agent-cli` 改依赖它：
`turn.rs` 从「直连 HTTP」改成「一轮到下一轮怎么接」（66 行），`repl.rs`（54）/
`print.rs`（177）/`main.rs`（129）跟着改。

### runner 需不需要知道 loop 内部状态？——不需要，接口没有漏

这是 issue 原文点名要验证的问题。答案：`provider_call::execute` 需要读的只是
`TurnState::messages`（组 `Ingredients` 用）和 `TurnState::prev_prefix`（同上）
——这两个字段在 `state.rs` 的文档注释里早就写明白是「012 宿主取料」专用的公开
字段，不是绕过封装偷看内部。`tool_exec::execute`/`CancelInFlight`/`Emit` 三个
effect 执行点甚至不需要 `&TurnState`——`Effect` 自己带的数据已经够。runner
拿到的 `Vec<Effect>` 本身就是完整的指令集，没有一处要反过来问 core「你现在
到底是什么状态」。接口是好的。

唯一一处**看起来**像是在替 core 做决定、其实是宿主自己领域的事：`TurnState`
的生命周期跨不跨用户输入。002/016 的转移表只定义了 `Idle+UserInput` 这一个
入口，没有「终态 + `UserInput`」这一格——这是有意的：**一个 `TurnState`
实例天然对应一整个「一次用户输入到给出答复」的周期**，会话层面「接着聊」
不是 loop 的职责。`agent-cli::turn::next_turn` 把上一轮的 `messages` /
`prev_prefix` / `next_message_id` / `max_turns` / `max_retries` 带进一份新
`TurnState`（`status` 回 `Idle`，槽位与本轮计数器清零），这是宿主/会话层的
构造，不是 core 接口漏了要补的东西——`agent-runtime::run_turn` 本身对这件事
一无所知，也不需要知道。

### CallProvider：计时器怎么做的

「用读线程的 channel recv 超时实现」字面照做：`io_thread::spawn` 起一个
**真的** `std::thread` 去跑 `client.post_stream(..)`（这正是 ADAPTER.md
时序图里的「IO 线程」，`provider_call::execute` 是「actor 线程」），流式增量
经一个 `mpsc::sync_channel(0)`（跟 `agent-transport::read_loop` 内部同一个
rendezvous 手法）转成 `RunnerEvent` 发回来；actor 线程只在这个 channel 上
`recv_timeout(20ms)` 轮询，轮询到自己的截止时间就是「到点」，注入
`Event::Timeout { call_id: None }`。取消标志的轮询完全交给 `post_stream`
内部已有的机制，这里不重复实现。

超时之后**放弃**那个 IO 线程，不 join、不主动断它的连接——理由记在
`provider_call.rs` 顶部：它可能正卡在 `agent-transport` 那边最长 60s 的死流
兜底里，join 会把 022/023 已经花力气解耦掉的问题重新接回来。放弃的线程下一次
尝试往 channel 发消息时会发现接收端没了（`rx` 被 drop），自己收手；真发生
数据的场景（`happy_two_hop` 用的服务器）沿用同一条路径也完全正常收尾，
测试没有另开分支。

### 四个 effect，不是 issue 原文写的「五个未实现返回错误」

写 issue 时 `Effect` 还没定形；001/002 落地后 `Effect` 只剩四个真实变体
（`CallProvider`/`ExecuteTool`/`CancelInFlight`/`Emit`），`SpawnChild`/
`Compact`/`Persist` 连空壳变体都没有（001 的判断：空壳比不定更糟，见
`effect.rs` 顶部文档）。`runner::run_effect` 的 `match` 因此天然穷举，没有
`_` 分支，也没有「未实现」路径可走——不是我们绕开了 issue 原文要求的报错，
是 core 那边已经不存在需要报错的变体了。`CancelInFlight` 是真实现（置取消
标志），不是占位。

### 工具执行的「发起时快照」——002 合并记录点名的宿主职责

`ToolTable::snapshot`（`tool_table.rs`）按工具全名前缀解析 `Location`
（`srv:`/`web:`/`desk:`），`Reversibility` 按白名单给已知的两个内置工具
（`srv:fs/read`、`srv:fs/list`）判 `Pure`，其余保守落 `Irreversible`——
拿不准就往安全的一边错（`value/tool.rs` 的判据）。这份快照进
`RunnerEvent::ToolExecuting`，是「调了什么、在哪跑、可不可逆」在 M1 唯一的
出口（`Persist` 效果不存在，没有地方落盘，002 早就写明这是留给 M2 `Entry`
的事）。

### GuardReport 第一次在真实 loop 里工作

每次成功收尾的 `CallProvider`（`guard.rs::report_success`）：第 2 层
`reconcile` 用这次的 `predicted_cache` 对账，第 3 层先把这轮记进
`RunnerCtx::guard_history` 再 `check_window`（顺序跟 `agent_core::cache`
模块文档的示例一致），拼成 `GuardReport` 经 `RunnerEvent::TurnGuard` 交回调。
第 1 层 `check_drift` 单独在发请求前判一次——`Unexpected` 时立刻经
`RunnerEvent::PreflightDriftAlert` 打出来，不等这一轮成功收尾（这一轮完全
可能失败/超时/被取消，等不到 `TurnGuard`）。CLI 侧 `print::print_turn_guard`
直接打 `GuardReport` 的 `Display`（三行，中文），换掉了 022 时代手写的
「对不上」措辞。

### 偏离 issue 原文字面的一处：`RunnerCtx` 用 `Arc` 不是 `Box`

issue 原文写「`Box<dyn Provider>`」，实做用了 `Arc<dyn Provider>`（`Client`
同理）。理由：放弃 IO 线程的设计要求那个线程能带着自己那份 provider/client
的引用独立跑到自然结束，`Box` 不能被两个所有者（actor 线程的 `RunnerCtx`
和已经放弃、仍在跑的 IO 线程）同时持有。这是本 issue 唯一一处偏离原文字面
类型的地方，记在 `ctx.rs` 顶部注释。

### 验收清单逐条

- 两跳真实对话跑通、流式增量实时打印、三层告警可见：`tests/happy_two_hop.rs`
  ——假 SSE 服务器（手写 `TcpListener`，手法照抄
  `agent-transport/tests/fake_sse.rs`）第一跳带 1 个 `ToolUse`（wire 形状
  复用 `agent-providers` 已经验证过的 DeepSeek 录制帧，不是这个测试自己现造
  的假设），runner 真执行 `srv:fs/read`（临时目录里的真文件），第二跳回
  `EndTurn`。断言终态 `Done{truncated:false}`、四条消息历史完整、两次
  `TurnGuard` 都产出。
- `Timeout` 注入路径：`tests/timeout.rs`——服务器写完响应头就挂住不回，
  `provider_timeout` 设 150ms，`max_retries=1`，断言恰好一次
  `Notice::Retrying{attempt:1,max_retries:1}`，最终落
  `Failed(Provider(Retryable))`，全程秒级完成（不是真的等到 60s 死流兜底）。
- 取消路径：`tests/cancel.rs`——服务器挂住不回，独立线程 200ms 后置位
  `RunnerCtx::cancel_flag()`（模拟 Ctrl-C），断言 `Failed(Cancelled)`、
  槽位清空、耗时远小于特意拉大的 5s 超时预算（证明是取消标志起的作用，
  不是超时机制凑巧撞上同一个终态）。
- 三家 provider 都能跑：`RunnerCtx`/`run_turn` 不对 `Provider` 做任何分支
  （唯一按名字分发具体 adapter 类型的地方还是 `agent-cli::main::
  build_provider`，红线 12 只约束 core/store，这条 match 合法），`providers.
  toml` 的 `[default]` 切换路径原样保留（023 的工作未改动）。集成测试只
  具体验证了 DeepSeek 的 wire 形状（复用已有录制帧，不重新摸 GLM/Kimi 的
  转义规则）——GLM/Kimi 各自的 encode/decode/accumulator 契约已经在
  `agent-providers` 自己的测试里覆盖，`agent-runtime` 这层不重复。
- **没有发出任何真实请求**：`cargo run -p agent-cli < /dev/null` 只用来验证
  配置加载/横幅打印/干净退出（stdin 立刻 EOF，`run_turn` 从未被调用，
  不产生任何网络流量）。

### 自测

`cargo test --workspace` 全绿（`agent-runtime` 新增 3 单元 + 6 集成 = 9，
`agent-cli` 新增 1）；`cargo clippy --workspace --all-targets -- -D warnings`
零告警；`scripts/check-invariants.sh --all` 通过。

### 真实验收（主会话，2026-08-01）

一轮带工具的真实对话（deepseek-v4-pro）：模型思考 → 发起 srv:fs/read
（参数流式拼接）→ 宿主查表构造真快照（location=Server reversibility=Pure，
002 合并手术的成果）→ 执行返回 1309 字节 → 第二跳正确回答。三层判读全程
用 024 正式文案：发前比对干净、第二跳对账 512/512 一致、滚动窗口如实报
「连续 1 轮低命中未到告警线」（工具跳 prompt 突增摊薄命中率，正常形态不误报）。
workspace 413/0。**loop、adapter、transport、tools、guard 五件东西第一次
在同一根线上转。**
