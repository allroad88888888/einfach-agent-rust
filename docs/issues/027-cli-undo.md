# 027 CLI 长出 /undo /redo，会话可恢复

**里程碑** M2 · **依赖** 026 + 011 · **模型** sonnet · **独立测试 agent** ✅ · **状态** 完成

## 目标

M2 验收的用户可见面：CLI 里 `/undo` 退回上一轮且一切一致；undo 越过
`shell/exec` 停下来问；杀进程重启会话还在。

## 做什么

1. **runner/CLI 换接 026 的 `Session`**（原 TurnState 驱动退役）；`FsExecutor`
   顺手改名（020 推迟的账——它早已不只管 fs）
2. `/undo` `/redo` 命令：turn 粒度（决策 5 的默认档），打印回退了哪一轮、
   多少条目
3. **屏障接通**：`shell_spec()` 进 CLI 工具表（020 攒的开关此刻才许打开——
   屏障 UI 齐了）；undo 撞上 `barrier=true` → 打印 `undo_blocked`（哪一步、
   什么工具、为什么不能自动越过）+ 明确的确认指引（M2 先做「换谓词重调」的
   显式二次命令，如 `/undo!`；对话式确认是 M3 的 UI 事）
4. **持久化接通**（011 的端口在这里上岗）：每条 Entry fire-and-forget 进
   `SessionStore`（Jsonl）；`DropEvent` 转发 `drop_oldest`/`drop_after`；
   快照按策略落（每 N turn 一张，N 可配默认 10）
5. **崩溃恢复**：启动时发现会话文件 → 载最近快照 + `apply_next` 重放（010 已
   钉「恢复=redo」）→ 继续对话。在飞工具的中断语义（`Interrupted{may_have_executed}`，
   020 推迟的账）：恢复时发现未收敛的 `ToolSlot` → **不自动重发**，
   打印「这个操作可能已经执行过」并把该 turn 标记为需要用户裁决

## 验收（M2 验收原文逐条）

- CLI 十轮对话后 `/undo`：上一轮消失、派生值一致、下一轮 prompt 不含被退内容
  （缓存兜底第 1 层顺带验证前缀回退正确）
- `/undo` 越过一次 `shell/exec` → 停下推 `undo_blocked`，`/undo!` 才越过
- `kill -9` 进程 → 重启 → 会话还在、能接着聊、undo 栈还能用
- 恢复时在飞的 `shell/exec` 槽 → 「可能已执行」提示，不重发
- Jsonl 文件损坏（截断最后一行）→ 明确报错指出哪里坏了，不 panic 不静默丢

## 注意

红线 6/8；`/undo!` 的措辞要让人明白自己在确认什么（越过的是哪个不可逆操作）。
崩溃恢复的时序测试用 011 的 `Memory` 后端 + 手工构造「重启」，真 kill 的
端到端由主会话真跑验收。

## 实做记录（实现 agent，2026-08-02）

### 落地的文件

**新增**：

| 文件 | 行数 | 职责 |
|------|------|------|
| `agent-core/src/command/restore.rs` | 236 | `Session::restore`：崩溃恢复重建会话（构图 + 灌快照 + `apply_next` 推进到游标） |
| `agent-runtime/src/persist/mod.rs` | 30 | 持久化接线层门面 + `SessionBackend` 类型别名 |
| `agent-runtime/src/persist/meta.rs` | 79 | `PersistedMeta`：`EntryMeta` 的可落盘姊妹类型 + 双向转换 |
| `agent-runtime/src/persist/sync.rs` | 164 | 每条命令之后把 `Session` 变化转发进 `SessionStore` |
| `agent-runtime/src/persist/snapshot.rs` | 108 | 快照节奏：每 N turn 一张 |
| `agent-runtime/src/persist/recover.rs` | 171 | 崩溃恢复：`SessionStore::load()` → 翻译 → `Session::restore` + `has_unresolved_tool_calls` |
| `agent-runtime/src/persist/backend.rs` | 56 | 挑后端：有路径 `Jsonl`，没有 `Memory` |
| `agent-cli/src/undo.rs` | 177 | `/undo` `/redo` `/undo!` + 取消轮自动策略 + `describe_barrier` |
| `agent-cli/src/session_path.rs` | 52 | `--session <path>` / `AGENT_SESSION_PATH` 解析 |

**改动到的存量文件**：`agent-runtime/src/{ctx,runner,provider_call,tool_exec,tool_table,lib}.rs`（Session 换接 + 工具表 `with_shell()` + 屏障派发）、`agent-core/src/{lib,command/mod,command/meta,command/txn,command/session}.rs`（`known_label` + `Session::clear_prev_prefix` + `AgentEntry` 根导出）、`agent-cli/src/{lib,main,repl,model_switch,print}.rs`（Session 换接、恢复流程、`/undo` 系命令）、`agent-tools/src/lib.rs`（`FsExecutor` → `ToolExecutor` 改名，全仓引用同步）。

**退役（删除）**：`agent-core/src/engine/transitions.rs` + `engine/transitions/`（7 文件）、`agent-core/src/engine/state.rs` 里的 `TurnState`（类型本体，`TurnStatus`/`Failure`/`ToolSlot`/`SlotState` 保留）、`agent-core/src/engine/mod.rs` 里的 `step` 与内联 epoch 闸测试、`agent-cli/src/turn.rs`（`next_turn` 整个函数，022 时代「截断消息列表」手法）、`agent-core/tests/support/harness.rs` + `harness/`（005 的 mock 脚手架，唯一消费者一起退役）、19 个 M1 专属测试文件（见下「退役清单」）。

### 策略落地情况

1. **取消轮**：`agent_cli::undo::after_cancelled_turn` 调 `Session::undo_turn()`（非 force）。`Applied` → 打印擦除了几条；`Blocked`（本轮已执行不可逆工具）→ 保留该轮 + 打印说明，不擅自越过。`agent-cli/tests/cancel_after_shell_kept.rs` 端到端钉住后半句（真的跑一次 `srv:shell/exec` 再取消）。
2. **屏障恢复**：`Session::restore` 不恢复 `irreversible` 运行时列表（`Vec::new()`）——`barrier` 位随 `EntryMeta` 落盘，`undo_turn` 读的是日志本身。`agent-core/src/command/restore.rs` 的测试 + `agent-runtime/tests/shell_exec_undo_barrier.rs` 确认屏障恢复后仍然生效。
3. **快照节奏**：默认每 10 turn 一张（`DEFAULT_SNAPSHOT_EVERY`，`RunnerCtx::with_snapshot_every` 可覆盖），在 `run_turn` 收尾（终态或卡住）时检查。载入后 `Session::restore` 的 `history_cap` 参数重调日志上限（默认 `agent_core::DEFAULT_HISTORY_CAP` = 100）。

### 恢复的具体形状

`Session::restore(agent, snapshot, entries, cursor, next_seq, history_cap, on_unknown_key)`：

1. `History::from_parts(entries, cursor, next_seq)` 校验落盘三元组的不变量，失败原样返回 `InvalidHistory`（不硬凑）。
2. 全新构图（`build_agent`，跟 `Session::new` 同一条路）。
3. 有快照就 `agent_store::restore`（非创建 `resolve`，见 010 判断 5）灌回 primitive；`snapshot` 为 `None`（从没落过快照）就跳过，图已经是构图函数给的默认值。
4. **只把 `[0, cursor)` 这一段 entries 走 `apply_next` 写回 store**——`[cursor, len)` 是被 undo 掉还没 redo 回来的尾巴，写回去等于替用户悄悄撤销一次他做过的 undo；`History` 本身仍然完整持有这一段，`redo_turn` 能找回来（`entries_past_the_cursor_are_not_replayed_but_stay_redoable` 测试钉住）。
5. `turn_id` 取 entries 里出现过的最大值（没有就退回 1，跟 `Session::new` 一致）；`epoch` 取「见过的最大值 + 1」——**不是精确复原崩溃前的真实世代**，undo/redo 本身会 bump 世代但不产生 entry，那一下的凭证随进程一起没了。这个近似是安全的：世代号唯一的作用是拦「在飞 effect 的过期回执」（红线 6），进程重启之后不可能还有旧进程的在飞 effect，选哪个值都不影响正确性。

`agent-runtime::persist::recover` 是接进 `SessionStore` 的那一层：翻译 `PersistedMeta → EntryMeta`（`agent_core::known_label` 是那张对照表，label 是有限编译期常量集，不认识的字符串报 `UnknownLabel` 拒绝加载，不编一个假标签）、`Snapshot.values` 直接透传、失败原样交给宿主。`agent_runtime::has_unresolved_tool_calls(&session)` 是宿主判断「有没有一个工具调用发出去了、结果还没落地」的入口——`main.rs` 恢复成功后调它，命中就打印「可能已经执行过」，**不自动重发**（020 推迟的账在这里兑现：不是新状态变体，是宿主看到 `ToolsPending` 且未收敛就知道不能揣着 `ToolCallRequest` 假装没发生过）。

### 一个真 bug：`SessionStore::drop_after` 的转发顺序

011 实做记录写的调用契约是「append + set_cursor 之后再转发裁剪事件」。**这条对 `DropEvent::RedoTail` 是错的，会真的丢数据**——写 `persist::sync` 的回归测试
（`overwriting_a_redo_tail_does_not_resend_seqs_already_told_to_the_store`）时当场炸出来：`SessionLog::record_drop_after(first_seq, _)` 的实现是 `held.retain(|e| e.seq < first_seq)`，一个绝对阈值；而新写入的 entry 的 seq **必然大于** `first_seq`（seq 只增不减，新 entry 是在截断 redo 尾之后才铸的号）。如果先把新 entry append 进 store 再转发这条 `RedoTail`，`retain` 会把刚追加的新 entry 一并冲掉。

已用 `agent-store/tests/session_log_replay.rs::drop_after_only_trims_the_tail_and_leaves_the_front_offset_untouched`（`record_drop_after` 在 `record_append` **之前**调用）反向验证：那才是唯一对的顺序。`agent_runtime::persist::sync` 因此按事件种类分两段转发：`RedoTail` 在追加新条目**之前**、`Oldest`（cap 驱逐）在**之后**——后者没有这个问题，`History::enforce_cap` 本身就在一次 `append` 完成之后才跑，新条目已经在 `entries` 里、可能连它自己都在驱逐范围内，先 append 再转发 `Oldest` 才是跟 `History` 内部顺序一致的那一个。两类事件对顺序的要求正好相反，只有拆开转发才都对。这条没有改 `agent-store`（不在改动范围内），完全在 `agent-runtime` 这一层通过转发顺序解决。

### 退役清单执行情况

026 等价重写对照表里标注「随 runner 一起迁」的项目全部落地：

- `agent-core/src/engine/transitions.rs` + `engine/transitions/*.rs`（7 文件，`cancel`/`provider_done`/`provider_failed`/`timeout`/`tool_outcome`/`user_input` + 门面）：删除，转移语义唯一住在 `command::transitions`。
- `engine::step`、`engine/mod.rs` 内联的 3 条 epoch 闸测试：删除，闸唯一住在 `Session::step`。
- `TurnState`（类型本体 + `new`/`push_message`/`record_turn_attempt`/`record_retry_attempt`/`mint_message_id`/`tools_converged`/`bump_epoch`）：删除；`TurnStatus`/`Failure`/`ToolSlot`/`SlotState`/`DEFAULT_MAX_TURNS`/`DEFAULT_MAX_RETRIES` 保留（`Session` 的接缝词汇）。
- 19 个 M1 专属测试文件删除（`turn_transitions_grid.rs`、`turn_state.rs`、`tools_converged.rs`、`tool_outcome_convergence.rs`、`tool_convergence_{all_failed,duplicate_call_id,error_reaches_prompt,scan_not_counter}.rs`、`provider_done_stop_reason.rs`、`provider_error_classification.rs`、`timeout_transitions.rs`、`cancel_any_state.rs`、`max_turns.rs`、`epoch_gate.rs`、`harness_{happy_path,tool_reorder,cancel_in_flight,timeout_injection,provider_scripts}.rs`）——每一条在 026 对照表里都有 `session_*.rs` 的等价物接手。`turn_state.rs::terminal_statuses_are_exactly_done_and_failed` 唯一该留的一条，发现跟 `turn_status_terminal.rs` 现有断言完全重复，未新建文件。
- **原地不动**（026 对照表明确标注，本次一字未改）：`turn_status_terminal.rs`、`event_epoch_extractor.rs`、`call_provider_thin.rs`、`serde_roundtrip.rs`、`no_clock_meta_test.rs`（扫描路径已经在 026 时就扩到 `engine/`+`command/`+`graph/`，本次删 `engine/transitions/` 不需要跟着改）。
- `agent-core/tests/support/harness.rs` + `support/harness/`（`MockProvider`/`MockExecutor`/`Harness`，005 的脚手架）：确认无任何存活测试文件引用 `harness::`（`grep -l "harness::"` 只命中被删的 5 个 `harness_*.rs`）后删除；`support/mod.rs` 里 `TurnState` 导入与 `turn_state()` 构造函数一并删除，`session` 子模块与顶层事件构造函数（`session_*.rs` 仍在用）保留。
- `agent-cli/src/turn.rs`：删除，`Session::begin_turn` 接过「一轮到下一轮」，`undo::after_cancelled_turn` 接过「取消轮怎么处理」。

`agent_cli::model_switch::switch` 原来直接赋值 `state.prev_prefix = None`（`TurnState` 是裸字段结构，合法）；`Session` 字段全私有，红线 2 逼着新增一个会话命令 `Session::clear_prev_prefix()`（`agent-core/src/command/{txn,session}.rs`，新 label `clear_prev_prefix` 已登记进 `known_label` 的封闭集合）。

### 验收对照

| issue 验收 | 谁保证 |
|---|---|
| CLI 十轮对话后 `/undo`：上一轮消失、派生值一致、下一轮 prompt 不含被退内容 | `agent-runtime/tests/undo_after_turns.rs`（两轮，含一次工具调用；断言 `primitives()` 逐值相等 + 真实 `encode()` 的 body 字节不含被退内容） |
| `/undo` 越过一次 `shell/exec` → 停下推 `undo_blocked`，`/undo!` 才越过 | `agent-runtime/tests/shell_exec_undo_barrier.rs`（Session 层机制，真实执行 shell）+ `agent-cli/tests/shell_undo_flow.rs`（走 `agent_cli::undo::undo`/`undo_force` 真正的公开函数）+ `agent-cli/src/undo.rs` 内联单测（`describe_barrier` 抠工具名/call_id） |
| `kill -9` 进程 → 重启 → 会话还在、能接着聊、undo 栈还能用 | `agent-runtime/tests/jsonl_restart_continues.rs`（真 `Jsonl` 文件，整个后端 drop 再重开，`recover` 载回 + `undo_turn` + 接着聊）；真 `kill -9` 端到端留给主会话 |
| 恢复时在飞的 `shell/exec` 槽 → 「可能已执行」提示，不重发 | `agent-runtime/src/persist/recover.rs::unresolved_tool_calls_are_detected` + `main.rs` 恢复流程里的 `has_unresolved_tool_calls` 分支 |
| Jsonl 文件损坏（截断最后一行）→ 明确报错指出哪里坏了，不 panic 不静默丢 | 011 的 `session_store_jsonl_corrupt_files.rs` 三条原样保留（未改动）；`main.rs` 把 `Jsonl` 的 `on_error` 接到 `eprintln!`，`recover()` 的 `RecoverError`（语义层面的翻译/重建失败）导致进程 `fail()` 退出，不吞、不猜 |
| 取消轮自动擦除；含已执行 shell 的取消轮保留+说明 | `agent-cli/tests/cancel_flow.rs`（干净取消）+ `agent-cli/tests/cancel_after_shell_kept.rs`（含已执行 shell，保留） |

### 命令输出（逐条贴原文）

```
$ cargo test --workspace
（680 passed; 0 failed；跨 7 个 crate，125 个测试二进制/文档测试组）

$ cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.85s
（0 警告）

$ bash scripts/check-invariants.sh --all
红线检查通过
规则与理由：docs/INVARIANTS.md

$ cargo build --workspace 2>&1 | grep -i "dead_code\|never used\|never read\|never constructed"
（无匹配，无 dead_code 告警）
```

按 crate 拆分（`cargo test -p <crate>` 单独跑，`passed` 求和）：`agent-core` 229 / `agent-runtime` 34（含本次新增 3 个集成测试文件）/ `agent-cli` 11（含本次新增 3 个集成测试文件）/ 其余（`agent-providers`/`agent-store`/`agent-tools`/`agent-transport`，本次未改动 src）合计 406，总计与 `--workspace` 的 680 一致。

行数（`wc -l`，全部 ≤300；`agent-runtime`/`agent-cli` 的 `src/` 最大文件分别是 `ctx.rs` 219 与 `print.rs` 266）：见上表逐文件行数，另附全仓扫描「任何非 tests/benches 的 `.rs` 文件超过 300 行」——零命中。

### 异议

- 无实质异议。上面的 `SessionStore::drop_after` 转发顺序是本次实现时发现并在 `agent-runtime` 这一层修正的问题，未触碰 `agent-store`；如果主会话认为这条也该回填进 011 的实做记录，我没有权限改那份文件（任务边界里 011 已经是「完成」状态），留给主会话裁决要不要补一笔。
- `main.rs` 对 `RecoverError`（标签不认识 / `History` 不变量违反）选择的是**直接 `fail()` 退出**，不是「退化成 Memory 新会话」——理由是这两种错误意味着「有真实数据但这一版代码没法安全重建」，静默退化等于在用户没同意的情况下让下一次持久化写入把旧数据压掉。如果这个选择偏保守，是刻意的，跟 011「诚实优于整洁」的取向一致。

### 契约更正（独立测试 agent 发现两个真 bug，2026-08-02）

独测 agent 用真 `Jsonl` 跑「连续重启周期」与「中部损坏会话文件」两条场景，抓到
上面「异议」第二条自己都没料到的后果——`fail()` 出口选对了，但**到达那个出口之前
的两段管道各自埋了一个会绕开它的真 bug**。两条都已修复，回归测试红转绿证据、
命令输出见交付报告；这里只记结论与来由，细节分别在 011 的「契约更正」两条里
（`Jsonl`/`LoadOutcome` 是 011 的端口与实现，改动性质上属于那份文档，这里只做
索引，不重复内容）。

1. **会话文件永久搞坏（`RunnerCtx::persisted_seq` 未在恢复路径初始化）**：
   `persist::sync` 判断「这个 seq 有没有告诉过 store」的水位在 `RunnerCtx::new`
   里恒为 `None`——`persist::recover` 把整段历史读回 `Session`，但没人告诉新
   `RunnerCtx` 这些 entries 已经在盘上，下一次 `sync` 把它们当新条目重新
   append，连续几次重启后 seq 在文件中段跌回 0，`History::from_parts` 撞
   `SeqNotIncreasing` 硬失败，会话搁浅。修法：新增
   `agent_runtime::persist::seed_after_recover(&mut ctx, &session)`
   （`agent-runtime/src/persist/sync.rs`），`main.rs` 在 `RunnerCtx::new` 之后
   无条件调用一次（对全新会话是无害空操作）。
   独测过程中又带出**第二个、更隐蔽的同族 bug**：`agent-runtime/src/jsonl/
   io_thread.rs` 里连续存活的 `mirror: SessionLog` 每次「重启」都从
   `SessionLog::new()` 起步，对文件里已有的、未经快照压实的内容一无所知——
   落盘的 `cursor` 因此被系统性算小，下一次重启 `recover()` 读到一个
   `cursor < entries.len()` 的会话（明明没 undo 过），它自己的下一次写入被
   `History` 当成"覆盖 redo 尾"，上一个周期真实写过的整轮对话被一条
   `drop_after` **悄悄冲掉**——不 panic、不报错，比 seq 撞硬失败更隐蔽。两个
   周期的既有测试 `jsonl_restart_continues.rs` 测不到（它只重启一次就结束，
   没有第三次读盘验证第二轮数据在不在），独测的验收原文明确要求「三个周期」
   才现形。修法：`jsonl/load.rs` 拆出 `seed_from_disk`，`io_thread::run()`
   起步用它把 `mirror` 追平到文件已有内容。这条严格说超出了原始 bug 报告点名
   的机制（`persisted_seq`），但同属"连续重启周期永久搞坏会话文件"这个标题下
   ——不修就没法让"三个周期"的回归测试真正通得住，没有单独拆成第三个 bug 报。
   回归测试：`agent-runtime/tests/jsonl_three_restart_cycles_keep_seq_increasing.rs`
   （真 `Jsonl`，三个周期，seq 严格递增且互不重复 + 第三次恢复的
   `messages().len()` 一条不少）。

2. **中部损坏被当成"没有会话"（`SessionStore::load()` 的 `Option` 身兼两职）**：
   见 011「契约更正」条目——`load()` 三态化为 `LoadOutcome`，`recover()` 新增
   `RecoverError::Refused`，`main.rs` 既有的 `Err(e) => fail(...)` 出口不用改
   一行就自动接住。`agent-cli/tests/indep_corrupt_session.rs` 的中部损坏测试
   （`a_broken_middle_line_...`）原来断言的是"开新会话、退出码 0"——那是当时的
   真 bug 行为，不是验收原文要的"启动报错"；按新语义改成断言"非零退出码 + 原
   文件字节不变"，模块顶部文档记了这条语义修正的来由（不是掩盖，是订正）。

### M2 终局验收记录（主会话真跑，2026-08-02，deepseek-v4-pro）

三阶段脚本：对话+shell+屏障+kill-9 → 恢复续聊+undo+再 kill-9 → 第二次恢复+干净退出。

| M2 验收条 | 实录 |
|---|---|
| undo 退回上一轮、派生值一致 | `/undo!` 撤掉 shell 轮后，恢复的会话里问「聊过什么」，模型只答熵与 1+1——**被撤的轮在模型记忆里不存在**，prompt 级真回滚 |
| 越过 Irreversible 停下问 | `[撤销受阻] …srv:shell/exec(call_id)…输入 /undo!` → `/undo!` → `[已越过]…副作用不会被回滚` |
| 杀进程重启会话还在 | **连续两次** kill -9，两次 `[会话已恢复] 接着第 N 轮继续`，undo 栈跨重启可用，第三进程干净退出 |
| 恢复不重发在飞工具 | 独测 SIGKILL 于 ToolsPending + 断言服务器零重发（indep_unresolved_tool_recovery） |

全程缓存对账逐轮一致（一次「好于预期，不是问题」按 024 文案如实播报）。

### 收官前的独测战果（本 issue 的完整弧线）

实现自测 → 独测黑盒推到第二个重启周期 → 挖出**三颗同族雷**（sync 水位未播种 /
io_thread 镜像未播种 / load 三态混同）→ 修复带红转绿证据 → 真验收按「两周期」跑。
一个重启周期的绿测试会让这三颗雷全部带进 M3。
