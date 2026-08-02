# 023 三家适配与 `Capabilities`

**里程碑** M1 · **依赖** 022 · **模型** sonnet · **独立测试 agent** ✅ · **状态** 完成

## 目标

`providers.toml` 换一行 `[default]`，三家都能跑同一段 core 逻辑。

## 为什么排在 022 后面而不是一起做

022 只接一家时，`Capabilities` 里每一位都只有一个取值，**看不出哪些位真的被用到、
哪些是照着差异清单抄下来没人用的**。接第二家时才会知道。

上一版一次性定了九个字段，其中有几位从没被两种取值分别走过，还有一位
（`siblings_share_prefix`）压根不对应任何一个动作——纯粹是照着差异列的。

## 做什么

### `Capabilities` 不出 adapter（红线 12）

它是 adapter 内部干活的依据，**`agent-core` 里 grep 不到这个词**。

「思考关不掉导致强制指定工具永久不可用」这类事，core 不事先问——core 直接说
`intent: MustUse(fs/read)`，adapter 降级并回一条
`Adjustment::ToolChoiceDowngraded`。core 一条路径走到底，本来就得校验模型有没有
听话（**强制调用在任何一家都不是保证**）。

实测数据在 [probes/PROVIDERS.md](../../probes/PROVIDERS.md) 的速查表，**照抄**，
别重新调研。数据带日期，价格和块粒度会随版本变。

### 三个各自的文件夹

```
agent-providers/deepseek/   mod / encode / decode
agent-providers/kimi/
agent-providers/glm/
agent-providers/wire.rs     三家共享的 OpenAI 兼容骨架
```

共享骨架里的前缀顺序是 `[Tools][System][History]`——**实测确认顶层 `tools`
渲染在 prompt 最前面**（PROVIDERS.md §一）。这个顺序不能改，改了整个缓存前缀失效。

### 组装差异落在各自的 `encode` 里（决策 15）

这是 022 那条划分第一次真正被使用。至少这四处三家不一样：

| 差异 | 各家怎么办 |
|---|---|
| 中途加载的工具 | 有消息级 tools 的家追加到末尾（零代价）；没有的家只能并进顶层，代价从 2x 到 120x |
| skill 注入位置 | 前缀树匹配的家可以放兄弟分支；仅扩展匹配的家只能追加在末尾 |
| thinking | 有的家 `thinking.type` 进缓存前缀，改一下前缀就作废；有的不进 |
| temperature | 有一家只接受 `1`，传别的直接 400 |

**这些细节只出现在 `encode.rs` 里**，core 一个字都不该提到厂商名。

## 验收

- 三家各自的 `encode` 单测：给同一份料，产出三份不同的 wire JSON，
  且每份**跑两次逐字节相同**（红线 11）
- `intent: MustUse(name)`：在做不到的家上降级，**回一条 `Adjustment`，不是静默忽略**
- `cargo run -p agent-cli` 三家各跑通一轮，CLI 把 `Adjustment` 打出来
- **`agent-core` 里 grep 不到厂商名、`Capabilities`、`caps.`**（红线 12）
- `Capabilities` 的每一位，**至少有两家取值不同，且 adapter 内部真的用到**——
  两条有一条不满足就删

最后一条是这个 issue 的核心判据：它把「真的在用的位」和「照着差异抄的位」分开。

## 注意

- 红线 11：三份 encode 都要过逐字节确定性检查，不是只查一份。
- 红线 9：三个文件夹各自 ≤300 行，共享的进 `wire.rs`。
- **文档不可信，以实测为准。** 已确认两处直接矛盾：一家文档说 `tool_choice`
  四种取值全支持、实际两种直接 400；另一家文档说只支持 `auto`、实际四种全支持。
  要确认就重跑 `probes/api`。

**为什么派独立测试 agent**：红线 11 违反后功能全绿，只在账单上浮出来。
测试 agent 只看本文的验收标准和红线条目，看不到 `encode` 的实现体。

## 实做记录（2026-08-01）

实现与独测并行，独测 18 个测试**未改一字**对着实现一次全过。合并后又经真实
调用逼出三个问题，全部修完，最终 workspace 325/0：

1. **CLI 写死 DeepSeek adapter**（真跑 Kimi 时横幅打 `provider=deepseek
   model=kimi-k3`）。022 写它时世上只有一家，023 只动 agent-providers——
   「按 [default] 选 adapter」两个 issue 都没认领。修：CLI 按配置名分发
   `Box<dyn Provider>`，未知名启动即报错。**宿主里的 match provider 合法**
   （红线 12 只管 core），注释已写明防误删。
2. **500ms 读超时杀死慢首字节的家**（Kimi 等状态行即超时，重试 3 次全灭）。
   且这里不能靠重试兜：请求已送达，重发=双倍计费。修：读线程 + channel，
   socket 超时 60s 只做死流兜底，取消灵敏度由主流程 recv_timeout(100ms) 决定。
   DeepSeek 之前能过纯属首字节快，这 bug 对三家都在。
3. **预测模型漏了起效门槛**（GLM 真跑第 2 轮 predicted=448 / actual=0 误报）。
   GLM 有实测零区（~460 完全不缓存），零区里按块取整是确定地错。修：
   `prefix::compare` 加 `min_predict` 参，GLM 传夹逼上界 860、之下不预测
   （=0，第 2 层视为无预测不判）；DeepSeek/Kimi 无零区传 0。实现 agent 原注释
   说「门槛处理归第 2 层」——不成立，门槛值是模型数据，core 不许知道（红线 12）。
   复跑 GLM 两轮：两轮都「predicted=0 / actual=0 一致」，误报消失。

**三家真跑记录**：DeepSeek 两轮（第 2 轮 512/512 一致）；Kimi 一轮（冷启动
`cached=None`——字段整个缺失，CLI 如实打「这家没报」，None/Some(0) 语义
第一次在真实世界露脸）；GLM 修后两轮（0/0 一致 ×2）。

**Capabilities 审计结论：没有造 Capabilities 结构**。每个差异位都是各家
mod.rs 里的 `pub(crate) const` 或各自 encode 里的独立代码路径；错误分类
甚至三家共用一个按 error.type 关键词的函数、零分支。审计表（8 位：块粒度/
消息级 tools/指定函数/温度/工具上限/usage 语义/错误分类[删]/thinking[文档化]）
在实现 agent 的报告里，此处不重复——「至少两家不同且真的用到」两条判据
全部满足。Kimi 工具上限未公布 → 不发明数字，不裁不报 ToolsTruncated。

**独测披露**：测试 agent 误读过一眼 `deepseek/mod.rs`（禁区）。泄露内容全部
是 PROVIDERS.md 已公开的事实，且其断言刻意避开了具体转义格式——独立性无实质
损伤，采信。
