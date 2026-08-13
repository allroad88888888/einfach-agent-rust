# 196 wasm 宿主暴露 undo

**里程碑** L · **依赖** [169](169-wasm-artifact-recheck.md) · **模型** sonnet · **独测** ✅ · **估时** 20min · **状态** 完成（2026-08-13，四条验收真机全过）

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

---

## 实做记录（2026-08-13）

**四条验收真机全过。[172](172-demo-gif.md) 的退路（拿 CLI 录）不需要了。**

### 怎么做的

新建 `crates/agent-wasm/src/undo.rs`（81 行），**不往 `host_session.rs` 里堆**——
「撤销/重做的 JS 面 + `UndoReport` 的 JSON 化」是一件事，够格单独一个文件。
骨架照抄 `agent_cli::undo`：先调 `Session` 命令、再 `agent_runtime::persist::sync`。
`sync` 那步不是可选的，漏了撤销就只活在内存里，刷新一次就回来了。

`host_session.rs` 只加三个薄方法（`undoTurn`/`undoTurnForce`/`redoTurn`）+ 一个
`with_live` 把「取活会话」那三行收掉。它们是**同步方法**：`undo_turn` 不 await
任何东西，`borrow_mut()` 不跨 await 点，所以不背 `send()` 那节借用纪律。

**结果用结构化 JSON 而不是 `format!("{report:?}")`**。`send()` 的 `cancelledTurn`
那样干是因为它只是给人看的附注；这里页面要据此决定显示什么（尤其 `Blocked` 要弹确认框），
Debug 串会逼页面去解析 Rust 的枚举写法。

页面侧：撤销后**重画整份历史**（`renderHistory(historyJson())`），而不是在 DOM 上
往回删。真值在 `Session` 里，UI 哑渲染——跟 M7 定的「树由 core 权威算、UI 哑渲染」
同一条原则，让 UI 维护一份「现在应该有几轮」的影子状态就是在造第二个真值源。

### 真机验收（DeepSeek `deepseek-v4-flash` + Kimi 识图，浏览器直连）

**① 口令实验**（会话 `undo-1`）——问句里全程不出现「undo」「撤销」，
按 [169](169-wasm-artifact-recheck.md) 记的那个坑来：

```
设口令 quokka-88          → 好
口令是什么？               → quokka-88
[按「撤销一轮」×2]         → 撤了 3 条（turn 1），消息列表空
「不要调用任何工具。仅根据我们的对话记录回答：我之前有没有告诉过你一个口令？」
                          → 「没有」        ✅
```

中间有个**意外但更有说服力**的插曲：撤销后第一次直接问「口令是什么」，模型不知道，
于是**跑去调 `web:page/title` / `web:page/url` / `web:host/callback-probe` 满页面找**
——它记忆里真没有，不是嘴上说没有。（那次它答的 `PAGE-CALLBACK-OK` 是 probe 工具
自己的返回串，跟口令无关。）为了拿到干净可引用的一句，才补问了上面那句禁用工具的。

**② 落盘也回退了**：刷新页面 → 重建宿主 → 重开 `undo-1`，
「从 IndexedDB 重放出 8 条消息」，`quokka-88` 全文零命中。撤掉的轮次没回来。✅

**③ 屏障 `Blocked`**（会话 `barrier-1`）——demo 里唯一的不可逆工具是
`web:source/vision`（`page-tools.js` 声明 `reversibility: "irreversible"`，
理由同 native 那条：调第三方 API 计费，undo 不该重放）。造一张 64×64 红色 PNG
上传，让模型识图，然后撤销：

```json
{"kind":"Blocked","entries":1,"barrierSeq":3,
 "barrier":{"label":"tool_failed","tool":"web:source/vision",
            "callId":"call_00_w0KY5zwiuVgiiRfeSU407391"}}
```

**拦住了，而且说得出是被什么拦住的**——工具名 + call_id，不是甩一个 `barrier_seq`
数字让用户猜。（`label` 是 `tool_failed` 因为那次识图真失败了；`mark_irreversible`
在宿主**派发**工具那一刻就落，不等结果——所以失败的不可逆调用同样立屏障，这是对的：
钱可能已经花了。）

**④ force 越过**：`undoTurnForce()` → `{"kind":"Applied","entries":3,"turnId":2}`，
`historyJson()` 长度 0。✅

### 门禁

`check-invariants.sh --all` ✅ / `cargo test --workspace` 0 失败 /
`clippy -D warnings` ✅ / `build-wasm.sh` ✅

### 副作用：解锁了什么

[165](165-launch-positioning-decision.md) 的**一号钩子现在能在浏览器 demo 里当场演**。
这是 [172](172-demo-gif.md) 的 GIF 脚本，也是 [191](191-launch-post.md) 首发时
「读者自己点进去就能复现」的那件事——从「他说他能」变成「我自己试了」。

### 留的尾巴（不阻塞）

页面的确认框用的是原生 `confirm()`。够用，但 [171](171-demo-first-screen.md) 打磨首屏时
可以换成页面内的提示——原生弹窗在录屏里很难看，而这一步恰好是 GIF 里最该被看清的一帧。
