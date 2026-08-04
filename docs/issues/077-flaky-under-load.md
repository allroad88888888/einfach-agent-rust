# 077 测试套件在高负载下会假红——其中一条可能根本不是测试的问题

**里程碑** M10 期间捞到 · **依赖** — · **模型** opus · **独测** ✅（碰计费与截止线两条护栏）

076 收工代收时发现的。主会话连跑 `cargo test -p agent-server --features ts`，**同一份代码、
同一台机器**，有时全绿（197 passed）、有时红一条，而且**两次红的还不是同一条**。

## 现场（都是真实输出）

### 一、`web_tool_never_answered_times_out`（060 的截止线护栏）

```
thread 'a_web_tool_the_host_never_answers_is_failed_at_its_deadline' panicked at
crates/agent-server/tests/web_tool_never_answered_times_out.rs:69:5:
远端调用该在截止线上被判失败（is_error），而不是无声无息地永远等下去
test result: FAILED. 0 passed; 1 failed; ... finished in 0.05s
```

单独跑 **8/8 全过、每次 0.32s**。红的那次 **0.05s——比正常还快**，说明循环在看到超时之前
就撞上终态跳出了。

**这条已经处理**（本 issue 不再欠它）：诊断不够，报文一口咬定「截止线没生效」，
把人指向 060 的 `remote_tool_timeout`——**而改宽它正好把护栏拆了**。已经改成先分流
「这一轮是怎么结束的」：`Failed` → 明说是 provider 调用自己没成功、跟截止线无关；
`Done` 才轮到截止线的账。断言一个字没放松。

### 二、`two_sessions_dont_cross`（**这条才是重点**）

```
thread 'two_sessions_run_concurrently_without_crossing_events' panicked at
crates/agent-server/tests/two_sessions_dont_cross.rs:39:5:
assertion `left == right` failed
  left: 2
 right: 1
```

第 39 行是 `assert_eq!(server_a.request_count(), 1)`。**一轮对话打了两次上游请求。**

假上游的脚本只有一条文字回复，模型答完就是终态——**一轮就该一次**。

## 为什么第二条不能当 flake 糊过去

如果高负载下运行时真的会**重复发一次 provider 请求**，那不是测试抖动，是**同一轮的钱付两遍**。
而且它的症状只在**账单**上——功能完全正常，事件流看不出，日志看不出。这正是本仓
把红线 11（每轮全价）、红线 6（在飞 effect 带 epoch）列成红线而不是建议的那一类：
**错了不报错，只在钱上浮出来。**

「测试写得不严」和「运行时真的重发了」是两个完全不同的结论，**必须分清再下判断**。

## 要查什么（按顺序，别跳）

1. **先稳定复现**：`--no-fail-fast` 连跑 N 轮，或人为加负载（并行跑满 CPU）。
   拿到能重现的命令再往下走，**不要靠读代码猜**（059 hub 泄漏的先例：静态分析怀疑 →
   实测坐实，而且实测推翻了主会话给的候选）。
2. **分清是「谁」发的第二次**：`FakeServer::request_count()` 数的到底是**HTTP 请求**
   还是**连接**？如果 keep-alive 断了重连也计数，那第二次可能根本不是 loop 发的。
   **这一步不搞清楚，后面全是空谈。**
3. 若确实是 loop 发的第二次，往这三个方向查（都带 `文件:行号` 给证据）：
   - **transport 层有没有重试**（超时/连接错误重发）——如果有，它是有意的吗？
     重试一次 provider 调用意味着**同一轮付两遍钱**，且模型可能已经产出过一次内容。
   - **缓存兜底三层**（024）会不会在某个 verdict 下触发一次 rebuild + 重发。
   - **泵的静止条件**（`runner.rs` 的 `calls.is_empty() && mcp_calls.is_empty()`）
     在竞态下有没有可能把同一次 `CallProvider` 派两遍。
4. **顺带扫一遍别的负载敏感测试**：本轮只是随机撞上两条，不代表只有两条。
   把 agent-server / agent-runtime 的测试连跑若干轮，**列出所有出现过红的**。

## 验收（可判定）

- **给出结论并有证据**：第二条是「测试写得不严」还是「运行时真的重发」，**明说**，带行号。
- 若是**测试问题** → 修测试（但**不许放松语义**：`request_count()==1` 这条断言的价值就是
  「一轮一次」，不许改成 `<=2` 或删掉；该改的是构造或计数口径）。
- 若是**运行时问题** → **单列新 issue**（本 issue 只负责查清并定性），并给出最小复现。
- 无论哪种，**扫描结果要留下**：跑了多少轮、哪些测试出现过红、各几次。
  「跑了 3 轮没红」不算扫过。
- 既有测试全绿；不许为了让套件稳定而给任何断言开后门。

## 注意

- **不要用「加 sleep」当修法**。sleep 只是把假红的概率压低，护栏还是不可信，而且会让
  套件变慢——下次负载再高一点它又回来了。
- **不要靠重跑到绿收工**（WORKFLOW §四 -1 的精神）：flaky 会训练人「再跑一次」，
  而那正是真回归溜过去的方式。
- 红线 9：≤300 行。
- 收工验证前台跑完，含 `--features ts`（收工清单那条）。

---

## 实做记录（2026-08-04）

### 结论：**是测试写得不严，不是运行时重发**

`FakeServer::request_count()` 数的**不是 HTTP 请求，是被 accept 的 TCP 连接**——而且是
「连请求都没读到也照记一条」的那种。第二次上游请求**确实是 loop 发的**
（`agent-core/src/command/transitions/provider_failed.rs:42`），但它是**假服务器先把一条
好连接弄坏**之后的正常重试，而且**有声**（`Notice::Retrying`）。运行时没有在任何一条
正常路径上重复付钱，红线 11 那一类「只在账单上浮出来」的病**不存在**，因此**不开新
issue**。

### 根因：accept 出来的 socket 继承了 listener 的 O_NONBLOCK

`crates/agent-server/tests/support/server.rs:38` 把 listener 设成非阻塞（为了 accept 循环
不被单条连接挡住）。**BSD/macOS 上 `accept()` 返回的 socket 会继承这个标志**（Linux 不
继承）。于是每个连接线程里那段「阻塞式」读其实是非阻塞的，一条正常连接会这样烂掉：

- `server.rs:117` `if reader.read_line(&mut line).unwrap_or(0) == 0 { return String::new(); }`
  —— `WouldBlock` 被 `unwrap_or(0)` 吞掉、当成 EOF，返回**空串**。
- `server.rs:96` `guard.push(body)` —— 这条空串**照样记账**，`request_count()` +1。
- `server.rs:97` `guard.len() - 1` 当脚本下标 —— 空账**吃掉一个脚本槽位**，后面所有
  脚本响应**整体错位一格**。
- `server.rs:100` 照样把响应写完就关连接 —— 客户端那条**还在路上的真请求**从此没人读，
  close 撞上未读数据变成 RST。

所以只要「accept 早于请求字节落地」这一瞬发生（高负载下客户端在 `connect` 和 `write`
之间被抢走 CPU 就够了），一条连接就同时干三件坏事：**多记一次假请求 + 错位脚本 + 把
客户端那次真调用弄坏**。三件事各自都能让不同的测试红——这就是「每次红的还不是同一条」
的来源。

### 先红后绿：决定性实验（不是静态分析）

临时诊断（查完已删）：起 `FakeServer`，裸 `TcpStream` 连上去，**故意睡 200ms 再发请求**。
socket 若真是阻塞的，服务端就该老老实实等这 200ms 把真请求读出来：

```
client write result = Ok(())
client read result = Ok(159), resp_len = 159
count = 1
#0 len=0 body=""

panicked at ...: 记下来的应该是真的请求体，不是空串
  left: ""
 right: "{\"x\":1}"
```

`count=1` 且 `body=""` —— 服务端**在请求到达之前就「读完」了**。修之前红、修之后绿。

**发生率实测**：一个 server + 300 条连接 × 4 进程并发（机器同时在跑别的测试），
**1 / 1200 条连接**。一轮 `cargo test -p agent-server` 有几百次连接，跟「几轮红一次」对得上。

### 第二次请求是谁发的：loop 发的，而且是有声的

也是实测。自造一个上游：**第一条连接一个字节都不读就把脚本应答写完再关**（正是上面那种
畸形连接对客户端呈现的样子），第二条起正常读。跑一轮真会话：

```
CONNS = 2          ← 服务端接了两条连接
REAL_BODIES = 1    ← 只有一条带了真请求
TEXT = "real-reply"
EVENT Notice(TurnStatusChanged { status: Thinking })
EVENT TransportTrouble("Invalid argument (os error 22)")
EVENT Notice(Retrying { attempt: 1, max_retries: 2 })
EVENT TextDelta("real-reply")
EVENT Notice(TurnStatusChanged { status: Done { truncated: false } })
```

链路逐段带行号：

| 段 | 位置 | 干了什么 |
|---|---|---|
| 连接坏掉 | `agent-transport/src/read_loop.rs:102` | IO 错误 → `StreamOutcome::Broken` |
| 归类 | `agent-runtime/src/provider_call.rs:142` | `Broken` → `ErrorClass::Retryable` |
| 决定重试 | `agent-core/src/command/transitions/provider_failed.rs:39` | `Retryable` 且预算没耗尽 |
| **喊一声** | 同上 `:44` | `Notice::Retrying { attempt, max_retries }` |
| 真发 | `agent-core/src/command/transitions/mod.rs:105` | `Effect::CallProvider` |

**关键：`provider_failed.rs:39` 那个条件只有传输真的失败了才成立。** 正常一轮走不到这里。
假红那次 `assert_eq!(text_a, "from-A")`（`two_sessions_dont_cross.rs:33`）**是过的**——
第一次尝试一个 delta 都没吐出来就死了，正印证「第一条连接死在响应之前」。

### 任务书点名的三个方向，逐条排掉

1. **transport 层有没有重试**：有，`agent-transport/src/client.rs:114-119`，但只对
   `ConnectAttemptError::Connect`（DNS / 拒连 / 握手）退避；而且**测试里
   `Backoff { max_attempts: 1 }`**（`tests/support/mod.rs:36-40`）——本仓测试路径上传输层
   **一次都不重试**。不是它。
2. **缓存兜底三层（024）会不会 rebuild + 重发**：不会。`provider_call.rs:94-99`
   拿到 `DriftVerdict::Unexpected` 只 `emit` 一条告警，注释写着「照发不拦」，没有任何重发。
3. **泵的静止条件会不会把同一次 `CallProvider` 派两遍**：不会。全仓 `Effect::CallProvider`
   **只有一个产出点**（`transitions/mod.rs:105`），`session.step` 全程跑在 actor 单线程上；
   `runner.rs:136` 一个 effect 推一张凭据、`runner.rs:286` 按 agent 取走即删。竞态只存在于
   「IO 线程往 channel 发消息」那一侧，产不出第二次派发。

### 顺带：脚本错位才是 060 那条的死法

`server.rs:97` 拿 `guard.len() - 1` 当脚本下标，假请求占掉 `idx=0` 之后重发那次真请求拿到
的是 `scripts[1]`。`web_tool_never_answered_times_out.rs:42-45` 的脚本是
`[工具调用回复, 文字回复]`，错位之后模型第一句就是文字、根本没有远端工具在等，于是
「0.05s 就到终态、`saw_timeout` 为假」。本次扫描里原样复现到了这一条（`Done` 结尾）。
主会话已经把那条的**诊断**改对了（先分流「这一轮是怎么结束的」），本 issue 补上的是
**它为什么会那样结束**——那条断言一个字都不用再动。

同一个错位在 `queued_input_is_processed_in_order.rs:26` 上是另一副面孔：
`left: "second-reply" right: "first-reply"`，第一条输入直接拿到了第二条的脚本。

### 改了什么（断言一个字没动）

`two_sessions_dont_cross.rs:39` 的 `request_count() == 1` **原样保留**：没改成 `<=2`、
没删、没加 sleep。改的是**计数口径和构造**。三份假服务器同一处病、同一个修法：

| 文件 | 改了什么 |
|---|---|
| `crates/agent-server/tests/support/server.rs` | accept 之后 `set_nonblocking(false)`；`read_request_body` 改返回 `Option<String>`，读不到请求返 `None`；`handle_connection` 见 `None` 直接丢连接——**不记账、也不消耗脚本槽位** |
| `crates/agent-server/tests/http_indep_support/fake_upstream.rs` | 同上（`handle_one` / `read_request_body`） |
| `crates/agent-cli/tests/indep_support/fake_server.rs` | 同上。它的 `bodies()` 文档本来就写着「第 N 个就是第 N 次网络请求」（`:76`），这次是让代码兑现这句话 |

**不是新发明的修法**：`agent-transport/tests/fake_sse.rs:260` 早就在 accept 之后写了
`stream.set_nonblocking(false).unwrap()`——那份假服务器一直没这毛病，另外三份是抄手法时
把这一行漏了。

`request_count()` 的语义从此是「**到达的 HTTP 请求数**」，跟全仓 25 个调用点本来就假定的
含义一致（包括 `agent-cli/tests/indep_unresolved_tool_recovery.rs:59` 那条
`request_count() == 0`——旧口径下它随时可能被一条幽灵连接打红）。

### 扫描结果：**修前 20 轮 × 2 包**（`--features ts`，外加 8 个自旋进程压满 CPU）

「跑了 3 轮没红」不算扫过，所以两边各跑满 20 轮、同样的负载配方。**修前**：

| 测试 | 红几次 | 报文（都是幽灵连接的指纹） |
|---|---|---|
| `two_sessions_run_concurrently_without_crossing_events` | 1 | `two_sessions_dont_cross.rs:39` `left: 2 right: 1`（**本 issue 点名那条，原样复现**） |
| `repeated_chatid_reattaches_to_the_live_session_without_clearing_history` | 2 | `http_chatid_sessions.rs:41` bodies = `["", 真体, 真体]` |
| `closed_chatid_recovers_history_from_its_default_session_file` | 2 | `http_chatid_sessions.rs:69` 同款 `["", ...]` |
| `undoing_past_the_declaration_takes_the_injected_tools_out_of_the_table` | 1 | `http_capabilities_survive_restart.rs:238` 请求体不是 JSON：EOF at column 0 |
| `a_recovered_session_brings_its_declared_tools_back_without_being_told_again` | 2 | 同上，同一行 |
| `a_recovered_session_brings_its_declared_skills_back_without_being_told_again` | 1 | `http_capabilities_skills_survive_restart.rs:170` 同款空体 |
| `a_disabled_builtin_is_invisible_here_and_untouched_next_door` | 1 | `http_capabilities_disable_builtin.rs:177` 同款空体 |
| `a_declaration_only_reaches_the_session_that_declared_it` | 1 | `http_capabilities_scoped_to_one_session.rs:111` 同款空体 |
| `undoing_the_activation_takes_the_body_out_of_the_next_round` | 1 | `http_capabilities_skills_survive_restart.rs:99` 计数提前满足 → 读到错的那一条 |
| `activating_one_skill_injects_only_its_own_body_and_tools` | 1 | `http_capabilities_skills.rs:104` 同上 |
| `two_inputs_sent_back_to_back_both_run_and_in_submission_order` | 1 | `queued_input_is_processed_in_order.rs:26` 脚本错位 |
| `a_web_tool_the_host_never_answers_is_failed_at_its_deadline` | 1 | `web_tool_never_answered_times_out.rs:85` 脚本错位 → `Done` 收尾 |
| `undo_turn_outcome_frame_and_upstream_body_no_longer_carries_the_undone_turn` | 1 | `http_indep_undo_redo_wire.rs:53` `left: 3 right: 2` |
| `browser_action_result_is_matched_then_resumes_the_waiting_turn` | 1 | `web_tool_result_resumes_turn.rs:24` 等不到帧 |
| `undo_blocked_by_a_shell_barrier_carries_the_tool_name_and_call_id_over_sse` | 1 | `undo_blocked_frame_carries_tool_and_call_id.rs:79` 没撞上 barrier |
| `two_children_run_in_parallel_and_the_parent_waits_for_both` | 2 | `subagent_parallel.rs:82` —— **不是幽灵连接**，见下 |

agent-server 20 轮里 **10 轮红过**，共 15 条测试；agent-runtime 20 轮里 **2 轮红过**，
只有 `subagent_parallel` 一条。

**唯一的例外：`two_children_run_in_parallel_and_the_parent_waits_for_both`。**
它用的是 `agent-runtime/tests/support/routed.rs`，那份的 listener 是**阻塞**的
（`routed.rs:82` `listener.incoming()`），没有本 issue 这个病；报文里那个 `""` 是
`subagent_parallel.rs:84` 打印的**路由 needle**（root 首跳那条路由的 needle 就是空串），
不是请求体，虚惊一场。它红的真实原因是 `overlapped()`——一条**挂钟重叠**断言
（029 验收「并行是真的，不是跑得快」），在人为把 CPU 压满时两个子 agent 的服务区间会被
调度器错开。**没动它**：这条断言的价值就是挂钟重叠，放宽等于拆掉 029 的验收；
而唯一「稳」的办法是把脚本延迟拉长，那就是本 issue 明令禁止的加 sleep。如实记在这里，
供后续判断值不值得换一种证法。

### 扫描结果：**修后 20 轮 × 2 包**（同样的负载配方）

**agent-server（`--features ts`）20 轮：0 红，全绿。** 修前红过的那 15 条一条都没再出现。

**agent-runtime 20 轮：1 轮红，只有 `two_children_run_in_parallel_and_the_parent_waits_for_both`**
一条（`subagent_parallel.rs:82`，报文一字不差还是那句 `["", "任务B", "任务A", "结果A"]`）。
修前 2/20、修后 1/20 —— **同一个量级，本来就该没变**：它用的是另一份假上游
（`agent-runtime/tests/support/routed.rs`，listener 本来就是阻塞的），压根不在本次修法的
射程内。**它不属于幽灵连接那一族**，别把两件事当成一件去修：那个 `""` 是
`subagent_parallel.rs:84` 打印的**路由 needle**，不是请求体。

**为什么 20 轮（或 10 轮）就够，不用更多**：修前 agent-server 的**每轮**红率是
**10/20 = 50%**。若修法完全无效，10 轮全清的概率是 `0.5^10 ≈ 0.1%`，20 轮全清是
`0.5^20 ≈ 0.0001%`。也就是说 10 轮全绿已经把「碰巧没撞上」压到千分之一以下，再往后买到的
额外置信度极其有限，而时间是实打实的。本次实际跑满了 20 轮（启动得早，跑完了就如实记
20），**结论上 10 轮即可收工**。

### 收工验证（主会话代跑，2026-08-04）

| | |
|---|---|
| `cargo test --workspace --no-fail-fast` | **1537 passed / 0 failed** |
| `cargo test -p agent-server --features ts --no-fail-fast` | **197 passed / 0 failed** |
| `cargo clippy --workspace --all-targets -- -D warnings` | **0 error** |
| `scripts/check-invariants.sh --all` | 红线检查通过 |

**代跑时抓到一处主会话自己的错**，一并记着：本 issue §一 那段诊断改进
（`web_tool_never_answered_times_out.rs`）是主会话写的，第一版留了
`value assigned to 'ending' is never read` 和 `this 'if' statement can be collapsed`
两个 clippy error——**测试是绿的、clippy 是红的**。已改成 `let ending = loop { … break … };`
加 let-chain。

这跟 043 那次是同一个形状（测试全绿、clippy 红），也再一次说明**收工清单四条要一起跑**：
少跑哪一条，红就藏在哪一条里。
