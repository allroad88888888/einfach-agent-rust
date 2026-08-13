# 170 GitHub Pages 部署 workflow

**里程碑** L · **依赖** [169](169-wasm-artifact-recheck.md) · **模型** sonnet · **独测** — · **状态** 待开始 · **估时** 20min

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
