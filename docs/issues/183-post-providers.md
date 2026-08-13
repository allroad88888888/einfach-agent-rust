# 183 文章：三家 provider 的真实差异（实测）

**里程碑** L · **依赖** [165](165-launch-positioning-decision.md) · **模型** **opus** · **估时** 20min（初稿） · **状态** 待开始

## 目标

把 `probes/PROVIDERS.md`（301 行实测）改写成一篇能发的文章。

**赌它是单篇流量最高的一件**：全网几乎没有人公开发过 DeepSeek / Kimi / GLM 的
逐条对比实测，尤其带 prompt cache 命中率的真实数字。这类内容的特点是
**长尾搜索流量**——发出去之后一年里持续有人搜到。

## 素材

- `probes/PROVIDERS.md` —— 唯一结论文档
- `probes/results/` —— 原始观测
- 决策 17 / 决策 18 的取舍（差异是怎么被消化掉的）

## 写什么

标题方向：*What actually differs between DeepSeek, Kimi and GLM — measured, not documented*

1. 为什么要实测：三家都自称 OpenAI 兼容，**文档说的和实际返回的不一样**
2. 逐条差异（拿真实响应片段，不是转述）
3. **prompt cache 那一节是重点**——命中率的真实数字、什么动作会打掉缓存、代价多大
   （「DeepSeek 上 120 倍」这个数字很有冲击力，但要写清它的前提）
4. 结尾轻轻带一句这些差异在 einfach-agent 里被 adapter 消化掉了，**给个链接就够**
   —— 文章的价值必须独立于项目存在，硬推销会毁掉转发

## 验收

- **不读代码、不用这个项目的人也能获得价值**（这条是硬标准，做不到就重写）
- 每个结论都能追溯到 `probes/results/` 里的一次真实观测
- 数字与 `PROVIDERS.md` 一致，**没有为了效果四舍五入**
- 项目链接**不超过两处**，且都在自然的位置

## 渠道

英文优先（[165](165-launch-positioning-decision.md) L1）。中文版价值同样高，
排 L2 第二波。
