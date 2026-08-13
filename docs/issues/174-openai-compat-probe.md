# 174 探针：OpenAI 兼容 endpoint 打一发

**里程碑** L · **依赖** [165](165-launch-positioning-decision.md) · **模型** sonnet · **独测** — · **估时** 20min · **状态** 待开始

## 目标

在写任何 adapter 代码之前，**先拿真实响应说话**。本仓的惯例是探针先行
（`probes/` 与 `PROVIDERS.md` 就是这么来的，025/038 都是这个顺序），
理由在 [../WORKFLOW.md](../WORKFLOW.md)：接缝定型必须建立在观测上，不是建立在文档上。

## 背景

[165](165-launch-positioning-decision.md) L1 定了英文社区优先，直接推论：**海外读者手里
没有 DeepSeek / Kimi / GLM 的 key**。README 是英文的，但 quickstart 跑不起来——
这是当前最大的漏斗断点。加一条认 `base_url` 的 OpenAI 兼容路径，一次拿下
OpenAI / OpenRouter / 硅基流动 / Ollama / vLLM，**其中 Ollama 还是零成本本地跑**
（对「先试试看」的人是最低门槛）。

## 做什么

用 `probes/api` 对**至少两个**兼容 endpoint 各打一发（建议 OpenAI 官方 + Ollama 本地，
一个是标杆一个是零成本），记录：

1. 请求形状与现有三家的差异（三家已经是 OpenAI 形状，差异应该很小——**要证实这一点**）
2. **流式**分片结构：`delta` 的字段、结束标记、`[DONE]` 有没有
3. **工具调用**的形状：`tool_calls` 的分片方式（这一条历来是各家最爱跑偏的地方）
4. **usage / 缓存字段**：有没有 `prompt_cache_hit_tokens` 一类的东西，字段名叫什么
   —— [024](024-cache-guard.md) 的三层兜底要读它
5. 报错形状：401 / 404 / 模型名不存在，各返回什么

## 验收

- 原始观测落 `probes/results/`（照既有文件的组织方式）
- 结论写成一页，明确回答：**「跟现有三家的 wire 差多少」**
- 有一条明确判断：**能不能复用现有 `wire` 层**——这是 [175](175-openai-compat-decision.md) 要拍板的输入

## 不做

不写 adapter，不动 `crates/`。这个 issue 的产出**只有观测和结论**。
