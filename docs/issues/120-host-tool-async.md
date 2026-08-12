# 120 `host_tool::execute` 执行侧 async 化（工具表不动）

**里程碑** M14 · **依赖** [119](119-browser-host-capability-decision.md) · **模型** sonnet · **独测** 否（无行为变化） · **状态** 待做

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
