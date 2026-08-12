# 124 `drain_host_tools` 认得 transient-source 工具

**里程碑** M14 · **依赖** [119](119-browser-host-capability-decision.md) · **模型** sonnet · **独测** 是 · **状态** 完成（真机已验收，见文末）

> **跟 [120](120-host-tool-async.md) 只有协调关系，没有逻辑依赖**：两条都动
> `turn.rs` 的 drain 循环，别同时在飞。本条要的东西（`drain_host_tools` 已经是
> `async fn`、`submit_remote_tool_result_async` 已经导出）今天就都有了。

## 目标

让浏览器宿主能执行 `web:source/` 前缀的工具。这是图片能走 transient-source 那一整套
机制（[119](119-browser-host-capability-decision.md) §三）的唯一接线工作。

**跟图片无耦合，可以立刻开工。**

## 现状：一个今天必然失败的分叉

```rust
// agent-wasm/src/turn.rs:88
match resolve_remote_tool_async(session, ctx, waiting.agent, waiting.call_id, output).await
```

而 `agent-runtime/src/remote_tool.rs:63-68`：

```rust
pub async fn resolve_remote_tool_async(...) -> Result<TurnStatus, ResolveRemoteToolError> {
    if ctx.pending_remote_tools.pending.iter().any(|pending| {
        ...&& crate::transient_source_policy::is_transient_source(&pending.request.tool)
    }) {
        return Err(ResolveRemoteToolError::InvalidResult(...));   // ← 显式拒绝
    }
```

transient-source 的正门是 `submit_remote_tool_result_async`（`lib.rs:179` 已导出）。
**浏览器宿主今天调的是被拒的那一个。**

## 做什么

1. `turn.rs` 的 drain 循环按工具名分流：`web:source/` → `submit_remote_tool_result_async`，
   其余 → `resolve_remote_tool_async`。
2. ⚠️ **`is_transient_source` 是 `pub(crate)`**（`transient_source_policy.rs:15`），
   `agent-wasm` 在另一个 crate 里够不着。两条路选一，**在实做记录里写明理由**：
   - 导出一个判定函数（前缀常量仍留在 `agent-runtime` 内部）
   - 让 `submit_remote_tool_result_async` 统一吃两类，调用方不需要判

   **不许在 `agent-wasm` 里重抄一份 `"web:source/"` 字面量**——两份前缀常量哪天
   被改歪一个，症状是「安全策略静默失效」：入参和结果照常进历史，不报错。
3. `turn.rs:44-47` 那段注释**要作废**。它现在写着：

   > **这个宿主的工具表里没有任何 transient-source 工具**（[`crate::tools`] 只声明
   > 两条 `web:`），所以它结构上不可达

   本条落地之后就可达了，`TransientSourceFailure` 那条**从没在浏览器里跑过的路**
   会第一次亮。注释改成说明它现在可达、以及页面会看到什么。

## 验收

- **native 可测（主证据）**：这条分流逻辑本身可以在 `agent-runtime` 侧测——
  构造一个带 `web:source/x` 等待槽的 ctx，断言走 `resolve_remote_tool_async`
  拿到 `InvalidResult`、走 `submit_remote_tool_result_async` 成功。
  **先查是否已有覆盖**，已有就指出文件名，别重复造。
- **真机**：硬编码一条最小的 `web:source/echo` 工具（回调原样返回入参），
  跑一轮，然后断言三件事：
  1. 模型拿到了真实结果（工具确实执行了）
  2. **历史里那条 `ToolUse` 的入参是 `{"transient_source":"redacted"}`**，不是真入参
  3. **历史里那条 `ToolResult` 是 `[transient_source_result_redacted]`**，不是真正文

  第 2、3 条是这条 issue 存在的全部理由——它们证明白捡的那套机制真的在工作。
- **反向锁**：普通 `web:page/title` 走完之后，历史里是**真入参和真结果**，
  没有被误当成 transient-source 遮掉。

## 注意

- `Effect::CallProvider` 起飞时对 one-shot 请求做的**安全重编码**
  （`provider_call.rs:176-194`）会跟着生效：前缀镜像、漂移报告、缓存预测都用
  占位历史重算一遍，只有第一份 body 的字节发给 provider。
  **这条不用你做任何事，但你要知道它在**——真机验收时那一轮会是全价重编码，
  第 1 层判读会报 `Intentional` 漂移而不是 `Reuse`，**那是预期内的，不是 bug**。
- 别在这条里加图片。图片是 [130](130-browser-vision-end-to-end.md)。
  本条的 `web:source/echo` 是一次性的验收脚手架，落地后可以删也可以留，
  留的话在 `tools.rs` 里注明它是验收用的。

## 实做记录

### 接口选择：导出判定函数，不是让 submit 统一吃两类

§做什么第 2 点给了两条路。选了**导出判定函数**：
`crates/agent-runtime/src/transient_source_policy.rs` 的 `is_transient_source`
从 `pub(crate)` 改成 `pub`（+11 行，主要是文档），`lib.rs` 加一行
`pub use transient_source_policy::is_transient_source;`。`SOURCE_TOOL_PREFIX`
常量本身保持私有不动。

没选「`submit_remote_tool_result_async` 统一吃两类」，因为深入读了协议才发现
两条路的协议形状差得远，不只是「判一下走哪边」：`resolve_remote_tool_async`
是免认领的简单二选一（`RemoteToolOutput::Success/Failure`），
`submit_remote_tool_result_async` 要求先 `claim_remote_tool`（CAS + 幂等重放/
指纹记录，为服务端多客户端竞争设计）。真把「统一吃两类」做到底，意味着**普通
`web:page/title` 也要走一遍认领协议**——多付一份状态开销（`RemoteToolReceipts`
记录、`submission_id`/指纹）换不来任何东西，浏览器宿主是唯一认领者，这层保护
在这里天然是摆设。导出判定函数是更小的改动面：只多一个纯函数，`resolve_remote_
tool_async` 服务的既有工具（`web:page/*`）协议形状一个字节没动。

### 认领不是协议形式，是拿到真入参的唯一路径

分流决定之后发现一个 issue 原文没细说但绕不开的事实：等待槽投影
（`ctx.pending_remote_tools()`）里 `web:source/*` 工具的 `request.input`
**永远是 dispatch 派发时脱敏过的占位符**（`{"transient_source":"redacted"}`，
`dispatch.rs:183-193`）——这是脱敏机制本身的设计，保护的是历史/prompt。所以
`drain_transient_source`（`turn.rs`）必须先 `claim_remote_tool` 拿到
`RemoteToolClaimGrant::request`（未脱敏的真入参，`remote_tool_claim.rs` 里
`request_for_grant` 对 transient-source 工具走 `ctx.transient_sources.raw_input`
这条特殊解析），再拿这份真请求去执行工具，最后带着同一个 `claim_id` 调
`submit_remote_tool_result_async`。这条「先认领再执行」的顺序参考了
`crates/agent-runtime/tests/it/transient_source_chain.rs` 里既有的
`claim_remote_tool` + `submit_remote_tool_result` 用法。`claim_id`/`submission_id`
是调用方现铸的不透明字符串（协议本身不认识它们的内容），单进程宿主没有并发
竞争，按 `call_id` 派生（`wasm-drain-claim:{call_id}` /
`wasm-drain-submit:{call_id}`）即可，不需要随机数/UUID 依赖。

### `TurnStatus` 之外新增的错误通道现在真的可达

`drain_host_tools` 签名从 `TurnStatus` 改成
`Result<TurnStatus, TransientSourceFailure>`，`run()` 里那句从
`let status = drain_host_tools(...).await;` 改成加 `?`。原因：
`ResolveRemoteToolError::TransientSource` 和 `submit_remote_tool_result_async`
的 `Err` 通道过去都靠「工具表没有 transient-source 工具」结构性挡住，
124 落地后工具表有了 `web:source/echo`，两条路都必须把这个 `Err` 老实冒泡给
`run()`，不能吞掉——`run()` 的返回类型本来就是
`Result<Outcome, TransientSourceFailure>`，链路是现成的，只是之前从没被真正
用到过。`turn.rs` 顶部 `run` 函数的文档注释已经按此改写（不再声称「结构上不
可达」）。

### 验收脚手架：`web:source/echo`

`crates/agent-wasm/src/tools.rs` 加了 `SOURCE_ECHO_TOOL = "web:source/echo"`
（`Reversibility::Pure`，schema 是任意 JSON 对象），`host_tool::execute` 加一个
匹配分支原样回显 `waiting.request.input`（在 transient-source 路径下这已经是
认领后的真值）。落地后**留着没删**——130（浏览器识图端到端）要接的是同一条
`web:source/` 缝，这条先把缝踩实，130 可以直接复用同一套 drain 逻辑，不用
重新验证一遍协议对不对。

### 改了哪些文件

- `crates/agent-runtime/src/transient_source_policy.rs`（+11 行）：
  `is_transient_source` 可见性 `pub(crate)` → `pub`，加文档。
- `crates/agent-runtime/src/lib.rs`（+3 行）：re-export 一行。
- `crates/agent-wasm/src/turn.rs`（84 → 210 行）：`drain_host_tools` 按工具名
  分流；新增 `drain_transient_source`（认领+执行+回传）与
  `to_submit_outcome`（`RemoteToolOutput` → `RemoteToolSubmitOutcome` 的纯搬运）；
  `run()` 与 `drain_host_tools` 的文档注释按上面两条重写。
- `crates/agent-wasm/src/tools.rs`（91 → 116 行）：加 `SOURCE_ECHO_TOOL` 声明。
- `crates/agent-wasm/src/host_tool.rs`（39 → 57 行）：加 echo 分支，模块文档
  同步改「两条」为「几条」、`resolve_remote_tool` 改为两条路都提。
- 全部文件仍 ≤300 行，无需拆分。

### 独立测试 agent 的产出

`crates/agent-runtime/tests/it/transient_source_resolve_rejection.rs`（新建，
176 行）+ `tests/it/main.rs` 加一行 `mod`。两个测试：
`resolve_remote_tool_rejects_a_transient_source_call_and_leaves_it_claimable`
（构造一个 `web:source/peek` 等待槽，断言 `resolve_remote_tool` 拿到
`InvalidResult` 且槽位没被消费，再用 `claim_remote_tool` + `submit_remote_
tool_result` 正常收尾）与
`resolve_remote_tool_completes_an_ordinary_web_tool_with_real_content_in_history`
（反向锁：普通 `web:page/title` 走 `resolve_remote_tool` 直接成功，历史里是真
内容）。写测试的 agent 没有看过上面这份实现，只给了 issue 验收原文 + 公开
API 签名 + 既有测试文件路径当参考——这是这条 issue 的主证据，因为
`agent-wasm` 侧的分流逻辑本身在 wasm32 独立 workspace 里，`cargo test
--workspace` 覆盖不到。

### 已知的局限（如实记录，不是含糊带过）

- **真机验收没有做**。issue 验收里那三条（模型拿到真实结果、历史里的 `ToolUse`
  入参被换成 `{"transient_source":"redacted"}`、`ToolResult` 被换成
  `[transient_source_result_redacted]`）需要 Chrome + 真 key 跑一轮
  `web:source/echo`，这次会话没有浏览器环境可用，**待真机**。脚手架
  （`SOURCE_ECHO_TOOL`）已经就位，下一次有真机环境时可以直接跑，不需要再写
  代码。
- **`host.rs` 里 `send()` 上方那句注释现在过期了但没有改**：那段注释还写着
  「M12 的 transient-source 出口（见 `turn::run`：这个宿主的工具表里结构上不
  可达）」——这句话在本条落地之后不准了（`run` 的文档已经在 `turn.rs` 里改
  过）。`host.rs` 不在本条被允许触碰的文件列表里（而且落地时该文件正被另一条
  并行 issue 大改，行号已经漂了，故意不写死行号），如实记在这里，留给下一个
  碰这个文件的 issue（或专门开一条文档修正）顺手带上。
- 编译期验证覆盖了 native workspace（`cargo test --workspace`，全绿）+
  `cargo check --target wasm32-unknown-unknown -p agent-wasm` + 完整
  `bash scripts/build-wasm.sh --dev`（产出 `www/pkg`，可以用
  `python3 -m http.server` 起服务人工点开验证两个既有工具 + 新工具都在
  `toolTableJson()` 里，但**没有跑真实的一轮对话**）。

## 真机验收（主会话，2026-08-11，Chrome via playwright MCP + 真 Kimi key）

**三条断言 + 反向锁全过。** provider = kimi / kimi-k3，会话 `m14-echo-1`。

工具表按名字排序后是 `web:page/title` / `web:page/url` / `web:source/echo` 三条
（`with_host_tools` 的排序生效，`web:source/` 排在 `web:page/` 之后）。

### 断言 1：模型拿到的是真结果

提示词让模型调 `web:source/echo` 并传 `{"probe":"MARKER_7F3A9"}`。页面事件流：

```
→ 调用宿主工具 web:source/echo  input={"transient_source":"redacted"}
← web:source/echo 返回 34 字节
assistant  工具返回的内容是：{"probe":"MARKER_7F3A9"}
```

**注意这两行的对比**：宿主看到的事件里入参是脱敏的，模型拿到的却是真入参回显。
这正是实做记录里那条发现（认领才拿得到真入参）在真机上的样子——**如果当初用了
`waiting.request.input`，这里回显的会是「redacted」**，而且一切看起来都正常。

### 断言 2、3：journal 里落的是占位符

直接读 IndexedDB 的 `journal` store 解出所有 `ToolUse` / `ToolResult` 块：

| 工具 | `ToolUse.input` | `ToolResult.content` |
|---|---|---|
| `web:source/echo` | `{"transient_source":"redacted"}` | `[transient_source_result_redacted]` |
| `web:page/title`（反向锁） | `{}`（真） | `agent-wasm 浏览器宿主（issue 114c）`（真） |

**两条工具在同一个会话里并排**，一条脱敏一条不脱敏——这比分两次跑更能说明分流是按
名字前缀走的，不是「整个会话都脱敏」或「整个会话都不脱敏」。

### 一个验收方法上的教训（值得留给下一个人）

第一次核查用的是「全文搜索 journal 里有没有 `MARKER_7F3A9`」，**结果命中了，
差点误判成泄漏**。原因是那个标记出现在两个完全合法的位置：用户自己在提示词里打的，
以及模型在终答里复述的。

transient-source 保证的是「**工具的入参与结果**不进历史」，不是「这段内容永远不许
出现在历史里」。**断言必须钉在位置上**（`ToolUse.input` / `ToolResult.content`），
不能用全文 grep。echo 这类「原样返回入参」的工具结构上没法用全文搜索验证——
[130](130-browser-vision-end-to-end.md) 的识图工具可以（识别结果是模型和用户都
没说过的新文本），那里适合再补一条全文断言。
