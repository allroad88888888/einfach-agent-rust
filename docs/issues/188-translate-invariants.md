# 188 英译 `INVARIANTS.md`

**里程碑** L · **依赖** [165](165-launch-positioning-decision.md) · **模型** sonnet · **估时** 20min · **状态** 待开始

## 目标

228 行。**三份英译里优先级最高的一份**——因为 [185](185-post-invariants.md) 那篇文章会指向它，
而且「红线」是这个项目最容易让工程师产生好感的东西：它证明作者知道什么会坏。

## 做什么

`docs/INVARIANTS.en.md`，与中文版**并存**（不替换）。中文版仍是权威，
英文版顶部注明「translated from INVARIANTS.md; the Chinese version is authoritative」。

**为什么并存而不替换**：这个仓的开发在中文进行，替换会让日常开发变慢，
而变慢的文档很快就会过期——[167](167-readme-stale-mechanism.md) 刚吃过一次过期文档的亏。

## 注意

1. **不要直译**。中文技术写作里的省略在英文里会读不通，尤其那些「靠上下文补主语」的句子。
2. 红线的**理由**比条目本身重要，理由段要译得特别准——译丢了它就退化成一张规则清单，
   而规则清单没人看。
3. 术语对照要跟 README 已用的英文一致：atom / derived / command layer / journal /
   reversibility barrier / prefix chunk。**建一个小对照表放文件底部**，
   后面 [189](189-translate-architecture.md)/[190](190-translate-state-model.md) 直接沿用。

## 验收

- 12 条齐全，每条的理由段都在
- 术语与 README 一致，且对照表建好了
- 「1–6 条错了不会立刻报错」那段总结要在——**它是全文最有价值的一句**
- 英文母语者读起来不像机翻（长句拆开、被动语态收敛）
