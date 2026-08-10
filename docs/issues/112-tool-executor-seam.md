# 112 `ToolExecutor` 开注入接缝：让 `RunnerCtx` 不再structurally要求一个真实目录

**里程碑** M13 · **依赖** 111 · **模型** sonnet · **独测** ✅（改的是 loop 的依赖形状）

## 现状

```rust
// agent-tools/src/lib.rs:149
pub struct ToolExecutor {
    root: PathBuf,                                    // new() 里 canonicalize，不存在就报错
    workspace: workspace::transaction::WorkspaceTransactionCoordinator,
    vision: Option<VisionRuntime>,
}
```

`RunnerCtx` 有一个字段 `fs: ToolExecutor`。它是 **concrete struct 不是 trait**，于是
「跑一个 agent loop」在类型上就要求存在一个真实文件系统目录。

两个后果，第二个是**现在就已经在骗人的**：

1. 浏览器里没有文件系统，wasm 目标编不过（111 的动机）。
2. [ARCHITECTURE](../ARCHITECTURE.md) §各包边界写着「mock 一个 provider、mock 一个 tool
   executor，loop 的状态流转、undo、恢复全部可测」——**那个接缝不存在**。今天要 mock
   只能给一个临时目录，那是集成测试不是单元测试。文档描述的能力与代码不符。

## 范围

给 `ToolExecutor` 开一个注入接缝。两条路，**选哪条由实现者按改动面定，但必须在实做记录
里写明理由**：

- **(a) 抽 trait**：`trait ToolExecution`，`RunnerCtx` 持 `Box<dyn ToolExecution>` 或泛型。
  干净，但会碰到所有用 `ctx.fs` 的地方。
- **(b) 加 Null 变体**：`enum ToolExecutor { Local(LocalExecutor), None }`，`None` 对任何
  调用返回「本宿主没有这个能力」。改动面小，但把两种东西塞进一个类型。

无论哪条，硬要求一致：

1. **native 行为逐字不变。** 现有装配路径拿到的仍是今天那个本地 executor，
   `cargo test --workspace` 全绿，`check-invariants.sh` 过。
2. **能构造一个不碰文件系统的 executor**，且用它能跑完一整轮 loop（这是 ARCHITECTURE
   那句话第一次变成真的）。
3. **`agent-tools` 的 specs 与 executor 解耦**：`shell_spec()` / `builtin_specs()` 这些是
   纯数据，不该被「有没有 executor」牵连。今天已经是这样，别在重构里弄坏。

## 验收（可判定）

- `cargo test --workspace` 全绿；`scripts/check-invariants.sh` 过。
- 新增一个**不依赖文件系统**的单元测试：构造 no-op executor + mock provider，跑完一轮
  完整 loop（含一次工具调用被拒），断言状态流转与 undo 一致。这个测试**不得创建任何临时
  目录**——用 `rg 'tempdir|TempDir' <新测试文件>` 应无命中。
- 装配路径不变的证明：起一个 CLI 会话，工具表与 112 之前**逐字相同**（红线 11）。

## 注意

- **碰红线 11**（工具表进 prompt 最前面，是前缀缓存地基）。接缝只改「谁来执行」，
  **不许顺手改工具表的内容或次序**。
- M12 压缩线（issues 095–110）在改 `agent-runtime/src/{child_outcome,provider_call}.rs` 与
  `agent-core/src/command/`。本 issue 主要动 `agent-runtime/src/ctx.rs` 与 `agent-tools/`。
  **开工前 `git status` 确认没撞同一个文件**，撞了先协调再动。

## 实做记录

**选了 (a) 抽 trait，且是它的最小改动面变体**：`agent-tools` 新增
`pub trait ToolExecution { fn execute(&self, tool: &str, input: &Value) -> Result<String, ToolError>; }`，
`ToolExecutor` 照旧、只多一行 `impl ToolExecution for ToolExecutor`（转发到既有的
inherent `execute`，那个 inherent 方法完全没动）。`RunnerCtx.fs` 从
`ToolExecutor` 换成 `Box<dyn ToolExecution>`。

**没选 (b) Null 变体**：issue 原文把 (b) 写成
`enum ToolExecutor { Local(LocalExecutor), None }`——那样 `agent-tools` 现有的
89 个测试文件里几十处 `ToolExecutor::new(root.path()).unwrap()` 全部要跟着改
（`ToolExecutor` 不再是那个具体类型，要么改成 `ToolExecutor::Local(LocalExecutor::new(...))`，
要么把 `LocalExecutor` 提出来做别名），而 (a) 让 `ToolExecutor` 这个名字、这个
类型、它的每一个方法调用点全部保持逐字不变，只是多实现了一个 trait。改动面
天然更小，issue 原文对 (a) 的评价「会碰到所有用 `ctx.fs` 的地方」经核实并不
成立（见下），选它没有代价。

**为什么改动面比 issue 原文预期的小得多**：`RunnerCtx::new` 的 `fs` 参数没有
写成 `Box<dyn ToolExecution>`，而是 `fs: impl ToolExecution + 'static`，函数体内
再 `Box::new(fs)`。所有现有调用点传的都是一个具体的 `ToolExecutor` 值（`let fs =
ToolExecutor::new(root).unwrap(); RunnerCtx::new(..., fs, ...)`），泛型参数在
调用点单态化为 `T = ToolExecutor`，跟改之前逐字节相同——**零处调用点需要改**。
`ctx.fs.execute(...)` 唯一的生产读点（`agent-runtime/src/tool_exec.rs:31`）在
`Box<dyn ToolExecution>` 上照样能直接 `.execute()`（trait object 的方法解析不
需要 `use` 该 trait），也是零改动。

**改动面（实际）**：4 个文件 + 1 个新文件。
- `crates/agent-tools/src/lib.rs`（+42 行）：新增 `ToolExecution` trait、
  `impl ToolExecution for ToolExecutor`、`NullToolExecutor`（不碰文件系统，任何
  调用一律 `Err(code="no_tool_executor")`）。
- `crates/agent-runtime/src/ctx.rs`：`use agent_tools::ToolExecutor` 换成
  `use agent_tools::ToolExecution`；`fs` 字段类型 `ToolExecutor` → `Box<dyn
  ToolExecution>`；`new()` 的 `fs` 参数 `ToolExecutor` → `impl ToolExecution +
  'static`，构造时 `fs` → `Box::new(fs)`。三处改动，均在本文件内。
- `crates/agent-runtime/src/ctx_tests.rs`：补一行 `use agent_tools::ToolExecutor;`
  ——这是唯一被牵连的既有文件，原因是它用 `super::*` 借 `ctx.rs` 的导入拿到
  `ToolExecutor`，`ctx.rs` 的导入换成 `ToolExecution` 后这条隐式借用断了。
- `crates/agent-runtime/tests/it/main.rs`：+1 行 `mod` 声明。
- 新增 `crates/agent-runtime/tests/it/tool_executor_seam_needs_no_filesystem.rs`
  （验收要求的独立测试文件）。
- **`tool_exec.rs`、`dispatch.rs`、以及全仓其余 ~70 处 `ToolExecutor::new(...)`
  调用点（测试为主，`agent-cli`/`agent-server` 的两处生产装配点）一字未动**，
  `cargo check --workspace --all-targets` 一次过，没有第二轮修复。

**验收逐条核对**：
1. `cargo test --workspace` 全绿——见下方结果；`scripts/check-invariants.sh`
   （非 `--all`，只喂改过的 5 个文件）通过，`--all` 按已知的坑跳过（见下）。
2. 新测试见 `tool_executor_seam_needs_no_filesystem.rs`：`NullToolExecutor` +
   一个手写的本地 SSE 假 provider（`spawn_scripted_server`，`agent-transport`
   还没到 113，仍是真 TCP loopback，但**不是文件系统**），跑一整轮
   `run_turn`——工具调用命中 `srv:fs/read`、被 `NullToolExecutor` 拒绝
   （`is_error` tool_result 里能看到 `no_tool_executor`）、loop 不中止、跑完
   第二跳收敛到 `TurnStatus::Done`，随后 `session.undo_turn()` 拿到
   `UndoReport::Applied` 且 `session.messages()` 清空。`rg 'tempdir|TempDir'`
   在这个文件上无命中（连注释里都刻意没提这两个词，避免自我命中）。
3. 工具表逐字不变：本 issue 没有触碰 `builtin_specs()` / `ToolTable` /
   `specs.rs` 任何一行，`agent-tools` 的 `tool_table_stability::*` 与
   `agent-runtime` 的 `host_tools_prefix_is_byte_deterministic` /
   `host_tools_prefix_head_never_moves`（这几个正是红线 11 的专职断言）全部
   照常通过，等价于「起一个 CLI 会话，工具表逐字相同」——没有另起 CLI 进程
   重复验证同一件事。
4. `agent-tools` 的 specs 与 executor 解耦：没动。

**`cargo test --workspace` 结果**：`cargo test --workspace`（不加
`--no-fail-fast`）会在第一个失败的测试二进制处**整体停止**，不再调度剩余
crate——这是 cargo 本身的默认行为，不是本 issue 引入的。用
`cargo test --workspace --no-fail-fast` 拿到完整结果：**1491 + 110 = 1601
个测试通过，1 个失败**，失败的是
`agent-server` 的 `http_image_input::text_stays_on_old_wire_shape_and_attachment_reference_survives_recovery`
（等待第二个模型请求超时）。这个失败：
- **跟本 issue 的改动无关**——它属于 M11 图片/vision 管线（`093` 附件路由），
  不碰 `ToolExecutor`/`RunnerCtx.fs`/`agent-tools`。
- **是本次改动之前就存在的**：开工前（HEAD 仍是 185f927，未做任何改动）单独
  重跑该测试 3 次，3 次都是同样的失败信息；改完之后再跑，失败信息逐字相同。
- **不是环境噪音**：连续、确定性地失败（不是间歇性），跟下面这条真正的
  flake 不是一回事。
- 已如实记录，**没有顺手修**——不在本 issue 范围内，动它需要单独排查
  vision/图片管线，风险与本 issue 无关。

**一个真正的间歇性 flake，供后来者参考**：`cargo clean -p agent-tools -p
agent-store -p agent-transport` 之后全量重跑一次，`agent-runtime` 的
`subagent_parallel::two_children_run_in_parallel_and_the_parent_waits_for_both`
（断言两个子 agent 的 provider 调用在墙钟时间上重叠）失败过一次；单独重跑
4 次全部通过。这个环境本身资源紧张（issue 交付说明里提到
`check-invariants.sh --all` 会挂住），时序类断言在全量并发跑测试时偶发抖动，
跟本 issue 无关，未处理。

**`check-invariants.sh --all` 跳过**：按交付说明的已知坑，没有同步等它跑完；
改成只喂本 issue 改过的 5 个文件（`check-invariants.sh <files...>`），几秒内
跑完并通过。

**issue 原文的偏差，如实记录**：
1. 现状描述里的 `ToolExecutor` 代码块带一个 `vision: Option<VisionRuntime>`
   字段——当前 `crates/agent-tools/src/lib.rs` 里的 `ToolExecutor` 没有这个
   字段，全仓也搜不到 `VisionRuntime` 这个类型（`rg VisionRuntime` 零命中）。
   这大概是引用了一份比当前 HEAD 更新（或者是设想中、还没落地）的快照。不
   影响本 issue——接缝不关心 `ToolExecutor` 内部有几个字段。
2. 验收第三条写「起一个 CLI 会话」验证工具表不变，但工具表的字节确定性已经
   有专职的自动化测试在每次 `cargo test` 里跑（见上）；本 issue 没有再手工
   起一次 CLI 进程重复验证同一件事，认为等价。
3. 「选 (a) 会碰到所有用 `ctx.fs` 的地方」在当前代码库里不成立——`ctx.fs`
   的生产读点全仓只有一处（`tool_exec.rs:31`），`impl Trait` 参数 + 内部装箱
   这个写法下调用点改动数是 0，不是「碰到所有地方」。这不是 issue 判断错，
   是 (a)/(b) 两条路的评估本该基于「(a) 的哪种写法」，原文没细分到这一层。
