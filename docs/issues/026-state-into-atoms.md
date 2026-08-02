# 026 把会话状态搬进原子图（command 层）

**里程碑** M2 · **依赖** 019 · **模型** opus · **独立测试 agent** ✅ · **状态** 完成

## 目标

agent 的全部会话状态成为原子图上的 primitive，每次写入经 command 层留下 `Entry`。
M2 的两句口号——「完整状态 = 所有 primitive」「持久化与 undo 是同一份代码」——
第一次接到真 agent 上。

## 为什么现在才做

M1 刻意用平结构 `TurnState` 走通了 loop（ROADMAP：每提前一步都是没有反馈的赌注）。
现在三件事齐了：store 泛型化经 45 个上游测试验证（007/015）、日志与 undo 全套
落地（009/017/018/019）、**019 实测钉住了三条硬约束**（STATE-MODEL §evict 与 undo）。

## 形状（已裁决，不再讨论）

**原生迁移，不做镜像**。「TurnState 照旧 + 旁路记账」的镜像方案否掉：双份状态
必然漂移，漂移不报错——正是本仓最恨的静默错值。

```
agent-core/src/value/atom_value.rs   AgentValue：Null/Pending/Bool/U64/Text(Arc<str>)/
                                     Json(Arc<Value>)/Messages(imbl::Vector<Message>)/
                                     Status(TurnStatus)/Prefix(PrefixImage)/
                                     Slots(Arc<Vec<ToolSlot>>)——**只定这些**，
                                     M1 教训：加变体要有真实写入点
agent-core/src/graph/                AtomKey{Agent(AgentId,Slot),ToolCall(..)} + Slot 枚举
                                     （M2 单 agent 子集，照 STATE-MODEL 的槽位表裁剪）
                                     + 构图函数（唯一的建图入口，019 硬约束的落点）
agent-core/src/command/              收口层（红线 2 的白名单目录）：
                                     Session { store, family, history } +
                                     每个转移的写入走 command → record_set → Entry
```

`impl AtomValue for AgentValue`（`null()` = `AgentValue::Null`）。`EntryMeta`
（009 的泛型 `M`）：`{ turn_id: u64, epoch: Epoch, label: &'static str,
barrier: bool }`——`barrier` 是 020 屏障谓词的落点。

**engine 的 step 迁移为 `Session::step(&mut self, Event) -> Vec<Effect>`**：
内部读写原子而非平结构字段。`tools_converged` 成为第一个 derived atom
（003 注意条预言的「扫槽位形状能搬」在此兑现）。M1 的引擎行为测试**允许等价
重写**（签名变了），每条重写要在实做记录里对得上原测试——行为一条不许变，
002/016/003 的转移表语义就是规格。

## 硬约束（前面 issue 用血换的，违反即返工）

1. derived 闭包**按逻辑键现查 family，不捕获 `AtomId`**（红线 4 孪生条款）
2. 逐出自叶向根、状态驱动（M2 单 agent 用不到逐出，但构图函数的形状要为 M3 留对）
3. **红线 6 在这里结账**：undo 命令 bump epoch（017 推的账）；工具回执带旧 epoch
   被闸丢弃的既有测试在新形态下必须照绿
4. 每次 primitive 写入必有 Entry；derived 重算必无 Entry（009 的结构性事实照搬）
5. 红线 1：derived read fn 纯函数（`agent-core/src/atoms/` 会被 hook 盯上——
   构图函数放 graph/ 还是 atoms/ 自定，红线 1 的检查路径跟着改）

## 验收

- M1 全部引擎行为在新形态下照绿（等价重写对照表在实做记录）
- 一轮完整对话（mock 事件）后：`history.len()` == 该轮 primitive 写入的 batch 数，
  每条 Entry 的 prev/next 与转移语义吻合
- **undo 一整 turn 后所有 primitive 逐值回退、所有 derived 重算一致**（M2 验收的
  核心句），redo 反演
- undo 后旧 epoch 的 `ToolResult` 被丢弃（红线 6 端到端）
- `barrier=true` 的 entry 让 `undo_turn` 返回 `Blocked`（020 的谓词接上真日志）
- 红线检查通过：command/ 外无裸 `store.set`（hook 白名单已就位）

## 注意

这是 M2 最重的 issue。**红线 1/2/3/4/6 全部压在上面**——opus + 独测没有悬念。
runner（agent-runtime）与 CLI 的接线**不在本 issue**：Session 的公开面做到
「能被 runner 以 TurnState 同样的方式驱动」即可，换接线是 027。

## 实做记录（实现 agent，2026-08-01）

### 落地的文件

| 文件 | 行数 | 职责 |
|------|------|------|
| `crates/agent-core/src/value/atom_value.rs` | 181 | `AgentValue` 十个变体 + 手写 `PartialEq`（`Arc`/`imbl` 指针快路）+ `impl agent_store::AtomValue` + 取值器 |
| `crates/agent-core/src/graph/slot.rs` | 144 | 地址空间：`AtomKey` / `Slot` / `ToolCallSlot` / `DerivedKey` 与**每个槽位的默认值** |
| `crates/agent-core/src/graph/build.rs` | 134 | 构图函数：`source_atom` / `derived_atom` / `build_agent`，建 atom 的唯一入口；`tools_converged` 的 read fn |
| `crates/agent-core/src/graph/mod.rs` | 16 | 门面 |
| `crates/agent-core/src/command/session.rs` | 145 | `Session` 结构 + 会话级命令（`new` / `begin_turn` / `set_max_*` / `mark_irreversible` / cap） |
| `crates/agent-core/src/command/read.rs` | 177 | 公开读口（宿主取料）+ `primitives()` 快照 |
| `crates/agent-core/src/command/txn.rs` | 264 | 一次转移的写入事务：类型化读写 + `record_set` 收口 + `Commit` 收账 |
| `crates/agent-core/src/command/commit.rs` | 58 | 一次转移 → 一个 batch → 一条 `Entry` |
| `crates/agent-core/src/command/step.rs` | 59 | `Session::step`：epoch 闸 + 分发 |
| `crates/agent-core/src/command/undo.rs` | 175 | `undo_turn` / `undo_turn_force` / `undo_step` / `redo_turn` / `redo_step` + `UndoReport` |
| `crates/agent-core/src/command/meta.rs` | 58 | `EntryMeta` + 三个日志类型别名 + 两个谓词 |
| `crates/agent-core/src/command/transitions/*.rs` | 25–120（7 个文件） | 转移表本体，逐格对应 `engine/transitions/` |
| `crates/agent-core/src/command/mod.rs` | 42 | 门面 + 那张「写怎么收成一条路」的图 |

**改动到的存量文件**（全部在 `agent-core`）：`lib.rs`（+2 模块、+4 re-export）、
`ids.rs`（`AgentId` 加 `Hash`、`ToolCallId` 加 `Hash`/`Ord`——`AtomFamily<K>` 要求
`K: Eq + Hash`，快照要按键排序）、`engine/state.rs`（两个默认上限常量改 `pub`，
让 `Slot::default_value()` 用同一个数）、`limits.rs`（把 `truncated_content_bytes`
从 `engine/transitions/tool_outcome.rs` 收上来，两条转移表共用）、
`engine/transitions/tool_outcome.rs`（改用上面那个函数，逻辑一字未改）、
`Cargo.toml`（依赖 `agent-store`）。

**改动到的脚本**（issue 允许的那一处）：`scripts/check-invariants.sh` 的
`check_derived_purity` 路径从 `agent-core/src/atoms/` 扩到
`atoms/|graph/`——026 把构图函数（derived 的 read fn 真正的住处）放在了 `graph/`，
issue 原文允许「构图函数放 graph/ 还是 atoms/ 自定，红线 1 的检查路径跟着改」。
`atoms/` 保留在名单里，M3 若长出那个目录不必再改一次脚本。改动处有注释写明来由。

### 设计判断

1. **两路并存，不是镜像**。`engine::step(&mut TurnState, …)` 原样留着，因为
   `agent-runtime` / `agent-cli` 还接在它上面，而本 issue 不动它们（027 的事：
   「原 TurnState 驱动退役」）。**并存的是两份转移表实现，不是两份状态**——
   issue 否掉的「镜像」是「TurnState 照旧 + 旁路记账」那种双份状态，那必然漂移
   且漂移不报错；两份实现各自被一套等价的行为测试钉着，任一侧改了行为，
   对照表上那一条当场炸。曾认真考虑过第三条路（转移表泛型化，两种状态各实现一个
   trait），否掉的理由：那要求 `Txn` 长成 `TurnState` 的字段访问器形状
   （`tool_slots()` 返回引用之类），等于把平结构的形状焊进原子图这一侧，
   而它在 027 之后就是纯负担。

2. **工具槽整体住一个槽位（`Slot::ToolSlots` = `Slots(Arc<Vec<ToolSlot>>)`），
   M2 不建 per-call atom**。003 的注意条预言的是「**扫槽位**的形状能原样搬过去」，
   搬过来就是「derived 现查这一个槽位、扫一遍」。另一种设计（`toolcall.<id>.result`
   逐个 atom 持 `Pending`）在 M2 会立刻制造**两个真值来源**：`ToolSlot.state` 里有
   一份、per-call atom 里有一份，因为 `Slots(Arc<Vec<ToolSlot>>)` 这个变体本身
   就带着 `state`。宁可少建几个 atom，也不留一个「读哪个都像对的」的字段。
   代价是 `Text` / `Json` / `Pending` 在 M2 没有 primitive 写入点（`Pending` 有
   derived 产出点）——**这是刻意的**，见判断 3。

3. **`AgentValue` 的十个变体和 `AtomKey` 的两个变体现在就封闭，`Slot` 才是 M2 子集**。
   前两者是**落盘的 schema**：一条 `Entry` 存过盘之后，事后加变体是一次迁移、
   事后改载荷是让旧日志静默解错。`Slot` 不一样——旧快照缺一个键就用
   `Slot::default_value()`，白拿 schema 演进（STATE-MODEL 写着）。所以 `Slot`
   照槽位表裁到 M2 真有写入点的九个（`config` / `system_base` / `skills_active` /
   `tools_registry_version` 一个都不定），而 `AtomKey::ToolCall` 与那三个还没用上的
   值变体照定。文件顶部的表逐个写明「谁在用 / 谁将来用」。

4. **`epoch` / `turn_id` / 不可逆登记不进原子图**。世代**只增不减**，进了图就会被
   undo 回滚——而 undo 恰恰是要 bump 它的那个动作，红线 6 会自相矛盾。`turn_id`
   是日志的**分组依据**，不是被日志记录的状态。两者都进 `EntryMeta`，崩溃恢复时
   从日志取最大值继续发。不可逆登记（`mark_irreversible`）是运行时提示：它真正的家
   是 `ToolCall(_, _, Request)` 那个发起时快照，但 core 没有工具表，现造一份就是
   002 裁决过的编造（假的 `Irreversible` 会让 undo 白拦一次 `fs/read`）。

5. **两张 family：source 用 `AtomKey`，derived 用 `DerivedKey`**。`AtomKey` 是落盘键，
   给 derived 也发一个 `AtomKey` 就等于给「把算出来的值也存进快照」开了口子
   （STATE-MODEL：快照只存 primitive）。分表之后 `Session::primitives()` 在类型上
   就装不下一个 derived。`DerivedKey` 刻意不 derive serde，同一个理由。

6. **`tools_converged` 未收敛时答 `AgentValue::Pending`，收敛答 `Bool(true)`。**
   `Pending` 是「还在等」的专用值，沿依赖图往下游传播（STATE-MODEL §「Pending 的
   来历」）。M3 的「等所有子 agent 完成」会在同一个位置汇聚，下游读到的仍然是这一个值
   ——现在就用它，M3 不用换语义。

7. **构图函数是唯一入口，默认值只有一处**。命令层写槽位、applier 的 `resolve`
   重建被逐出的槽位、derived 的 read fn 现查依赖，三条路调的是同一个 `source_atom`。
   019 的「重建走正常创建路径，不是特判分支」因此是字面意义上的同一行代码。
   `Slot::ALL` 是那个数组：`Session::new` 建图和快照遍历都用它，新增槽位改一处。

8. **derived 的 read fn 只捕获 `AgentId` + 两个句柄，一个 `AtomId` 都不捕获**
   （红线 4 孪生条款）。`args.get` 拿不到 `Slots` 时**不 panic**：那只可能是 DV-3
   的故障占位（超递归预算时 tracked getter 返回 `Null`，本次运行不会被提交），
   `agent-store` 的 read 契约要求 read fn 容忍它。写成
   `debug_assert!(args.is_faulted(), …)` + 返回 `Null`：debug 下真正的构图 bug 当场炸，
   release 下按契约容忍。

9. **019 的借用陷阱按它的记录避开了**：`source_atom` 借完即还，applier 的
   `resolve` 闭包里只有 `family.get_or_create` + `store.create_atom`，**没有
   `store.get`**——那会在 flush 时撞上 derived 现查 family 的 `already mutably
   borrowed`。

10. **undo：先 bump 世代，再 `apply_prev`**（017 推过来的账）。反过来会留出一个窗口：
    一条回执刚好在这中间到达，写进一个已经被回滚掉的世界。**屏障就在游标下时
    （`Blocked { entries: 0 }`）不 bump**——什么都没改，bump 只会白白作废一批还有效的
    在飞 effect。**redo 不 bump**：undo 是放弃一个世界，redo 是追回一个曾经存在过的点，
    那一代的在飞 effect 早在 undo 那一下就作废了。

11. **`undo_turn_force` 只放行第一条屏障**。谓词里带一个 `Cell<bool>`：第一条屏障
    放行并记账，之后的照挡。理由是用户看到的提示说的是「越过的是这一个
    `shell/exec`」——放行全部等于让一次确认替他答了几个没被问到的问题。

12. **加了 `undo_step` / `redo_step`（batch 粒度）**，超出 issue 点名的三个命令。
    不是顺手加的：决策 5 定的就是两层粒度，017 的 `undo_one` / `redo_one` 早就在，
    而 **003 验收 3「收敛判断不是计数器」的回滚式测试只有这一档到得了**——turn 粒度
    一次退回整轮，槽位回到「空」，翻不出「Pending → 收敛」那个翻转。M1 那条测试是
    直接给平结构字段赋值造出来的，原子图版本没有后门，`undo_step` 是它唯一的正牌替身
    （而且更强：它证明的是「回滚只写回 primitive，收敛是 derived 重算出来的」）。

13. **`begin_turn` 是显式命令，终态收到 `UserInput` 仍然是协议违规**。turn 边界是
    `undo_turn` 的分组依据；藏进一格转移里就意味着「一轮从哪开始」这个会话层面的概念
    被塞进了转移表（002/016 已经裁决过它不属于那里）。M1 的宿主本来也是显式调
    `agent_cli::next_turn`，这里是同一件事换了住处。M1 那份还会在取消时丢弃本轮新增的
    消息——**没有搬过来**：那是宿主的显示策略，而 M2 里「把这一轮抹掉」有了正牌答案
    （`undo_turn`）。

14. **`EntryMeta` 只 derive `Serialize`**。`label: &'static str` 是钉死的形状，而从
    运行时字节反序列化借不出 `'static`。落盘 schema 归 011，它那一侧的 `label` 是
    `String`——进程内的取值是有限的编译期常量集，落盘的是历史数据，允许出现这个版本
    不认识的取值，两者形状不同是对的。

15. **`kept_bytes` 的算法收进 `limits::truncated_content_bytes`**。它本来抄在
    `engine/transitions/tool_outcome.rs` 里（002 落地时 `limits.rs` 不在改动范围）。
    两条转移表各抄一份的话，一旦分叉，报给人的「模型实际看到了多少字节」就不是真的，
    而且不报错。

16. **`Session` 不暴露 store**（红线 2 的结构面），读口一律给值的克隆——所有可能变大的
    东西都在 `Arc` / `imbl::Vector` 后面，克隆是指针拷贝。`primitives()` 是 010 的
    `Snapshot` 形状（`Vec<(AtomKey, AgentValue)>`，按键排序），也是「完整状态 = 所有
    primitive」这句话在测试里的可断言形式。诊断探针 `debug_recompute_count` 标了
    `#[doc(hidden)]`：它存在的唯一理由是「derived 重算到一致」和「停在旧值碰巧也一致」
    在断言上长得一模一样。

17. **日志 cap 默认 100 在会话层设**（`Session::new` 调 `set_cap`），不在 `History`
    里硬编码——018 的裁决：`History` 对「一个会话该有多大」一无所知。

### 等价重写对照表（原测试 → 新测试）

M1 的测试**全部留在原地**：它们钉的是 `engine::step` 那一路，而那一路还接着
runtime/cli，027 换接时才随之退役。下表的「去向」指**同一条行为**在 `Session`
一侧由谁保证。

| M1 测试（文件::测试） | Session 侧 |
|---|---|
| `turn_transitions_grid.rs`（8 条，全部） | `session_transitions_grid.rs` 同名 8 条 |
| `turn_state.rs::turn_state_roundtrip` | `session_state.rs::the_snapshot_of_a_real_conversation_survives_a_serde_roundtrip` |
| `turn_state.rs::convergence_scans_slots` | `session_state.rs::a_fresh_session_starts_at_the_documented_defaults`（空槽=收敛）+ `session_tool_outcome.rs::convergence_happens_only_when_the_last_slot_lands` |
| `turn_state.rs::bump_epoch_advances` | `session_cancel.rs::cancel_from_idle_bumps_epoch_and_fails_as_cancelled` + `session_epoch_gate.rs::a_tool_result_from_before_an_undo_is_dropped_…`（undo 那条 bump 路径） |
| `turn_state.rs::mint_message_id_starts_at_one_and_increments` | `session_state.rs::message_ids_start_at_one_and_increment_across_roles` |
| `turn_state.rs::push_message_mints_and_appends` | 同上 + `session_tool_outcome.rs::message_ids_stay_monotonic_across_a_whole_turn` |
| `turn_state.rs::terminal_statuses_are_exactly_done_and_failed` | **留在原地**：测的是 `TurnStatus::is_terminal()`，跟状态住在哪无关 |
| `turn_state.rs::new_turn_state_starts_with_zero_usage_and_default_caps` | `session_state.rs::a_fresh_session_starts_at_the_documented_defaults` |
| `turn_state.rs::record_turn_attempt_stops_exactly_at_the_cap` | `session_max_turns.rs::turns_used_increments_once_per_call_provider` + `…hitting_the_cap_after_tool_convergence…` |
| `turn_state.rs::record_retry_attempt_stops_exactly_at_the_cap` | `session_provider_failed.rs::retryable_retries_until_budget_exhausted_then_fails` |
| `turn_state.rs::record_turn_attempt_with_zero_cap_rejects_immediately` | `session_max_turns.rs::zero_max_turns_rejects_the_very_first_attempt` |
| `tools_converged.rs`（3 条） | `session_state.rs::a_fresh_session_starts_at_the_documented_defaults`（空）+ `session_tool_outcome.rs::convergence_happens_only_when_the_last_slot_lands`（有 Pending / 全 Finished） |
| `turn_status_terminal.rs`（2 条） | **留在原地**（同上，纯类型方法） |
| `tool_outcome_convergence.rs`（5 条，全部） | `session_tool_outcome.rs` 同名 5 条 |
| `tool_convergence_all_failed.rs`（2 条） | `session_tool_convergence.rs` 同名 2 条 |
| `tool_convergence_duplicate_call_id.rs`（2 条） | `session_tool_convergence.rs::second_result_for_an_already_finished_slot_does_not_overwrite`（**两条并一条**：第二次投递换成 `ToolFailed` 的那半在同一个测试里接着断言，M1 拆两条只是为了各自的 fixture） |
| `tool_convergence_error_reaches_prompt.rs`（2 条） | `session_tool_convergence.rs::failed_tool_error_text_survives_verbatim_into_the_next_prompt_message`（**两条并一条**：错误文本本身就用多行 + 特殊字符，M1 的第二条是同一断言换个输入） |
| `tool_convergence_scan_not_counter.rs`（3 条） | `session_tool_convergence.rs::undoing_a_landed_result_flips_convergence_back_by_recomputation`（**三条并一条且更强**：M1 靠给字段赋值模拟回滚，这里是真的 `undo_step` + 断言 `debug_recompute_count` 涨了——「幂等」「直接 push 也看得见」两条在原子图里不再可表达，因为没有绕过 command 层改槽位的路） |
| `provider_done_stop_reason.rs`（6 条，全部） | `session_provider_done.rs` 同名 6 条（+ 新增 `the_reply_lands_in_history_verbatim_even_on_the_contradictory_branch`） |
| `provider_error_classification.rs`（5 条，全部） | `session_provider_failed.rs` 同名 5 条 |
| `timeout_transitions.rs`（5 条，全部） | `session_timeout.rs` 同名 5 条 |
| `cancel_any_state.rs`（5 条，全部） | `session_cancel.rs` 同名 5 条 |
| `max_turns.rs`（5 条，全部） | `session_max_turns.rs` 同名 5 条（+ 新增 `begin_turn_resets_the_per_turn_budget_and_keeps_the_conversation`） |
| `epoch_gate.rs`（2 条） | `session_epoch_gate.rs` 同名 2 条（+ 3 条新增，见下） |
| `event_epoch_extractor.rs`（2 条） | **留在原地**：`Event::epoch()` 是 001 的契约，本 issue 一个字没动 |
| `call_provider_thin.rs`（1 条） | **留在原地**：`Effect` 契约未动 |
| `serde_roundtrip.rs`（13 条） | **留在原地**：`Effect`/`Event`/`Notice` 契约未动 |
| `no_clock_meta_test.rs::engine_directory_never_reads_the_clock` | **原地扩容**为 `the_transition_tables_and_the_derived_never_read_the_clock`：扫 `engine/` + `command/` + `graph/`，模式加上 `rand::`/`thread_rng`（红线 1 的落点现在在 `graph/`） |
| `harness_happy_path.rs`（2 条） | `session_flow.rs::full_turn_with_two_parallel_tools_converges_to_done`（第二条「每次请求时的消息数」并进同一条的分步断言里） |
| `harness_tool_reorder.rs`（2 条） | `session_flow.rs::out_of_order_backfill_still_respects_slot_order_and_survives_a_failure`（两条并一条） |
| `harness_cancel_in_flight.rs`（1 条） | `session_flow.rs::cancel_while_tools_in_flight_gates_the_late_results` |
| `harness_timeout_injection.rs`（2 条） | `session_timeout.rs` 的两条转移语义断言；**`Harness` 本身留在 M1**——那是 005 交付的 mock 脚手架，接在 `engine::step` 上，027 换接 runner 时随之迁更合适 |
| `harness_provider_scripts.rs`（2 条） | `session_provider_failed.rs::non_retryable_classes_…`（错误脚本）+ `session_provider_done.rs::max_tokens_finishes_the_turn_truncated`（截断脚本） |

**新增的（M1 没有对应物，是 026 长出来的能力）**：
`session_history.rs` 5 条（一轮 = 几条 entry、违规不留幽灵步、一条转移的
`prev`/`next` 逐项吻合、derived 重算不产生 entry、会话级命令同样留痕）、
`session_undo_redo.rs` 7 条（**undo 一整 turn 后所有 primitive 逐值回退 + derived
重算**、redo 反演、连续跨 turn、屏障 `Blocked`、`undo_turn_force` 只越一条、
写入覆盖 redo 尾并报 `DropEvent`、applier 逐条写回 `prev`）、
`session_epoch_gate.rs` 3 条（undo 后旧 epoch 回执被丢弃的端到端、redo 不 bump、
用户意图不过闸）、`session_state.rs` 2 条（完整状态恰好是九个 source 槽位、
读口给的是克隆不是句柄）、`tests/atom_value.rs` 6 条（十变体封闭、两条指针快路、
serde 往返）。

### 收工数字

- `cargo test -p agent-core`：**271 passed / 0 failed**（M1 的 193 条一字未改照绿，
  026 新增 78 条 = 13 个 `session_*.rs` 72 条 + `tests/atom_value.rs` 6 条）
- `cargo test --workspace`：**701 passed / 0 failed**（010/011 在同一个工作树并行落地，
  总数是移动靶；收工前测过两轮，中途因 `agent-store/src/persist/` 半成品瞬时红过一次，
  最终快照全绿）
- `cargo clippy --workspace --all-targets -- -D warnings`：0 警告
- `bash scripts/check-invariants.sh --all`：红线检查通过（exit 0）
- 行数：新增源文件最大 `command/txn.rs` 264，全部 ≤300；改动到的存量文件
  `engine/state.rs` 247、`limits.rs` 123、`lib.rs` 52、`ids.rs` 90，全部 ≤300；
  新增测试文件最大 `session_undo_redo.rs` 218（`tests/` 本就豁免，仍留在 300 以内）

### 验收对照

| issue 验收 | 谁保证 |
|---|---|
| M1 全部引擎行为在新形态下照绿 | 上面的对照表，13 个 `session_*.rs` 共 72 条 |
| 一轮完整对话后 `history.len()` == batch 数，`prev`/`next` 与转移语义吻合 | `session_history.rs::one_full_turn_leaves_exactly_one_entry_per_transition_that_changed_something` + `…the_changes_of_a_single_transition_match_the_transition_semantics` + `session_flow.rs` 的五条 label 断言 |
| **undo 一整 turn 后所有 primitive 逐值回退、所有 derived 重算一致**，redo 反演 | `session_undo_redo.rs::undoing_a_whole_turn_restores_every_primitive_and_recomputes_every_derived` / `…redo_turn_is_the_exact_inverse_of_undo_turn` |
| undo 后旧 epoch 的 `ToolResult` 被丢弃 | `session_epoch_gate.rs::a_tool_result_from_before_an_undo_is_dropped_but_the_same_one_lands_after_a_rewrite` |
| `barrier=true` 的 entry 让 `undo_turn` 返回 `Blocked` | `session_undo_redo.rs::a_barrier_entry_blocks_undo_instead_of_silently_rolling_it_back` / `…undo_turn_force_crosses_exactly_one_barrier` |
| command/ 外无裸 `store.set` | `scripts/check-invariants.sh --all` 通过；`Session` 不暴露 store，业务代码物理上拿不到 |

### 推给别人的

- **027**：`Session::mark_irreversible(call_id)` 要由 CLI 的工具表在派发时调用，
  `shell/exec` 的 `Irreversible` 才接得上屏障；`UndoReport::Blocked { barrier_seq }`
  就是 `undo_blocked` 要打印的全部素材；`/undo!` = `undo_turn_force`。
  换接时把 `agent_cli::next_turn` 换成 `Session::begin_turn`——那份「取消时丢弃本轮
  消息」的策略要重新决定要不要留（M2 的正牌答案是 `undo_turn`）。
  `support/harness/` 那套 mock 脚手架接在 `engine::step` 上，随 runner 一起迁。
- **010/011**：`Session::primitives()` 就是 `Snapshot` 的形状（`Vec<(AtomKey,
  AgentValue)>`，按键排序）；`Session::history()` 给整份日志；`take_drop_events()`
  给 `drop_oldest`/`drop_after` 的素材。`EntryMeta` 不可反序列化（`label` 是
  `&'static str`），落盘 schema 要自己定一份 `String` 版。恢复后的 `epoch` 从日志里
  取最大值继续发，`turn_id` 同理。
- **M3**：`AtomKey::ToolCall(_, _, Result)` 与 `Text`/`Json`/`Pending` 三个变体已经
  定好但 M2 没写入点，子 agent 的 per-call 汇聚落在那里；`Slot` 还差 `config` /
  `system_base` / `skills_active` / `tools_registry_version` 四个（有真实写入点时再加，
  旧快照缺键用默认值，不需要迁移）。逐出策略必须状态驱动、自叶向根（019）。

### 合并记录（主会话）

双侧零分歧：独测 26 测试（rustdoc + scratchpad 探针项目取行为，未读实现）一次全绿，
agent-core 297/0。五项上报裁决全收：undo_step/redo_step 超纲采纳（决策 5 两层粒度
+ 003 回滚锚点）；AtomKey/AgentValue 三个暂无写入点的变体作为「零空壳」原则的
有据例外采纳（落盘 schema 封闭性优先，per-call atom 会造两个真值来源）；
mark_irreversible 不落日志正确（barrier 位经 EntryMeta 已入日志，恢复不丢屏障）；
取消轮丢弃策略移交 027 显式定；检查脚本 graph/ 路径扩展合规。
「两路并存」到 027 收口：等价对照表就是退役清单。
