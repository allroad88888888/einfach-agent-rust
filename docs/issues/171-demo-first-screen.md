# 171 demo 页首屏改造：从开发页变成展示页

**里程碑** L · **依赖** [170](170-pages-workflow.md) · **模型** sonnet · **独测** — · **估时** 20min · **状态** 完成（2026-08-13）

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

---

## 实做记录（2026-08-13）

### 改了什么

1. **`<title>`** 从 `agent-wasm 浏览器宿主（issue 114c）` 换成
   `einfach-agent — an agent runtime running entirely in your browser`。
   标题是搜索结果和分享卡片上的那一行，内部 issue 号对外没有意义。
2. **favicon**：内联 SVG data URI（`↺`，一号钩子的符号）。
   [169](169-wasm-artifact-recheck.md) 查出的唯一 console error 是 `favicon.ico` 404
   ——**现在控制台零 error**。用 data URI 而不是外链文件是有理由的：这个页面的卖点
   之一就是「除了 provider 不连任何东西」，多一个外部请求就多一个能打脸的地方。
3. **首屏英文化**（[165](165-launch-positioning-decision.md) L1）：一句话定位
   + key 安全 + GitHub 回链。
4. **key 说明**写成最强的那个形状：*This isn't a promise, it's an architectural fact:
   there is no backend to leak it to.* 并给了**核实方法**（DevTools → Network），
   外加三家拿 key 的链接。让人在陌生网页里贴 API key 是最大的流失点，
   而这里的真相恰恰是最好的卖点。
5. **四步口令实验**写成可照抄的话术（[196](196-wasm-expose-undo.md) 真机跑通的那一套）。
6. **七处访客必须碰的控件双语化**：api_key / 建宿主 / 会话 id / 打开会话 / 输入框 /
   发送 / 撤销一轮 / 重做。
7. 窄屏 media query（≤34rem）：标签占整行，输入框不被挤成一条缝。

### 中途返工一次（值得记）

第一版把「Try this」四步和「两件值得试的事」全放在**首屏顶部**。截图一看
——**hero 占满整个视口，api_key 输入框被挤到折叠线以下**。这就等于没回答
「我该点哪里」，而那是首屏三个问题里最要命的一个。

改法：顶部压到 5 行（定位 + key 安全 + 一句「paste it below … then try the 4-step demo」
带锚点），**四步实验整块挪到对话框正上方**（`<details open>`）——那才是用得着它的地方。
读者是先连上再玩，不是先读完再连。

> **可迁移的判据**：first screen 的成败不看写了多少，看**能操作的东西在不在屏幕里**。
> 写完一定要截图看，不能凭想象——我这次就是凭想象写的，一截图就露馅。

### 验收

- [x] 一个没读过 README 的人只看首屏能说出「这是什么」「key 去哪了」
- [x] 窄屏（390×844，iPhone 尺寸）不塌：`scrollWidth == innerWidth == 390`，零横向溢出
- [x] 只碰展示层——`page-tools.js` 等执行路径一个字节没动
- [x] 改完重跑 [169](169-wasm-artifact-recheck.md) 那条：建宿主 → 开会话 →
      真实一轮 → 撤销（「撤了 2 条（turn 2）」，历史清零）全通
- [x] **控制台零 error**（原先唯一那条 favicon 404 已消除）
- [x] 零外部资源：无 CDN、无外链字体、favicon 也是 data URI

### 留的尾巴

- **操作区的标签仍是中文**（`context_window` 的说明、`工具回调`、`工具声明` 那几行）。
  没有全量英文化，因为这个页面同时是 **114c/119/122 的验收夹具**，那些标签带着 issue 号
  和验收语义。访客必须碰的七个控件已双语，够用；真要全量英译是另一个决定，
  牵扯「验收页与对外 demo 要不要拆成两个」——**别顺手做**。
- `html lang` 仍是 `zh-CN`，与英文首屏不符。同上，等拆不拆那个决定。
- 撤销撞屏障时用的是原生 `confirm()`。够用，但录 GIF（[172](172-demo-gif.md)）时那一帧
  很难看，且它恰好是最该被看清的一帧。
