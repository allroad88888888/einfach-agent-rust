# 171 demo 页首屏改造：从开发页变成展示页

**里程碑** L · **依赖** [170](170-pages-workflow.md) · **模型** sonnet · **独测** — · **估时** 20min · **状态** 待开始

## 目标

`www/index.html` 今天是**开发调试页**（M13/M14 期间自己用的）。挂到公开 URL 上之后，
它是绝大多数人对这个项目的**第一印象，也常常是唯一一次印象**。

一个陌生人落到那个页面，30 秒内必须回答三个问题：**这是什么** / **我的 key 安不安全** /
**我该点哪里**。答不上来就关掉了。

## 现状

- 页面直接是操作界面，没有任何自我介绍。
- key 输入框没有解释——**这是最大的流失点**：让人在一个陌生网页里贴 API key，
  不解释清楚没人会做。而这里的真相恰恰是最好的卖点：**key 不出浏览器**。

## 做什么

1. 首屏加一段简介（英文，[165](165-launch-positioning-decision.md) L1）：一句话定位 +
   「no server — this page **is** the agent runtime, compiled to wasm」。
2. **key 说明**（这段值得单独写好）：
   - your key stays in this tab, goes straight to the provider
   - 页面**没有后端可以泄露到**——这不是承诺，是架构事实
   - 指一下怎么核实：打开 Network 面板，只有对 provider 的请求
   - 支持哪几家 + 各自拿 key 的链接
3. 三个钩子做成**可点的引导**（[165](165-launch-positioning-decision.md) 那三条），
   最重要的是 `/undo` ——给一句预置的话术让人照着试。
4. 加一行 GitHub 回链（demo 的目的是把人送回仓库）。

## 验收

- 一个没读过 README 的人，只看首屏能说出「这是什么」和「key 去哪了」
- 页面在窄屏（手机）不塌——很多人是从手机点开社交媒体链接的
- 改动**只碰 `index.html` 的展示层**，不动 `page-tools.js` 等执行路径
  （[169](169-wasm-artifact-recheck.md) 验过的东西不要在这一步动）
- 改完重新验一遍 [169](169-wasm-artifact-recheck.md) 的「跑通一轮」

## 注意

别为了好看引外部 CSS/字体 CDN——多一个外部依赖就多一个加载失败点，
而且会破坏「这个页面除了 provider 不连任何东西」这个卖点。**内联样式**。
