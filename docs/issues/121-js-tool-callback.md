# 121 JS 工具回调接缝：页面自己执行一条 `web:` 工具

**里程碑** M14 · **依赖** [120](120-host-tool-async.md) · **模型** opus · **独测** 真机验收 · **状态** 完成（真机已验收，见文末）

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

  实做时这条跟上面「验收」那句「声明先硬编码进 `tools.rs` 即可」正面撞车了，
  裁决与理由见下面实做记录第 4 条：**加一条新声明不违反红线 11**（表仍是编译期
  常量，`with_host_tools` 按名字排序），红线 11 禁的是改**已有**条目的文案与次序，
  那才是「刷新前后逐字节相同」那条验收的证据面。

## 实做记录（2026-08-12）

### 1. 派发顺序：内建优先，回调兜底

`host_tool::execute` 里三条内建（`web:page/title`、`web:page/url`、`web:source/echo`）
先全查一遍，一条都没命中才走 `callback::invoke_tool`。**回调优先是错的**：页面能
悄悄劫持一条已声明工具的语义，`toolTableJson()` 描述的东西和真正执行的东西对不上
且不报错，症状会以「模型行为莫名其妙」的形式出现在离这里很远的地方。

顺带钉死一件当时没想到的事：**分流（普通 / transient-source）在 `turn.rs`，不在
`host_tool.rs`**，所以这条兜底对两条路都生效——130 那条 `web:source/vision` 将来
落到的就是同一个分支，入参是认领后的真值，结果照样被 transient-source 策略遮进历史。
`turn.rs` 一行没改。

### 2. 重入：`try_borrow_mut` 这个手法**不适用**，而且理由不是「做不到」

128 的 `delete_session` 是**从借用外面**打进来的调用，「撞上在飞的一轮」是两个独立
操作的竞争，reject 说的「现在没删成，等这轮完再来」是**真话**。

工具回调里的 `send()` 不是竞争——它嵌套在自己那一轮里，**等多久都不会成立**。
把 panic 换成 reject 在机制上办得到（`send()` 改用 `try_borrow_mut` 即可），但那会
把一个结构性错误说成一次可重试的碰撞，页面照着 retry 就是死循环。所以
**`send()` 保持 `borrow_mut()` 不变**，约定仍然是「违反 → panic」。

约定按要求重写了，落在两处：

- `host_session.rs` 模块文档「借用纪律」第 2 条，措辞从「回调里只读、只画」改成
  对工具回调也成立的话——**工具回调天然要干活，能干的是「不经过这个 `AgentHost`
  的活」**；
- `AgentHost::on_tool_call` 的**文档注释里**一张三行表（panic / reject / 安全各哪些
  方法）+ 上面那段「为什么不 reject」。生成的 `agent_wasm.d.ts` 里这张表原样带过去了，
  页面作者在编辑器里就能看见。

顺带查实并写进表里的两条：`sessionId()` 和 `historyJson()` 要的是**共享**借用，
在飞的一轮里同样借不到，所以它们跟 `send()`/`openSession()` 同一档 panic
——旧文档只列了「安全的那四个」，从没正面说过这两个会炸。

### 3. 回调存放：`Inner` 上一个 `Rc<RefCell<Option<Function>>>`，外加一个线程局部的**视图**

按 119 的决定挂在 `Inner`（切会话 `RunnerCtx` 整个换掉，回调不该跟着掉）。

但落地时撞上一个 issue 正文没写的问题：**`host_tool::execute` 够不着 `AgentHost`**
——`turn::run` 只拿 `&mut Session` + `&mut RunnerCtx`，而回调又不许进 `RunnerCtx`。
解法是 `callback.rs` 里一个线程局部槽，`onToolCall` 时登记，登记的是**同一个
`Rc` 的克隆而不是函数副本**，所以没有第二份真相：页面再换一次回调这边立刻看得见。
线程局部在这里安全（wasm 主线程，这个 crate 本来就 `Rc`/`RefCell` 满地）；代价
写在模块文档里：同页建两个 `AgentHost` 各装回调时**后装的生效**。

### 4. 验收脚手架：`web:host/callback-probe`

`dispatch.rs:182` 只对**工具表声明过**的 `web:` 名字开等待槽（否则模型编个名字就能
给自己开一个永远等不到回传的槽，会话静默挂死）。所以「页面回调」这条路想在真机上
被走到，表里**必须**先有一条内建不实现的名字——而现有三条内建全都实现了，
「已声明但内建没实现」的名字一条都不存在。

于是加了一条，`Pure`，描述里写明「验收脚手架，不是给模型日常使用的能力」。
**声明体放在 `host_tool.rs`**（`callback_probe_declaration()`），`tools.rs` 只借一行
——这条工具的全部意义就是「Rust 侧不实现它」，跟派发规则住在一起才说得清。
122 落地后整条删掉，`tools.rs` 掉一行。

红线 11 复核：表仍是编译期常量，`with_host_tools` 有 `sort_by`（`tool_table_host_tests.rs:66`
那条测试专门钉它），书写位置不影响进 prompt 的字节。**代价照实说**：这次升级之后
工具表比 120 那版多一条，旧会话第一轮的前缀缓存会断一次——任何工具表变更都如此，
122 还会再断一次。

### 5. 拆分（红线 9）

`host.rs` 加完 `onToolCall` 必然顶破 300，按**碰不碰 `live`** 这条线拆——它正是
借用纪律管辖的边界，纪律跟它管的代码住在一起：

| 文件 | 行数 | 那一件事 |
|---|---|---|
| `host.rs` | 276 → 187 | `AgentHost` 上**不碰 `live`** 的那一面：构造、装两条回调、工具表/key 长度/识图 |
| `host_session.rs` | 新 155 | `AgentHost` 上**碰 `live`** 的那一面：开/切/删会话、说一句话、取消、查身份与历史。借用纪律住这里 |
| `callback.rs` | 新 134 | 页面装进来的 JS 函数：事件 sink、store 错误 sink、工具回调的登记与调用 |
| `host_tool.rs` | 61 → 121 | 派发顺序 + 脚手架声明 |

`#[wasm_bindgen] impl AgentHost` 分两个模块写是可行的，生成的 `agent_wasm.d.ts` 里
两边的方法都在（已核对）。

### 6. 命令

- `bash scripts/build-wasm.sh --dev`：过（agent-wasm 零警告）
- `cargo test --workspace`：过（exit 0）。**第一次跑出过 5 个红**，全在
  `agent-server` 的 `http_capabilities_*_survive_restart`，单独重跑与整轮重跑都绿
  ——是那套 it harness 并发下的既有 flake，跟本条无关（`agent-wasm` 根本不是主
  workspace 成员，`cargo test --workspace` 编都不编它）。**登记在此，不在本条修。**
- `cargo test -p agent-server --features ts`：过（本条没动协议面类型，属复核）
- `bash scripts/check-invariants.sh --all`：exit 0，无违规；15 条行数提示全部是存量文件，
  本条新增/改动的四个文件全在 300 以下

### 7. 待真机（主会话跑，页面脚手架已经就位）

`www/index.html` 加了一条示例回调 `onToolCall`（+ 两个勾选框：「建宿主时装上」、
「让它 throw」），四条验收共用它。**500ms 真的过去了**这件事被做成可观测的：回调里
`performance.now()` 记起止，实测毫秒数同时写进**返回给模型的正文**和事件流日志
——同步执行不可能报出 ≥500。

| # | 怎么验 | 期望 |
|---|---|---|
| 1 | 勾「装上」建宿主 → 开会话 → 说「调用 web:host/callback-probe 并把它返回的原话告诉我」 | 模型答案里带 `PAGE-CALLBACK-OK` 和一个 **≥500 的毫秒数**；事件流有 `[tool-callback] → 真的等了 5xx 毫秒` |
| 2 | 勾上「让它 throw」再说同一句 | 这一轮**不崩页面**（后续调用仍正常，不报 `unreachable`），`tool_executed` 事件带「（错误）」，模型收到 `is_error` 后自纠或如实报告失败 |
| 3 | **不**勾「装上」建宿主，说同一句 | 工具结果是「这个宿主没有实现工具 `web:host/callback-probe`」，会话**不挂死**，这一轮正常收终态 |
| 4 | 勾「装上」，说「这个页面的标题是什么」 | 拿到的是真标题；答案里**绝不能**出现哨兵串 `SENTINEL-页面回调劫持了内建工具`。`web:page/url`、`web:source/echo` 同理 |

第 4 条是内建优先的反向锁：哨兵一旦出现在模型答案里，就是回调劫持了内建。

## 真机验收（主会话，2026-08-12，Chrome via playwright MCP + 真 Kimi key）

**四条全过。** 工具表升到 4 条并按名字排序：`web:host/callback-probe` /
`web:page/title` / `web:page/url` / `web:source/echo`。

| # | 验收 | 结果 |
|---|---|---|
| 1 | 回调真的被 `await` 了 | ✅ **`504` 毫秒**。同步执行不可能报出 ≥500 —— 这是「可等待」区别于「同步」的唯一硬证据。模型答案里同时带着口令 `PAGE-CALLBACK-OK` 与那个 504 |
| 2 | 回调 `throw` → 不崩页面 | ✅ `← web:host/callback-probe 返回 4 字节（错误）`，模型如实报告 `boom`；**紧接着的 `web:page/url` 一轮正常返回真地址**，wasm 实例没被带走 |
| 3 | 没装回调 → 不挂死 | ✅ 14.7 秒内正常收终态 `Done { truncated: false }`，工具结果是「这个宿主没有实现工具 `web:host/callback-probe`」，模型如实转述 |
| 4 | 内建优先（反向锁） | ✅ `web:page/title` 返回真标题；哨兵串 `SENTINEL-页面回调劫持了内建工具` **在整份 transcript 里一次都没出现** |

第 2 条的关键不在「模型收到了错误」，而在**紧随其后那一轮还能跑**——`host_tool.rs`
模块文档那句「panic 会带走整个页面」的理由在这里第一次被真正用上。

第 4 条的哨兵设计是对的：只断言「拿到了真标题」不够，页面回调完全可以返回一个
以假乱真的标题。哨兵串出现 = 回调劫持了内建，不出现 = 内建赢了，二值可判。

### 顺带确认了一件文档里写对的事

`onToolCall` 文档注释里那张借用表**原样进了生成的 `agent_wasm.d.ts`**（已核对），
页面作者在编辑器里就能看见「回调里调 `send()` 会 panic、调 `inspectImage()` 安全」。
这条很重要——130 的识图回调正是要在回调里调 `inspectImage()`。
