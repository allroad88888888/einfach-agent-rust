# 005 无网络的测试脚手架

**里程碑** M1 · **依赖** 001 · **模型** sonnet · **独立测试 agent** 否 · **状态** 完成

## 目标

mock provider + mock tool executor，让整个 loop 能在无网络下跑完整流程。

## 为什么

这是红线 7 的**目的**，不是副产品。IO 一旦渗进 core，这些测试就变成集成测试，
然后就没人写了，然后红线 1–6 全部失去回归保护。

## 做什么

- `MockProvider`：按脚本返回预设响应序列（文本 / 工具调用 / 错误 / 截断），
  可断言收到的请求
- `MockExecutor`：按工具名返回预设结果，可注入失败与延迟
- **可注入 `Timeout` 事件**：计时器在 core 外面，所以测试能在零时间内模拟任意超时序列

## 验收

- 一个测试跑完「输入 → 组装 → 调模型 → 请求 2 个工具 → 回填 → 再调 → 收敛」，
  全程无网络、无 sleep
- 能构造出「工具在飞时用户 undo」的时序，为 P3 的 epoch 校验做准备
- `cargo test` 全绿且耗时不因它明显增加

## 注意

mock 站在**事件层**：`MockProvider` 拦下 `Effect::CallProvider` 直接回
`ProviderDone` / `ProviderFailed` 事件序列，`MockExecutor` 拦 `ExecuteTool` 回
`ToolResult`。**不 mock wire 层**——录制帧喂 decode/stream 的测试在
[025](025-provider-seam.md) / [023](023-three-providers.md) 已经覆盖，
这里再铺一遍 JSON 是重复劳动。

事件里的形状（usage 缺字段、`adjustments` 非空）参考 `probes/results/*.json`
的真实观测构造，别自己编——自己编的会漏掉真实差异（比如有一家未命中时
`cached_tokens` 字段整个消失）。

### 合并记录（主会话）

事件层脚手架（MockProvider/MockExecutor/Harness 驱动器）+ 11 测试全部 0.00s。
脚本形状取自 probes/results 真实观测（含 cached 字段缺失那一种），delay 是纯排序键
不是真等待。「工具在飞时取消 → 旧世代回执被闸丢弃」= P3 undo 校验的 M1 排练，已钉。
