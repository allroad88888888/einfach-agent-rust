# 153 `TimedRun` 加只读 `&Session`，ext:stats 删传话格 ← M16 终点

**里程碑** M16 · **依赖** [150](150-derived-extension-decision.md)（已拍板 = 决策 30） · **模型** sonnet · **独测** — · **状态** 完成（见文末，2026-08-12）

## 目标

决策 30 的唯一实现刀：`TimedRun` 签名加**只读** `&Session`——两个时机驱动
（`run_session_start` / `turn_end::fire`）手里本来就攥着 session，递进去而已。
149 里 ext:stats 被迫发明的内存传话格（`Ledger` + `seen_at` 标注）随之整个删除：
审计行改为轮末**现读**稳定态。

## 做什么

1. `TimedRun` 签名：`Fn(&ToolTable, &Session, &Value) -> Result<Arc<str>, Arc<str>>`
   （`&Session` 放中间，紧挨它要读的世界；只读——**谁要 `&mut` 谁去写截获工具**，
   类型即边界，v1 的「hook 不写状态」从纪律变成签名）。
2. 两个驱动递 session；`turn_end::fire` 的调用点签名跟上。
3. 所有既有 timed 执行体跟签名：skills 索引闭包（`with_skills`，参数忽略即可）、
   ext:stats 的 hook、`ExtensionPack::with_timed` 的文档与测试 fakes、各独测里的
   fake timed 工具（146/148/145 的独测文件里签名要改——**只改签名不改断言**，
   属于公开类型演进的机械跟随，报告里逐个列出）。
4. ext:stats：删 `Ledger` 传话格与 `seen_at`；审计行改为
   `turn=N entries=X/Y agents=Z tools=W`（数字来自轮末现读）；相应单测改写；
   `docs/EXTENSIONS.md` §五 教材同步（传话格那段整段删除，换成一句
   「hook 拿只读 Session 现读」）。
5. 149 实做记录里的审计样例**不改**（那是当时的真机事实）；本条实做记录注明
   格式自此变化。

## 验收

- 全部既有 timed 相关测试改签名后全绿；skills 索引闭包行为**逐字节不变**。
- ext:stats 审计行含轮末实读数字（新单测：跑两个完成轮，第二行的 entries
  等于当时 `history_len`）。
- hook 无法写状态：`&Session` 不可变，编译即证明（不需要测试）。
- `cargo test --workspace` 全绿 + `check-invariants` 过 + `build-wasm` 绿
  （TimedRun 是公开类型，wasm 侧编译要验）。

## 注意

- 公开类型签名变更（红线 11 不涉——签名不进 prompt），但独测文件要动：
  这是「公开类型演进的机械跟随」，不是改断言——diff 里独测文件只许出现
  闭包参数列表的变化。
- 别顺手给 SessionStart 的执行体开写口或加别的参数——决策 30 只批了只读这一刀。

## 实做记录（2026-08-12）

**最终签名**（`crates/agent-runtime/src/tool_table_timed.rs`）：

```rust
pub type TimedRun =
    Box<dyn Fn(&ToolTable, &Session, &Value) -> Result<Arc<str>, Arc<str>> + Send + Sync>;
impl TimedTool {
    pub fn run(&self, table: &ToolTable, session: &Session, input: &Value)
        -> Result<Arc<str>, Arc<str>>;
}
```

`&Session` 插在 `&ToolTable` 与 `&Value` 之间（issue 原文要的位置），只读——没有任何调用点
开 `&mut`，编译器本身就是证据。

**两个驱动**：`run_session_start`（`session_start.rs`）本来就收 `session: &mut Session`，
改动只是把它（隐式 reborrow 成 `&Session`）递进 `entry.run(tools, session, ..)`，函数自己的
签名一个字没动。`turn_end::fire`（`turn_end.rs`）原本只收 `&RunnerCtx`（`RunnerCtx` 不持有
`Session`），按 issue 原文改成 `fire(ctx: &RunnerCtx, session: &Session)`；唯一调用点在
`runner.rs` 的 B 分支（轮末收工判定 `TurnStatus::Done` 之后），那里 `session: &mut Session`
本来就在手边，改成 `turn_end::fire(ctx, session)` 一行，靠 `&mut T → &T` 的内置引用强制
转换，不需要显式 reborrow 写法。

**ext:stats 重写**（`crates/agent-cli/src/ext_stats.rs` + `ext_stats_tests.rs`）：
`Ledger` 删掉 `seen: Mutex<Option<Seen>>` 字段与 `Seen` 结构、`observe()`/`seen()` 方法，
只剩 `turns: AtomicU64` + `audit: Option<PathBuf>`。`report_run()` 不再收 `Ledger`（它现在
连仅剩的那一处「记一笔」副作用都没有了，`report` 因此比 149 时更纯）。`audit_run()` 的闭包
直接用轮末递来的 `&Session`，`append_turn_line(&self, session: &Session)` 现读一次账本
（复用 `ext_stats_report::count`，改成 `pub(crate)` 给同 crate 的 `ext_stats.rs` 调，
`agents` 传的是 `session.agent_tree().nodes.len()`——这条钩子没有「调用者」，不按红线 10
narrow）。审计行格式：

```
149 之前：turn=N entries=X/Y turns=T agents=Z tools=W seen_at=turnK（或全 `-`）
153 之后：turn=N entries=X/Y agents=Z tools=W
```

丢的两个字段：`turns=`（生效段里出现过几个不同 turn_id，`Counts` 结构体仍保留这个字段给
`report` 正文用，只是审计行不再印它——issue 原文给的格式本来就没有它）与 `seen_at=`（连同
它想回答的问题「这份数字是哪一轮观测的」一起作废——现在**每一行都是现读的**，不存在
「没被观测过」这个状态）。单测新增
`the_second_audit_line_reports_the_history_len_at_that_moment`：两次触发 hook 之间用
`session.set_max_turns(7)` 造一条真实 command log entry，断言第二行的 `entries` 字段
（`X/Y` 的物理条数 `Y`）等于**那一刻**的 `session.history_len()`——不是第一行的老数字，
证明 audit 真的在现读而不是复用什么缓存。

**docs/EXTENSIONS.md §五**：「今天的一条硬边界：`TurnEnd` 钩子看不见 `Session`」整节
（含 `seen_at` 示例块）删掉，换成「153（决策 30）：`TurnEnd` 钩子拿只读 `Session` 现读」，
带新格式的审计行示例；§七落地状态表 150/153 各记一行。

**全仓机械跟随**（只改闭包参数列表 / `.run()` 调用点，不改任何断言）：

| 文件 | 改动 |
|---|---|
| `crates/agent-runtime/src/tool_table_skill.rs` | `with_skills` 的 index 执行体闭包加 `_session` |
| `crates/agent-runtime/src/tool_table_timed_tests.rs` | `echo_run` 闭包 + 一处 `Box::new` 内联闭包 + `.run()` 调用点 |
| `crates/agent-runtime/src/tool_table_skill_assembly_tests.rs` | 一处 `.run()` 调用点补 `Session` |
| `crates/agent-runtime/src/spawn_tool_tests.rs` | `empty_session_start_run` 闭包 |
| `crates/agent-runtime/src/extension_pack_tests.rs` | `nop_timed` 闭包 |
| `crates/agent-runtime/src/tool_table_extension_fixtures.rs` | `recording_hook` 闭包 |
| `crates/agent-runtime/src/tool_table_extension_tests.rs` | 两处 `turn_end::fire()` 调用补 `&Session` |
| `crates/agent-runtime/src/tool_table_extension_guard_tests.rs` | 一处 `turn_end::fire()` 调用补 `&Session`，补 `Session`/`AgentId` 引入 |
| `crates/agent-runtime/src/subagent_tests.rs` | 两处 index 闭包 |
| `crates/agent-runtime/src/session_start.rs` | `echo_run`/`empty_run`/`fail_run` 三个测试闭包 |
| `crates/agent-runtime/src/turn_end.rs` | `recording_run`/`recording_fail_run` 闭包 + 三处 `fire()` 调用补 `&Session` |
| `crates/agent-runtime/tests/it/call_timing_indep.rs` | `ok_run`/`ok_run`(验收4内联)/`err_run` 闭包 + 三处 `.run()` 调用点 + 头部签名注释同步 |
| `crates/agent-runtime/tests/it/turn_end_indep.rs` | 七处内联闭包（`run`/`run_a`/`run_b`） |
| `crates/agent-runtime/tests/it/session_start_indep.rs` | `ok_text`/`err_text`/`counting_ok` 闭包 |
| `crates/agent-runtime/tests/it/session_start_prompt_indep.rs` | `ok_text` 闭包 + 一处内联闭包 |
| `crates/agent-runtime/tests/it/inherit_prefix_indep.rs` | `ok_text` 闭包 |
| `crates/agent-runtime/tests/it/inherit_prefix_restore_indep.rs` | `counting_ok` 闭包 |
| `crates/agent-runtime/tests/it/inherit_prefix_rejects_indep.rs` | `ok_text` 闭包 |
| `crates/agent-runtime/tests/it/extension_pack_indep.rs` | `counting_turn_end_run` 闭包 |

**验收**：`cargo test --workspace` 全绿（agent-runtime 269+185、agent-cli 45+12，其余各包
不变，全仓零失败）；`scripts/check-invariants.sh --all` 退出码 0（`runner.rs`/
`extension_pack_indep.rs`/`turn_end_indep.rs` 三个文件本条只做了机械跟随的最小改动，
行数警告是存量事实——改动前就超限，未新增超限文件，照 CLAUDE.md「存量小改指出不重构」
处理）；`scripts/build-wasm.sh --dev` 编译通过（`TimedRun` 是公开类型，wasm32 目标验证过）。

**没做的**（有意，issue 原文也没批）：没有给 `SessionStart` 或 `TurnEnd` 开写口、没有加
第三个/第四个参数、没有动 133 的迭代顺序或 136 的失败处置语义——这条 issue 只批了
「签名加一个只读引用」这一刀。
