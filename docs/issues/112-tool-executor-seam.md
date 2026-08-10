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
