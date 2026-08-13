# 185 文章：不会报错的那几类 bug

**里程碑** L · **依赖** [165](165-launch-positioning-decision.md) · **模型** **opus** · **估时** 20min · **状态** 待开始

## 目标

把 `docs/INVARIANTS.md` 的 12 条红线改写成一篇工程文化文章。

这类内容是 Hacker News / r/rust 的口味：**具体、有代价、可迁移到读者自己的项目**。
它不推销产品，推销的是「这个人知道自己在干什么」——而这恰恰是让人愿意试你项目的前提。

## 素材

`docs/INVARIANTS.md` 全文，尤其 CLAUDE.md §红线摘要里那段总结：

> 1–6 条错了不会立刻报错，会在 undo 或崩溃恢复时以静默错值的形式浮出来。
> 第 11 条同理——功能完全正常，只是每一轮都全价（DeepSeek 上 120 倍）。
> 第 12 条也一样静默——一直正常到加第四家 provider 时发现要改 core。

## 写什么

标题方向：*The bugs that don't fail — they just quietly give you the wrong value, three weeks later*

核心论点：**有一类 bug 的定义特征是「测试全绿、功能正常」**，它们只在
undo / 崩溃恢复 / 加第四家 provider 的那一刻显形。因为它们不会报错，
所以只能靠**结构性约束**挡，不能靠 code review 挡。

挑 3–4 条讲透，别 12 条平铺：

- **derived 的 read fn 必须纯**（放个时钟进去，undo 之后重算就是另一个值）
- **进 prompt 的东西序列化必须逐字节确定**（`HashMap` 迭代序 → 每轮缓存全掉 → 120 倍账单，
  **功能完全正常**）
- **core 里不许有模型判断**（一直正常到加第四家）
- **在飞的 effect 必须带 epoch**

结尾讲 `scripts/check-invariants.sh`：**能被 grep 判定的就自动化，
判断不了的写成文档 + skill**。这个分界本身是可迁移的方法论。

## 验收

- 每条都有**具体的失效场景**（什么时候、以什么形式浮出来），不是抽象原则
- 120 倍那个数字要写清前提，别当标题党用
- 读者能拿走一个可用于自己项目的判据：**「这条能不能 grep」**
