# 123 工具执行期的取消与超时

**里程碑** M14 · **依赖** [121](121-js-tool-callback.md) · **模型** opus · **独测** 是 · **状态** 完成（真机已验收，见文末）

## 目标

回答一个在 [121](121-js-tool-callback.md) 之前**结构上不存在**的问题：
一条宿主工具挂住了，会话怎么办。

今天 `host_tool::execute` 瞬时返回，「工具执行期间取消」这个状态压根没有。
可等待之后，一个 JS Promise 能挂 30 秒、能永不 settle。

## 现状盘点（实现者先核实，别信这段）

- `agent_runtime::sweep_remote_tool_deadlines_async` **已经存在并已导出**
  （`lib.rs:156`），但 `agent-wasm/src/turn.rs` **没有调它**。
- `turn.rs:88-94` 的 `Err(_)` 分支已经能正确处理「回传对不上等待槽」
  ——取消划掉槽之后晚到的结果不会改状态。**这条今天就是对的**，
  只是没人证明过它对。
- `AgentHost.cancel()`（`host.rs:153`）只翻取消标志，碰不到 `live`,
  所以它在工具 await 期间是可调的。

## 要拍的一条

**JS Promise 没法真 abort。** 两条路选一，在实做记录里写明理由：

| | 做法 | 代价 |
|---|---|---|
| (a) | 给回调多传一个 `AbortSignal`，页面自己响应 | 页面要配合，不配合等于没有；API 面变大 |
| (b) | 不传信号，接受「取消后回调还在跑、结果被 epoch 闸丢掉」 | 回调里那次 fetch 会跑完（浪费一次调用），但会话立刻干净 |

**倾向 (b)**：它今天就已经正确工作了，本条只是把它从巧合变成有意为之并锁死。
(a) 可以后补，不冲突。

## 做什么

1. 在 `turn.rs` 的 drain 循环里接上 `sweep_remote_tool_deadlines_async`
   （或说明为什么在这个宿主形态下不需要——**同进程、宿主就是我们自己**，
   `turn.rs:11-18` 那段模块文档给了理由，但那段是在「执行瞬时完成」的前提下写的，
   前提已经变了，得重新判一次）。
2. 给宿主工具定一条截止线。**默认值由实现者定并写明理由**；它是会话可用性与
   「页面里一次上传可能真的要 30 秒」之间的取舍。
3. 超时之后：这一轮怎么收尾、模型看到什么、页面看到什么——三个都要明确。

## 验收

- **真机**：回调里挂 10 秒，中途点 `cancel()` → 这一轮**立刻**干净收尾
  （`undo_turn` 正常，页面拿到 `cancelledTurn`）；10 秒后那个 Promise resolve 时，
  **状态一个字节不变**。
- **真机**：回调返回一个**永不 settle** 的 Promise → 截止线到了这一轮有结论，
  会话不挂死，页面能继续说下一句。
- **native 可测的那一半**：`sweep_remote_tool_deadlines_async` 的行为
  （到点划掉槽、晚到回传被拒）在 `agent-runtime` 侧用现有的 native 测试手段钉住
  ——先查这条是否已有覆盖，**已有就不要重复造**，在实做记录里指出测试文件名即可。
- **反向锁**：没超时、正常返回的工具**不得**被误判成超时。一条 3 秒的工具在
  10 秒截止线下必须正常完成。

## 注意

- **别把取消标志和截止线混成一件事。** 取消是用户动作（`AtomicBool`，
  `run_turn_async` 每轮清一次），截止线是时间。两者都要能单独触发。
- `agent-wasm` 里没有 `Instant::now()` 可以随便用——114b 已经把
  `Instant`/`SystemTime` 垫成了 `web-time`，`dd23637` 那条提交还专门补过两处漏网的时钟
  并加了守卫。**先读那条提交再动时间**，不要引入第三种取时间的方式。
- 红线 1：不许把时钟读进任何 derived 的 read fn。这条工作全在 runtime/wasm 侧，
  离 core 很远，但 `check-invariants.sh` 会查，别顺手把时间传进不该去的地方。

## 实做记录（2026-08-12）

### 0. 现状核查（issue 正文那三条，逐条实测）

| # | 正文说的 | 实际 |
|---|---|---|
| 1 | `sweep_remote_tool_deadlines_async` 已导出、`turn.rs` 没调它 | **对**。导出在 `agent-runtime/src/lib.rs`（行号已变，别再记行号）；`turn.rs` 全文没有 `sweep` 字样 |
| 2 | `turn.rs` 的 `Err(_)` 分支今天就能正确处理取消后的晚到结果 | **对，但它今天根本轮不到上场**（见下） |
| 3 | `AgentHost::cancel()` 在工具 await 期间可调 | **对**：它只碰 `inner.cancel` 那个 `RefCell`，碰不到 `live`。121 的 `.d.ts` 借用表写的是准的 |

第 2 条要补一句要紧的更正：那条分支**处理的是「结果回来了、槽没了」**。而 123 之前
根本没有任何东西会在工具执行期间划掉槽——取消标志翻了也没人看，`turn.rs` 老老实实
`await` 到页面 Promise settle 为止，然后才走 `resolve_remote_tool_async`。所以
「取消后晚到的结果被拒」这条**逻辑上成立、路径上不可达**：取消之后没有任何一步会先
把槽划掉。123 之后它才第一次真的可能被走到（也仍然是第二道闸，见第 2 节）。

**结论：`sweep` 接上去是必要的，但远远不够。**「在 drain 循环里接上 sweep」这个说法
默认了「循环还在转」——而挂住的那次 `await` 就在循环体里，循环转不到下一圈，sweep
永远没机会被调用。真正要做的是**把那次 await 本身变成可打断的等待**。

### 1. 拍板：选 (b)，不传 `AbortSignal`

**(b)**：不给回调传信号，接受「取消后回调还在跑、结果被丢掉」。

理由三条，前两条是 issue 已经说过的（页面不配合等于没有；API 面变大），第三条是核实
之后新增的，也是决定性的那条：

> **(a) 解决不了这条 issue 要解决的问题。** 挂死分两种——「回调在等一个慢的东西」和
> 「回调根本不 settle」。`AbortSignal` 只对**配合的**回调有用，而永不 settle 的回调
> 按定义就是不配合的那种。会话可用性不能建立在页面守规矩之上。

**核实过 (b) 今天是否已经成立：不成立**，issue 那句「它今天可能就已经正确工作了」
只对了一半：

- 「晚到的结果不改状态」——**成立**，而且比预想的更硬。打断的做法是把那次 `JsFuture`
  丢掉，所以晚到的结果**压根回不到 Rust**（wasm-bindgen 那两个 `then` 闭包跟着 future
  一起没了）。等待槽被划掉是**第二道闸**，不是唯一那道。两道都不依赖页面配合。
- 「会话立刻干净」——**不成立**，得由本条补上：取消标志在工具执行期间没有任何人读。

(a) 将来要加就是在 (b) 之上给回调多传一个参数，不冲突，不返工。

### 2. 做法：那次 `await` 变成可打断的等待

新文件 `crates/agent-wasm/src/interrupt.rs`（177 行）。**两条出口互相独立**，各有各的
触发源、各有各的收尾，一条都不借另一条的判据：

| | 谁触发 | 判据 | 怎么被发现 | 到了怎么收 |
|---|---|---|---|---|
| 取消 | 用户 | `AtomicBool`（`run_turn_async` 每轮清一次） | `cancel()` 翻完标志**顺手叫醒**那一轮 | `cancel_pending_remote_tools_async` |
| 截止线 | 时间 | 等待槽登记那一刻算好的绝对时刻（060 的账） | 一个 `setTimeout` | `sweep_remote_tool_deadlines_async` |

**取消为什么不用轮询**：备选是每 100ms 醒一次自己看标志。否掉它的理由不是「慢一点」，
而是**后台标签页里 `setTimeout` 被节流到 1s 以上**——取消延迟会随着标签页可不可见而
变，那是最难复现的一类问题。改成 `until_settled` 每次 poll 把 `Waker` 存进一个线程
局部槽、`cancel()` 顺手 `wake()`：手法与理由跟 121 那个 `ACTIVE_TOOL_SLOT` 同款。
`host_session.rs` 里 `cancel()` 因此多了一行（本条对该文件的**全部**改动）。

**收尾没有发明任何新语义**，两条各自复用一个既有 runtime 入口，跟 `agent-server` 的
`handle_cancel` / `handle_remote_tool_timeout` 是同一句话；浏览器只是没有那条命令队列
替它调。落点是 `turn.rs` 新增的 `settle_interrupt`。

### 3. 一个不接上就会咬人的细节：截止线要按**槽**问，不是按表问

第一版想用 `RunnerCtx::next_remote_deadline()`（现成的）。**它是错的**：那给的是全表
最早的一条。同一批派出的多个调用截止线只差微秒，但「全表最早的到点了」不等于「我手里
正在执行的这条到点了」——B 到点而 A 还没到点时，A 那次**正在正常执行**的调用会被丢掉、
槽还留在表里，下一圈把同一条工具**再执行一遍**。副作用做两遍，不报错。

所以 `agent-runtime/src/deadline.rs` 新增一个 `remote_tool_deadline_in(ctx, agent,
call_id) -> Option<Duration>`，按 `(agent, call_id)` 问**这一条**槽还剩多久。

两个刻意的形状：

- **返回 `Duration` 不是 `Instant`**：把时钟读取留在 `agent-runtime` 里。114b/`dd23637`
  已经把 `Instant`/`SystemTime` 统一垫成 `web-time` 并留了守卫，`agent-wasm` 因此
  **一次时钟都不读**——不引入第三种取时间的方式，这是 issue「注意」那条的直接兑现。
- **`Some(Duration::ZERO)` ⇔ 这一刻 sweep 一定扫得到它**：两边判过期用同一条判据
  （`deadline <= now`），时间只会往前走。这条等价关系是那个循环不空转的全部依据，
  所以它被单独钉成了一条测试（见第 6 节）。

放在 `deadline.rs` 而不是 `ctx_remote_tools.rs`（`next_remote_deadline` 的邻居）有两个
理由：语义上它属于「截止线怎么判」，而且 `ctx_remote_tools.rs` 已经 283 行，加进去当场
破 300（红线 9）。

### 4. 默认截止线：**60 秒**

落在 `assemble.rs` 的 `HOST_TOOL_TIMEOUT`（跟 `BASE_SYSTEM` 同款：装配时定的一个参数），
经既有的 `RunnerCtx::with_remote_tool_timeout` 进 ctx。**比 native 默认小一个数量级**
（`DEFAULT_REMOTE_TOOL_TIMEOUT` = 10 分钟）。

两个边各由一件事定死：

- **下边界**：这个里程碑里最慢的一条合法回调——一张浏览器侧上限 2 MB 的图（119 §五-1）
  走 multipart 上传 + 一次识图往返。issue 正文点名的「一次上传可能真的要 30 秒」就是这个，
  60s ≈ 两倍余量。
- **上边界**：挂住的代价。而**浏览器这边的代价比 server 大得多**：`send()` 在整轮期间
  握着 `live.borrow_mut()`，一条挂住的回调不是「一次调用慢」，是整个 `AgentHost` 对页面
  失去响应（`sessionId()`/`historyJson()` 借不到、`deleteSession()` reject、那个 Promise
  永不 settle）。server 形态下 actor 只是空闲着，所以它忍得起 10 分钟，浏览器忍不起。

10 分钟那个数是按「另一头是个真人：读完问题、切个标签页、回来作答」定的
（`ctx.rs` 那段文档的原话）。**这一头不是真人**，是同一个标签页里的一个 JS 回调。
真要在回调里等人（`showOpenFilePicker` 之类），正确做法是回调立刻返回、把人机交互
放在工具之外——而不是把整个宿主按住 10 分钟。

取消是更快的那条逃生舱（用户一按立刻生效），所以这条线只需要兜住**没人看着**的挂死。

**不做成页面可配**：那是宿主声明能力时该一起带进来的东西（HOST-CAPABILITIES §四），
现在给的是一个固定默认 + `with_remote_tool_timeout` 这个既有的口。

### 5. 超时之后：三个视角

| 谁 | 看到什么 |
|---|---|
| **这一轮** | `sweep` 把过期槽翻成一条 `Event::ToolFailed`（**带登记那一刻的 epoch**，红线 6 的判据在 runtime 那边，wasm 侧不重抄）喂回泵，泵接着把这一轮跑完 → 落终态。**不是**永久 `ToolsPending`，`send()` 那个 Promise 正常 settle |
| **模型** | 一条 `is_error` 的 `tool_result`。普通工具的正文是 `[remote_tool_timeout][remote_tool_unclaimed_timeout] 远端工具领取超时：宿主在 60s 内没有领取 …`；`web:source/` 那条走认领，正文被 transient-source 策略换成 `SAFE_ERROR` |
| **页面** | 一条 `ToolExecuted { is_error: true }`（跟真回传落地同款，超时不新造一种可见性）+ 这一轮的终态 |

⚠️ **一处措辞不准，本条没有改**：普通工具在浏览器里不走认领（`turn.rs` 已有的裁决：
CAS 那套是为 transient-source 的幂等重放存在的），所以到点时它落在 `claim_id == None`
那一支，正文说的是「宿主**没有领取**」——而实际上浏览器宿主确实在执行它，只是没登记
认领。对模型的行为没有影响（`is_error` + 「按失败收尾」，它会自纠），但这句话在浏览器
形态下字面上不准。改它要动 `agent-runtime/src/deadline.rs` 里 server 也在用的共享文案，
不在本条范围，**登记在此**。

### 6. native 可测的那一半：已有的不重造，缺的补两条

**已有覆盖，不重复造**（验收要求指名，这里指名）：

| 文件 | 钉住的 |
|---|---|
| `agent-runtime/tests/it/remote_tool_deadline_fails_the_call.rs` | 060 验收二/四条：**到点划掉槽**（`pending_remote_tool_count() == 0`）、轮次收尾、模型拿到 `is_error`、**超时之后晚到的回传被安全拒绝** |
| `agent-runtime/tests/it/remote_tool_deadline_epoch_writeback.rs` | 红线 6：超时事件带的是登记时的 epoch，undo 之后的幽灵回写在闸前就死 |

**新增一条文件、两个测试**（`agent-runtime/tests/it/remote_tool_deadline_countdown.rs`）：

1. `a_tool_that_answers_inside_its_budget_is_never_judged_late` —— **反向锁**。600ms 的活
   在 2s 的截止线下（issue 那条「3 秒 / 10 秒」的同一个比例）：倒计时不归零、这一刻扫描
   **一个槽都不许被划掉**、正常回传被接受、`is_error` 为假、一条 `ToolExecuted(is_error)`
   都没有。顺带钉住「查无此槽 = `None`」——分不清 `None` 和「零」的话，宿主会把一条根本
   不存在的槽判成到点。
2. `the_countdown_hits_zero_exactly_when_the_sweep_expires_that_slot` —— 第 3 节那条等价
   关系。归零的**同一刻**扫描必须真的划掉它，否则就是「放弃了执行、槽还在表里」= 同一条
   工具执行两遍。

**两条都做了突变测试**（不然只是两条会绿的代码）：

- `remote_tool_deadline_in` 改成「永远返回整个预算」（倒计时永不归零）→ 只有第 2 条红，
  报 `left: Some(80ms) / right: Some(0ns)`；
- `take_expired_remote_tools` 改成「不看截止线，见槽就划」→ 只有第 1 条红，
  报「没到点的槽被扫描判了失败」。

各自只打中该打中的那一条，两条测试没有重叠。

### 7. 顺带补的一格守卫（1 行）

`wasm_clock_source.rs` 的 `WASM_REACHABLE` 里**一直没有 `agent-wasm`**——它是独立
workspace，`cargo test --workspace` 编都不编它，那条文本扫描是它唯一够得着的守卫，而它
偏偏是唯一只在浏览器里存在的 crate。本条让它第一次跟时间打交道，所以把它加进那张表，
**并做了突变验证**：往 `interrupt.rs` 塞一行 `use std::time::Instant;`，守卫当场变红并
指名到行。今天 `agent-wasm/src` 里一处 `std::time` 时钟都没有（`Duration` 是纯值类型，
守卫本来就不管它），加进去是零代价。

### 8. 改了哪些文件

| 文件 | 行数 | 改了什么 |
|---|---|---|
| `agent-wasm/src/interrupt.rs` | 新 177 | **那一件事**：一次宿主工具执行的 await 怎么被打断（取消 / 到点），以及为什么选 (b) |
| `agent-wasm/src/turn.rs` | 210 → 284 | 两条执行路各接一次 `until_settled` + `settle_interrupt` + 模块文档多一节 |
| `agent-wasm/src/host_session.rs` | 155 → 162 | `cancel()` 多一行 `interrupt::wake()`（+ 理由） |
| `agent-wasm/src/assemble.rs` | +26 | `HOST_TOOL_TIMEOUT` 常量与理由 + `.with_remote_tool_timeout(…)` 一行 |
| `agent-wasm/src/lib.rs` | +2 | `mod interrupt;` + 模块表一行 |
| `agent-runtime/src/deadline.rs` | 167 → 202 | 新增 `remote_tool_deadline_in` |
| `agent-runtime/src/lib.rs` | +4 | 导出它 |
| `agent-runtime/tests/it/remote_tool_deadline_countdown.rs` | 新 192 | 上面第 6 节那两条 |
| `agent-runtime/tests/it/wasm_clock_source.rs` | +6 | 守卫多管一个 crate |
| `agent-runtime/tests/it/main.rs` | +1 | `mod` 一行 |

**全部 < 300**。但 `turn.rs` 已经 284——**下一个动它的人（124/130）大概率要先拆**，
拆法现成：「一轮怎么开始/怎么收」和「等待槽怎么排空」是两件事。

### 9. 命令

- `bash scripts/build-wasm.sh --dev`：过（`agent-wasm` **零警告**）
- `cargo test --workspace`：**exit 0**，31 个 `test result: ok`，一条 flake 都没遇上
- `bash scripts/check-invariants.sh --all`：**exit 0**，无违规；行数提示 15 条全是存量文件，
  本条新增/改动的文件**一个都没被点名**
- `cargo test -p agent-server --features ts`：exit 0（本条没动协议面类型，属复核）
- `cargo fmt`：新增/改动的两个 wasm 文件已 rustfmt 干净（该 crate 另有 3 处**存量**
  格式漂移：`db.rs`×2、`session_id.rs`，不是本条造成的，也不顺手动）

### 10. 待真机（主会话跑）

⚠️ **不需要改 `www/index.html`**（那是 122 的地盘）。三条全部可以在控制台用
`host.onToolCall(...)` 现装一个回调完成——页面已有的那个变量名照 122 落地后的为准，
工具名同理（下面用 121 的 `web:host/callback-probe` 举例）。

| # | 怎么验 | 期望 |
|---|---|---|
| 1 | **取消**。`host.onToolCall(() => new Promise(r => setTimeout(() => r("LATE"), 10000)))` → 说「调用 web:host/callback-probe」→ 看到 `→ web:host/…` 那条事件后**立刻**在控制台 `const before = host.historyJson(); host.cancel()` | `send()` 的 Promise **当场** settle（不是 10 秒后），结果里 `cancelledTurn` 非空；12 秒后 `host.historyJson() === before` 为 `true`——那个 Promise resolve 时**状态一个字节没变** |
| 2 | **永不 settle**。`host.onToolCall(() => new Promise(() => {}))` → 说同一句 | 60 秒后这一轮**有结论**（终态，Promise settle），事件流里有 `← web:host/callback-probe …（错误）`，模型如实报告超时；**接着能正常说下一句**（会话没挂死） |
| 3 | **反向锁**。`host.onToolCall(async () => { await new Promise(r => setTimeout(r, 3000)); return "SLOW-OK-3s" })` → 说同一句 | 3 秒的工具在 60 秒线下**正常完成**：模型答案里带 `SLOW-OK-3s`，事件流里那条 `←` **不带**「（错误）」 |

第 1 条的关键**不是**「取消生效了」，是**「当场」**这两个字：123 之前它也会取消，只不过
要等那 10 秒过完才开始取消。分辨这两者的唯一办法就是掐表看 `send()` 什么时候 settle。

第 2 条要**真等满 60 秒**（默认值就是 60s，没有页面可配的口）。嫌慢就把
`assemble.rs` 的 `HOST_TOOL_TIMEOUT` 临时改成 `from_secs(5)` 重新 `build-wasm.sh --dev`
验完改回来——**改回来之后要重新构建一次**，别把 5 秒的产物留在 `www/pkg` 里。

## 真机验收（主会话，2026-08-12，Chrome + 真 Kimi key）

**三条全过。** 回调全部用控制台 `host.onToolCall(...)` 现装，没有改页面。

| # | 验收 | 结果 |
|---|---|---|
| 1 | 取消：10 秒回调中途 `cancel()` | ✅ **4 毫秒**内 settle（不是 10 秒），`status=Failed(Cancelled)`、`cancelledTurn=Applied { entries: 3, turn_id: 1 }`；此时回调**还在跑** |
| 1b | 晚到结果不改状态 | ✅ 等那条回调真的跑完（10s）之后，`historyJson()` 与取消前**逐字节相同**，哨兵 `SHOULD_NOT_REACH_MODEL` 没有出现在历史里 |
| 2 | 永不 settle 的 Promise | ✅ 73 秒收尾（60s 截止线 + 模型两跳），`Done { truncated: false }`，事件流带「（错误）」，接着能说下一句 |
| 3 | 反向锁：3 秒回调 | ✅ 正常完成，`THREE_SEC_OK` 进了历史，没被误判成超时 |

**第 1 条的 4 毫秒是本条的全部意义**。123 之前也会取消——只是要等那 10 秒过完。
实做记录里那句「光接上 sweep 没用，挂住的那次 await 就在循环体里」在这里得到证实：
waker 唤醒（不是轮询）才让取消变成当场生效。

### 顺带：那条超时文案在真机上确实误导了模型（已修）

实做记录第「两条要告诉你的」预告过普通工具走 `claim_id == None` 那支，文案字面不准。
真机上模型**照着它向用户复述了一遍**：

> 工具返回：`远端工具领取超时：宿主在 60s 内没有领取 web:host/callback-probe`
> 模型：「页面端的宿主在 60 秒内没有领取这次调用，按超时失败处理了」

浏览器宿主明明在执行它。**这段文字是给模型看的**，说「没回传」是可观测事实，
说「没领取」是对原因的猜测。主会话把 `deadline.rs` 那句改成
「`{}s 内没有收到 {} 的结果`」——在两种宿主形态下都成立（拉取式没来领 = 没回传；
同进程挂住了 = 也没回传），全仓零测试钉这个串，改动即生效。
