# 022 打通一家 provider：`cargo run` 能跟模型说上话

**里程碑** M1 · **依赖** 025 · **模型** sonnet · **独立测试 agent** 否 · **状态** 完成

## 目标

跑一个二进制，输入一句话，模型的回复流式打在终端上。**M1 第一个「能用了」的点。**

接缝与一家的 encode/decode/stream 在 [025](025-provider-seam.md) 已对录制帧全绿，
本 issue 是纯接线：transport + 最小 CLI，让那家真的被打通。

## 为什么它排在 loop 契约前面

因为 loop 契约（[001](001-loop-contract.md)）需要知道「一次真实调用长什么样」才定得对。
上一版反过来了：先定契约再接真东西，结果契约里 `Effect::CallProvider { payload }`
带着组装好的请求，而组装归 adapter（决策 15）——契约错了。

**先让一条最细的线端到端通，再往上加结构。**

## 做什么

```
crates/agent-transport/   阻塞 HTTP（ureq）+ 指数退避 + jitter + providers.toml 解析
crates/agent-cli/         最小壳：读一行，打一段流
```

- 退避重试：**流式请求一律不重试**——已经吐出去的增量收不回来，重试等于重复输出。
  连接建立失败可以退避；流断在中途交给上层（loop 层要重发就是一次全新请求，
  epoch 换新，那是 core 的判断不是 transport 的）
- 三家都不返回限流头（PROVIDERS.md §一），退避节奏只能自己定
- CLI 把 `Adjustment` 打出来——adapter 每做一次妥协，用户看得见

## 验收

- `cargo run -p agent-cli` 输入一句话，回复**边收边打**，不是等整轮结束才出
- 断网时报明确错误（不是 panic、不是无限重试）
- 402（余额耗尽）不退避，直接报给人
- **key 任何时候不打印**——日志里只出现长度或状态码
- `Ctrl-C` 能中断正在流的响应，进程不退出

## 注意

- 红线 7：`ureq` 只能出现在 `agent-transport` 的依赖里，`check-invariants.sh` 会查。
- **`Ctrl-C` 中断阻塞流是本 issue 唯一的硬骨头**：ureq 的阻塞 read 没有外部中断句柄。
  办法是给 socket 设短 read timeout，循环里每次超时检查取消标志——不优雅但可测。
  这里偷懒（比如干脆不做中断），M1 验收的 `Ctrl-C` 那条会在 [014](014-cli-shell.md)
  加倍还回来。
- 别在这里顺手写 loop——runner 是 [012](012-wire-loop-to-transport.md) 的事。
  这个 CLI 是直连的（读一行 → encode → POST → 打印），就该这么薄。

## 实做记录（2026-08-01）

sonnet 接线 + 主会话真跑验收。workspace 175/0 全绿。

**真实两轮验收**（deepseek-v4-pro）：流式边收边打、思考暗色区分、key 只打长度。
缓存兜底第一次在真实世界转：第 2 轮 `predicted=512 / actual=512` 一致
（596 按块 128 向下取整），`drift: 无`。第 1 轮 `predicted=0 / actual=512`——
冷启动不预测但命中了 512：是此前一次误发调用焐热的同前缀，账能对上。
顺带暴露一个措辞问题：predicted < actual（好于预期）时 CLI 打「对不上」，
读起来像告警——**好于预期不该吓人**，024 做判读函数时把三种情况分开表述。

- **Ctrl-C 选了 `ctrlc` crate**：验收原文是「Ctrl-C 中断流」，纯 std 方案要么进程
  被杀要么变成测 `/cancel`。取舍：装了 handler 后空闲时 Ctrl-C 不再退出进程
  （只有 `/quit` 退）；每轮开始清标志，防止空闲期的误按预取消下一轮。
  真 SIGINT 打真进程验过：流中打断 → 打「已取消」，进程活着。
- **transport 声明只走流式**（025 记的那笔：`Encoded.body` 恒 `stream:true`），
  写在 lib.rs 文档注释。将来要非流式，加方法不是改语义。
- 重试只包连接期（`ureq::Error::Transport`），响应到手后（含 402/429/500）
  一律不退避直接上抛；假 SSE 服务器测了连接数 == 1。
- 读循环用 500ms read timeout 轮询取消标志——超时是「这个 tick 没数据」不是错误。
- **一次违规如实记录**：实现 agent 手工冒烟时 `VAR=x cmd | cmd2` 的作用域错误
  让第一次冒烟落到了真 API（一句「你好」，587/44 token）。它自己发现、改正、
  上报。教训：`VAR=x` 只作用于管道第一段，要 `export`。
