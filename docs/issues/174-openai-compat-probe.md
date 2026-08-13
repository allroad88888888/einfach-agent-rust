# 174 探针：OpenAI 兼容 endpoint 打一发

**里程碑** L · **依赖** [165](165-launch-positioning-decision.md) · **模型** sonnet · **独测** — · **估时** 20min · **状态** 代码侧分析完成；**真实探针缺凭据，卡住**（2026-08-13）

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

---

## 实做记录（2026-08-13）

### ⚠️ 先说卡在哪

**本机没有任何可打的 OpenAI 兼容端点**：无 `ollama`（`11434` 端口也没别的服务）、
环境里无 `OPENAI_API_KEY` / `OPENROUTER_*` / `SILICONFLOW_*`。
所以「对至少两个兼容 endpoint 各打一发」**没做成**，`probes/results/` 没有新文件。

**要解锁只需二选一**（任一即可，都不用改代码）：

- `brew install ollama && ollama pull qwen3:4b`（零成本，且这正是 [177](177-openai-compat-config.md)
  要主推的入口——**顺带就把那条路验了**）
- 给一个 OpenAI 官方或 OpenRouter 的 key

### 但代码侧的问题问出答案了

174 真正要交给 [175](175-openai-compat-decision.md) 的判据是**「能不能复用现有 wire 层」**。
这条不用外部凭据就能查清，查完的结论比预想的强：

**一、`wire/` 与 `stream/` 本来就是共享层，不是三家各写一份。**

```
wire/     decode messages names numeric prefix tools errors   ← 三家共用
stream/   mod tool_parts usage stop                            ← 三家共用
deepseek/ kimi/ glm/   各自只有 encode + decode + errors + mod
```

`wire::names`（工具名 `srv:fs/list` ⇄ `srv_3Afs_2Flist` 的转义）的文档已经把这条
写死了：*「它住在 `wire/` 而不是某一家的目录下，因为它不是厂商差异——三家的
`function.name` 都受同一条 OpenAI 惯例字符集约束」*。

**二、`Provider` trait 只有四个纯函数**（`encode` / `decode` / `accumulator` / `classify`），
且它的文档就是判据本身：

> 方法数 = 各家真的不一样的**动作**数。只差常量的适配是数据不是方法。

`accumulator()` 已经返回**共享类型** `StreamAccumulator` 而不是 trait 方法群——
流式那一半三家早就是同一份代码了。

**三、真正的厂商差异，实测就这么多**（`grep Adjustment:: */encode.rs`）：

| 家 | encode 里的妥协点 |
|---|---|
| deepseek | `LateToolsForcedIntoPrefix` / `ToolsTruncated` / `ThinkingDisabledForToolChoice` / `ToolChoiceDowngraded` |
| glm | 同上去掉 thinking 那条 |
| kimi | 只有 `ToolChoiceDowngraded` + `TemperatureOverridden{used:1.0}` |

以及 usage 的缓存字段路径——**已经是常量而不是代码**：

```rust
deepseek: &[&["prompt_cache_hit_tokens"]]
glm:      &[&["prompt_tokens_details", "cached_tokens"]]   // kimi 同款
```

**四、三家其实已经是三个不同厂商的 OpenAI 兼容端点。**
`api.deepseek.com` / `api.moonshot.cn/v1` / `open.bigmodel.cn/api/paas/v4` ——
本仓已经在跨三个厂商吃同一套 OpenAI 形状了，差异的量级见
[PROVIDERS.md](../../probes/PROVIDERS.md)。**这是 [175](175-openai-compat-decision.md)
最重要的输入**：不是「要不要支持一种新形状」，而是「已经支持三家的那套东西，
退化到没有特殊处的情形」。

### 给 [175](175-openai-compat-decision.md) 的判据

代码结构强烈指向 **B 案的一个弱化版**：不需要「抽出共享基座」（**基座早就在了**），
只需要**再加一个 `openai/` 目录，里面是三个文件的退化实现**——
`encode` 不做任何妥协、`CACHED_PATHS` 走 `prompt_tokens_details.cached_tokens`
（OpenAI 官方口径）、`classify` 按标准 `error.type`。**存量三家一个字节不用动。**

但**这个判据是从代码读出来的，不是从真实响应读出来的**，[175](175-openai-compat-decision.md)
拍板前应该等上面那个探针补上——尤其是两件事：

1. **Ollama 这类本地实现的 `tool_calls` 分片**是否与云端一致（历来是各家最爱跑偏的地方）
2. **没有缓存字段的家**（Ollama 大概率没有）会让 `CACHED_PATHS` 取到什么，
   `stream/usage.rs` 会不会把「没有」读成 0 ——那会让 [024](024-cache-guard.md)
   的三层兜底得到**假绿**，这是本 issue 列的第 4 条观测项，也是最有可能咬人的一条
