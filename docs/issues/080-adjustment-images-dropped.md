# 080 `Adjustment::ImagesDropped` 变体

> ⚠️ **已废弃（superseded by s5）**：本文描述的 images 管线（`ContentBlock::Image` /
> `POST /files` / `upload_base_url` / `ImagesDropped` / 前端选图 / vision 子 agent 委托）
> 已被 s5 重构整体移除，现以 `POST /uploads` 上传端点 + `srv:vision/inspect` 工具取代。
> 正文仅作历史决策档案保留，不再反映当前实现。

**里程碑** M11 · **依赖** — · **模型** **haiku** · **独测** 不需要（照抄现有变体，错了编译不过或一致性测试红）· **状态** 完成

**无依赖，可以立刻开工**，跟 [079](079-image-content-block.md)、
[084](084-transport-files-upload.md) 并行。**它是 [083](083-image-provider-fallback.md)
的前置**，先落地能让 083 少一次改动。

## 范围

`crates/agent-core/src/seam.rs` 的 `Adjustment` 加一个变体：

```rust
/// 这一轮的历史里有图片，但这家吃不下（实测 DeepSeek/GLM 400，
/// PROVIDERS.md §八），已编成占位文本。**必须报**——静默丢图是这个功能里
/// 唯一用户永远发现不了的失败：图传了、模型没看见、回答还挺像样。
ImagesDropped { count: u32 },
```

摆放位置照现有变体的风格（`ToolChoiceDowngraded` / `ToolsTruncated` 那一批），
文档注释要说清「什么时候报」和「为什么必须报」。

## 验收（可判定）

1. `cargo test` 全绿、`cargo clippy --all-targets` 零 error。
2. **serde 往返**：`seam.rs` 已有的 `serde_roundtrip` 测试里把新变体加进那个
   `Vec<Adjustment>`，`to_string` → `from_str` → 相等。
3. **ts 导出一致**：`cargo test --features ts` 绿。`Adjustment` 挂着
   `#[cfg_attr(feature = "ts", derive(ts_rs::TS))]`，新变体要能过一致性测试。
4. **前端能拿到**：重新生成 `packages/protocol` 的类型，`pnpm typecheck` 绿。
   前端如果有对 `Adjustment` 的穷举 `switch`，补上新分支（**不要**用 `default` 吞掉）。

## 注意

- **本 issue 不写任何「什么时候报」的逻辑**——那是 083 的活。这里只加类型。
  加完之后没有任何地方构造它，这是对的。
- `count` 用 `u32` 跟现有 `ToolsTruncated { kept: u32, dropped: u32 }` 保持一致，
  别用 `usize`（那个不好跨 TS 边界）。
- 收工验证前台跑完，含 `--features ts` 与 `pnpm typecheck`。
- 实做时 core 不能照范围注释抄写厂商名或加能力判断（红线 12）；变体文档只描述
  输入与结果，实测依据仍留在 `PROVIDERS.md`，何时构造仍由 083 负责。

## 实做记录（完成 · 2026-08-04）

- `Adjustment` 增加 `ImagesDropped { count: u32 }`，位置紧邻
  `ToolsTruncated`；没有新增构造点。
- `seam::tests::serde_roundtrip` 将 `ImagesDropped { count: 2 }` 放入
  `Vec<Adjustment>`，实际执行 `to_string` → `from_str` → 与原值相等。
- 已运行协议生成器，`packages/protocol/src/generated/Adjustment.ts` 增加
  `ImagesDropped` 联合分支。前端只对 adjustment 做通用标签渲染，没有穷举 `switch`，
  无需补分支。
- 定向验证：`cargo test -p agent-core seam::tests::serde_roundtrip`（1 passed）；
  `cargo test -p agent-server --features ts`（80 unit tests + integration tests，0 failed）；
  `cargo clippy -p agent-core --all-targets -- -D warnings`（0 warning/error）；
  `pnpm typecheck`（protocol、web 均通过）。全工作区收尾验证由主会话在合并本批改动后执行。

### 突变验证（真实红报文，已还原）

把 `ImagesDropped { count: u32 }` 暂改为
`ImagesDropped { #[serde(skip)] count: u32 }`，运行
`cargo test -p agent-core seam::tests::serde_roundtrip`，得到以下报文；之后已还原字段并重跑为绿：

```text
thread 'seam::tests::serde_roundtrip' (42314841) panicked at crates/agent-core/src/seam.rs:141:9:
assertion `left == right` failed
  left: [ToolChoiceDowngraded { wanted: "srv:fs/read", used: "required" }, ThinkingDisabledForToolChoice, LateToolsForcedIntoPrefix { count: 3, est_cost_multiple: 120.0 }, ImagesDropped { count: 0 }, LateSystemReshapedPrefix { est_cost_multiple: 11.0 }]
 right: [ToolChoiceDowngraded { wanted: "srv:fs/read", used: "required" }, ThinkingDisabledForToolChoice, LateToolsForcedIntoPrefix { count: 3, est_cost_multiple: 120.0 }, ImagesDropped { count: 2 }, LateSystemReshapedPrefix { est_cost_multiple: 11.0 }]
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    seam::tests::serde_roundtrip

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 90 filtered out; finished in 0.00s

error: test failed, to rerun pass `-p agent-core --lib`
```
