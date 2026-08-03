# 三家 provider 的实测差异

2026-07-31 实测，`probes/api`。模型 `deepseek-v4-pro` / `kimi-k3` / `glm-5.2`。
原始观测在 `results/*.json`。

**这份文档记录的全是模型侧的差异，不是架构。** 它们由 `agent-providers` 的 adapter
层吸收。**core 里一条模型相关的判断都不许有**（红线 12）——没有 `match provider`，
也没有 `if caps.xxx()`。本文的每一条差异都在 adapter 内部消化掉；消化不掉的
（比如某家强制指定工具做不到）由 adapter 事后回一条 `Adjustment`，
而不是让 core 事先问。主线设计一个字都不该引用本文。

**请求组装也在 adapter 里**（决策 15）：本文第一节到第三节列的差异——工具晚加放哪、
thinking 进不进前缀、temperature 能不能改——每一条都是组装时的分叉。
放在 core 里做，就只能做成一个不做任何模型相关判断的搬运函数。

**文档不可信，以实测为准。** 已确认两处直接矛盾：DeepSeek 文档说 `tool_choice` 四种
取值全支持，实际两种直接 400；GLM 文档说只支持 `auto`，实际四种全支持。

## 速查

| | DeepSeek | Kimi | GLM |
|---|---|---|---|
| **缓存匹配语义** | 仅扩展 | 仅扩展 | **真前缀树** |
| 块粒度 | 128 | 256 | 64 |
| 起效门槛 | ~380 | 256 | **~860** |
| 命中折扣 | **120x**（flash 50x） | 10x | 2x |
| 缓存字段 | `prompt_cache_hit_tokens` | `prompt_tokens_details.cached_tokens` | 同 Kimi |
| 未命中时字段 | 值为 0 | **整个缺失** | 值为 0 |
| 消息级 tools | ✗ | **✓** | ✗ |
| 消息级 system 注入 | **✗ 先归零**，重建反而更省 | ✓ 零代价 | ✓ 零代价 |
| 思考可控性 | 可开关，**默认开** | **常开且关不掉** | 可开关 |
| `thinking.type` 进前缀 | ✗ | — | **✓** |
| `tool_choice` 指定函数 | 需先关思考 | **永久不可用** | 直接可用 |
| 流式 usage 位置 | finish 同帧 | **finish 后另起一帧** | finish 同帧 |
| 服务端工具 | ✗ | formulas（可见） | retrieval/web_search/MCP（**不可见**） |
| temperature | 自由 | **只接受 1** | 自由 |
| 工具数上限 | 128 | 未公布 | 128 |

## 一、缓存

### 匹配语义分两类——最要紧的一位

- **GLM 是真前缀匹配**：任意请求命中到最长公共前缀，块对齐
- **DeepSeek / Kimi 只认扩展**：新请求必须是已见过请求的严格延长，中途任何分歧归零

两个独立实验一致指向它：同 system+tools 只换末尾用户消息（E5），GLM 首次即命中 5440，
另两家均 0；改写对话中段第 2 轮（E6），GLM 保留 96.7%，另两家均 0。

**后果**：

| | 中间改写（压缩）损失 | 一次压缩 ≈ 多少轮命中的花费 | 兄弟 agent 共享前缀 |
|---|---|---|---|
| DeepSeek | 100% | **~120 轮** | ✗ 收益为零 |
| Kimi | 100% | ~10 轮 | ✗ 收益为零 |
| GLM | ~3% | 可忽略 | ✓ 成立 |

「牺牲工具裁剪精度换缓存共享」这条路只在 GLM 上走得通，另两家收益为零——按每个子
agent 实际需要的工具子集精确裁剪就好。

### 「归零」不是「作废」

冷轮 0% 曾被误读成「改工具表把缓存打掉了」。复跑第二轮全部命中，说明旧前缀一直都在，
归零只是**这个变体第一次见**。准确的代价是：那一轮整段对话全价重编码一次，之后在新
前缀上继续命中。

### 顶层 tools 在 prompt 最前面

三家改顶层 tools 后冷轮命中全部为 0。而 GLM 做的是真前缀匹配——若 system 排在 tools
之前，它至少能保住 system 的块，但它也是 0。**所以 tools 确实在 system 之前。**

所以前缀镜像的分段顺序必须是 `[Tools][System][History]`——见
[issue 023](../docs/issues/023-three-providers.md)。

### 块粒度与小上下文

覆盖率 = cached / prompt：

| prompt | DeepSeek | Kimi | GLM |
|---|---|---|---|
| ~330 | 88.5% | 77.6% | **0%** |
| ~460 | 89.2% | **54.5%** | **0%** |
| ~860 | 92.8% | 89.1% | 98.1% |
| ~3100 | 97.5% | 92.4% | 99.1% |

块粒度由「所有 cached 值的公约数 + 尾巴恒小于块」夹出。**GLM 在 ~460 时完全不缓存**，
~860 才跳到 98%，门槛没夹逼。Kimi 在 470 时只缓存 1 块（256），浪费 214。

**小子 agent 不值得做前缀优化**，GLM 上尤其。

### 三家都不返回限流头

`Retry-After` / `X-RateLimit-*` / `RateLimit-*` / `X-Quota-*` 一个都没有。退避节奏只能
自己定（指数退避 + 抖动）。

## 二、工具与 tool_choice

### 强制工具调用与思考模式互斥

| | `none` | `required` | 指定函数 |
|---|---|---|---|
| DeepSeek 默认 | ✅ | **400** | **400** |
| DeepSeek 显式关思考 | ✅ | ✅ | ✅ |
| Kimi | ✅ | ✅ | **400** |
| GLM 开/关思考 | ✅ | ✅ | ✅ |

错误原文：DeepSeek `Thinking mode does not support this tool_choice`；
Kimi `tool_choice 'specified' is incompatible with thinking enabled`。

- **DeepSeek v4-pro 默认开着思考**。用 `required`/指定函数必须同请求显式
  `thinking.type=disabled`——adapter 自动做，但要记一笔 `Adjustment`，因为它改变了模型行为。
- **Kimi K3 上「指定函数」永久不可用**：思考常开、API 里没有关闭字段（参数表没有
  `thinking`，而每个响应都带 `reasoning_tokens`）。adapter 降级成 `required` 并记一笔。

### 并行工具调用：三家都支持

同一 prompt 三家都在一次响应里返回 2 个 `tool_calls`。不是差异位。

### 中途加载工具的代价

| | 通道 | 代价 |
|---|---|---|
| Kimi K3 | 追加 `role:system` + `tools`（无 `content`）的消息 | **零**——实测 prompt 5276→5382，命中仍 5120 |
| GLM | 只能改顶层 | 全价重编码一轮，2x |
| DeepSeek | 只能改顶层 | 全价重编码一轮，**120x，别做** |

## 三、流式

三家都是 `data: {json}` + 末尾 `data: [DONE]`，骨架 OpenAI 兼容。三处实质差异：

**1. Kimi 的 usage 在 finish 帧之后另起一帧，且那帧 `choices` 为空：**

```
data: {"choices":[{"delta":{},"finish_reason":"stop"}]}
data: {"choices":[],"usage":{"prompt_tokens":110,"cached_tokens":110,...}}
data: [DONE]
```

假定每帧都有 `choices[0]` 的解码器要么 panic 要么丢掉 usage。丢了 usage 三层缓存兜底
全部失明，而对话内容看起来一切正常。

**2. 空值表达不同**：DeepSeek 显式 `"content": null`，另两家省略字段。不能用「字段存在」
判断有没有内容。

**3. GLM 每帧重复 `role: "assistant"`**，累积时忽略。

思考内容三家都在 `delta.reasoning_content`，且都排在 `content` 之前流出。
工具参数都按 `index` 累加。

## 四、错误

骨架一致 `{"error": {"message", "type", ...}}`，DeepSeek 额外有 `code`/`param`。
但**状态码分配不一致**：

| 场景 | DeepSeek | Kimi | GLM |
|---|---|---|---|
| 模型名不存在 | 400 | **404** `resource_not_found_error` | 400 |
| key 无效 | 401 `authentication_error` | 401 `invalid_authentication_error` | 401 |
| 过载 | 503 | **429** `engine_overloaded_error` | — |
| 余额耗尽 | **402** | — | — |

**按状态码分类不够**——Kimi 的模型名错误是 404，而 404 在别处通常意味着不可恢复的路径
问题。要先按 `error.type` 判，落不到再按状态码。

**402 必须单列**：余额耗尽退避重试毫无意义，且要立刻告警到人；混进限流会让系统安静地
退避到天荒地老。

## 五、服务端工具

**GLM 的对我们完全不透明。** `web_search` 调用返回 200，但响应里没有 `tool_calls`、没有
任何搜索痕迹，`finish_reason` 就是 `stop`，顶层也没多出字段。

它不表现为一次工具调用——在模型内部发生，router 不参与也无从观测。所以它**不进
command log、undo 回滚不了、副作用等级无从判断、审计链路有洞**。

正确的建模不是「第四种执行位置」，而是**一个会话级开关，开了就等于放弃这部分的可审计性**。
这是要显式承认并让用户知情的取舍，见 `docs/TOOLS.md`。

**Kimi 的 formulas 不同**：声明成普通 `function`，我们收到 `tool_calls` 后自己去
`POST /v1/formulas/{uri}/fibers` 执行——那个是可见的，正常走 `Location` 与 effect 判定。
两者别混。

### 缓存写入是异步的：背靠背请求会撞上延迟窗口（2026-08-01 生产实测）

DeepSeek 真实十轮里，工具跳的第二跳（距上一跳完成毫秒级）命中 640 而非预期的
2304——640 恰为**数轮之前**镜像的块取整，说明缓存以异步/批量方式入库，写入延迟
可达秒到数十秒；下一次正常节奏的调用即恢复满命中（2304/2375=97%）。

**后果**：兜底第 2 层「只跟上一次比」在紧凑连调时会真阳性告警。缓解方向是留最近
N 个镜像、把「命中恰等于某个旧镜像的取整」判为「写入延迟」而非「语义理解错误」
（ROADMAP §四那条已知局限，已从「等真遇到」变成「真遇到了」）。

## 六、顺带

**GLM 思考量很大**：「只回答一个字：好」花了 `completion_tokens: 194`，其中 191 是
reasoning；DeepSeek 同 prompt 只用 15。它输出单价虽低，但每次简单调用都思考两百 token
的话实际成本要重算，默认档位该不该调低值得单独测。

**DeepSeek 的 `system_fingerprint` 带 `kvcache` 字样**
（`fp_9954b31ca7_prod0820_fp8_kvcache_20260402`）。指纹变了可能意味着后端换了、缓存
随之失效——可作为兜底第 3 层的辅助信号，用来区分「他们换后端」和「我们的 bug」。

## 七、消息级 system 注入（038，2026-08-03）

回答 [038](../docs/issues/038-skill-injection-probe.md) 的四问：会话中途在历史尾部追加
一条 `{"role":"system","content":...}` 消息，三家收不收、听不听、保不保前缀、跟「同内容
改并入顶层 system 重建」比代价差多少。探针：`probes/api/src/exp/system_inject.rs`，
结果文件 `probes/results/system-inject.json`。方法：先建一条 ~4400 token 的普通对话
前缀（4 轮），再在尾部插入指令「每次回答末尾加【标记X】」+ 新问题，跑 3 次取多数；
另起一次把同一条指令直接拼进顶层 system 内容的尾部（消息数不变，只改第一条消息的
`content`）作对照。行为断言用真实回答有没有出现`【标记X】`，不是状态码。

| | 收（3 次） | 听（3 次） | 前缀保持（注入首次命中/基准） | 对照差值（重建首次命中 − 注入首次命中） |
|---|---|---|---|---|
| DeepSeek | 3/3 | 3/3 | **0/4352 = 0%** | 3968 − 0 = **+3968**（重建反而更省） |
| Kimi | 3/3 | 3/3 | 4352/4352 = 100% | 0 − 4352 = **−4352**（注入更省） |
| GLM | 3/3 | 3/3 | 4352/4352 = 100% | 0 − 4352 = **−4352**（注入更省） |

**三家都收、都听**——HTTP 全部 200，且三次里三次都在回答末尾看到标记，包括 DeepSeek
（首次那次差点被误判「未听」：`max_tokens` 一开始给了 400，DeepSeek 默认开着思考，
一次就吃掉 314 个 `reasoning_tokens`，正文被截成空字符串；调到 1000 后正文和标记都
完整出来。这本身是个提醒：测「听不听」必须给够 `max_tokens`，否则「未听」可能只是
「被截断」，跟行为无关）。

**DeepSeek 是反直觉的那一个**——前缀保持这一项跟 Kimi/GLM 反着来：

- Kimi、GLM：插入消息**不影响**已缓存的历史，注入首次调用即命中基准的 100%
  （GLM 甚至复发到 4416，说明新增的尾巴一部分也进了下一块）。跟 PROVIDERS.md 已有的
  「Kimi 消息级 tools 零代价」结论一致，现在确认**消息级 system 对 Kimi、GLM 也是零代价**。
- DeepSeek：插入消息首次命中是 **0**——即使插入点之前的每一个字节都跟已缓存的
  基准逐字节相同。但反过来，把同一段文字直接拼进顶层 system 内容的尾部（结构不变，
  只是那条消息变长了），首次命中反而有 **3968/4352 ≈ 91%**。合理的猜测：DeepSeek 的
  前缀匹配可能跟「消息条数/结构」绑定得比字节序列更紧——原本排在最后的 assistant
  消息，一旦后面又接上新消息（从「末位」变成「中间」），序列化边界就不再是原来那个，
  即使内容字节相同也判「新请求」；而只改**已有消息**的内容尾部不动消息结构，命中
  按最长公共前缀正常算。**这只是推测，没有反查 DeepSeek 的 tokenizer/模板源码**，
  但两次独立观测（这里的 system 注入、`二`节已有的 tools 表）方向一致，够写进结论。
- 三家的「首次归零」都只是「这个变体第一次见」，不是「作废」——轮 3（注入后再来
  一轮）三家都在新前缀上正常延展（DeepSeek 98.4%、Kimi 98.0%、GLM 99.6%），代价只
  出在切换的那一轮。

**对 039 的直接含义**：「消息级追加天然比重建省」这个假设**不能跨三家通用**。
Kimi、GLM 上按 039 的原计划（追加消息）就是对的；**DeepSeek 上应该反过来**——把
注入内容拼进已有顶层 system 消息的尾部，代价不到重建的十分之一（9% vs 100%），
而不是插入一条新的 `role:system` 消息。DeepSeek 的命中折扣是三家里最陡的 120x，
这个方向错了代价最大。est_cost_multiple 的量级：DeepSeek 选错方案时这一轮的额外
成本 ≈ 4352 个 token 从 120x 折扣价打回原价；Kimi/GLM 选错时是 4352 个 token 从
10x/2x 折扣价打回原价——同一个错误在 DeepSeek 上贵得多。

（一处旁观察，不进结论：对照组的重建请求只跑了 1 次而非 3 次——它测的是 cached_tokens
不是听不听。Kimi 那一次回答是 `tier=1`，没带标记；DeepSeek、GLM 都带了。n=1，不排除
是「指令混在一大段参考文档尾部、被长上下文稀释」的巧合，值得听，但不够格写成结论。）

本组花费：21 次调用，三家共约 ¥0.3（大头是 `max_tokens=1000` 的思考 token，实付
`completion_tokens` 普遍在 60–400 之间；prompt 侧靠 4352 token 的基准前缀反复命中，
接近零成本）。

## 端到端验证（2026-07-31）

> 跑这组数据的那份实现已随 `crates/` 一起删掉（[ROADMAP §二](../docs/ROADMAP.md)）。
> **数字仍然有效**——它们是 provider 的行为，不是我们代码的行为。
> [024](../docs/issues/024-cache-guard.md) 做完后可以照这张表复验一遍，
> 对不上就是新实现有 bug。

三家各跑四步：两次严格延长 + 一次改工具表 + 一次流式。

| | 严格延长 | 改工具表 | 流式收到 usage | 会话花费 |
|---|---|---|---|---|
| DeepSeek | 2432/2463，不报警 | 命中 0，**报出前缀被动** | ✅ | 61.3x |
| Kimi | 2304/2399，不报警 | 命中 0，**报出前缀被动** | ✅ **尾帧收到** | 5.7x |
| GLM | 2624/2683，不报警 | 按字节比例估算故不报警 | ✅ | 1.5x |

三家的严格延长都判 `Ok`——**第 2 层在正常多轮里零误报**。会话花费的 61.3 / 5.7 / 1.5
就是折扣比的直接体现：同样两次冷调用，DeepSeek 的代价是 GLM 的四十倍。

**露出一个局限**：第 2 层只跟「上一次」比，而 provider 的缓存是多变体并存的。
来回切前缀时会判成「好于预期」。不误报但会漏，取舍记在
[ROADMAP §四](../docs/ROADMAP.md) 和 [024](../docs/issues/024-cache-guard.md)。

## 怎么重跑

```bash
cd probes/api
cargo run --bin cache_prefix              # 前缀缓存的五组实验
cargo run --bin wire_shape                # wire 行为差异
cargo run --bin cache_prefix -- --help    # 单跑某组
```

需要 `providers.toml`（已 gitignore）。每次运行用时间戳 nonce 保证冷缓存开始。
全跑一轮三家共约 ¥2 以内。

**结论以本文为准，`results/*.json` 是原始观测。** 价格会变，块粒度、匹配语义、
`tool_choice` 支持都可能随版本变——怀疑就重跑，别靠读文档。

## 未测

- 限流时是否带 `Retry-After`（正常响应下三家都没有任何限流头）
- 多模态入参形状
- JSON mode / structured output
- stop 序列上限（Kimi 文档说 5，另两家未查）
- GLM 的 retrieval / MCP 两种服务端工具（web_search 已测）
- Kimi formulas 的 `POST /v1/formulas/{uri}/fibers` 执行流程
