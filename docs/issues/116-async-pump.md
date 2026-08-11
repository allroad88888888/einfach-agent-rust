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
