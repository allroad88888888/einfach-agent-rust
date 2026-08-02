# 006 子 agent 由谁 spawn（决策）

**里程碑** M3 · **依赖** M1 完成 · **模型** opus · **独立测试 agent** 决策类不派 · **状态** 完成

## 目标

定下子 agent 的触发方式。这是**两个完全不同的产品形态**。

## 为什么挪到 M3 而不是一开始定

单 agent 的 CLI 已经有用。在没有真实使用反馈的情况下定这个，等于猜——用两周 M1 的
CLI 之后，「什么任务真的需要分解」会变得具体得多。

## 两条路

**A. 模型主动 spawn**：给模型一个内置工具（如 `spawn_agent`），它自己决定何时分解任务。

- 灵活，能处理没预料到的任务结构
- 但成本不可预测，需要深度/总数/预算的硬限制兜底
- spawn 本身成为一次 tool call，天然进 command log，undo 语义清晰

**B. 编排层按计划 spawn**：上层代码根据任务类型决定分解方式，模型只在各自的子任务里工作。

- 可控、可预算、可测
- 但只能处理预设的任务结构
- spawn 是编排层的动作，要单独设计它怎么进 command log

## 要定的

除了选 A 还是 B，还要定：

- **深度上限、子 agent 总数上限、子树预算上限**（两条路都需要）
- **结果怎么回到父 agent**：作为一个 `tool_result`，还是专门的 `ChildFinished` 事件
- 子 agent 的定义（system prompt / 可用工具子集 / 模型）从哪来 ——
  当前 `Slot` 里**缺 `ToolsAllowed`**，选定后要补

## 验收

决策与理由写进 `docs/ROADMAP.md` 第一节；选中那条路的类型定义完成；
`Slot` 补齐缺的槽位并保证 `Visibility` 分类正确（红线 10）。

## 注意

无论选哪条，**spawn 时都要快照定义**——和 `ToolCallRequest` 存发起时 `Reversibility` 是同一个
道理。否则 undo 回到 spawn 时刻，用的是现在的工具表而不是当时的。

### 决策记录（2026-08-02，主会话拍板）

**选 A：模型经内置工具 spawn**，理由三条见 ROADMAP 决策 20。issue 里「要定的」逐条：

- 上限：深度 ≤3 / 每 agent 子数 ≤8 / 子树轮预算——数字参数（红线 12 禁分支不禁
  参数），超限 = `is_error` 的 tool_result（003 哲学：让模型看到全貌自己收敛）
- **结果回父 = tool_result，不需要 `ChildFinished` 事件**——spawn 的槽位天然走
  ToolsPending 收敛，001 推迟该事件时的直觉（「未必长成事件」）验证为正确
- 子 agent 定义（system prompt / 工具子集 / 模型）**作为 spawn 工具的入参进
  ToolCall 快照**——与 `ToolCallRequest` 存发起时语义同一个原则；`Slot::ToolsAllowed`
  在 028 补
- 类型落地归 028（多 agent 原子图）/ 029（spawn 工具本体）
