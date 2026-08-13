# 186 文章：别在 core 里 `match provider`

**里程碑** L · **依赖** [165](165-launch-positioning-decision.md) · **模型** **opus** · **估时** 20min · **状态** 待开始

## 目标

讲决策 17：**把「事前问能力」改成「事后报调整」**。

这是本仓最有转载价值的单个架构洞见——它解决的是一个**所有做多 provider 抽象的人
都会撞上**的问题，而通行解法（能力位 + 分支）是错的。

## 素材

决策 14（被取代的原案）、决策 15、决策 17，以及红线 12。

## 写什么

标题方向：*Capability flags are `match provider` wearing a hat*

论证链：

1. **通行解法**：抽一个 `Capabilities { supports_tools, supports_prefix, ... }`，
   core 里 `if caps.supports_x()` 分支。看起来干净——provider 名字不出现在 core 里了
2. **为什么它是错的**：N 个能力位 = 2^N 种组合，**其中绝大多数从没跑过**。
   加一家新 provider 仍然要回来改 core（因为它的组合是新的）。
   能力位只是把 `match provider` 换了层皮，**分支还在 core 里**
3. **改法**：core 只说**意图**，adapter 做不到就报一条 `Adjustment`。
   测试组合从 2^N 掉回 1
4. **额外白拿的**：`Adjustment` 是可观测数据——降级不再是静默的，
   它在这一轮就以事件形式浮出来，而不是下个月在账单上
5. 边界：什么该进 adapter、什么该留 core（决策 15：**请求组装归 adapter，core 只供料**；
   决策 18 压缩三分是这条原则的一次精细应用——触发和实现在 core，摆盘在 adapter，
   **因为 adapter 是纯函数无权改世界**）

## 验收

- 第 2 点的论证要**站得住**——这是全文的支点，写虚了整篇就垮了。
  关键是「2^N 组合里多数没跑过」和「加一家仍要改 core」两句都要有说服力
- 有一个具体例子贯穿（建议 prefix 续写：DeepSeek 要换 `base_url`、Kimi 不用，
  这个差异很具体且好懂）
- 读者能拿走可迁移的判据，不需要用这个项目
