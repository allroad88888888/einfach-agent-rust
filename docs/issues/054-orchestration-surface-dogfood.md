# 054 编排面板呈现 + 真机 dogfood

**里程碑** M8（收官） · **依赖** 051/052/053 · **模型** sonnet · **独测** 真机验收

把模型侧编排接上人眼，并在**真实上游**跑一遍——本仓的老规矩：只有真机能现形「测试绿、世界
不对」（046 单测 + 048 emit 测试都绿，undo 漏投影只有 049 真机才现形；见 STATE-MODEL §M7）。

## 范围

1. **活树面板呈现后台/collect 状态**（`packages/web/src/render/agent_tree.ts` + CLI `/agents`）：
   M7 的活树（046-049）已经把 `AgentActivity` 画出来了。本 issue 确认后台子 agent（父不阻塞
   时并发跑的那些）在活树里**正确显示**：并发的多个后台子同时 Working、被 collect 的转 Done、
   被孤儿取消的转终态。若 M7 的哑渲染已覆盖（它按 `agent_tree()` 快照整棵重画）→ 本项可能零
   代码，只加验收断言；若后台并发暴露了渲染缺口（如同层多子的排布）→ 在此补。**不新造状态源**。
2. **真机 dogfood**（主会话驱动，deepseek 真实上游，playwright + curl 直打）：
   - 起真 server（`examples/serve.rs`，读 `providers.toml`，spawn 开满档）+ web。
   - 给模型一个**要分解并观测**的任务，诱发 `spawn(background)` × N → `status` 观测 → `collect`。
   - 浏览器活树面板 + `GET /sessions/{id}/agents` 同刻一致（推拉两路同一棵树，复用 048 验收形）。
   - `/undo` 一个含后台子的 turn → 树回退（复用 049 逮 undo 漏投影的验收形，确认后台子也被
     `emit_tree_snapshot` 覆盖）。

## 验收（可判定）

- 真机上模型自发用 `spawn(background)`/`status`/`collect` 完成一个分解任务（决策 20 精神：
  模型自己决定怎么编排，工具是给它的手段）；面板上**同时看到多个后台子在 Working**（这是本
  里程碑相对 M7 的新现象——M7 时子是阻塞串行的）。
- 面板树 == `GET /agents` 树（推拉一致）。
- 含后台子的 turn `/undo` → 面板树回退（后台子一并消失）。
- 真机跑通的截图/日志留档进本 issue 的实做记录（照 049）。

## 注意

- **不新增状态 / 不新增 primitive**：面板仍是 `agent_tree()` 快照的哑渲染（OBSERVABILITY §哑
  渲染）。本 issue 是「接人眼 + 真机验」，不是造监控。
- **红线 8**：真机 server 默认 loopback，不硬编码 `0.0.0.0`。
- **providers.toml 只读不印不提交**：真机要读它拿 key，但**任何输出/日志只出长度/状态，绝不
  出 key 正文**；不提交它（gitignored）。
- 真机验收若逮到新的漏投影/可用性摩擦（像 049 逮到 undo 漏投影、050 逮到工具名编码）→ 单列
  新 issue，不塞进本 issue 硬修。
- 真机步骤前台跑完、如实报（WORKFLOW §四 -1）：vite dev 代理在本环境历史上发飘——若 SSE 经
  代理不稳，用 `curl` 直打 `GET /agents` 定验（它是面板数据源，它对面板就对），并如实标注是
  dev 环境问题非 fix（照 048/049 的处置）。

## 真机 dogfood（主会话跑完 · 2026-08-04 · **M8 收官**）

真 server（`examples/serve`，`ToolTableSpec::Full` = spawn+status+collect）+ **真 deepseek 上游**，
`curl` 直打 API（用户明确「不用浏览器看」；`GET /agents` 是面板的数据源，它对面板就对——
048/049 的既定处置）。providers.toml 只读不印，启动行只出 `key=已配置（N 字符）`。

**任务**：让模型用 `background=true` 并行发三个子 agent，先 `status` 观测再 `collect` 逐个领回。

**证据一：模型自发跑通完整闭环**。帧里 `srv:agent/spawn` × 18 / `srv:agent/status` × 4 /
`srv:agent/collect` × 15 提及，`background` × 20。模型自己的叙述还原了全过程：

> 先把三个子 agent 都发出去：三个已发出：`root/a1`、`root/a2`、`root/a3`。现在看看它们的
> 状态：三个都已完成。逐个领回结果：全部到手。汇总成一句：**苹果、香蕉和樱桃，三样都齐了！**

**证据二（本里程碑的核心，M7 下结构上不可能的那一帧）**：推出去的 `agent_tree` 快照序列——

```
125  r:Working(spawn,spawn,spawn)                          三个 spawn 同时在飞
133  r:Working(spawn,spawn) | a1:Idle a2:Idle a3:Idle      子已建出，第一个 spawn 槽当场收敛
137  r:Working(spawn)  | a1:Thinking a2:Thinking
139  r:Thinking        | a1:Thinking a2:Thinking           ← root 不再被挡，自己在想
141  r:Thinking        | a1:Thinking a2:Thinking a3:Thinking   ★ root 在想 + 三子同时在跑
162  r:Thinking        | a1:Thinking a2:Done  a3:Thinking   陆续完成
236  r:Thinking        | 三子全 Done
256  r:Working(srv:agent/status)                            观测
287  r:Working(collect,collect,collect)                     领取
344  r:Done            | 全 Done
```

**帧 141 就是 M8 相对 M7 的全部增量**：阻塞 spawn 下 root 必然卡在 `ToolsPending`
（面板显示 `Working(spawn,spawn,spawn)`）直到子收敛，这一帧**结构上不可能出现**。
代码侧那条 `two_background_children_are_on_the_tree_at_once_while_the_parent_thinks`
断言的就是它，此处在真机真模型上重现。帧 133 另证 background 的语义：三个子都已存在
（`Idle`）而 root 还剩两个 spawn 槽在飞——**第一个槽已经当场收敛了**，这正是「发射即返回」。

**证据三：含后台子的 turn `/undo` → 整棵子树回滚**。

```
346 undo        {"type":"applied","data":{"entries":21,"turn_id":1}}
347 agent_tree  {"nodes":[{"id":"root", ..., "activity":"Idle"}]}
GET /agents  →  root:Idle          （推拉一致）
```

一整轮 21 条 entry 连带三个后台子一起退干净，**`turn_id` 继承 + `ToolsAllowed→Null` 对后台子
同样成立，一行新代码都没写**（决策 24「子 agent 不跨 turn」换来的正是这个）。

**顺带验到 M9 的两块**（同一次真机）：**055** 指定的 `{"id":"m8-dogfood"}` 直接成为 session id；
**056** 的 `GET /events/poll` 真机可用——拉到 256 帧（= ring 容量上限，`next=345` 说明这轮共
345 帧、更早的按设计被挤掉），带 `Last-Event-ID: 344` 续拉精确接上后续帧。

**一条如实记的操作失误（不是产品问题）**：`POST /undo` 首次返回 **400**——因为该端点用
`ApiJson<UndoRequest>`，**必须带 JSON 体**，我没带 body 也没带 `content-type`，撞的是
`ApiJson` 的 rejection（固定文案、刻意不回显请求体，`http/json.rs` 的既定设计）。带 `{}`
立刻 202。**端点没有问题**，记在这里是因为下一个手打 curl 的人会踩同一脚。

### 真浏览器补验（playwright 驱动 · 同一天 · 面板这一半）

上面那轮是 curl 打 API（验的是**数据源**）。用户要求「你直接操作 mcp playwright 看看」，
于是把**面板本身**也在真浏览器里验了一遍。

**先解决了一个卡了两个里程碑的假问题**：M7（048/049 记录）和本 issue 早先都写了
「vite dev 代理在本环境发飘 / 502」，据此绕开浏览器改用 curl。这次查明：**502 来自
`curl` 被系统代理劫持**——本机有 `http_proxy=http://127.0.0.1:7897` /
`all_proxy=socks5://...`，curl 默认走它、它转发本机端口失败。加 `--noproxy '*'` 一切正常。
**那两处「代理发飘」的归因存疑**（vite 代理本身很可能一直是好的）。

绕过它的落地办法（本次顺手做的唯一代码改动）：`examples/serve` 认 **`AGENT_STATIC_DIR`**
环境变量 → 走 036 就有的 `ServerConfig::with_static_dir`（tower-http `ServeDir`）由 server
自己托管 `packages/web/dist`，**同源、根本不需要 dev 代理**。不设该变量时行为一字不变。

**面板实测（真 deepseek，SSE 实时推送，DOM 采样带时间戳）**：

```
t0.4s   root:Thinking | a1,a2,a3:Done（上一轮遗留）
t4.4s   root:Thinking | a4:Thinking  a5:Thinking  a6:Thinking   ★ 三个后台子同时在跑
t6.4s   root:Thinking | a4:Thinking  a5:Done      a6:Thinking
t7.6s   root:Thinking | a4:Done      a5:Done      a6:Thinking
t10.4s  root:Thinking | a4,a5,a6 全 Done
t15.2s  root:Done
```

`t4.4s` 这一帧就是 curl 那轮帧 141 的浏览器版本，**肉眼可见**：root 自己在想的同时三个后台子
并行跑。面板还各自显示了子的 task 文本（「请写一句关于春天的话，20字左右」），
状态灯随 `Thinking→Done` 实时变。**M7 的哑渲染确实零代码覆盖了后台子**（054 代码部分的结论
在真浏览器上兑现）。

**undo 走完整链路**（点页面上的 Undo 按钮，不是 curl）：树 **9 个节点 → 7 个**，
`a7`/`a8`（最后一轮 spawn 的两个后台子）消失，`a1`–`a6`（前两轮的）**完整保留**——
turn 粒度精确，按钮 → API → SSE → 面板重画整条链路通。

**真机彩蛋（决策 20 的硬限在真实场景下生效 + 模型自纠）**：第三轮模型想发第三个子时撞上
`DEFAULT_MAX_CHILDREN = 8`（树上正好 `a1..a8`），收到超限的 `is_error` 后**没有重试，自己
收敛**——它的原话是「…但仍在计数，名额还是满的。没关系，「星空」我替你补上」，然后自己写完
了那一句。这正是决策 20 说的「超限 = `is_error` 的 tool_result 让模型自己收敛」，M6 那次
（工具名编码被拒后自纠）之后的第二次重演。

**红线 11 顺带受检**：9 个 agent 的会话里缓存对账 `predicted=4480 / actual=4480 Match`、
滚动窗口命中 **89%**——后台子 agent 大量进出没有破坏前缀稳定性。

## 实做记录（代码部分 · 2026-08-04）

本 issue 的**代码部分**三件事全部落地。第 2 项（真机 dogfood，deepseek 真实上游 + 浏览器）
**没做**——它要 `providers.toml` 的真实 key 和一个跑起来的浏览器，归主会话。所以这个
issue **不是完全完成**，`docs/ROADMAP.md` / `issues/README.md` 别按「M8 收官」勾掉。

### 一、`agent-server` 接上 `collect`（053 按硬约束留的那笔）

`ToolTableSpec::Full` 这一档的整个意思就是「开子 agent」，而 053 只给 CLI 接了
`.with_collect()`（它的硬约束点名不碰 `agent-server`）。**开了 `background` 却不开
`collect` 是陷阱**：模型能发后台子，却领不回结果，只能眼看它在轮末被当孤儿拆掉。

```rust
ToolTableSpec::Full { spawn_limits } => agent_runtime::ToolTable::with_shell()
    .with_spawn(spawn_limits)
    .with_status()
    .with_collect(),
```

看门狗两条（`registry/spec.rs` 新开的 `mod tests`，照 051 给 `with_status` 加的那条）：
`the_full_table_declares_spawn_status_and_collect_together` 正面断言三件套同时在；
`no_other_tier_declares_the_orchestration_trio` 反面断言别的四档一个都没有——不然「反正
每一档都有」也能让正面那条绿。

**突变验证**（去掉 `.with_collect()`，改回后重跑绿）：

```
$ cargo test -p agent-server --lib registry::spec
test registry::spec::tests::the_full_table_declares_spawn_status_and_collect_together ... FAILED
thread '...' panicked at crates/agent-server/src/registry/spec.rs:126:9:
Full 该有 collect
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 45 filtered out
```

### 二、专属告警变体：`RunnerEvent::OrphanedChild`（收掉 052 记的欠账）

052 的孤儿告警借的是 `RunnerEvent::TransportTrouble`，它的实做记录**诚实标注**了那个
名字对不上语义（这不是传输故障，是编排失误），并判定专属变体属 054 范围。收掉了。

**形状**——载荷是**事实**，不是句子：

```rust
// agent-runtime/src/event.rs
RunnerEvent::OrphanedChild { child: AgentId, fate: OrphanFate }

pub enum OrphanFate {
    Despawned { descendants: usize },      // 还活着 → 连同后代一起拆了
    Kept { reason: String },               // 拆不掉（DespawnRefused），活着留到下一轮
    Discarded { bytes: usize, is_error: bool }, // 跑完躺 stash 没人领，结果丢弃
}
```

三个决定，各有理由：

1. **`child` 单独一个字段，事件归属（`AgentEvent.agent`）统一是父。** 「spawn 了后台子却
   没领」是**父**的编排失误，告警该出现在父的时间线上；出事的那个子是这条告警的**宾语**，
   不该跟主语挤在同一个位置。052 那会儿两类告警的归属是分裂的（拆孤儿挂父、stash 没人领
   挂子），现在统一了——为此给 `Subtree::Stashed` 补了一个 `parent` 字段（`harvest_detached`
   里 `Detached` 本来就记着父），而不是从 `child.parent()` 反推：反推要给一条结构上不可能
   发生的 `None` 留兜底分支。
2. **`fate` 是判别枚举，不是一段文本。** 三种收场对模型编排的含义完全不同（被拆了 =
   工作丢了；跑完没人领 = 白烧了 token；拆不掉 = 下一轮还会看见它），面板要分得开。
   措辞归呈现层：CLI 一份（`print::events::describe_fate`）、web 一份
   （`render/notice.ts::describeOrphanFate`）——跟 `AgentActivity` 在两个壳各有一份呈现
   是同一条既有规矩，不是本 issue 发明的分叉。
3. **`agent-server` 侧另开一个可序列化姊妹类型**（`event/orphan_fate.rs`），不给
   `agent-runtime` 的那个挂 `Serialize`/`ts_rs::TS`：runtime 至今没有 `ts` feature，
   没有理由为「跨 SSE 长什么样」背一个代码生成依赖。这不是新先例——`UndoOutcome` 对
   `agent_core::UndoReport` 就是这么处理的，同一个模块里的同一种做法。

**连锁改的清单**（一次跨 crate 的穷举 match 连锁，漏一处编译不过——好事）：

| 文件 | 行 | 干什么 |
|---|---|---|
| `agent-runtime/src/event.rs` | 114（79→，+35） | 新变体 `OrphanedChild` + 新枚举 `OrphanFate` |
| `agent-runtime/src/lib.rs` | 103 | `pub use event::OrphanFate` |
| `agent-runtime/src/orphan.rs` | 125（126→，−1） | 改发新变体；**不再拼句子**（三个事实报出去）；模块文档换掉「为什么借 TransportTrouble」那一节 |
| `agent-runtime/src/subtree.rs` | 298（288→，+10） | `Stashed.parent`（告警归属统一挂父） |
| `agent-cli/src/print/events.rs` | 266（213→，+53） | 穷举 match 新臂 + `describe_fate` + 一条「四种收场不该渲染成同一句话」的测试 |
| `agent-server/src/event/mod.rs` | 218（183→，+35） | `SessionEvent::OrphanedChild` + `From<RunnerEvent>` 新臂 + 映射测试；子模块表 三→四 |
| `agent-server/src/event/orphan_fate.rs` | 81 | 新：可序列化姊妹类型 + `From` + 邻接标签的 serde 实检 |
| `agent-server/src/lib.rs` | 69 | 重导出 `OrphanFate` |
| `agent-server/src/ts_protocol/fixtures.rs` | 224（211→，+13） | 骨架 + `cast_sample` 穷举新臂（样本挑字段最多的 `Discarded`，理由同 `Undo` 挑 `Blocked`） |
| `agent-server/src/ts_protocol/consistency.rs` | 154（152→，+2） | 变体数 15→16 + `session_event_kind` 穷举新臂 |
| `packages/protocol/src/generated/OrphanFate.ts` | 生成物 | 新——`{"type":"despawned"\|"kept"\|"discarded", "data":…}` |
| `packages/protocol/src/generated/SessionEvent.ts` | 生成物 | 多了 `{ "type": "orphaned_child", "data": { child, fate } }` |
| `packages/protocol/fixtures/events.{json,ts}` | 生成物 | 多一条 `orphaned_child` 样本 |
| `packages/protocol/src/index.ts` | 39（+3） | 收拢 `OrphanFate`（web 直接点名了它） |
| `packages/web/src/render/dispatch.ts` | 108（100→，+8） | `case "orphaned_child"` |
| `packages/web/src/render/notice.ts` | 86（61→，+25） | `renderOrphanedChild` + `describeOrphanFate` |
| `agent-runtime/tests/spawn_bg_support/mod.rs` | 90（77→，+13） | `warned_about` 改比**结构化的 `child` 字段**（不再 `contains` 文本子串）+ 新 `orphan_warnings` |
| `agent-runtime/tests/collect_three_out_of_order.rs` | 113（118→，−5） | 那条「一句告警都没有」改用 `orphan_warnings` |

**副产品：告警断言变强了。** 052/053 的 `warned_about` 是在告警**文本**里 `contains` 一个
agent id 子串——文案改一个字它就跟着红，而 id 恰好是别人的前缀时它会假绿。现在比的是
`RunnerEvent::OrphanedChild { child, .. }` 那个字段本身。

TS 重新生成（`cargo run -p agent-server --features ts --example gen_protocol_ts`）之后
032 的三条锁全绿，见下面验证输出——**这是「TS 已重新生成」的操作证据**，不是承诺。

### 三、面板呈现 bg/collect：**零代码（活树），补了告警的落点**

先确认再动手，结论分两半：

**活树面板本身零代码，M7 的哑渲染已经覆盖。** 理由是结构性的，不是运气：面板（web
`render/agent_tree.ts` / CLI `/agents`）整棵重画 `Session::agent_tree()` 快照，而
`agent_tree()` 是 `live_agents()` 的纯派生读（`observe.rs`）。后台子 agent 在 store
里跟别的 agent **没有任何区别**——`background=true` 只改了「父那个槽什么时候收敛」和一张
运行时局部表（`Subtree`），一个 atom 都没加。于是：

- 并发的多个后台子同时在跑 → 它们各自的 `TurnStatus` 就是 `Thinking`/`ToolsPending`，
  投影成 `Thinking`/`Working` → 面板上同时亮着，**零代码**；
- 被 collect 的转 `Done` → collect 不拆人，子留在活名单上、状态是 `Done`，**零代码**；
- 被孤儿收尾拆掉的从树上消失 → `despawn_child` 把它移出活名单，下一帧快照自然没有它，
  **零代码**（052 已经在 `orphan::reap` 之后补了一次 `maybe_emit_tree`，因为那条路不经过
  `session.step`，A 段的变化检测看不见它）。

所以这一项**加的是验收断言，不是渲染代码**——`tests/tree_snapshot_background_children.rs`
（171 行，新），两条，各钉一个 M7 时不存在的现象：

1. `two_background_children_are_on_the_tree_at_once_while_the_parent_thinks`：推出去的快照
   里存在**一帧 root `Thinking` + 两个子同时 `Thinking`**。这一帧在阻塞 spawn 下**结构上
   不可能**——那时 root 卡在 `ToolsPending`（面板上是 `Working(spawn, spawn)`）直到子收敛。
   顺带断言同层两个子的 `depth`/`parent`/`task` 各是自己的（「同层多子的排布」那条顾虑），
   以及最后一帧三个节点全 `Done`（collect 不拆人）、最后一帧 == 此刻现读的 `agent_tree()`
   （哑渲染的前提本身）。
2. `a_reaped_orphan_disappears_from_the_pushed_tree`：孤儿**确实上过树**（否则下一条会白绿），
   而拆掉之后**最后一帧只剩 root**。

**突变验证**（把脚本里的 `"background":true` 全改成 `false`，= 退回阻塞 spawn；改回后绿）：

```
$ cargo test -p agent-runtime --test tree_snapshot_background_children two_background
test two_background_children_are_on_the_tree_at_once_while_the_parent_thinks ... FAILED
thread '...' panicked at crates/agent-runtime/tests/tree_snapshot_background_children.rs:108:28:
该有一帧是 root 在想、两个后台子同时在跑：[
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 4 filtered out
```

**唯一真的补了代码的渲染缺口是「新告警没地方显示」**：`SessionEvent::OrphanedChild` 是
web 端的新变体，`dispatch.ts` 不接的话它会被静默吞掉（TS 的 `switch` 没有穷举检查，
不会红）。补在时间线上（`renderOrphanedChild`，`warn-line`），**不进树面板**——树面板仍是
`agent_tree()` 快照的哑渲染，不新造状态源。

### 验证（前台跑完，真实输出）

```
$ cargo test -p agent-runtime -p agent-server -p agent-cli
exit=0
binaries=93 passed=382 failed=0 ignored=0

tests/collect_child_failure.rs                 test result: ok. 4 passed; 0 failed; ... 0.27s
tests/collect_matches_blocking_spawn.rs        test result: ok. 5 passed; 0 failed; ... 0.32s
tests/collect_refusals.rs                      test result: ok. 4 passed; 0 failed; ... 0.27s
tests/collect_three_out_of_order.rs            test result: ok. 4 passed; 0 failed; ... 0.42s
tests/collect_waits_for_a_running_child.rs     test result: ok. 4 passed; 0 failed; ... 0.42s
tests/spawn_bg_epoch_writeback.rs              test result: ok. 5 passed; 0 failed; ... 0.03s
tests/spawn_bg_orphan_reaped.rs                test result: ok. 4 passed; 0 failed; ... 0.92s
tests/spawn_bg_tail_cut.rs                     test result: ok. 5 passed; 0 failed; ... 1.52s
tests/spawn_bg_two_children_no_block.rs        test result: ok. 4 passed; 0 failed; ... 0.72s
tests/tree_snapshot_background_children.rs     test result: ok. 5 passed; 0 failed; ... 0.32s   ← 新
tests/tree_snapshot_emits_on_change.rs         test result: ok. 2 passed; 0 failed; ... 0.35s

$ cargo test -p agent-server --features ts        # 032 的锁：证明 TS 真的重新生成过了
exit=0
binaries=36 passed=141 failed=0 ignored=0
test ts_protocol::consistency::generated_ts_matches_committed_snapshot ... ok
test ts_protocol::consistency::fixtures_json_matches_committed_snapshot ... ok
test ts_protocol::consistency::sample_events_cover_every_variant_at_least_once ... ok
test event::orphan_fate::tests::from_runner_fate_translates_field_for_field ... ok
test event::orphan_fate::tests::orphan_fate_serializes_round_trip ... ok
test event::tests::from_runner_event_maps_orphaned_child_and_its_fate ... ok

$ cargo clippy -p agent-runtime -p agent-server -p agent-cli --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.25s
exit=0

$ bash scripts/check-invariants.sh --all
红线检查通过
规则与理由：docs/INVARIANTS.md
exit=0

$ pnpm -r typecheck
Scope: 3 of 4 workspace projects
packages/protocol typecheck$ tsc --noEmit
packages/protocol typecheck: Done
packages/web typecheck$ tsc --noEmit
packages/web typecheck: Done
exit=0

$ cargo check --workspace --all-targets
exit=0

$ cargo test --workspace                          # 顺带确认全仓无回归
exit=0
binaries=251 passed=1388 failed=0 ignored=0
```

红线 9：改动后全部文件 ≤300（最大的是 `subtree.rs` 298）。红线 11 不适用于本变体——
`OrphanedChild` 只走协议面（SSE/面板），**不进任何 prompt**（进 prompt 的是
`status`/`collect` 的 tool_result 正文，本 issue 一个字节没碰）。

### 没做（留给主会话）

**真机 dogfood**：起真 server（`examples/serve.rs` + `providers.toml`）+ web、诱发模型
自发 `spawn(background)`×N → `status` → `collect`、浏览器活树面板与
`GET /sessions/{id}/agents` 同刻一致、含后台子的 turn `/undo` 树回退、截图/日志留档。
本仓的老规矩是只有真机能现形「测试绿、世界不对」（049 的 undo 漏投影），上面那些断言
证明的是**推出去的快照对**，证明不了**浏览器上画出来的对**。
