# 175 OpenAI 兼容层落在哪（决策）

**里程碑** L · **依赖** [174](174-openai-compat-probe.md) · **模型** **opus** · **独测** 决策类 · **估时** 20min · **状态** 完成（2026-08-13）

## 目标

定一件事：**「OpenAI 兼容」是第四家 provider，还是一个参数化的既有家？**

这个决策贵，因为它直接顶在**红线 12**（core 里不许有任何模型相关的判断）和
**决策 17**（事前问能力 → 事后报调整）上。选错了，加第五家的时候要改 core——
而那正是决策 17 当初要根治的病。

## 拍板（三条，全部有 [174](174-openai-compat-probe.md) 的实测支撑）

### 一、新增 `openai/` 目录，与三家并列（A 案），**不抽共享基座**

否决 B 案「抽出共享基座、`openai` 是退化情形」——**理由是基座早就在了**：

```
wire/     decode messages names numeric prefix tools errors   ← 三家共用
stream/   mod tool_parts usage stop                            ← 三家共用
deepseek/ kimi/ glm/     各自只有 encode + decode + errors + mod
```

`Provider` trait 只有四个纯函数，`accumulator()` 已经返回**共享类型**
`StreamAccumulator` 而不是 trait 方法群——流式那一半三家早就是同一份代码。
B 案要做的事**已经做完了**，现在提「抽基座」只会去动三家已经真机验过的代码，
换不到任何新东西。

于是 `openai/` 就是第四个目录，四个文件，**存量三家一个字节不动**。

> `wire::names` 的文档早就把这条判据写死了：「它住在 `wire/` 而不是某一家的目录下，
> 因为它不是厂商差异」。判据一直在，只是没人来用过。

### 二、能力位怎么填：**只发最小内核，一律不问「这家支不支持」**

这是本决策最难的一点，也是 [174](174-openai-compat-probe.md) F 组直接裁决掉的。

**问题**：「OpenAI 兼容」是一个**开放集合**——同一个 adapter 后面可能是 OpenAI、
可能是 Ollama 跑的 4B 小模型、可能是某个自研网关。能力天差地别，而 adapter
**事前不可能知道**。

**实测的两半**：

- 发全套 OpenAI 字段 → Kimi **400**：`temperature: 0.0` 被拒
  （0.0 是 OpenAI 的合法值，通用 adapter 没理由知道这家只收 1.0）
- **只发最小内核**（`model`/`messages`/`max_tokens`/`stream`/`tools`）→ **三家全过**，
  含刚才 400 的 Kimi；带 tools 也三家全过

**结论**：契约写成「**只发每个兼容实现都必须支持的字段，取值交给对面的默认**」。
`temperature` / `top_p` / `n` / `stream_options` 这些**一律不发**。

这不是保守，是把问题**消掉**：不发就不会被拒，「合法值被这家拒绝」这一整类
在结构上不存在，于是**不需要任何 per-endpoint 怪癖表**。

**否决「怪癖表」案**（让用户在配置里声明这家的规矩）：那是 `match provider`
换了个地方住——从代码搬到配置，红线 12 的形状原样保留，而且把「配错了静默降级」
的风险转嫁给了不掌握细节的使用者。

**决策 17 的关系**：`Adjustment` 的能力**保留但用不上**。事后报调整这条路仍然
成立（`openai/` 的 `encode` 照样能 push `Adjustment`），只是最小内核契约下它
没有触发点。这是好事——决策 17 要的是「妥协必须可见」，而这里的答案是「不妥协」。

**代价照实记**：通用 adapter **给不了确定性采样**（不发 `temperature`）。
对「要可复现输出」的用户，答案是**用专门那家的 adapter**。
通用 adapter 的定位是**够得着更多端点**，不是替代已适配的三家——
这一条要写进 [177](177-openai-compat-config.md) 的配置注释里，别让人拿它当默认档。

### 三、`base_url` 由用户带全路径，adapter **不许自己拼 `/v1`**

[174](174-openai-compat-probe.md) 结论一的直接后果：`/v1` 不是通用约定。

| | 兼容端点 |
|---|---|
| DeepSeek | `https://api.deepseek.com/v1/chat/completions` |
| Kimi | `https://api.moonshot.cn/v1/chat/completions` |
| **GLM** | `https://open.bigmodel.cn/api/paas/v4/chat/completions` — **没有 `/v1`** |

我第一跑按「OpenAI 就是 `/v1`」给 GLM 拼路径，整组 404，错误体还是 Spring 风格的
`{timestamp,status,error,path}`，连 `error.type` 都没有
（证据留在 `probes/results/openai-compat.json` 的 `glm_wrong_path_v1` 键下）。

所以：**配置里的 `base_url` 就是最终路径的前缀，adapter 只在后面接
`/chat/completions`**，跟既有三家的 `caps::endpoint` 同款语义。用户填
`http://localhost:11434/v1` 或 `https://open.bigmodel.cn/api/paas/v4`，各自负责。

### 附带：`beta_base_url` 一类的东西怎么办

**不给。** 那是 DeepSeek 为 prefix 续写准备的第二个端点，属于「这家的特殊能力」。
通用 adapter 的定位是最小内核，特殊能力走专门 adapter。字段不存在，不是留空。

## 落地清单（交给 [176](176-openai-compat-adapter.md)）

1. 新目录 `crates/agent-providers/src/openai/`，四个文件：`mod.rs` / `encode.rs`
   / `decode.rs` / `errors.rs`，照三家的形状
2. `encode`：**只发最小内核**。`Ingredients` 里 core 给的 `temperature` 等
   意图**丢弃且不报 `Adjustment`**——见下「要 [176](176-openai-compat-adapter.md) 注意的坑」
3. `CACHED_PATHS = &[&["prompt_tokens_details", "cached_tokens"]]`（OpenAI 官方口径；
   [174](174-openai-compat-probe.md) 实测 DeepSeek 两条路径数值一致、GLM 只有这条）
4. `classify`：先按 `error.type`，落不到按状态码。**必须能吃非 OpenAI 形状的错误体**
   ——GLM 的 `{"code":"1214","message":"modelCode：不存在"}` 与 Spring 风格的 404
   都是真实存在的形状，认不出就落 `Unknown`（保守不重试），不许猜
5. `accumulator()` 直接复用 `StreamAccumulator`，`wire::names::from_wire` 照挂

## 要 [176](176-openai-compat-adapter.md) 注意的坑

**「丢弃 temperature 意图」到底报不报 `Adjustment`？**

倾向**不报**，理由是：`Adjustment` 的语义是「你要的这件事我做不到，我改成了别的」，
而这里是「这个 adapter 的契约里本来就没有采样控制」——**是契约边界，不是运行时妥协**。
每轮都报一条 `TemperatureOverridden` 会把 `Adjustment` 变成噪音，而它的全部价值在于
稀有（决策 17：「空的时候才叫原样执行了」）。

但这条**不是硬拍**：如果 [176](176-openai-compat-adapter.md) 实做时发现「用户设了
temperature 却静默没生效」在真实使用里咬人，就改成建会话时报一次（不是每轮）。
**留给实做时看，别在这里提前定死。**

## 验收

- [x] 三件事各有结论**和理由**
- [x] 明确写出否决案及其理由（B 案抽基座 / 怪癖表案 / `beta_base_url` 案）
- [x] 给 [176](176-openai-compat-adapter.md) 的可执行判据：改哪些文件、不改哪些文件
- [x] 结论进 [../ROADMAP.md](../ROADMAP.md) §一的决策表 → **决策 33**（2026-08-13 补，178 收官时）

## 红线自查

- **红线 12**：结论不要求 core 里出现任何 `match provider` 或 `if caps.xxx()`。
  「只发最小内核」是 `openai/encode.rs` 内部的事，core 照旧只供料。✅
- **决策 15**：请求组装归 adapter，core 只供料——最小内核的裁剪发生在 `encode` 里，
  没有漏回 core。✅
- **决策 17**：事后报调整的通道保留，只是最小内核契约下没有触发点。✅
