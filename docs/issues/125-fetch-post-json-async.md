# 125 `post_json_async`：补上 wasm transport 最后一个只报错的 stub

**里程碑** M14 · **依赖** 无 · **模型** sonnet · **独测** 否（几乎全是 fetch 绑定） · **状态** 完成

## 目标

`agent-transport` 的 wasm 侧补一个 `post_json_async`。

**无依赖，第一天就能开工。**

## 现状：仓库自己写了这条 issue 该做什么

```rust
// agent-transport/src/fetch_client.rs:189-192
/// 这条缝是同步的，所以浏览器里它只能失败——没有这个 stub，`agent-tools` 在
/// wasm32 上编不过 ... 这里不配 `post_json_async`：没有调用方，而真要让浏览器里的
/// 识图工作，要动的是 `ToolExecutor` 那条同步缝，不是在这里先摆一个没人用的方法。
pub fn post_json(...) -> Result<(u16, String), TransportError> {
    Err(TransportError::Connect { ... })   // WASM_SYNC_BLOCKING_UNSUPPORTED
}
```

这条注释在两件事上是对的：不该摆一个没人用的方法；同步签名等不了 fetch。
[119](119-browser-host-capability-decision.md) 提供了它缺的那个前提——
**现在有调用方了**（[127](127-agent-host-inspect-image.md)），而且不需要动
`ToolExecutor`。

## 做什么

加 `pub async fn post_json_async(&self, url, api_key, payload) -> Result<(u16, String), TransportError>`。

**同步的 `post_json` 原样留着不动**——`agent-tools` 在 wasm32 上还要靠它编过
（`lib.rs` 顶部那段「加方法时两份都要动，这不是可选项」讲的就是这个约束）。

形状照抄同文件里已有的两个 async：`post_stream_async` 与 `upload_image_async`。
非流式，所以比 `post_stream_async` 简单——不需要 `ReadableStream`、不需要分帧。

错误分类沿用 `post_stream_async` 那套：非 200 打包成 `TransportError::Http { status, body }`，
**不在这里分类成 `ErrorClass`**（`lib.rs` 顶部「错误分类不在这里」）。

## 验收

- `cargo check --target wasm32-unknown-unknown -p agent-transport` 过。
- `cargo test --workspace` 全绿（native 侧一个字节都不该受影响——
  `git diff` 里 `client.rs`/`read_loop.rs`/`upload.rs` 应为零改动）。
- **key 不泄漏**：响应体进错误消息前必须过 `redact(&message, api_key)`，
  跟 `fetch_upload.rs:44-48` 同一条。写一条断言证明 api_key 不出现在任何
  `TransportError` 的 `Debug`/`Display` 里——**这条能在 native 上测**
  （`redact` 是纯函数，`upload.rs` 里已经是 `pub(crate)`）。
- 运行时行为由 [127](127-agent-host-inspect-image.md) 的真机验收覆盖，本条不重复。

## 注意

- `fetch_client.rs` 的 `WASM_SYNC_BLOCKING_UNSUPPORTED` 常量文案里点名了
  `post_stream/upload_image/post_json` 三个。加了 async 版之后，
  **那句文案要跟着更新**（现在它说「改调 post_stream_async/upload_image_async」，
  漏了新的这个）。文案里错一个方法名，下一个人会照着找一个不存在的方法。
- 别顺手给 native 的 `Client` 也加 `post_json_async`。native 那条是阻塞的、
  有调用方、工作得好好的，加一个没人用的 async 版就是死代码。

## 实做记录（2026-08-11）

### 做了什么

`crates/agent-transport/src/fetch_client.rs` 加了
`pub async fn post_json_async(&self, url, api_key, body) -> Result<(u16, String), TransportError>`：
单次请求、不重试（跟 native `client.rs::post_json` 一样——`post_json` 不是流式连接，
没有「服务端可能已经在生成」那条不能退避重试的理由，但也没有必要重试）；非 200
打包成 `TransportError::Http`，网络层失败打包成 `TransportError::Connect`，两条路径的
错误信息都先过 `upload::redact(&message, api_key)` 再塞进 `TransportError`（`redact`
已经是 `pub(crate)`，`upload.rs` 一个字没动）。同步的 `post_json` 原样保留，只更新了
它上面的文档注释（不再说「没有调用方」，改成指向 `post_json_async` 与 127）。
`WASM_SYNC_BLOCKING_UNSUPPORTED` 的文案按注意里说的加了 `post_json_async`。

### 判断：拆出 `fetch_json.rs`

`post_json_async` 的 web_sys 接线（建 `Headers`/`Request`、拿全局 `fetch`、读
`Response::text()`）加完之后 `fetch_client.rs` 顶到 327 行，被 `check-invariants.sh`
的红线 9 挡住。按仓库已有的架构拆分——`fetch_request.rs` 已经是「一次流式连接尝试的
`web_sys` 接线」独立文件，`fetch_upload.rs` 是「一次上传尝试」——新增
`crates/agent-transport/src/fetch_json.rs` 承载「一次非流式 JSON POST 尝试」
（`attempt_json_fetch` + `read_json_response_body`，`pub(crate)`），`fetch_client.rs`
退回到只管「要不要重试」这层策略（跟它自己模块文档说的一致）。拆完 262 行。没有复用
`fetch_request::attempt_fetch`：`Accept` 头不同（流式发 `text/event-stream`，这里发
`application/json`，跟 native `post_json` 逐字对齐），复用会把两种语义压进一个函数。

### 判断：native 可测的 redact 断言放哪

验收要求「api_key 不出现在任何 `TransportError` 的 Debug/Display 里」且要能在 native
上测，但 `redact` 是 `upload.rs` 的 `pub(crate)` 函数，只有本 crate 内部代码能调，
而 `post_json_async` 所在的 `fetch_client.rs` 整个模块在 `lib.rs` 里是
`#[cfg(target_arch = "wasm32")]`，`cargo test --workspace`（跑在 native）根本不编译
它，测试放进去等于摆设。仿照 `lib.rs` 里 `framing_parity_tests.rs` 的先例（同样是
「测试内容平台无关，但要跨模块组合」需要单独在 `lib.rs` 挂
`#[cfg(all(test, not(target_arch = "wasm32")))]`），新增
`crates/agent-transport/src/post_json_redaction_tests.rs`，在 `lib.rs` 加四行
`mod` 声明挂载它（`lib.rs` 不在原本圈定的可改文件清单里，但新建文件必然要在某处声明
`mod`，且这四行只做声明、不碰其余任何一行——判断是「新建文件（如果拆分需要）」这条
授权隐含了这一步）。两条测试直接构造 `TransportError::Http`/`Connect`，把含 key 的
字符串先过 `redact` 再塞进去，断言 `Display`/`Debug` 都不含 key——钉的是
`post_json_async` 实际做的同一条组合链路（redact 在前，塞进 TransportError 在后），
不是重新测 `redact` 本身。

### 没做到 / 明确不做的部分

- **响应体不限长**：native `post_json` 用 `MAX_JSON_BODY`/`read_bounded_error_body`
  限长读取，`post_json_async` 里 `read_json_response_body` 整段读完不截断——issue
  验收没有要求对齐这一点，且 `fetch` 没有「读 N 字节就停」的底层句柄，要做需要复刻
  `fetch_request.rs` 里私有的 `truncate_to_byte_limit`（该文件不在可改范围内）。如实
  记在这里，留给后续 issue 判断是否需要补。
- **不重试**：`post_json_async` 单次请求，没有退避循环——这是照抄 native
  `post_json` 的行为（它也没有退避循环），不是遗漏。
- **2xx 成功响应体不 redact**：只在两条错误路径 redact，成功响应体原样返回——
  `redact` 是字面字符串替换，用在业务正文上有把合法内容误伤的风险，验收原话也是
  「响应体进**错误消息**前」。

### 主会话复核修正的一处：那条 redaction 测试锁不住任何东西

实现 agent 交的第一版**实现是对的**（两条错误路径都调了 `redact`），
**测试是废的**：它自己 `redact` 一遍、自己构造 `TransportError`、再断言不含 key。
它证明的是「`redact` 这个函数管用」——而 `redact` 本来就有自己的测试。

真正要防的回归是**调用点漏掉遮罩**，第一版对它完全无感：把 `post_json_async` 里
那两句 `redact(...)` 删掉，测试照样绿。

> 锁死测试不会红就是废的——[097](097-subagent-ingredient-audit.md) §变异检验。

**改法**：新增 `crates/agent-transport/src/redacted_error.rs`（54 行），
把「遮罩 + 装进 `TransportError`」合成两个有名字的构造器
（`redacted_error::{connect, http}`），`post_json_async` 只许经它们造错误。
测试改成钉这两个构造器——**调用点实际会用到的那一个函数**。

模块挂 `#[cfg(any(target_arch = "wasm32", test))]`，跟 `line_framer`/`stream_drive`
同一套条件、同一个理由：生产调用点只在 wasm，但**钉它的测试要在 native 上跑**。

**变异检验（主会话做，不是 agent 自评）**——把两个构造器里的 `redact` 换成
`to_string()`：

```
post_json_redaction_tests::connect_error_message_is_redacted_by_the_constructor  FAILED
post_json_redaction_tests::http_error_body_is_redacted_by_the_constructor        FAILED
post_json_redaction_tests::an_empty_api_key_leaves_the_message_intact           ok  ← 测的是另一条性质，正确地不受影响
```

已还原，40 个单元测试全绿，wasm32 目标零 unused 警告。

**仍然测不到的那一半（如实登记）**：「`post_json_async` 确实走了构造器而不是
就地构造 `TransportError`」这一步没有测试覆盖——`fetch_client.rs` 整个模块
只在 wasm32 编译。靠的是那里的一句注释 + review。要变成结构性保证得给
`TransportError` 的字段套 newtype，代价大于收益。

### 验收结果

- `cargo check --target wasm32-unknown-unknown -p agent-transport`：过，仅一条
  pre-existing 警告（`backoff::sleep_cancelable` 在 wasm32 目标未使用，与本 issue
  无关，改动前就存在）。
- `cargo test -p agent-transport`：52/52（39 单元 + 13 集成），含新增的两条
  redaction 测试。
- `cargo test --workspace`：全绿（`agent-tools` 编译期间撞上另一个并行 agent
  正在写的文件，重试几次后稳定通过；不是本改动引入的问题——`git diff` 里没有
  `agent-tools/` 的任何一行）。
- `bash scripts/check-invariants.sh --all`：exit 0；报的 15 条行数超限全部是仓库里
  本来就存在、与本 issue 无关的文件（`agent-cli/mcp.rs`、`agent-core/observe.rs` 等），
  不含 `fetch_client.rs`/`fetch_json.rs`/`post_json_redaction_tests.rs`。
- `git diff --stat -- crates/agent-transport/`：只有 `fetch_client.rs`（+67/-12）与
  `lib.rs`（+14）；`client.rs`/`read_loop.rs`/`upload.rs` 零改动。
