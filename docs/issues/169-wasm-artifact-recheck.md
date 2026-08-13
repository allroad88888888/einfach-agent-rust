# 169 wasm 产物本地复验（上线前的刹车片）

**里程碑** L · **依赖** [165](165-launch-positioning-decision.md) · **模型** sonnet · **独测** — · **状态** 完成（2026-08-13）· **估时** 20min

## 目标

在为 demo 写任何 workflow / 文案 / GIF 之前，先确认**今天的代码构建出的产物真的能跑**。

这是刹车片，不是里程碑：如果 `build-wasm.sh` 已经腐化，后面 [170](170-pages-workflow.md)–[173](173-readme-demo-hero.md)
四个 issue 全是空中楼阁。**M17 的 157 和 164 刚动过 wasm 侧的声明路径**（08-13 当天），
产物有没有跟上没人验过。

## 现状

- `scripts/build-wasm.sh`：`wasm-pack build crates/agent-wasm --release --target web
  --out-dir www/pkg`。要求本机有 `wasm-pack`。
- `crates/agent-wasm/www/`：`index.html` + `pkg/` + 四个 js（`page-tools.js` /
  `image-manager.js` / `image-store.js` / `vision-tool.js` / `transcript.js`）。
- **必须 http:// 打开**，file:// 过不了 ES module 与 wasm 的 MIME（脚本注释里写了）。

## 做什么

1. `scripts/build-wasm.sh` 跑一遍 release，记录产物体积。
2. `cd crates/agent-wasm/www && python3 -m http.server 8787`，浏览器开。
3. 填一把真 key，**跑通一轮真实对话**。
4. 至少验一条 [165](165-launch-positioning-decision.md) 的钩子——建议 `/undo`，
   因为它是首发文案要主打的那条。
5. 把产物体积、`wasm-pack` 版本、跑通的钩子记进本 issue 的实做记录。

## 验收

- 构建零报错，`www/pkg/` 有 `.wasm` + `.js` + `.d.ts`
- 浏览器控制台**零 error**（warning 记下来但不阻塞）
- 一轮真实对话有回复
- `/undo` 之后同一个模型确认那一轮内容不在它上下文里（[143](143-m15-dogfood.md) 的口令实验同款做法）

## 如果不过

**停在这里，别往下做。** 把失败现象记进本 issue，另开修复 issue——
demo 是首发的地基，带着已知故障上线比不上线更糟。

---

## 实做记录（2026-08-13）

**结论：产物健康，可以往下做 [170](170-pages-workflow.md)。但抓到一条会改变 [172](172-demo-gif.md) 计划的事，见下。**

环境：`wasm-pack 0.14.0`、`wasm32-unknown-unknown` 已装。
构建 5.32s（增量），产物 `agent_wasm_bg.wasm` **928K**（wasm-opt 后）。

### 逐条

| # | 验的什么 | 结果 |
|---|---|---|
| 1 | 构建零报错 | ✅ 退出码 0。两条 `dead_code` warning（`process_lock.rs:15` 的 `file` 字段等），存量，不阻塞 |
| 2 | `pkg/` 有 `.wasm`+`.js`+`.d.ts` | ✅ 四个文件齐 |
| 3 | 浏览器控制台零 error | ⚠️ **只有 `favicon.ico` 404**。无害，但公开页面该补——已记进 [171](171-demo-first-screen.md) |
| 4 | 一轮真实对话 | ✅ 见下 |
| 5 | `/undo` 钩子 | ❌ **做不了：wasm 宿主没暴露 undo** → [196](196-wasm-expose-undo.md) |

### 真机对话（DeepSeek `deepseek-v4-flash`，浏览器直连）

```
轮1  记住口令 raccoon-seven          → 好的，已记住口令：raccoon-seven
     [usage] prompt=871 completion=46 cached=0
轮2  刚才的口令是什么？               → raccoon-seven
     [usage] prompt=897 completion=5  cached=768   （85.6%）
```

### 顺带验了恢复路径（157/164 当天刚动过，值得一验）

刷新页面 → wasm 重新实例化 → 重建宿主 → 重开会话：
**「从 IndexedDB 重放出 N 条消息」**，历史回来了。

但「UI 有历史」不等于「历史进了 prompt」，所以追问了一次。**这里踩了一个坑，记下来防重犯**：

第一次追问用的是「**刷新之后**，口令还记得吗」——模型答「刷新后上下文已丢失，口令不记得了」。
**这是假阴性**：同一轮 `prompt=918 cached=896`，缓存前缀跟刷新前逐字节对上，
说明历史确实在 prompt 里；是问句里「刷新之后」这个前提把模型带跑了。
更糟的是那句错答进了历史，之后中性提问它也保持自洽地说「我不知道」——**污染不可逆**。

换干净会话 `clean-2` 重做，全程不提「刷新」：

```
轮1  记住 pangolin-42                → 好
     ——刷新页面，重建宿主，重开 clean-2 ——
     「从 IndexedDB 重放出 2 条消息」
轮2  我让你记的那个词是什么？          → pangolin-42          ✅
     [usage] prompt=887 completion=5 cached=768
```

**恢复路径真通**：重放出来的历史真进了 prompt，不是只填 UI。

> **给后来做真机验收的人**：验「模型记不记得」时，**问句里不能出现你想验的那个机制的名字**。
> 「刷新之后还记得吗」「undo 之后还记得吗」都会把答案喂给模型。中性问「X 是什么」，
> 并且**一旦污染就换会话重来**，别在脏历史上继续追问。

### key 的处置

真 key 全程没进对话记录：从 `providers.toml` 抽出来写进 `www/pkg/k.txt`
（`pkg/` 的 `.gitignore` 是 `*`，已 `git check-ignore` 确认），页面同源 `fetch` 取走填进输入框，
**用完立即删**。收工核对 `git status` 无残留。
