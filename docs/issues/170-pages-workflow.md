# 170 GitHub Pages 部署 workflow

**里程碑** L · **依赖** [169](169-wasm-artifact-recheck.md) · **模型** sonnet · **独测** — · **状态** workflow 已写，**等你开 Pages 开关**（2026-08-13）· **估时** 20min

## 目标

把 `crates/agent-wasm/www/` 变成一个**公开可点的 URL**。

这是整个推广里 ROI 最高的单件：「点一个链接就看见它跑」和「clone + 装 Rust + 填 key」
之间的转化率差一个数量级。而且 demo 本身就是技术宣言——**别人的 agent 框架 demo 都要起服务，
这个不用**。

## 现状与前提

- `www/` 是**纯静态**：`index.html` 直接 `import` `pkg/agent_wasm.js`，不需要 bundler。
- 没有服务端 agent 进程——静态托管只发三种字节，不参与任何一次模型请求
  （`build-wasm.sh` 的注释里已经把这条边界写清楚了，[114](114-wasm-host.md) 验收第一条同款口径）。
- key 由访问者自己填，**直连 provider**（决策 26 已实测三家 CORS 放行任意 origin）。
- ⚠️ **与 [165](165-launch-positioning-decision.md) L3 的关系**：本 issue 会新建 `.github/workflows/`，
  而 CI 是 08-05 主动删的。**这两件事要分清**——本 issue 只加**部署** workflow，
  不复活测试门禁。要不要复活 CI 是独立的未决项。

## 做什么

1. `.github/workflows/pages.yml`：
   - 触发：push 到 `main` 且 `crates/agent-wasm/**` 或 `scripts/build-wasm.sh` 变化，
     加 `workflow_dispatch` 手动触发
   - 装 Rust + `wasm32-unknown-unknown` target + `wasm-pack`（用 `jetli/wasm-pack-action`
     或直接 `cargo install`，前者快很多）
   - 跑 `scripts/build-wasm.sh`
   - `actions/upload-pages-artifact` 传 `crates/agent-wasm/www`
   - `actions/deploy-pages` 部署
   - 权限：`pages: write` + `id-token: write`
2. `.gitignore` 确认 `www/pkg/` 的处置——**产物不进仓库**，由 workflow 现构建。
   如果现在 pkg 被 ignore 了就保持；没被 ignore 要加上。

## 验收

- Actions 里这条 workflow 绿
- 公开 URL 能打开，页面**不是 404、不是空白**
- 浏览器 Network 面板确认 `.wasm` 以 `application/wasm` 送达（MIME 错的话页面会静默不工作）
- 在那个公开 URL 上**用真 key 跑通一轮**（不是本地跑通就算）

## 需要用户

仓库 Settings → Pages → Source 选 **GitHub Actions**。这一步我做不了。

---

## 实做记录（2026-08-13）

`.github/workflows/pages.yml` 已写。**在你打开 Pages 开关之前它跑不了**，
所以下面「验收」那四条一条都还没打勾——不是忘了打。

### 落地的几个决定

**与 `ci.yml` 分成两个文件**（不合并）：一个是门禁一个是部署，触发条件与权限都不一样。
`ci.yml` 的 `wasm` job 保证「编得出来」，`pages.yml` 只负责「把编出来的挂上去」。

**触发限定路径**（`crates/agent-wasm/**` / `scripts/build-wasm.sh` / 本 workflow 自身）
+ `workflow_dispatch`。改文档不该触发一次部署。

**`concurrency: cancel-in-progress: false`**：部署跑到一半被砍会留下半个站点，宁可排队。

**`rust-cache` 要显式列 `crates/agent-wasm`**——它是独立 workspace（自带
`Cargo.lock` / `target`），不列就每次全量编 128k 行。

**`upload-pages-artifact` 必须排在 `build-wasm.sh` 之后**：`www/pkg/` 在仓库里是
gitignore 的（产物不进版本控制），顺序反了传上去就是一个没有 wasm 的空壳页面。
这一条在 workflow 里写了注释，因为它是那种「一眼看不出错、上线才发现白屏」的顺序依赖。

### 已核实

- 两份 workflow YAML 都能被解析（`ci.yml` 三个 job、`pages.yml` 一个）
- **页面全部用相对路径**（`./pkg/agent_wasm.js` 等 4 处 import），
  挂在 `/einfach-agent-rust/` 这种子路径下不会断
- 部署上去的就是 `www/` 下这 12 个文件：`index.html` + 5 个 js + `pkg/` 里 5 个产物
  （外加一个 `pkg/.gitignore`，无害）
- 本地起静态服务器跑通过完整对话 + 撤销（[169](169-wasm-artifact-recheck.md) /
  [196](196-wasm-expose-undo.md) 两轮真机），**部署的是同一份字节**

### 你要做的两步

1. Settings → Pages → Source 选 **GitHub Actions**
2. `git push` —— 首推会同时触发 `ci.yml` 与 `pages.yml`

推完把 URL 给我，我接着做 [173](173-readme-demo-hero.md)（README 挂 demo + 填 homepage）。
