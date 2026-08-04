# 070 MCP 调用被一把全局锁串行化（043 承诺修但没修）

**里程碑** 待归类（MCP / 并发） · **依赖** — · **模型** opus · **独测** ✅

文档一致性审计（`docs/DOC-AUDIT.md`）捞到的**确定 bug**，不是文档问题。

## 现象

`crates/agent-mcp/src/registry.rs` 的 `with_client` **在 `Mutex<HashMap<..>>` 的锁内执行
阻塞式 JSON-RPC 往返**：

```rust
pub fn with_client<T>(&self, id: &str, f: impl FnOnce(&McpClient) -> T) -> Option<T> {
    let guard = self.clients.lock().unwrap();   // ← 锁住整张表
    guard.get(id).map(f)                         // ← f 里是一次阻塞的 tools/call，最长 30s
}
```

于是**调 server `a` 会挡住并发调 server `b`**，最长堵到超时。整张表一把锁，粒度是「所有
MCP server」而不是「这一个 server」。

**代码自己的注释承诺过要修**（`registry.rs` 模块文档大意：「阻塞往返在锁内，043 落地异步
执行时要换成 per-client 锁/句柄借出」）——**043 发了，这条没修**。

## 为什么现在要紧

M6 刚做完时只有一个 MCP server、单 agent 串行调用，这把锁看不出来。**M8 之后不一样了**：
`spawn(background)` 让多个子 agent 真并发跑（真机 dogfood 见过三个子同时 `Thinking`），
它们各自调 MCP 就会互相排队。这已经是活的问题，不是理论风险。

## 范围

1. **把锁粒度从「整张表」降到「单个 client」**。方向由 opus 判，两条候选：
   - 表里存 `Arc<Mutex<McpClient>>`：取 `Arc` 时短暂锁表 → 立刻放开 → 在 client 自己的锁上做往返。
   - 或者 `RwLock` 表 + client 内部自己带锁。
   **注意 `McpClient` 内部持有 `StdioTransport`（子进程 stdin/stdout），同一个 client 的
   并发往返本来就必须串行**（一条管道，应答靠 id 匹配）——所以要串行的是「同一个 server」，
   不是「所有 server」。
2. **红线 3 不变**：句柄仍住 `McpRegistry`（store 外），不进任何 command/atom。
3. **不要顺手改协议或超时**（041/042/043 的地盘），只动并发结构。

## 验收（可判定）

- **并发不互相阻塞（本 issue 的全部意义）**：起两个 mock MCP server，`a` 的响应故意慢
  （如 1s），`b` 很快。**并发**发起 `a` 和 `b` 各一次调用 → **`b` 的往返不等 `a`**
  （断言 `b` 的完成时刻 < `a` 的响应延迟，用真时序，别 sleep 猜）。
  **这条在修之前必须是红的** —— 先写它、跑一次看红、再修（照 059「先红后绿」的先例，
  贴出真实的红/绿输出）。
- 同一个 server 的并发调用**仍然串行**（一条 stdio 管道，不能交错）——也要有断言。
- 既有 MCP 测试全绿不回归：`crates/agent-mcp/tests/` 全部（含真 npx `everything_server_042.rs`）、
  `crates/agent-runtime/tests/mcp_*.rs`。
- 收工 `ps` 无残留 `npx`/`server-everything`。

## 注意

- **先红后绿是硬要求**：这个 bug 的性质是「功能全对、只是慢」，不写会红的测试就没法证明修好了。
- `registry.rs` 模块文档里那句**没兑现的承诺**要一并改掉（审计的元观察：本仓有两处代码
  注释在说假话，这是其中之一）。
- 红线 9：文件 ≤300 行（`registry.rs` 现在 116 行，有余量）。
- **不要碰** `crates/agent-server/`（M10 的 062 在改）、`crates/agent-runtime/src/status_tool.rs`
  （071 在改）。
- 收工验证前台跑完（WORKFLOW §四 -1）；主 target 被占就用独立 `CARGO_TARGET_DIR`。

---

## 实做记录（2026-08-04）

### 先红后绿：bug 是真的，而且红得一点不含糊

先写会红的测试：`crates/agent-mcp/tests/registry_concurrency_070.rs`，两个 `sh` 假 server
（照 `handshake_translate_042.rs` 的手法，零网络零 npm）——`slow` 收到 `tools/call` 后
`sleep 1` 才回，`fast` 立刻回。

**时序是真的，不靠 sleep 猜**：慢调用**进到 `with_client` 的闭包里之后发一个信号**
（旧实现里那一刻整张表的锁已经攥在它手上），主线程收到信号才开始计时打 `fast`。于是
「fast 有没有被挡住」被压成一个可判定的数字。

**修之前（红）**：

```
running 2 tests
test two_concurrent_calls_to_the_same_server_stay_serialized ... ok
test a_slow_server_does_not_block_a_call_to_another_server ... FAILED

---- a_slow_server_does_not_block_a_call_to_another_server stdout ----
thread 'a_slow_server_does_not_block_a_call_to_another_server' (38544540) panicked at
crates/agent-mcp/tests/registry_concurrency_070.rs:113:5:
对 fast 的调用不该等 slow 的往返（issue 070）；fast 花了 1.011049333s，slow 花了 1.008329625s

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.03s
```

`fast` 花了 **1.011s**，`slow` 花了 **1.008s**——快 server 的往返（本身几毫秒）被慢 server
整整一秒**一毫秒不差地全额挡住**。静态分析的结论实测坐实了。

**修之后（绿）**：

```
running 2 tests
test two_concurrent_calls_to_the_same_server_stay_serialized ... ok
test a_slow_server_does_not_block_a_call_to_another_server ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.02s
```

整个二进制仍然只花 **1.02s**（就是 `slow` 那一秒），`fast` 现在是几毫秒——两条断言
（`fast < 300ms` 且 `slow >= 900ms`）同时成立，说明**不是把慢 server 也变快了**（那会是
测试写错），是它们真的并行。

### 选了候选 1：表里存 `Arc<Mutex<McpClient>>`

`with_client` 拆成两段，表锁**只用来查一次表**：

```rust
let handle = {
    let clients = self.clients.lock().unwrap();
    Arc::clone(clients.get(server_id)?)
};                                   // ← 表锁在这里就还回去了
let mut client = handle.lock().unwrap();
Some(f(&mut client))
```

**为什么不选候选 2（`RwLock` 表 + client 内部带锁）**：`RwLock` 只解决「查表这一下能不能
并发」，可 client 那把锁还是得有（下一节），于是两条候选的**内层完全一样**，差别只在外层
是 `Mutex` 还是 `RwLock`。而外层现在只包着一次 `HashMap::get` + 一次 `Arc::clone`——纳秒
量级、跟往返长度无关，换成 `RwLock` 换不来任何可测的东西，只多一个类型。真正的收益全在
「把往返挪出表锁」这一步，两条候选都做了这一步，所以取更简单的那条。

`Arc` 顺带修掉一个没人提的坑：`remove` / 覆盖式 `insert` 不会再把子进程从**在飞调用**的脚
下抽走——旧句柄活到最后一个持有者（就是那个还在等响应的调用者）落地才 drop、才杀子进程。
代价是 `remove` 的返回类型从 `Option<McpClient>` 变成 `Option<ClientHandle>`
（`= Option<Arc<Mutex<McpClient>>>`）：有并发就没有独占所有权，签名如实说出来，比
`Arc::try_unwrap(..).ok()` 那种「有人在飞就静默返回 `None`」（跟「server 不存在」撞成同一
个值）诚实。全仓 `remove` 只有 registry 自己的单测在调，没有外部调用点要跟着改。

### 「同 server 仍串行」怎么保住的

两道：**类型层**——`with_client` 给的是 `&mut McpClient`，`Mutex` 是拿到它的唯一路径，两次
往返在类型上就不可能同时在飞；**测试层**——`two_concurrent_calls_to_the_same_server_stay_serialized`
用两段**真实区间**断言：两个线程各在闭包内记下进入/离开时刻（假 server 每次 `sleep 0.3`，
区间非退化），断言两段**不相交**，再加一条「两个线程各拿到 `first`/`second` 中不同的一条」
证明应答没串号。

这条断言**能区分两个世界**：真让两次往返同时在飞的话，两个请求先后写进同一条管道，假
server 顺序处理，两个线程的区间会变成 `[0, .3]` 和 `[0, .6]`——重叠，测试红。串行时是
`[0, .3]` 和 `[.3, .6]`——不相交，绿。它在修之前就是绿的（那时靠表锁串行），修之后仍然
绿（靠 client 自己的锁），正好钉住「粒度降了、串行没丢」。

### 那句假承诺改成了什么

删掉的原文（`registry.rs` 结构体上方）：

> 锁只在单次操作期间持住……持锁期间调用方会等一整个 JSON-RPC round trip，**这是暂时的：
> 043 的异步执行路会把「发请求」和「等响应」拆开，不再需要在锁里跨一次完整往返**。

换成模块文档里的一节「两层锁」，**把 043 到底做了什么如实写出来**：

> **这里曾经写着一句没兑现的承诺**：「持锁跨一整个往返是暂时的，043 的异步执行路会把
> 『发请求』和『等响应』拆开」。043 发了，但它做的是**另一件事**——把整次阻塞往返挪到背景
> 线程（`agent-runtime/src/mcp_call.rs`），协议层照旧是「写一行、等一行」，表锁一个字没动。
> 粒度问题因此活到了 070，靠上面这两层锁修，不靠等某个未来的重构。

要点是不留新的空头支票：新文档描述的是**现在的代码**（两层锁各自负责什么、为什么内层不能
省、锁中毒的爆炸半径），没有一句「以后会……」。

`agent-runtime/src/mcp_call.rs` 的模块文档同时补了一节「背景线程之间也不互相挡」——那边原
话「锁只在这个背景线程上持住，actor/泵线程从不因此阻塞」本身没错，但只说了泵不阻塞，
**没说背景线程之间会互相阻塞**，读起来像是「没问题」。现在点明排队粒度由 registry 的两层
锁决定，以及 070 之前是什么样。

### 改动文件（行数为改后，红线 9 全部 ≤300）

| 文件 | 改了什么 | 行数 |
|---|---|---|
| `crates/agent-mcp/src/registry.rs` | 表值 `McpClient` → `ClientHandle`（`Arc<Mutex<..>>`）；`with_client` 拆成「查表放锁 / 在 client 锁上跑 `f`」；`remove` 返回句柄；模块文档新增「两层锁」一节并改掉那句假承诺；新增一条 `reinsert_replaces_the_handle_the_table_hands_out` 单测，`remove` 那条改成断言摘下来的句柄仍然活着 | 165 |
| `crates/agent-mcp/tests/registry_concurrency_070.rs` | **新增**：本 issue 的两条独测（跨 server 不阻塞 / 同 server 仍串行） | 174 |
| `crates/agent-mcp/src/lib.rs` | 导出 `ClientHandle`（`remove` 的返回类型要能被外部说出名字） | 71 |
| `crates/agent-runtime/src/mcp_call.rs` | 模块文档新增「背景线程之间也不互相挡（issue 070）」一节；**代码零改动**（`with_client` 的调用点签名没变） | 154 |

红线 3 不变：句柄仍然只住 `McpRegistry`，`Arc<Mutex<..>>` 是表内部的存法，没有任何东西进
command/atom；`registry_not_in_snapshot_042.rs` 那两条结构性证明照常绿。协议与超时一个字
没动（041/042/043 的地盘）。

### 收工验证（前台跑完，独立 `CARGO_TARGET_DIR`，真实输出）

```
### TEST: cargo test -p agent-mcp -p agent-runtime ###
exit=0
64 个 `test result: ok`；合计 passed=332 failed=0（grep 全文无 FAILED / failures: / panicked）

逐条点名（MCP 相关全部）：
  agent-mcp lib .......................... 58 passed（含 registry 的 6 条单测）
  tests/everything_server_042.rs .......... 1 passed  ← 真 npx server-everything，7.31s
  tests/handshake_translate_042.rs ........ 6 passed
  tests/initialize_041.rs ................. 4 passed
  tests/jsonrpc_codec_041.rs .............. 7 passed
  tests/jsonrpc_malformed_041.rs .......... 5 passed
  tests/mcp_config_044.rs ................. 5 passed
  tests/mcp_loader_044.rs ................. 4 passed
  tests/registry_concurrency_070.rs ....... 2 passed  ← 本 issue 新增
  tests/registry_not_in_snapshot_042.rs ... 2 passed  ← 红线 3 的结构性证明
  tests/tools_call_041.rs / tools_list_041.rs / translate_*_041.rs（4 个）... 全 ok
  agent-runtime tests/mcp_epoch_writeback.rs .. 1 passed
  agent-runtime tests/mcp_execution.rs ........ 2 passed
  agent-runtime tests/mcp_undo_barrier.rs ..... 2 passed

### CLIPPY: cargo clippy -p agent-mcp --all-targets -- -D warnings ###
    Checking agent-mcp v0.1.0 (/Volumes/work/self/einfach-agent-rust/crates/agent-mcp)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.05s
（0 warning）

### CLIPPY: cargo clippy -p agent-runtime --all-targets -- -D warnings ###
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 25.21s
（0 warning）

### CHECK: cargo check -p agent-cli --all-targets ###
    Checking agent-cli v0.1.0 (/Volumes/work/self/einfach-agent-rust/crates/agent-cli)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.20s
（agent-cli 是 agent-mcp 的另一个下游；`remove` 换签名后确认它不受影响——全仓
 `remove` 的调用点只有 registry 自己的单测）

### INVARIANTS: bash scripts/check-invariants.sh --all ###
红线检查通过
规则与理由：docs/INVARIANTS.md
```

**收工 `ps`**：无 `server-everything` / `modelcontextprotocol` 残留（`everything_server_042`
跑完 7.31s 就被 `StdioTransport::Drop` 杀干净收尸了），无 `sh -c read …` 假 server 残留，
无属于本 issue 的 cargo/rustc 孤儿（当时在跑的那个 `cargo test -p agent-server -p agent-runtime`
是并行会话的，不是本 issue 的）。全程用 scratchpad 下的独立 `CARGO_TARGET_DIR`，没碰主
`target/`。
