# 196 wasm 宿主暴露 undo

**里程碑** L · **依赖** [169](169-wasm-artifact-recheck.md) · **模型** sonnet · **独测** ✅ · **估时** 20min · **状态** 待开始

## 目标

让浏览器宿主能 undo 一轮。

## 为什么这条是 L 波的关键路径

[165](165-launch-positioning-decision.md) 的**一号钩子**是「`/undo` 之后那一轮在模型记忆里真的不存在」，
[172](172-demo-gif.md) 要录的 GIF 就是它。而 [170](170-pages-workflow.md) 的 Pages demo 是首发时
**唯一一个陌生人点一下就能亲自验证**的地方。

**现在这个 demo 演不了一号钩子。** 这不是 bug——是能力没接出来——但对推广的影响
跟 bug 一样大：最能打的那句话，在最能传播的那个场合，没法当场证明。

## 现状（[169](169-wasm-artifact-recheck.md) 查的）

能力在，入口没有：

- `crates/agent-wasm/src/turn.rs:91` 调了 `session.undo_turn()`，但**只在取消轮的内部路径**
  （「取消轮丢弃」，模块注释写明是 `agent_cli::undo::after_cancelled_turn` 的同一句）
- `agent_wasm.d.ts` 里搜不到任何 undo 导出
- `www/index.html` 与四个 js 里没有 undo 按钮

对照：CLI 有完整的 `/undo`（027），server 也有。**只有 wasm 这一路没接出来**——
M13/M14 当时的验收清单里没有它，不是有意省的。

## 做什么

1. `crates/agent-wasm/src/host.rs`（或 `host_session.rs`）加一个 `#[wasm_bindgen]` 方法，
   把 `Session::undo_turn()` 接出来，返回 `UndoReport`。
2. **`UndoReport::Blocked` 必须原样传到页面**——撞上不可逆屏障时要能让人看见「为什么没撤」，
   不能吞掉变成静默失败。这正是可逆性屏障存在的意义，演示价值也在这里。
3. `www/index.html` 加按钮；撤销后**消息列表和事件流都要跟着回退**。
4. 落 IndexedDB：撤销后刷新页面，撤掉的那一轮**不能回来**。

## 验收

- 页面上 undo 一轮，消息列表回退
- **口令实验过**（[169](169-wasm-artifact-recheck.md) 那套，注意那里记的坑：
  问句里不能出现「undo」二字，且一旦污染就换会话重来）：
  设口令 → 问得出 → undo → **中性追问，模型说不知道**
- undo 后刷新页面重开会话，撤掉的轮次不复现（落盘也回退了）
- 撞上不可逆工具时页面显示 `Blocked` 的原因，不是静默无反应
- `cargo test --workspace` + `check-invariants.sh --all` + `build-wasm.sh` 三门禁绿

## 注意

- 红线 2：不许直接 `store.set()`，走 command 层——`undo_turn` 本来就是，别绕。
- 这条**排在 [170](170-pages-workflow.md) 之前还是之后都行**，但**必须在 [172](172-demo-gif.md) 之前**。
  [172](172-demo-gif.md) 若等不及，退路是拿 CLI 录——但那样就丢掉了「读者能自己点进去复现」，
  说服力差一档。
