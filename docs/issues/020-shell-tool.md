# 020 `shell/exec` 工具

**里程碑** M2 · **依赖** 017 · **模型** sonnet · **独立测试 agent** ✅ · **状态** 完成

## 目标

第一个 `Reversibility::Irreversible` 的工具。

## 为什么等到 M2

它能删东西。M1 没有 undo 屏障挡不住它——写了也只能默认关着，那等于在仓库里留一段
从没跑过的代码。等 [017](017-undo-redo.md) 的 undo 能在 `Irreversible` 上停下问用户，
它才有安全网。

## 做什么

`srv:shell/exec`，`Location::Server`，`Reversibility::Irreversible`。

`Irreversible` 不是保守取值而是准确取值——**拿不准才填 `Irreversible`，而 shell 拿得准**。

约束：

- 超时（默认 30s，可配）
- 工作目录锁在仓库内，**不能跑到外面**
- 输出走 004 定的截断上限

## 验收

- undo 越过一次 `shell/exec` 时**停下并推 `undo_blocked`**，不静默回滚
- 崩溃恢复时它被标记为 `Interrupted { may_have_executed: true }`，**不自动重发**
- 超时能中断子进程，不留孤儿
- 工作目录越界被拒绝

## 注意

前两条验收才是这个 issue 的真正目的——它是 `Reversibility` 那一整套设计的**第一次真实检验**。
只有读工具的话，`Pure` / `Reversible` / `Irreversible` 的区分到 M2 结束都验证不了。

## 范围裁决（主会话，2026-08-01）

原验收里「undo 越过时停下推 `undo_blocked`」「崩溃恢复时标记 `Interrupted`」需要
CLI 的 `/undo` 与恢复链路——那是「状态搬进原子图」集成 issue 的事。本 issue 只做：

1. 工具本体（下面「做什么」的规格）
2. **屏障机制的 `History` 级演示测试**：用 `agent_store::History`（meta 带
   `irreversible: bool`）+ barrier 谓词证明「undo 越过 shell/exec 的条目 →
   `UndoOutcome::Blocked`」——机制端到端，差的只是 UI
3. **不把 shell/exec 加进 CLI 的默认工具表**（集成 issue 连同 undo 屏障一起开闸）

## 实做记录（实现 agent，2026-08-01）

### 落地的文件（均在 `crates/agent-tools/`，未碰 `agent-store`）

| 文件 | 行数 | 职责 |
|------|------|------|
| `src/shell.rs`（新） | 298 | `srv:shell/exec` 的执行本体：入参校验、spawn、超时、进程组清理、输出格式化 |
| `src/barrier_demo.rs`（新，`#[cfg(test)]`） | 150 | 范围裁决第 2 条：`History` 屏障机制演示，证明 `srv:shell/exec` 的 Irreversible 判断接得上 017 的屏障 |
| `src/specs.rs` | 173（+62） | 加 `shell_spec()`（`pub(crate)`），**不进** `builtin_specs()` |
| `src/exec.rs` | 216（+6） | 分发新增 `"srv:shell/exec" => shell::execute(..)` |
| `src/lib.rs` | 76（+29） | 出口新增 `pub fn shell_spec()`；`FsExecutor` 文档更新（见下「签名」） |
| `Cargo.toml` | — | `[target.'cfg(unix)'.dependencies] libc = "0.2"`（组级 kill）；`[dev-dependencies] agent-store`（仅 `barrier_demo.rs` 用） |

### 规格落地

- 名字 `srv:shell/exec`；`agent_core::ToolSpec` 本身没有 `location`/`reversibility`
  字段（只有 `name`/`description`/`schema`，那两个维度是调用方工具表的元数据，
  见 `agent-runtime/src/tool_table.rs`）——`srv:` 前缀天然编码 `Location::Server`，
  且它不在 `tool_table.rs` 的 `Pure` 白名单里，天然落 `Reversibility::Irreversible`
  的保守默认值，不需要额外声明。
- schema：`cmd`（string，必填）、`timeout_secs`（integer，可选，`1..=300`，
  缺省 30）。
- 执行：`sh -c <cmd>`，`current_dir` 锁在 executor 的 `root`——只锁**起点**，
  `cmd` 内容本身不受约束（`cd`、绝对路径、`rm -rf /` 都拦不住），文档注释
  写明这正是它被判 `Irreversible` 而不是靠更强隔离降级的原因，没有假装能 jail。
- 输出：stdout 原样打头；stderr 非空追加 `\n[stderr]\n<内容>`；非成功退出
  （含被信号杀死，`status.code()` 为 `None` 时用 `-1`）追加 `\n[exit code: N]`。
  只有 spawn 失败（`spawn_failed`）或超时（`timeout`）才是 `Err`，命令跑起来但
  失败仍是 `Ok`——003 的部分失败语义，executor 不替模型下判断。
- 超时清理：Unix 上 `process_group(0)` 让子进程自成一个进程组（pgid = 自己的
  pid），超时后对 **负 pgid** 发 `SIGKILL` 一并带走它 fork 出的孙进程；`sh` 本身
  的 `stdin` 显式 `Stdio::null()`（用户 CLAUDE.md 那条「后台跑 CLI 必须显式关
  stdin」的同一类坑，会读 stdin 的命令在这里不该挂起）。杀信号发出后阻塞等一次
  后台线程的 `wait_with_output()`，确保子进程被真正 reap，不留半秒的僵尸窗口。

### 设计判断

1. **`wait_with_output()` 挪进后台线程，用 `mpsc::recv_timeout` 加超时**——std
   没有「等一个子进程但最多等 N 秒」的现成 API。手动顺序 `read` stdout 再
   `read` stderr 会在其中一个管道写满时死锁，`wait_with_output` 内部并发读两个
   流再等退出，规避了这个坑，所以让后台线程整个调它而不是自己拆着读。
2. **`FsExecutor` 没有跟着改名**，尽管它现在分发的不只是 fs 工具。改名要牵动
   `agent-runtime`/`agent-cli` 里所有引用它的地方，而这两个 crate 不在本 issue
   范围内（范围裁决第 3 条已经明确不把 `shell/exec` 接进 CLI 默认工具表）。
   `execute()` 的公开签名（`&self, tool: &str, input: &Value) -> Result<String,
   ToolError>`）**没有变**，只是内部 `match` 多一支——对现有调用方零破坏。留给
   集成 issue 一并改名更合适：那时反正要碰这两个 crate 来接线。
3. **`barrier_demo.rs` 刻意不用 `agent_store::Store`**。第一版按字面写了
   `Store` + `record_set` + 手动 `store.set()` 回放 undo/redo 的效果，撞上了
   `scripts/check-invariants.sh` 的红线 2（禁止业务代码裸调 `store.set()`）——
   它的豁免名单是 `agent-store/src/*`、`agent-core/src/command/*`、`*/tests/*`、
   `benches/*`，`agent-tools/src/` 不在里面，而这个文件必须在 `src/` 下才能被
   `lib.rs` 当 `#[cfg(test)] mod` 声明（放 `tests/` 违反了任务给的「不建
   `tests/`，独测的地盘」）。真正接进原子图时「把 `prev`/`next` 写回状态」本来
   就该走 agent-core 的 command 层（红线 2 的原话），而那层此刻在 `agent-tools`
   里还不存在——所以改成完全不用 `Store`：`History<String, i64, ToolCallMeta>`
   直接手工构造 `Change`，undo/redo 的「应用」只是把值赋给一个本地 `i64`
   变量。这样既避开了红线 2，也更准确地反映了这个演示的真实边界：它证明的是
   `History` 屏障本身的行为，不是「怎么把变更写回 store」——后者留给集成 issue。
4. **`timeout_secs` 的校验发生在 spawn 之前**，与「只有起不来/超时才是 Err」
   不矛盾：那句话说的是**执行结果**，入参形状不对（类型错、越界）跟 `fs/read`
   的 `offset`/`limit` 一样归 `bad_input`，压根没到「尝试执行」这一步。
5. **`kill_group` 只在 `#[cfg(unix)]` 下真正杀**，非 Unix 留空占位（当前只在
   Unix 上跑，验收原文也只提 Unix 语义）——不在这条路径上假装做了清理。

### 推给别人的（范围裁决已定，这里只是逐条对应）

- **`undo` 越过时推 `undo_blocked` 事件、崩溃恢复标记 `Interrupted { may_have_
  executed: true }`**：都需要 CLI 的 `/undo` 与恢复链路，`agent-tools` 这一层
  没有 command log 也没有崩溃恢复的概念可言。`barrier_demo.rs` 证明的是这两条
  验收依赖的地基（`History` 屏障）已经工作，UI/集成留给「状态搬进原子图」。
- **把 `shell_spec()` 接进某张工具表、`FsExecutor` 改名**：都要碰
  `agent-runtime`/`agent-cli`，同一个集成 issue 一起做。

### 自测（逐条贴原文输出）

**超时**（`shell::tests::timeout_returns_err_promptly`：`sleep 60` + `timeout_secs: 1`）：

```
running 1 test
test shell::tests::timeout_returns_err_promptly ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 50 filtered out; finished in 1.01s
```

**孤儿**（`shell::tests::timeout_kills_the_whole_process_group_no_orphans`：
`sh -c 'echo $$ > marker; sleep 60 & sleep 60'` + `timeout_secs: 1`，之后用
`kill(-pgid, 0)` 探测整组已死）：

```
running 1 test
test shell::tests::timeout_kills_the_whole_process_group_no_orphans ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 50 filtered out; finished in 1.01s
```

**非零退出**（`shell::tests::nonzero_exit_is_ok_with_exit_code_marker`：`exit 3` →
`Ok("\n[exit code: 3]")`）：

```
running 1 test
test shell::tests::nonzero_exit_is_ok_with_exit_code_marker ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 50 filtered out; finished in 0.00s
```

**stderr 合并**（`shell::tests::stderr_is_appended_after_stdout`）：

```
running 1 test
test shell::tests::stderr_is_appended_after_stdout ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 50 filtered out; finished in 0.00s
```

**History 屏障演示**（`barrier_demo::*`，3 个测试：门口即屏障 `Blocked`、
`undo_turn` 中途撞屏障停在其后一格、redo 不受屏障约束自由越过）：

```
running 3 tests
test barrier_demo::undo_stops_at_the_door_of_a_shell_exec_entry ... ok
test barrier_demo::redo_crosses_the_shell_exec_barrier_freely ... ok
test barrier_demo::undo_turn_stops_one_slot_past_a_mid_turn_shell_exec ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 48 filtered out; finished in 0.00s
```

**`cargo test -p agent-tools`**：51 个内联单测（`shell.rs` 新增 12 个、`specs.rs`
新增 2 个、`barrier_demo.rs` 新增 3 个、原有 `exec.rs`/`fs_read.rs`/`fs_list.rs`
30 个不变）+ 47 个集成测试（独立测试 agent 的 7 个新文件
`shell_exec_happy` / `shell_exec_status` / `shell_input_validation` /
`shell_orphan_cleanup` / `shell_spec_declaration` / `shell_timeout` /
`shell_undo_barrier`，共 17 个测试，与原有 4 个文件 30 个测试并存），98/98
绿——**独测方的 17 个测试落地时零改动一次通过**，包括他们自己用
`agent_store::Store` + `record_set` 写的另一版屏障演示
（`shell_undo_barrier.rs`，与本记录第 3 条提到的 `barrier_demo.rs` 殊途同归，
双方对 `Blocked`/`barrier_seq`/游标位置的断言逐条吻合）。

**`cargo test --workspace`**：全部 `test result: ok`，0 `FAILED`。

**`cargo clippy --workspace --all-targets -- -D warnings`**：0 警告。

**`bash scripts/check-invariants.sh --all`**：

```
红线检查通过
规则与理由：docs/INVARIANTS.md
```

**行数**（`wc -l`，均 ≤300）：`shell.rs` 298 / `exec.rs` 216 / `fs_read.rs` 174 /
`specs.rs` 173 / `fs_list.rs` 153 / `barrier_demo.rs` 150 / `lib.rs` 76。

### 合并记录（主会话）

工具本体 + History 级屏障演示落地，CLI 接线按范围裁决推迟到集成 issue。
双侧独立写的屏障断言完全吻合。红线 2 拦下过 barrier_demo 初版的裸 store.set
——拦得对，演示重构成不碰 Store。孤儿测试 pgrep 零残留。shell_spec 不进
builtin_specs：没有 undo 屏障 UI 就默认关着。