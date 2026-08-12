# 120 `host_tool::execute` 执行侧 async 化（工具表不动）

**里程碑** M14 · **依赖** [119](119-browser-host-capability-decision.md) · **模型** sonnet · **独测** 否（无行为变化） · **状态** 完成（真机已验收，见文末）

## 目标

把 `drain_host_tools` 那条 await 链上**唯一剩下的同步点**改成异步，**行为一字不变**。

这是整个 M14 的地基，也是最小的一刀：不引入任何 JS 回调、不动工具表、不加新工具。

## 现状

```rust
// agent-wasm/src/turn.rs:78
async fn drain_host_tools(...) -> TurnStatus {
    loop {
        let Some(waiting) = ctx.pending_remote_tools().into_iter().next() else { ... };
        let output = host_tool::execute(&waiting);        // ← 唯一的同步点
        match resolve_remote_tool_async(...).await { ... }
    }
}
```

`host_tool::execute` 的文档注释写着「**同步**——两个工具读的都是当场就有的值，
没有 IO 可等」。这句话对今天的两条工具是对的，对下一条就不对了。

## 做什么

1. `host_tool::execute` 改 `pub(crate) async fn`。
2. `turn.rs:87` 加 `.await`。
3. 两条 `web:page/*` 的实现体**一行不动**——它们仍然是同步读 `document.title` /
   `location.href`，只是包在一个立刻就绪的 future 里。
4. 更新 `host_tool.rs` 模块文档那句「**同步**」——改成说明「签名是异步的，
   今天这两条实现体是同步的，下一条不是」。

## 验收

- `bash scripts/build-wasm.sh` 过（这是本条**唯一**的编译期验收，`cargo test
  --workspace` 覆盖不到 `agent-wasm`，见 [119](119-browser-host-capability-decision.md) §八）。
- 真机：浏览器里跑一轮，模型调 `web:page/title`，拿到真实页面标题并用它回答。
  **跟 M13 验收第二条逐字相同的一次操作**——这条 issue 的全部意义就是「什么都没变」。
- `git diff` 里 `tools.rs` 零改动、`agent-runtime`/`agent-core` 零改动。
  改动面应当只有 `turn.rs`（一个 `.await`）与 `host_tool.rs`（签名 + 文档）。

## 注意

- **不要顺手把两条工具的实现改成"真异步"**（比如为了对称去 `await` 一个立刻
  resolve 的 Promise）。它们读的是当场就有的值，包一层没有任何好处，只是多一次
  微任务调度。签名异步、实现同步，是这条 issue 的正确终态。
- **不要在这条里动工具表**。页面声明工具表是 [122](122-page-declared-tools.md) 的事，
  在这里做会让「什么都没变」这条验收失去意义。

## 实做记录（2026-08-12）

- 动手前的现状核查：`host_tool::execute` 的调用点**不是 issue 描述的一处，是两处**。
  124 已落地并改过 `turn.rs`（当前 210 行），除了 `drain_host_tools`（原 issue 引文
  那一处，现第 116 行）之外，多了 `drain_transient_source`（第 175 行，`web:source/`
  前缀那条走 `submit_remote_tool_result_async` 的分支）。两处都调了
  `host_tool::execute`，函数签名一改 async，两处不加 `.await` 编译不过。这不算「顺手
  多做」——是同一次签名改动的必然连带，issue 引文写的是 97 行时代的 `turn.rs`，代码
  已经往前走了一版。
- `crates/agent-wasm/src/host_tool.rs`：`execute` 从 `pub(crate) fn` 改
  `pub(crate) async fn`；函数体一行未动（`document_title()`/`location_href()`/
  `SOURCE_ECHO_TOOL` 的匹配分支原样）。模块文档那句「**同步**」改写成「签名是异步的，
  今天三个工具的实现体仍是同步的，下一条（浏览器识图）不是」，并把「不要为了对称
  await 一个立刻 resolve 的 Promise」的告诫写进文档，不只留在 issue 里。
- `crates/agent-wasm/src/turn.rs`：两处调用点各加一个 `.await`（第 116、175 行），
  没有别的改动。
- 未改：`tools.rs`、`host.rs`、`www/`、`agent-runtime`、`agent-core`、工具表。
  `git diff --stat` 只有 `host_tool.rs`（+8/-4）与 `turn.rs`（+2/-2）两个文件。
- 验收结果（均前台跑完）：
  - `bash scripts/build-wasm.sh --dev`：通过，`Finished dev profile`，wasm-bindgen
    产物正常生成，无新增警告。
  - `cargo test --workspace`：全绿，无 `FAILED`，各 crate `test result: ok`。
  - `bash scripts/check-invariants.sh --all`：exit 0；报的 15 条行数超限提示均为
    存量文件（`agent-cli/src/mcp.rs`、`agent-core/observe.rs` 等），跟本次改动的两个
    文件无关。
- **没做到的部分**：真机验收（浏览器里跑一轮、模型调 `web:page/title` 拿到真实标题）
  没做——环境里没有可交互的浏览器 + 真 provider key，如实报「待真机」，交给下一步或
  用户自己跑。

## 真机验收（主会话，2026-08-12，Chrome via playwright MCP + 真 Kimi key）

**过。** 会话 `m14b-1`，两个调用点各验一条路——这正是本条改动面的形状
（124 之后 `execute` 有两个调用点，签名一改两处都得 `.await`）：

| 路径 | 调用点 | 结果 |
|---|---|---|
| 普通 `web:` 工具 | `drain_host_tools` | `web:page/title` → 模型答出真实标题 |
| transient-source | `drain_transient_source` | `web:source/echo` → 模型拿到真入参回显 |

且脱敏纪律没被 async 化弄坏：journal 里 `web:page/title` 落真入参真结果、
`web:source/echo` 落 `{"transient_source":"redacted"}` 与
`[transient_source_result_redacted]`。

**「行为一字不变」是可判定的**：`host_tool.rs` 的非注释 diff 只有 `async` 一个关键字，
`turn.rs` 是 +2/-2 两个 `.await`。
