# 174 探针：OpenAI 兼容 endpoint 打一发

**里程碑** L · **依赖** [165](165-launch-positioning-decision.md) · **模型** sonnet · **独测** — · **估时** 20min · **状态** 完成（2026-08-13，三家真机实测）

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

### 探针跑成了：`probes/api/src/bin/openai_compat.rs` + `probes/results/openai-compat.json`

> 第一版记录里我写「本机没有可打的兼容端点，卡住」。**是我把问题想窄了**——
> 用户点出 DeepSeek 的 `/v1` 就是标准 OpenAI 接口。三家本来就都是兼容端点，
> 探针早就能跑，缺的不是凭据是想法。

**探的不是「这家能不能用」，而是**：一个**完全不知道对面是谁**的通用 adapter，
发一份标准 OpenAI 形状的请求（无 `thinking`、无 `caps::temperature` 特例、
零 provider 分支），打到自称 OpenAI 兼容的 `base_url` 上会怎样。
这正是 [175](175-openai-compat-decision.md) 那个「退化实现」方案在 wire 上的样子。

跑法：`cd probes/api && cargo run --bin openai_compat`（默认 deepseek；
`PROBE_PROVIDERS=kimi,glm`、`PROBE_NO_V1=1` 可调）。28 条观测已落
`probes/results/openai-compat.json`。

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

### 实测结果：裸 OpenAI 请求打三家

| | DeepSeek | Kimi | GLM |
|---|---|---|---|
| **端点路径** | `/v1/chat/completions` ✅ | `/v1/chat/completions` ✅ | **`/chat/completions`，没有 `/v1`** |
| **裸请求** | ✅ 200 `PONG` | ❌ **400** | ✅ 200 `PONG` |
| **缓存字段** | **两条路径都给，且数值相同** | — | **只有** OpenAI 标准那条 |
| **tool_calls 非流式** | 标准形状 + `index` | — | 标准形状 + `index` |
| **流式收尾** | `data: [DONE]` | — | `data: [DONE]` |
| **模型名不存在** | 400 / `invalid_request_error` | **404** / `resource_not_found_error` | 400 / `{"code":"1214"}` |
| **`n: 2`** | **400 拒绝** | — | **200，静默按 1 处理** |
| **裸请求缓存命中** | 1280/1301 = **98.4%** | — | 1216/1231 = **98.8%** |

### 四条会改变 [175](175-openai-compat-decision.md) 的结论

**一、`/v1` 不是通用约定。** GLM 的兼容端点是 `/api/paas/v4/chat/completions`——
我第一跑按「OpenAI 就是 `/v1`」拼路径，整组 404，错误体还是 Spring 风格的
`{"timestamp","status","error","path"}`，连 `error.type` 都没有。
（那一跑没删，留在结果文件的 `glm_wrong_path_v1` 键下——**它就是这条结论的证据**。）
→ 通用 adapter **不能自己拼 `/v1`**，`base_url` 必须由用户带全路径。

**二、合法的 OpenAI 值会被硬拒。** Kimi 对 `temperature: 0.0` 直接
`400 invalid temperature: only 1 is allowed for this model`——0.0 是 OpenAI 的
合法值，通用 adapter 不可能知道这家的规矩。**这一条否掉了「一个退化实现打天下」**：
通用 adapter 必然会撞上「我发的是标准的，但这家不收」，所以它**必须有把
`Adjustment` 报出来的能力**，不能假设自己永远不需要妥协。
这恰好是决策 17 的形状——事后报调整，而不是事前问能力。

**三、静默降级真的存在，而且就在手边。** `n: 2` 在 DeepSeek 上 400 拒绝，
在 GLM 上 **200 + 一条 choice + 零错误**。探针注释里我写「静默忽略比拒绝糟——
通用 adapter 会以为自己拿到了要的东西」，一跑就撞上了。
→ 通用 adapter 对**自己发出去但对面可能不认的字段**必须保守：能不发就不发。

**四、缓存字段这条是好消息。** DeepSeek 同时给 `prompt_cache_hit_tokens`
和 `prompt_tokens_details.cached_tokens`，且 E 组实测**两个数一模一样**（1280/1280）。
GLM 只给 OpenAI 标准那条。→ 通用 adapter 用
`prompt_tokens_details.cached_tokens` 在这两家都读得对。

> 但 174 原列的第 4 条隐患**没被证伪，只是没在这三家上出现**：
> 「一家什么缓存字段都不给时，`stream/usage.rs` 会读成 0 还是读成『不知道』」。
> 三家都给了字段，所以这一跑碰不到。**Ollama 那类本地实现大概率不给**，
> 到 [178](178-openai-compat-dogfood.md) 真机时必须专门验——读成 0 就是让
> [024](024-cache-guard.md) 的三层兜底拿到**假绿**。

### 给 [175](175-openai-compat-decision.md) 的判据（已按实测修正）

代码侧的结构分析仍然成立（`wire/` + `stream/` 是共享层、`Provider` 只有四个纯函数、
真正的厂商差异是每家 2–4 个 `Adjustment` 点 + 一个常量路径），所以**方向仍是
「加一个 `openai/` 目录，存量三家一字不动」**。

但实测把「退化实现」这个说法**修正掉了**——它不是「什么都不做的那一版」：

1. **不许自己拼 `/v1`**（结论一）
2. **必须能报 `Adjustment`**，因为它一定会撞上合法值被拒（结论二）
3. **可选字段能不发就不发**，因为静默降级在真实兼容实现里存在（结论三）
4. `CACHED_PATHS` 用 `prompt_tokens_details.cached_tokens`（结论四），
   但**「字段缺失」与「字段为 0」必须能分开**——这条留给 [178](178-openai-compat-dogfood.md) 钉死
