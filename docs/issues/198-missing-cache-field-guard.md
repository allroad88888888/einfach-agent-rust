# 198 缓存字段缺失时不许被读成 0

**里程碑** L · **依赖** [176](176-openai-compat-adapter.md) · **模型** sonnet · **独测** ✅ · **估时** 20min · **状态** 完成（2026-08-13）

## 目标

钉死一条**静默失效**：一个什么缓存信息都不返回的端点，`cached` 必须读成
`None`（「不知道」），**不许读成 `Some(0)`**（「确定没命中」）。

## 为什么这条值钱

读成 0 的后果全程不报错：

1. [024](024-cache-guard.md) 的第 2 层拿 `predicted_cache` 跟真实 `usage.cached` 对账
2. 通用 adapter 的 `predicted_cache` 恒为 0（[176](176-openai-compat-adapter.md)：
   对面的缓存参数未知，**恒不预测**）
3. 如果 `cached` 也被读成 `Some(0)`，那么每一轮都是「预测 0、实际 0、完美吻合」
4. **那道闸从此永远不响** —— 而它看起来一直在正常工作

这正是 CLAUDE.md §红线摘要说的那一类：*「功能完全正常，只是每一轮都全价」*。
`None` 则会被第 2 层当成「无实测可对账」跳过，不产生假的吻合。

## 为什么不放进 [178](178-openai-compat-dogfood.md)

[174](174-openai-compat-probe.md) 实测三家（DeepSeek/Kimi/GLM）**都**返回了缓存字段
——真机上碰不到这个分支。原案是「装 Ollama 打一发」，撤销了（[178](178-openai-compat-dogfood.md) §拿什么当被测端点）。

而这条根本**不需要真机**：一个不返回缓存字段的假端点就能钉死，
**而且能进 CI 永久守着**。一次性真机验证证明的是「那天没坏」，
单测证明的是「以后坏了会红」——后者贵得多。

## 做了什么

`crates/agent-providers/src/openai/usage_guard_tests.rs`，四条：

| 测什么 | 断言 |
|---|---|
| 非流式：`usage` 里完全没有缓存字段 | `cached == None` |
| 非流式：显式 `cached_tokens: 0` | `cached == Some(0)` —— **跟上一条是两件事** |
| 流式：末帧 `usage` 无缓存字段 | `cached == None` |
| 流式：只给别家的私有路径（`prompt_cache_hit_tokens`） | `cached == None`，不顺手兜底 |

第二条是这组的关键：**「不知道」和「确定是 0」必须能分开**。
只测第一条的话，一个把两者都返回 `None` 的实现也能通过——那会丢掉
「这家确实没命中」这个真实信号，属于把假绿换成了假红。

## 验收

- [x] 四条全绿、零网络
- [x] `cargo test --workspace` 退出码 0
- [x] `check-invariants.sh --all` 退出码 0
- [x] **反向验证**：把 `wire::decode` 的缺失路径临时改成回退 0，第一条与第三条
      必须红——测试真的在守这件事，不是碰巧通过

## 注意

这条只覆盖 `openai/` 这个通用 adapter。三家各自的 `CACHED_PATHS` 走的是同一份
`wire::decode` / `stream::usage`，所以机制是共享的；但**它们的实测事实不同**
（Kimi 未命中时整个字段缺失、DeepSeek 与 GLM 显式给 0，见
[PROVIDERS.md](../../probes/PROVIDERS.md) §一），各自的回归测试留在各自目录里，
不在这里合并——合并会让「哪家是什么行为」这件事变得说不清。
