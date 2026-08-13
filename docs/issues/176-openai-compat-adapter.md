# 176 OpenAI 兼容 adapter 实现

**里程碑** L · **依赖** [175](175-openai-compat-decision.md) · **模型** sonnet · **独测** ✅ · **估时** 20min · **状态** 完成（2026-08-13）

## 目标

按 [175](175-openai-compat-decision.md) 拍的案子写代码。**只写 adapter 与 wire，不碰配置面**
（配置在 [177](177-openai-compat-config.md)）——拆开是为了让这一步能被录制帧独测覆盖，
不牵扯文件 IO。

## 做什么

按 [175](175-openai-compat-decision.md) 的结论走。无论 A 案 B 案，这几条都要满足：

1. 实现在 `crates/agent-providers/`，**不新增 HTTP 依赖**——那个 crate 依赖只有
   `agent-core` + serde，IO 全在 `agent-transport`（决策 26 里核实过的事实，别破坏它，
   wasm 目标靠它活着）。
2. 流式分片按 [174](174-openai-compat-probe.md) 的实测形状解析，尤其 `tool_calls` 的分片。
3. usage / 缓存字段按实测填；**没有缓存字段的家要能优雅地报「不知道」**，
   不能填 0 冒充命中——那会让 [024](024-cache-guard.md) 的三层兜底得到假绿。
4. 做不到的意图**报 `Adjustment`**，不要静默降级（决策 17）。

## 验收

- **录制帧独测全绿、零网络**（照 [025](025-provider-seam.md) 的形状）：
  至少覆盖普通回复、流式、工具调用、401 报错四条
- `cargo test --workspace` 绿
- `check-invariants.sh --all` 退出码 0
- **红线 12 自查**：`git diff crates/agent-core/` 为空。core 一个字节都不该动——
  动了就说明 [175](175-openai-compat-decision.md) 的结论有问题，回去重定，别在这里将就

## 不做

- 不改 `providers.example.toml`（[177](177-openai-compat-config.md) 做）
- 不跑真机（[178](178-openai-compat-dogfood.md) 做）

---

## 实做记录（2026-08-13）

`crates/agent-providers/src/openai/`，六个文件（四个实现 + 两个测试）：
`mod.rs` / `encode.rs` / `decode.rs` / `errors.rs` / `test_support.rs` / `encode_tests.rs`。
`lib.rs` 加一行 `pub mod openai;`。**存量三家一个字节没动。**

### 落地时定的三个「不知道就不猜」

[175](175-openai-compat-decision.md) 只说了「最小内核」，实做时发现还有三处
「三家的 `mod.rs` 里有实测常量、这里填什么」的问题。判据统一成
**「不知道就别装知道」**：

| 常量 | 三家 | `openai/` | 理由 |
|---|---|---|---|
| `CACHE_BLOCK` / `PREDICT_MIN` | 64 / 860 之类实测值 | 1 / `u32::MAX` ⇒ **恒不预测** | 瞎猜一个数去跟真实 usage 对账只会制造假告警，最后没人再看告警。第 2 层兜底按「无预测不判」处理 |
| `MAX_TOOLS` | 128 | `usize::MAX` ⇒ **从不截断** | 对面真有上限会报错，那是**可见的失败**；我们自己先裁才是静默的——模型会发现工具「不见了」，而没有任何地方说过它被丢了 |
| `LateToolsForcedIntoPrefix.est_cost_multiple` | 2.0（GLM）/ 120（DeepSeek） | **1.0** | 意思是「我们不知道，别拿这个数当估算」，不是「它很便宜」。这条 `Adjustment` 的价值在于**让人看见前缀被作废了**，不在于那个倍数 |

`drift` 仍然算——它比对的是**我们自己**两轮之间的字节，跟对面是谁无关，
是第 1 层兜底（在花钱之前抓我们自己的序列化 bug），对任何端点都成立。

### `tool_choice` 属不属于最小内核？属于

这条 [175](175-openai-compat-decision.md) 没写，实做时要定。**属于**，判据是：

- `temperature` 是**采样偏好**——不发就用对面的默认，**语义无损**
- `tool_choice` 是**语义要求**——不发就等于悄悄把「必须调工具」降级成「随你」，
  那是静默妥协，本层头号大忌

而且一个连 `tool_choice` 都不认的端点也不会认 `tools`，那种情况下请求本来就跑不通。

### 一处我自己写错的断言，值得记

`errors.rs` 里我先断言「Spring 风格的裸 404 → `BadRequest`」，实际是 `Unknown`
（`wire::errors::by_status` 里根本没有 404 这一档）。

**查完发现实际行为比我写的更对**：通用 adapter 收到裸 404 + 非 OpenAI 错误体，
最可能的真相是**用户的 `base_url` 填错了**——[174](174-openai-compat-probe.md) 里
我自己就是这么撞出来的（给 GLM 拼了个 `/v1`，整组 404）。那是配置错误要人去看，
不是「这次请求内容不合法」。`Unknown` = 保守处理、不自动重试，正合适。

而带 OpenAI 形状的 404（Kimi 的模型名不存在）会经 `error.type` 判成 `BadRequest`。
**两条 404 分类不同但都不可重试**——安全性质守住了。断言改成 `Unknown` 并把
这段理由写进了模块文档。

### 验收

- [x] 单测 24 条全绿、零网络。覆盖：**没发什么**（temperature/top_p/n/stream_options
      各一条）、发了什么（最小内核五项）、`tool_choice` 三种意图、从不截断、
      恒不预测、晚加工具仍报 `Adjustment`、四种错误体形状、缓存字段的
      「缺失 vs 显式 0」、流式一整轮、端点不拼 `/v1`
- [x] `cargo test --workspace` 退出码 0
- [x] `check-invariants.sh --all` 退出码 0
- [x] **红线 12 自查**：`git diff --stat crates/agent-core/` **空**——core 一个字节没动
- [x] `build-wasm.sh` 退出码 0（新 provider 会进 wasm 产物）

### 两条测试是特意写成「测不到就等于没测」的

1. `test_support::config()` 的 `temperature` **有意设成 `Some(0.0)`**，
   并在测试里先 `assert_eq!(i.config.temperature, Some(0.0))` ——留 `None` 的话
   「设了也不发」那条断言等于什么都没测。
2. `optional_sampling_fields_are_never_sent` 里 `n` 单列，因为
   [174](174-openai-compat-probe.md) 实测它在 GLM 上是**静默按 1 处理**（200 + 无错误），
   静默降级比拒绝更糟。
