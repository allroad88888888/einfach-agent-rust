# 121 JS 工具回调接缝：页面自己执行一条 `web:` 工具

**里程碑** M14 · **依赖** [120](120-host-tool-async.md) · **模型** opus · **独测** 真机验收 · **状态** 待做

## 目标

`AgentHost` 收一个页面装的工具执行回调，形如 `(name, inputJson) => Promise<string>`。
模型调一条 Rust 不认识的 `web:` 工具时，交给这个回调；回调返回的 Promise 被 `await`。

**这是需求 2 的正身**，也是 [130](130-browser-vision-end-to-end.md) 图片能落地的前提。

## 做什么

### 接口（定死，实现与真机验收都照这个来）

```rust
// agent-wasm/src/host.rs
/// 装一条工具执行回调：`handler(name, inputJson) -> Promise<string>`。
///
/// - resolve 的值当成工具结果正文（非字符串 → 按 `Failure` 处理，不 panic）
/// - reject → `RemoteToolOutput::Failure`，正文取异常的 message
/// - 没装回调而模型调了内建之外的工具 → `Failure`，措辞同今天的 `_ =>` 分支
#[wasm_bindgen(js_name = onToolCall)]
pub fn on_tool_call(&self, handler: js_sys::Function);
```

### 派发顺序

`host_tool::execute` 里：先查两条内建 `web:page/*`，**没命中才交给回调**。
内建优先而不是回调优先——页面不能悄悄劫持一条已声明的内建工具的语义
（那会让 `toolTableJson()` 描述的东西和实际执行的东西对不上，且不报错）。

### 回调的存放位置

跟 `on_event` 同款：`Rc<RefCell<Option<js_sys::Function>>>` 挂在 `Inner` 上，
**不放进 `RunnerCtx`**——切会话时 `RunnerCtx` 整个换掉，回调不该跟着掉
（`host.rs:47-49` 已经为 `on_event` 写过这条理由）。

## 验收

- `bash scripts/build-wasm.sh` 过。
- **真机主证据**：页面装一条自定义工具（声明先硬编码进 `tools.rs` 即可，
  页面声明是 [122](122-page-declared-tools.md) 的事），回调里
  `await new Promise(r => setTimeout(r, 500))` 之后返回一个可辨认的字符串，
  模型用它回答。**500ms 必须真的过去**——这是「可等待」区别于「同步」的唯一证据。
- **真机反向锁**：回调 `throw new Error("boom")` → 这一轮**不崩页面**，
  模型收到 `is_error` 的结果并自纠。`host_tool.rs` 模块文档那条
  「panic 会带走整个页面」的理由在这里第一次被真正用上。
- **真机反向锁**：不装回调，模型调一条内建之外的工具 → 同样是 `Failure` 不是挂死。
- 内建仍然走内建：`web:page/title` 在装了回调之后**仍然由 Rust 执行**
  （回调里对这个名字返回一个哨兵字符串，断言模型拿到的不是那个哨兵）。

## 注意

- ⚠️ **重入是这条 issue 唯一真正微妙的地方**（[119](119-browser-host-capability-decision.md) §七-1）。
  `send()` 在整轮期间持有 `live.borrow_mut()`，**工具回调正是在这个借用之内被调用的**。
  页面在回调里调 `send()`/`openSession()` = `already borrowed` panic。

  今天 `host.rs:11-17` 那两条约定是写给**事件回调**的，措辞是「回调里只读、只画」
  ——事件回调没人想在里面干活。**工具回调天然想干活**，这条约定必须重写，
  写成对工具回调也成立的话，并且在 `onToolCall` 的文档注释里就说清楚。
  这不是可选的文档工作：不写，第一个用它的人必踩。
- **超时与取消不在本条范围**，归 [123](123-host-tool-deadline.md)。本条只保证
  「能等」，不保证「等不到会怎样」。
- **transient-source 工具（`web:source/`）不在本条范围**，归
  [124](124-transient-source-in-browser.md)——那条路要走
  `submit_remote_tool_result_async` 而不是 `resolve_remote_tool_async`。
  本条只处理普通 `web:` 工具。
- 红线 11：**不许在本条里改工具表的内容或次序**。
