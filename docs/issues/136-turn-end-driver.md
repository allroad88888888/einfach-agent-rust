# 136 收尾驱动：完成轮后跑 `TurnEnd` 工具

**里程碑** M15 · **依赖** [133](133-call-timing-field.md) · **模型** sonnet · **独测** ✅ · **状态** 完成（2026-08-11）

## 目标

每个**正常完成**的轮（模型交回控制权，非取消、非失败）之后，按注册顺序执行
`timed(TurnEnd)` 的工具。**纯副作用**：结果丢弃、不落 store、不进 prompt，
失败只记日志。

v1 边界（决策 27 明确拍死，写进文档注释）：**不能续 loop、结果不回灌、
不发协议事件**。将来要「hook 拦停 / 结果进下一轮 / SSE 可观测」，单开 issue
重议——那会碰 001 的 loop 契约，复杂度完全不同。

## 现状

轮的收敛点在 `agent-runtime` 的 runner；`TurnStatus` 已区分完成 / 取消 / 失败
（002/016 的状态转移表）。

## 做什么

runner 在「完成」分支之后逐个调 timed 条目**自带的执行体**（133 修订后的形状：
本地同步函数，不走 dispatch/executor）→ 丢结果：返回 `Err` 也只记日志，
不影响轮的结果。取消 / 失败分支不跑。

## 验收

- fake hook 计数：N 个完成轮 = 恰好 N 次；被取消的轮 = 0 次。
- **有 hook 与无 hook 两个会话对照**：journal entry 序列逐条相同（hook 不落
  store）；下一轮 encode body **逐字节相同**（hook 不进 prompt）。
- fake hook 返回 `is_error`：轮结果不变、无 panic、日志恰一条。
- hook 跑过之后 undo 的行为与无 hook 时逐步相同（深度、每步状态）。

## 注意

- hook 没有任何状态写入点，所以不存在「绕过 command 层」的口子（红线 2 的
  反面用法——最安全的遵守方式是根本没有写入路径）。
- **审计面要显式承认**：hook 的副作用不进 command log（无 entry），这与
  TOOLS.md §「服务端工具不是第四种 Location」同一个警觉——区别在于 hook 是
  **部署者显式装配的**，知情前提成立。这段要写进模块文档注释，别让半年后的
  读者以为是漏了记账。
- `Irreversible` 的 hook 与 undo 屏障无关（无 entry 可标屏障）——hook 里干
  不可逆的事是部署者自己的选择。

## 实做记录（2026-08-11）

- 落点：`turn_end.rs`（246 行）；挂点 runner.rs B 收工分支、`maybe_snapshot` 之后、只认
  `TurnStatus::Done{..}`（远端回传补完的轮同样触发，刻意）。hook 间相互独立：单个 Err
  记日志继续跑下一个（与 135 的全有或全无相反，理由在模块文档：独立副作用无合并结果）。
- 新依赖：`tracing 0.1`（agent-server 已用同版本；无 subscriber 时零开销；wasm 编译干净）。
- 独测 6 条全绿：完成轮恰一次、注册序、取消轮零次、Err 不改轮结果、journal 逐条相同、
  第二轮真实请求体无 hook 痕迹。
