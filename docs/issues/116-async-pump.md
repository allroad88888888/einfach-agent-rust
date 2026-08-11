# 116 泵 async 化：引 `futures` 最小子集，native 先跑通

**里程碑** M13 · **依赖** 115 · **模型** sonnet · **独测** ✅（碰核心执行路径）

115 拍板「一套路径、两边都 async」之后的第一步。**本 issue 完全不碰 wasm**——
做完的状态是「native 侧泵已经是 async 的，行为一字未变」，这就是它可独立验证的中间态。

## 为什么先在 native 上做完

泵在 `agent-runtime` 核心执行路径上，同时改「异步化」和「换目标平台」，出了问题分不清是谁的。
先把 async 化在 native 上验到 `cargo test --workspace` 全绿，wasm 才有资格进场（117 之后）。

## 范围

1. **引 `futures` 最小子集**：`futures-core` + `futures-util`，**不要全量 `futures`**。
   限定子集是 115 的明确决定——不给「核心路径可以随便引 async 库」开口子。
   加进 `agent-runtime` 的 `Cargo.toml`；`agent-cli` 也需要（见 3）。
2. **`runner.rs` 的泵 async 化**：`run_turn` 变 `async fn`，`receive()` 里的
   `rx.recv_timeout(POLL_INTERVAL)` 改成 await。
   > **本 issue 只改「怎么等」，不改「等什么」**——`io_thread` 仍然是 `std::thread`，
   > channel 仍然是 `sync_channel(0)`。换载体是 117 的事。
   > 所以这一步要在「同步 channel」与「async 泵」之间搭一座临时的桥，
   > 桥怎么搭由实现者定，但**要在实做记录里写明它是临时的、117 会拆掉**。
3. **调用方跟着改**：`agent-cli` 现在没有任何 async 运行时，需要一个 `block_on`
   （`futures_util::executor::block_on` 即可，不用自己写）；`agent-server` 已有 tokio，
   在它的 runtime 里 await 即可。
4. `agent-server` 的 axum/SSE 层**不动**。

## 验收（可判定）

- `cargo test --workspace --no-fail-fast` —— 除 `agent-server` 那条**既有失败**
  （`http_image_input::text_stays_on_old_wire_shape_and_attachment_reference_survives_recovery`，
  在未修改的 HEAD 上同样失败，112/113 都验过）之外全绿。
- **CLI 真机跑一轮对话**：`cargo run -p agent-cli`，模型能回话、能调工具、`Ctrl-C` 取消后
  进程还活着。async 化最容易坏的就是「取消」和「进程收不了工」，测试不一定抓得到。
- **前缀缓存没断**：第 2 轮起 `cached_tokens / prompt_tokens ≥ 0.9`（M1 验收的老指标）。
  async 化不该碰 prompt 组装，但这条能兜住「不小心改了工具表次序」这类事故。
- `rg 'futures' crates/*/Cargo.toml` 只命中 `agent-runtime` 与 `agent-cli`，
  且只有 `futures-core` / `futures-util`，**没有全量 `futures`、没有 tokio 新增**。

## 注意

- **碰红线 11**（工具表在 prompt 最前面，是前缀缓存地基）。async 化不该改 prompt 组装，
  但「不该」不等于「不会」——验收里那条 `cached/prompt ≥ 0.9` 就是为此。
- **碰红线 6**（epoch 回写）。`(agent, attempt)` 的认领与晚到丢弃逻辑一行都不该动，
  本 issue 只改等待方式。
- `agent-cli` 的 `Ctrl-C` 走 `ctrlc` crate 设标志位，async 化后**确认那条路还通**
  ——它现在依赖泵在 `recv_timeout` 超时后回到循环顶部去看标志。

## 实做记录（2026-08-11）

**状态：完成。** `cargo test --workspace --no-fail-fast` 全绿（除既有的
`http_image_input` 那条，已 `git stash` 验过在未改动的 HEAD 上同样失败）。

### 临时桥搭在哪，117 要拆什么

桥**只在 `runner.rs::receive` 这一个函数体内**，没有沿着调用链散开：

- `io_thread`/`sync_channel(0)` 一字未动。
- `receive` 从 `fn` 改成 `async fn`，函数体里还是原来那句
  `rx.recv_timeout(POLL_INTERVAL)`——同步阻塞，**没有真正的 `.await` 点**。
  这是刻意选的最小改动：调用方（`run_turn`/`resume_after_first_commit`）真
  正 async 化、能被外面 `.await`，但泵内部「怎么等」这一步的血肉完全没变。
- `run_turn`/`run_turn_with_images`/`resume`/`resume_after_first_commit` 以及
  `remote_tool.rs`/`remote_tool_submission.rs`/`deadline.rs` 里另外四个公开
  函数（`resolve_remote_tool`/`cancel_pending_remote_tools`/
  `submit_remote_tool_result`/`sweep_remote_tool_deadlines`，它们都会兜回
  `runner::resume`/`resume_after_first_commit`）全部变成 `async fn`，但**只是
  逐句加 `.await`，一行控制流没改**（逐文件 diff 已确认：改动只有
  `fn`→`async fn` 和插入 `.await`）。

117 要拆的就是 `receive` 函数体那几行：把 `io_thread` 换成并发 future、
`sync_channel(0)` 换成 `futures` 的 mpsc 之后，`receive` 要变成对新 channel
的真正非阻塞 `.next().await`。除了这一个函数，`runner.rs` 和那四个远端工具
函数的其余部分不需要再动——它们已经是「形状对了，内容还没换」的状态。

### 一处偏离 115 原文，已验证：`futures_util::executor::block_on` 不存在

115 与本 issue都写「`futures_util::executor::block_on` 即可，不用自己写」——
**实测这个路径不存在**（`cargo build` 报 `E0432: unresolved import
futures_util::executor`，换 `features = ["executor"]` 后 cargo 直接报
`futures-util` 没有这个 feature，可选 feature 列表见 `Cargo.lock`）。
`executor`/`block_on` 属于独立的 `futures-executor` crate（`futures` 全量门面
才转发成 `futures::executor`），`futures-util` 本身从来没有这个模块。

加 `futures-executor` 能解决，但会在「futures 最小子集」之外再添一个 crate，
直接违反验收第 4 条的字面要求（`rg` 只认 `futures-core`/`futures-util`）。
115 原文自己留了口子——「`block_on`（约 30 行）自己写完全没问题，错了当场
暴露；risky 的是手写会合 channel」——所以改成手写：
`crates/agent-runtime/src/block_on.rs`，`std::task::Wake` 官方文档给的教科书
形状（`Arc<ThreadWaker>` + `thread::park`/`unpark`），约 60 行含文档注释、
两个单测（一个测 `Ready` 直接返回，一个测至少经过一次 `Pending`+`wake`）。
经 `agent-runtime` 的 `lib.rs` 导出为 `agent_runtime::block_on`，`agent-cli`
与 `agent-server` 都调这一个函数，不是各写各的。

`futures-core`/`futures-util` 仍然按 115 的决定留在 `agent-runtime` 与
`agent-cli` 的 `Cargo.toml`（验收第 4 条要求它们出现在这两个 crate 里）——但
**目前没有代码真正用到它们**，是为 117 要接的 `futures_util::channel::mpsc`
预留位置。两个 Cargo.toml 里都写了这一点，别被这两行依赖误导成「已经在用
futures 的组合子」。

### 调用方怎么接线

- `agent-cli`：`repl.rs` 与 `main.rs` 两处调用点分别包一层
  `agent_runtime::block_on(...)`。
- `agent-server`：**不是**「已有 tokio runtime 里 await」——session actor
  （`actor/mod.rs`）是裸 `thread::Builder::spawn` 起的 OS 线程，从未经过
  `tokio::spawn`/`rt.enter()`，`tokio::runtime::Handle::current()` 在那条线程
  上会直接 panic。所以跟 `agent-cli` 是同一个手法：`actor/commands.rs`（4 处：
  `handle_input`/`handle_cancel`/`handle_remote_tool_result`/
  `handle_remote_tool_timeout`）、`actor/remote_tools.rs`（`submit`）、
  `actor/body.rs`（恢复阶段那次 `cancel_pending_remote_tools`）一共 6 个调用点
  各包一层 `agent_runtime::block_on`。`agent-server` 的 `Cargo.toml` 没有新增
  任何依赖——`block_on` 是从 `agent-runtime` 借来的，不是自己拉的 `futures-util`。
- axum/SSE 层（`http/`、`hub/`）一行没动，范围条款 4 兑现。

### 测试怎么接线（60 个测试文件，机械改动，逐条列在这里而不是逐文件贴）

`run_turn`/`run_turn_with_images`/`resolve_remote_tool`/
`cancel_pending_remote_tools`/`submit_remote_tool_result`/
`sweep_remote_tool_deadlines` 变成 `async fn` 之后，`agent-runtime/tests/it/`
下 51 个用例、`agent-cli/tests/it/` 下 3 个用例原来直接同步调用这些函数——
逐个改成 `agent_runtime::block_on(原调用)`，**只改这一层包装，调用参数、
返回值处理、断言一个字没动**（脚本化改的：找函数名+左括号，配对括号插入
`agent_runtime::block_on(` / `)`，对每个改动点跑过 `cargo build --tests`
确认零编译错误；6 个调用点原文已经写成 `agent_runtime::run_turn(...)` 全限定
路径，脚本第一版误插出 `agent_runtime::agent_runtime::block_on(run_turn(...))`
的重复前缀，已手工修成 `agent_runtime::block_on(agent_runtime::run_turn(...))`，
逐个 diff 核对过）。这些改动全部是「函数从同步变 async 之后接线方式跟着变」，
不碰任何断言、任何期望值。

另有一处非机械的测试代码改动：`agent-runtime/tests/it/support/mod.rs` 里
`build_ctx` 的文档注释原文说「`run_turn` 是同步阻塞的」，改成「经
`block_on` 跑，仍然阻塞调用线程」——纯文档措辞，不是断言，说明的还是同一个
事实（回调只在调用线程上被喊到，不需要 `Arc<Mutex<_>>`）。

### 验收四条的实际结果

1. **`cargo test --workspace --no-fail-fast`**：110 passed / 1 failed（
   `agent-server` 的 `http_image_input::
   text_stays_on_old_wire_shape_and_attachment_reference_survives_recovery`），
   其余全部 crate 全绿。`git stash` 到未改动的 HEAD 单独跑这一条，同样
   panic 在同一行（`等待第 1 个模型请求超时`），确认是既有失败、不是本次
   引入。
2. **CLI 真机对话**：**没有跑到「模型能回话」这一步**——环境里唯一能找到的
   `DEEPSEEK_API_KEY` 是无效的（真实请求返回 HTTP 401 `invalid_request_error`），
   没有别的可用 provider 凭据（仓库只支持 deepseek/kimi/glm 三家，找不到
   `providers.toml`，也没有 kimi/glm 的 key）。跑出来的部分信号：连续 4 轮
   对话在 401 报错下 REPL 循环都正常继续（`[连接异常] HTTP 401 ...` →
   `[本轮失败: Provider(Auth)]` → 回到 `>` 提示符），中途真实发送 `SIGINT`
   给进程后 `kill -0` 确认进程存活，之后仍能继续接受输入直到 `/quit` 干净
   退出（wait 后进程自然消失，没有靠超时兜底 `kill -9`）。这证明了「取消不
   炸、进程收得了工、错误路径不卡死」，但**没有验证真实模型对话与工具调用**
   ——如实记录，不算过这一条。
3. **前缀缓存 `cached/prompt ≥ 0.9`**：**没跑**，同上——没有一次成功的模型
   请求，没有 usage 数据可看。
4. **`rg 'futures' crates/*/Cargo.toml`**：只命中 `agent-runtime`（
   `futures-core`/`futures-util`）与 `agent-cli`（`futures-util`）的依赖声明
   行（另外几行是同一文件里解释这件事的注释）；`agent-transport/Cargo.toml`
   的 `wasm-bindgen-futures = "0.4"` 是子串误命中，113 就有、这次没碰，跟
   `futures-core`/`futures-util` 无关。没有新增 `tokio`（`rg 'tokio'
   crates/agent-runtime/Cargo.toml crates/agent-cli/Cargo.toml` 零命中）。

### 遗留问题

- 验收条 2、3 没有真正跑通，需要一把有效的 provider key 才能补——补测不需要
  再动代码，只需要重跑「CLI 真机跑一轮对话」那一步。
- `runner.rs` 在本 issue开始前就已经 332 行（超出 300 的硬上限），本次加了
  文档注释后到 376 行。它是事件泵本体、单一状态机，够得上「复杂文件」候选
  （上限 500），但没有为它正式走一遍认定；本 issue 范围只改「怎么等」，没有
  去拆分或重新认定这个文件，原样标记给下一次触碰它的 issue。


## 验收补跑（2026-08-11，拿到可用 key 之后）

当初欠的条 2、条 3 已经跑掉。在 `ba9b70b` 的干净 worktree 上、用真 DeepSeek
（`deepseek-v4-pro`）跑的，不是假 server。

**条 2 · CLI 真机跑一轮**：模型正常回话；`shell_macos` 工具被调用并拿回输出
（`echo hello-from-tool-116` → `hello-from-tool-116`）；`/quit` 干净退出。

**条 3 · 前缀缓存没断**：

| 第几次 provider 调用 | prompt | cached | cached/prompt |
|---|---|---|---|
| 1（冷启动，不计） | 6399 | 0 | — |
| 2 | 6446 | 6272 | **0.973** |
| 3 | 6541 | 6400 | **0.978** |
| 4 | 6561 | 6528 | **0.995** |

第 2 次起全部 ≥ 0.9，达标。运行时自己的漂移检查也一路报
「发前比对：该复用的段逐字节没变」「对账：预测 X / 实际 X，一致」——
这正是红线 11 要守的东西，async 化确实没碰 prompt 组装。

**SIGINT 那一条改在 117 的 tip 上验**（117 supersedes 116 的同一条路径），见 117 文档。
