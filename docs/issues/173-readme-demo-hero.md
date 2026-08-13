# 173 README 第一屏挂 demo + 填 homepage

**里程碑** L · **依赖** [170](170-pages-workflow.md) + [172](172-demo-gif.md) · **模型** sonnet · **估时** 15min · **状态** 待开始

## 目标

把 [170](170-pages-workflow.md) 的 URL 和 [172](172-demo-gif.md) 的 GIF 放到**人一定会看见**的两个位置。

做完这条，L1a 这一波才算真的兑现——前四个 issue 的产出如果没被链接到，等于没做。

## 做什么

1. `README.md` 第一屏（标题下面、任何散文之前）：
   - 一行 **▶ Try it in your browser — no install, no server** + 链接
   - 紧跟 GIF
   - **顺序不能反**：链接在图上面。滚动看图的人可能不往下翻找链接
2. `README.zh-CN.md` 同款。
3. `gh repo edit --homepage <pages-url>` —— 填上 [168](168-repo-metadata.md) 有意留空的那个字段。
   它会显示在仓库页右上角「About」区，是 star 之外点击率最高的位置。

## 验收

- README 顶部链接可点、GIF 正常渲染（**在 GitHub 上看**，不是本地 markdown 预览——
  两者对图片路径的处理不一样）
- `gh repo view --json homepageUrl` 回显 Pages URL
- 从仓库首页「About」的链接点过去能打开 demo

## 注意

现在 README 的开头是 `> English is the primary project language.` 那行语言切换。
**demo 链接要放在它上面还是下面？** 放上面——语言切换是给已经决定要读的人用的，
demo 链接是给还没决定的人用的，后者更早流失。
