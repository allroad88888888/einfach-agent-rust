# 043 执行路由 + 可逆性元数据 + epoch 回写

**里程碑** M6 · **依赖** 042 · **模型** opus · **独测** ✅

MCP 工具第一次真的被模型调起来。三件事焊在这一个 issue 里，因为它们互相咬合：dispatch
的第四路、工具表的可逆性映射、在飞结果的 epoch 校验。碰红线 6，是 M6 唯一的 opus。

## 范围

1. **dispatch 第四路**（`agent-runtime/src/dispatch.rs` 的 `Effect::ExecuteTool`）：tool 名
   以 `mcp:` 开头且工具表声明了它 → 起一次**异步 MCP 调用**（发给 `McpRegistry` 里对应
   server 的 client），返回在飞凭据，**不走 `ctx.fs`**。参照 spawn/skill 的截获位置与
   `provider_call::start/finish` 的在飞机制。
2. **工具表可逆性映射**（`agent-runtime/src/tool_table.rs`）：`ToolTable` 携带 `mcp:` 工具的
   `name → Reversibility` 映射（041 翻译产出、042 握手时装进来）。`snapshot()` 撞 `mcp:`
   前缀查映射，**查不到落保守 `Irreversible`**。`location_of("mcp:...")` → `Server`
   （宿主本地执行 MCP 调用）。
3. **epoch 回写**（红线 6）：MCP 调用在飞时用户 undo → epoch bump → 结果回来 epoch 不符
   **丢弃不回写**。复用异步路泵已有的 epoch 校验，不新写一套。

## 验收（可判定）

- `snapshot("mcp:everything/echo", ...)` 在映射里标 readOnly → `Pure`；`mcp:everything/x`
  不在映射 → `Irreversible`；`location` 恒 `Server`。
- 模型发起 `mcp:` 调用 → dispatch 走第四路（不进 `ToolExecutor`），结果作为 `tool_result`
  进下一轮 prompt；MCP 调用失败（server 返回 error / 超时）→ `is_error` 的 `tool_result`，
  loop 继续（和 spawn refuse 同精神，不 panic 不卡死）。
- **红线 6**：mock 一个慢 MCP 响应，调用在飞期间 bump epoch（模拟 undo），响应回来时
  **被丢弃、状态不含它**——断言回写前的 epoch 比对真的挡住了幽灵结果。
- readOnly 的 MCP 工具结果 entry **无屏障位**，`/undo` 能干净越过；非 readOnly 的落屏障，
  `/undo` 撞它停下推 `undo_blocked`（复用 020/027 的既有屏障机制，MCP 不新造）。

## 注意

- **红线 6**（在飞 effect 带 epoch、回写前校验）：这是本 issue 违反后**不报错**的那条——
  幽灵结果写进已回滚的世界，偶发依赖时序。**派独立测试 agent**，且必须有一条「在飞时
  bump epoch，结果被丢弃」的断言（测试能把静默失败变红，但设计本身是 opus 判断：
  在哪一点比对、比对失败怎么收尾）。
- **红线 3**：client 句柄来自 `McpRegistry`（store 外），dispatch 只拿 server id 去查，
  不把句柄塞进任何 command / atom。
- **红线 12 不适用但要写清**：按 `mcp:` 前缀分派在**宿主侧**（dispatch），不是 core；
  和 spawn/skill 截获同款合法性（宿主持有工具表，无模型相关判断）。
- 异步在飞凭据：MCP 调用的凭据和 `ProviderCall` 是两类东西（一个是工具结果、一个是模型
  响应），泵的在飞表要能同时容纳两类并各自按 epoch 落地——设计时想清楚它们怎么共存。
- 接口先定 → 实现与测试并行 → 合并。

## 实做记录（完成 · 2026-08-03）

三件事焊在一个 issue 里，全部落地：dispatch 第四路、工具表可逆性映射、epoch 回写。

### 建了什么

| 文件 | 行 | 干什么 |
|---|---|---|
| `agent-mcp/src/tool_result.rs` | 77 | 新：`tools/call` result → `ToolCallOutput{text,is_error}`。MCP wire 形状（`content` 块数组、`isError`）**留在 agent-mcp 里**，runtime 只接一段文本 + 一个布尔（接缝：wire 不过缝） |
| `agent-runtime/src/mcp_call.rs` | 145 | 新：第四路的**异步在飞机制**（`start`/`finish`/`take`），跟 `provider_call` 同款——起背景线程跑阻塞 `tools/call`，泵落地。**红线 6 的回写点：`finish` 把 credential 的 epoch 原样盖回事件** |
| `agent-runtime/src/dispatch.rs` | 226 | 改：`Effect::ExecuteTool` 加第四路截获 + `start_mcp` 助手；`Dispatched` 加 `McpCall` 变体 |
| `agent-runtime/src/tool_table.rs` | 238 | 改：`ToolTable` 携带 `mcp:` name→`Reversibility` 映射（`with_mcp` 装入），`snapshot` 撞 `mcp:` 查映射、查不到落 `Irreversible`，`location_of` 显式 `mcp`→`Server` |
| `agent-runtime/src/runner.rs` | 279 | 改：泵加**第二张在飞表** `mcp_calls`，收工条件两表都空，`receive` 认领 `McpDone` |
| `agent-runtime/src/io_thread.rs` | 139 | 改：`IoMsg` 加 `McpDone{agent,call_id,content,is_error}`（两类生产者共用泵 channel） |
| `agent-runtime/src/ctx.rs` | 244 | 改：`RunnerCtx` 加 `mcp: Arc<McpRegistry>`（红线 3，store 外）+ `mcp_timeout`，`with_mcp`/`with_mcp_timeout` 建造器 |
| `agent-mcp/src/{protocol,lib}.rs`、`agent-runtime/src/lib.rs`、`Cargo.toml` | — | 导出 `flatten_tool_result`；`agent-runtime` 依赖 `agent-mcp`、`mod mcp_call`、`pub use McpRegistry` |
| 拆分（红线 9） | — | `ctx.rs`/`tool_table.rs` 顶破 300 → 单测各挪进 `ctx_tests.rs`/`tool_table_tests.rs`（`#[path]` 子模块，源文件只留实现，和既有 `standard_local_tests.rs` 同款） |
| 测试 | — | `tests/mcp_execution.rs`（第四路+失败 is_error）、`tests/mcp_undo_barrier.rs`（屏障/无屏障）、`tests/mcp_epoch_writeback.rs`（**红线 6 对抗**）、`tests/support/mcp.rs`（假 server + wire 脚本共用件）、可逆性快照单测在 `tool_table_tests.rs` |

### 接口决策：泵的在飞表怎么同时容纳两类凭据

**两张并列的同质 `Vec`，共用一条泵 channel**——不是一个 `enum InFlight`。

- `calls: Vec<ProviderCall>`（既有，按 `agent` 认领——一个 agent 最多一次 provider 调用，那是 `Thinking` 的唯一性）
- `mcp_calls: Vec<McpCall>`（新，按 `(agent, call_id)` 认领——一轮里模型可以并列发多个工具调用，工具槽不是唯一的）

两类凭据载荷真不一样（provider 带 drift/预测命中/adjustments/前缀镜像；MCP 带工具名/call_id），落地成不同事件（`ProviderDone` vs `ToolResult`/`ToolFailed`），认领键也不同。做成一个 `enum` 会逼既有 provider 路的每一处访问（`take_call`/`sweep_deadlines`/`receive`/`speak_for_root_on_cancel`）都改成 match+解构，动了在跑的代码却零收益。两张同质表让每次查找是一句 typed `position()`，provider 路一个字节没动。收工条件 = 两表都空；`IoMsg` 加一个 `McpDone` 变体，两类生产者（provider IO 线程 / MCP 背景线程）发同一条 `sync_channel(0)`，泵按各自的键落地。

MCP 没有泵级截止线（provider 有）：`tools/call` 自带客户端侧超时（`ctx.mcp_timeout` 传给背景线程），线程必在超时内报回一条 `McpDone`（成功/错误/超时都算），所以 MCP 凭据一定会被排空，不需要在泵里扫截止线。

### 红线 6：epoch 在哪一点比对

**回写点**：`agent-core/src/command/step.rs:71` —— `if event.epoch().is_some_and(|e| e != self.epoch) { return Vec::new(); }`。这道闸是**既有的**（M2 就在），MCP **复用**它，不新写一套。

MCP 侧要做的只有一件事：让在飞结果带上**起飞那一刻的 epoch**。链路是——
1. dispatch 第四路起飞时，`mcp_call::start` 把当时的 `epoch` 存进 `McpCall` credential（背景线程只报内容，伪造不了 epoch）。
2. 结果回来，`mcp_call::finish` 把 `call.epoch` **原样盖回** `Event::ToolResult`/`ToolFailed`。
3. 这条事件进 `Session::step`，撞上面那道闸：调用在飞时用户 undo/cancel bump 了 epoch，`event.epoch != self.epoch` → 丢弃、不写 primitive、不落 entry、不发通报——幽灵结果进不了已回滚的世界。

关键设计选择：`Dispatched::CancelAll` **不清** `mcp_calls`（跟不清 `calls` 同款）。取消只 bump epoch + 清待办队列，在飞 credential 留着——好让迟到的结果**回来撞那道闸被正当丢弃**，而不是在泵这层无声抹掉。这样红线 6 的闸是真的被走到的那条路，`tests/mcp_epoch_writeback.rs` 断言的正是这一点（`ToolExecuted` 证明结果确实回来了 + 消息历史没有它 = 闸挡住了；把闸拆掉这条立刻红）。

### 红线 3 / 12 怎么守住

- **红线 3**：`client` 句柄来自 `McpRegistry`（store 外的进程内表），dispatch 只把 `Arc<McpRegistry>` + 从工具名解析出的 server id 交给背景线程，`with_client` 借出 client 跑往返——句柄从不进任何 command/atom。锁只在背景线程上持住，actor/泵线程从不因此阻塞。
- **红线 12**：按 `mcp:` 前缀分派在**宿主侧**（dispatch），不是 core。core 的 `provider_done` 对任何 `ToolUse` 块一视同仁地发 `Effect::ExecuteTool`，不认识 `mcp:`（grep 不到）；宿主持有工具表，按名字截获跟 spawn/skill 同款合法。

### 坑

- `ctx.rs`(300)/`tool_table.rs`(295) 本就贴着红线 9，这次加字段/方法必然顶破 → 把各自的 `#[cfg(test)] mod tests` 挪进 `#[path]` 子文件（源文件只留实现），不是硬塞。
- `tool_result.rs` 独立成文件而非塞进 `protocol.rs`：加进去 protocol 顶破 300，且「result → 扁平文本」是独立职责（一个文件一件事）。

### 收工验证（主会话代收，真实输出）

**过程如实记**：impl agent 又犯「收尾自旋」——把 `cargo test` 甩给后台等监视器，turn 结束时
验证没跑完、本节留了「待填」占位、且连着起了第二个等待器空转（顽固失效模式，见 048 同款）。
主会话 `TaskStop` 掐掉自旋、外科手术杀 orphan cargo（`36507`，只杀本 issue 的树，excel 会话
另一 manifest/target 不碰），重跑三道门禁定验：

```
### TEST (agent-runtime + agent-core + agent-mcp) ###
test result: ok.  ... 0 failed  （全绿；含 tests/mcp_execution.rs、mcp_epoch_writeback.rs、
                                   mcp_undo_barrier.rs 三个 MCP 集，及 agent-core/agent-mcp 全量）
### CLIPPY (agent-runtime -D warnings) ###
   一开始 RED：mcp_call::start 8/7 参数触发 clippy::too_many_arguments
   → 主会话修：加 #[allow(clippy::too_many_arguments)]，与本 crate 既有同款
     （io_thread.rs:73 / ctx.rs:76 皆此写法，是线程发射类函数的既定房风；start 要把
      owned 值 move 进背景线程，显式列参比借 &ctx 更清楚）→ 重跑 Finished、0 warning
### INVARIANTS ###
红线检查通过
```

三道门禁最终全绿（test / clippy / invariants）。**代收教训重申**：红线 6 的对抗测试写得好
不等于收工——clippy `-D warnings` 那道门 impl 自旋跳过了没确认，恰好是红的；WORKFLOW §四 -1
「前台跑完再交」就是拦这个。主会话补跑 + 修 + 复验后 043 方为真done。
