# 060 远端工具两个挂死面：未声明的 `web:` 名字进等待槽 + 等待无超时

**里程碑** 待归类（remote tools） · **依赖** — · **模型** opus · **调查 + 修复**

勘查「宿主注入能力」的地基时捞到的两条**既存**缺陷，跟那件事无关，单列。两条都让会话
**无限期卡在 `ToolsPending`**，且都不报错。

## 现象一：`is_remote()` 那一支没有 `declares()` 保护

`agent-runtime/src/dispatch.rs` 的 `Effect::ExecuteTool` 分支里，五条截获路
（spawn / collect / status / skill / mcp）**每一条都有 `ctx.tools.declares(..)` 把关**，
唯独远端那一支没有：

```rust
if request.location.is_remote() {
    ctx.register_remote_tool(agent.clone(), call_id.clone(), epoch, request.clone());
    ctx.emit(&agent, RunnerEvent::ToolExecuting { call_id, request });
    return Dispatched::Nothing;      // 挂起，不产事件
}
```

而 `location_of` 是**纯按名字**判的自由函数：`web:` / `desk:` 前缀 → `is_remote()` 为真。

于是模型只要吐一个**工具表里根本没有**的 `web:whatever/x`：位置判成 `Web` → 登记进等待槽
→ `Dispatched::Nothing` → 泵这一圈撞 B（`calls`/`mcp_calls` 都空）→ `run_turn` 返回
`ToolsPending` → actor 退回命令队列 → **永远等一个不会来的回传**。

对比：同样是模型编的名字，如果它以 `srv:` 开头，会落进 `ctx.fs.execute()` 拿一个
`unknown_tool` 错误，模型看到 `is_error` 会自纠（决策 20 的兜底）。**只有 `web:`/`desk:`
这条路把「模型编了个名字」变成了挂死。**

## 现象二：远端等待没有任何超时

`PendingRemoteTool { agent, call_id, epoch, request }`（`ctx_remote_tools.rs`）
**没有 deadline 字段**。`runner.rs` 的 `sweep_deadlines` 只扫 `Vec<ProviderCall>`；
MCP 有 `ctx.mcp_timeout` 兜底；远端工具**两者都没有**。

正常路径靠 `POST /tool_result` 回传，异常路径靠用户 `Cancel`。但「前端页面崩了 / 网关挂了 /
客户端根本没实现这个工具」这三种情况下，会话就永久停在 `ToolsPending`——
M9 的宽限取消能救 SSE/poll 断开的那一类，救不了「客户端还连着但永远不回传」。

## 范围

1. **补 `declares()` 把关**：远端那一支加 `&& ctx.tools.declares(&tool)`，与其它五条一致。
   不在表里的名字应当落进「未知工具」那条既有错误路（拿 `is_error` 让模型自纠），
   **不是**挂起。
2. **给远端等待一个截止线**：`PendingRemoteTool` 加 deadline，`sweep_deadlines` 一并扫，
   到点注入 `Event::ToolFailed`（`is_error` 的 tool_result，让模型自己收敛，别 panic）。
   超时值参数化（照 `ctx.mcp_timeout` 的形状），**默认值要 opus 判**：太短会误杀真人
   交互类工具（`ask_user_question` 本来就该等人几分钟），太长等于没有。
   **考虑按工具分类给不同默认**，或者让宿主在声明时带（见 [HOST-CAPABILITIES.md](../HOST-CAPABILITIES.md)）。

## 验收（可判定）

- 模型吐一个表里没有的 `web:nope/x` → **拿到 `is_error` 的 tool_result、loop 继续**，
  `run_turn` 有界返回；**不进等待槽**（断言 `pending_remote_tools` 为空）。
- 一个真在表里的远端工具，客户端**永不回传** → 到截止线后拿 `is_error`、轮次收尾，
  `run_turn` 有界返回（不是永久 `ToolsPending`）。
- 既有远端闭环不回归：`web_tool_result_resumes_turn.rs`、
  `http_tool_result_is_not_implemented_not_missing.rs` 全绿。
- 超时后**迟到的回传**被安全拒绝（`take_remote_tool` 找不到 → 既有的 `TransportTrouble` 路）。

## 注意

- **红线 6 邻近**：超时注入的 `ToolFailed` 要带**登记时的 epoch**（跟正常回传同款），
  过 `Session::step` 的闸。不要用「现在的 epoch」。
- **不要顺手改 `location_of` 的名字规则**——那是 050 在拍的事，两个 issue 别撞。
- `Location::Desktop` 在全仓没有任何执行路径（只是 `is_remote()` 的第二个 true 分支），
  本 issue 不动它，但修 `declares()` 把关时它会一并受益。
- 收工验证前台跑完（WORKFLOW §四 -1）。

---

## 实做记录（完成 · 2026-08-04）

两条都修了。`agent-core` **一行没改**——两个现象都在宿主侧：一个是分派处少了一道
既有的闸，一个是等待槽少了一个既有形状的截止线。`location_of` 一个字没动（050 的
地盘）。

### 改了什么

| 文件 | 行 | 干什么 |
|---|---|---|
| `agent-runtime/src/dispatch.rs` | 190（181→，+9） | 改：远端那一支加 `&& ctx.tools.declares(&tool)`，与其余五条截获路一致 |
| `agent-runtime/src/deadline.rs` | 113 | 新：截止线判定与到点注入——provider 在飞 + 远端等待两类共用一处 |
| `agent-runtime/src/runner.rs` | 288（299→，−11） | 改：`sweep_deadlines` 搬进 `deadline.rs`（红线 9：只剩 1 行余量，先拆再加） |
| `agent-runtime/src/ctx.rs` | 289（244→，+45） | 改：`DEFAULT_REMOTE_TOOL_TIMEOUT` + 字段 + `with_remote_tool_timeout` |
| `agent-runtime/src/ctx_remote_tools.rs` | 106（60→，+46） | 改：`PendingRemoteTool.deadline`、`take_expired_remote_tools`、`next_remote_deadline`、`pending_remote_tool_count` |
| `agent-runtime/src/lib.rs` | 106（104→，+2） | 改：`mod deadline` + `pub use sweep_remote_tool_deadlines` |
| `agent-server/src/actor/body.rs` | 223（185→，+38） | 改：`next_command`——有等待槽时至多等到最早那条截止线，没有槽就还是裸 `recv()` |
| `agent-server/src/actor/commands.rs` | 147（133→，+14） | 改：`handle_remote_tool_timeout` |
| `agent-server/src/registry/spec.rs` `http/config.rs` `http/state.rs` `bootstrap.rs` | +3/+5/+1/+5 | 改：`remote_tool_timeout` 一路穿过去（照 `provider_timeout` 的形状） |
| `agent-runtime/tests/remote_undeclared_tool_is_not_a_hang.rs` | 86 | 新：验收 1 + 对照组（真在表里的远端工具照旧进槽） |
| `agent-runtime/tests/remote_tool_deadline_fails_the_call.rs` | 89 | 新：验收 2 + 验收 4（迟到回传被安全拒绝） |
| `agent-runtime/tests/remote_tool_deadline_epoch_writeback.rs` | 85 | 新：红线 6（带突变验证） |
| `agent-server/tests/web_tool_never_answered_times_out.rs` | 71 | 新：验收 2 的**生产形态**（HTTP + SSE，从没人回传到轮次落终态） |
| `agent-runtime/tests/support/{mod,mcp}.rs` | 195/107 | 改：`sse_tool_call`/`sse_text` 提到公共处，`build_ctx_with` 转 `pub`；`mcp::hop_*` 改成委托 |

`agent-tools/`、`examples/java-gateway/`、`agent-server-bin/`：一行没碰
（`BootstrapOptions` **刻意不加字段**，加了会打断 bin 的结构体字面量）。

### 现象一：一行闸，但它挡的是「模型编个名字就挂死」

```rust
if request.location.is_remote() && ctx.tools.declares(&tool) { /* 登记等待槽 */ }
```

判据跟另外五条（spawn / collect / status / skill / mcp）逐字一致：**表里没有 =
这个名字不存在**，落进 `ctx.fs.execute` 的 `unknown_tool`，模型看到 `is_error`
自纠（决策 20 的兜底）。改之前 `web:` / `desk:` 是唯一一条「模型编的名字变成
永久挂起」的路——因为 `location` 是纯按名字推的，而挂起这个动作本身不产事件、
不报错、也没有任何东西会来收场。

对照组跟正例写在同一个文件里：`browser_action`（真在表里、同样是 `Location::Web`）
照旧进槽。这道闸判的是「声明了没有」，不是「远端一律不许挂起」。

### 现象二：截止线归谁驱动——这一步定错就是死锁

`PendingRemoteTool` 加 `deadline: Instant`（登记时算好的**绝对时刻**，不是
`Duration` + 起点：判过期是热路径，一次比较就够）。到点做什么：取走槽 → 发一条
`ToolExecuted { is_error: true }`（可见性跟真回传同款）→ 组 `Event::ToolFailed`
喂回泵。模型收到 `is_error` 自己收敛，不 panic。

**谁来驱动**是这个 issue 唯一需要想的地方。issue 原文说「`sweep_deadlines` 一并
扫」——实做时发现只做这一半是无效的：远端调用派出后 `Dispatched::Nothing`，泵这
一圈立刻撞上「在飞表空」收工返回 `ToolsPending`，C 那段压根不会再被执行。而泵
**不能**赖着等：远端回传是从宿主的命令队列进来的（`Command::RemoteToolResult`），
泵不走那条命令就没人收——当场死锁，`web_tool_result_resumes_turn.rs` 会立刻变红。

所以两半都要，各覆盖一半世界：

1. **`deadline::sweep`（泵里）**：泵本来就活着的时候顺手扫——root 在等远端、同时
   后台子 agent 还在飞。到点比等泵收工早。
2. **`sweep_remote_tool_deadlines`（宿主侧，公开）**：泵已经收工、控制权在命令
   队列上的那一半。`agent-server` 的 actor 空闲时问 `ctx.next_remote_deadline()`：
   `None` → 照旧裸 `rx.recv()`（**没有远端等待的会话一分钱开销不多付**，不轮询、
   不起定时器）；`Some(t)` → `recv_timeout` 到 `t`，到点扫一次再回来等。

两半共用 `deadline::expired`，产出的事件逐字节同款。`sweep_deadlines` 从
`runner.rs` 搬进新的 `deadline.rs` 是红线 9 逼的（299 行只剩 1 行），但拆得住
职责：`runner.rs` 是**泵的主循环**，`deadline.rs` 回答**「谁到点了、到点变成哪条
事件」**——这个文件本来就要因为这个 issue 从一类等待长到两类。

### 超时默认值：10 分钟，理由是两侧代价不对称

`DEFAULT_REMOTE_TOOL_TIMEOUT = 600s`。这个数**不是**「一次远端调用该花多久」的
UX 预算，它唯一的职责是保证会话不可能永久停在 `ToolsPending`——任何有限值都已经
达成目标，所以选值只按误判两侧的代价算：

- **误杀的代价高，且落在健康会话上**。`ask_user_question` 就在
  `ToolTable::standard` 里，它天生要等一个真人：读完问题、切个标签页、回来作答，
  几分钟是常态。到点注入 `is_error` 会让模型对一个**正在正常等人**的调用道歉或
  重问，用户直接看得见。
- **迟到的代价低，且落在已经坏掉的会话上**。真撞到这条线说明宿主永远不会回传，
  会话已经废了；而更快的逃生舱**本来就有**：`POST /cancel`（立刻），以及 M9 的
  宽限取消（最后一个订阅者断开 5s 后）已经覆盖「页面崩了 / 标签页关了」那一类。
  这条线只兜最后一格——**客户端还连着，但永远不说话**，而那时用户就坐在屏幕前，
  他自己按取消比任何自动超时都快。

于是取「一个真人跟一次提问打交道绝不会超过、而人又绝不愿意再多等」的量级：
10 分钟。它同时是 provider 超时（120s）的 5 倍——「人比模型慢」这件事写进数字里，
而不是写成无限期。

**不按工具分类给不同默认**（issue 原文点名要考虑的那个选项，考虑过，否了）：
位置与可逆性目前都由**名字**经自由函数推（`location_of` / `reversibility_of`），
`ToolSpec` 只有 `{name, description, schema}`，**没有任何 per-tool 元数据的位置
可挂**。在这里现造一套分类要么去动名字规则（050 的地盘，明令禁止），要么给
`ToolSpec` 加字段——而那正是 HOST-CAPABILITIES.md §四 的地盘：宿主声明自己的能力
时把截止线一起带进来才是它的正位（`ask_user_question` 这种「本来就该等人几分钟」
的判断，只有声明它的宿主知道）。所以现在给的是**一个宽松默认 + 一个可配置的口**
（`RunnerCtx::with_remote_tool_timeout`、`SessionTemplate::remote_tool_timeout`），
等声明入口落地再细分。测试用毫秒级覆盖它，不真等十分钟。

### 红线 6 的突变验证

`deadline::expired` 用的是 `pending.epoch`（登记那一刻），跟正常回传
（`resolve_remote_tool` 用 `pending.epoch`）同一份判据。探针测试
（`remote_tool_deadline_epoch_writeback.rs`）的时序：登记（epoch 0）→ 用户
`undo_turn`（**epoch bump 到 1**，这一轮回滚成 `Idle`）→ 截止线到 → 扫描。

正确实现下 `ToolFailed{epoch:0}` 撞 `Session::step` 入口的闸 `0 != 1` 被丢弃，
一个 primitive 都不写、一条 effect 都不产，宿主什么都听不到。用「现在的 epoch」
则过闸落进已回滚的 `Idle`——`Idle + ToolFailed` 是转移表 25 个非法格之一，
`Notice::ProtocolViolation` 一路发到宿主。于是断言「**一条 `ProtocolViolation`
都没有**」就是这条红线的探针。

突变（`sweep_remote_tool_deadlines` 里把事件的 epoch 换成 `session.epoch()`）：

```
running 1 test
test a_timeout_that_fires_after_an_undo_is_dropped_by_the_epoch_gate ... FAILED

---- a_timeout_that_fires_after_an_undo_is_dropped_by_the_epoch_gate stdout ----
thread '...' panicked at crates/agent-runtime/tests/remote_tool_deadline_epoch_writeback.rs:76:5:
超时注入的 ToolFailed 用了「现在的」epoch，过闸落进已回滚的世界（红线 6）：[
    Notice(
        ProtocolViolation {
            state: Idle,
            event: "ToolFailed { agent: AgentId(\"root\"), epoch: Epoch(1), call_id: ToolCallId(\"call_card\"), error: \"[remote_tool_timeout] 远端宿主在 0.06s 内没有回传 browser_action 的结果，这次调用按失败收尾\" }",
        },
    ),
]

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.09s
```

改回来：

```
running 1 test
test a_timeout_that_fires_after_an_undo_is_dropped_by_the_epoch_gate ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.09s
```

顺带给现象一也配了一次突变（拆掉 `&& ctx.tools.declares(&tool)`），确认那条验收
不是空过：

```
test a_web_name_the_table_never_declared_gets_an_is_error_result_instead_of_a_waiting_slot ... FAILED
assertion `left == right` failed: 该跑完第二跳收敛，而不是停在 ToolsPending
  left: ToolsPending
 right: Done { truncated: false }
```

（对照组 `a_declared_web_tool_still_parks_in_the_waiting_slot` 在突变下仍然 ok
——它测的是另一半，本来就不该受影响。）加回闸之后两条都 ok。

### 验收对照

| 验收 | 落在哪 | 断言 |
|---|---|---|
| 编的 `web:nope/x` → `is_error`、loop 继续、**不进等待槽** | `remote_undeclared_tool_is_not_a_hang.rs` | `Done{truncated:false}` + `pending_remote_tool_count() == 0` + `next_remote_deadline() == None` + tool_result `is_error` 且含 `unknown_tool` |
| 真在表里、客户端永不回传 → 到点 `is_error`、轮次收尾 | `remote_tool_deadline_fails_the_call.rs`（runtime，80ms 预算）+ `web_tool_never_answered_times_out.rs`（HTTP/SSE，300ms 预算） | 先 `ToolsPending` → 扫描返回 `Some(Done)`、槽清空、tool_result 含 `remote_tool_timeout`、发过 `ToolExecuted(is_error)`；HTTP 侧另断言 upstream 被请求了 2 次（超时结果真的驱动了同一轮的第二跳） |
| 既有远端闭环不回归 | `web_tool_result_resumes_turn.rs` / `http_tool_result_is_not_implemented_not_missing.rs` | 全绿 |
| 迟到的回传被安全拒绝 | `remote_tool_deadline_fails_the_call.rs` 末尾 | 超时之后 `resolve_remote_tool` 返回 `Err`（`take_remote_tool` 找不到 → 既有 `TransportTrouble` 路） |
| 红线 6 | `remote_tool_deadline_epoch_writeback.rs` | 见上（含突变红/绿） |

### 门禁（前台跑完）

- `cargo test -p agent-runtime -p agent-core -p agent-server`：**全绿，0 failed**
  （含新增 4 个测试二进制、5 条用例；`web_tool_result_resumes_turn` /
  `http_tool_result_is_not_implemented_not_missing` 在内）。
- `cargo clippy -p agent-runtime --all-targets -- -D warnings`：`Finished`，零警告。
- `bash scripts/check-invariants.sh --all`：`红线检查通过`。改动后最大的文件是
  `ctx.rs` 289 / `runner.rs` 288，全部 ≤300。
