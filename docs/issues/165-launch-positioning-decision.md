# 165 对外推广的定位与主战场（决策）

**里程碑** L · **依赖** — · **谁做** 用户拍板 · **状态** 完成（2026-08-13）

## 目标

在写任何一行对外文案之前，把三件会反复回头改的事定死：**给谁看**、**归哪个品类**、
**CI 要不要回来**。这三条错了，后面每一篇文章、每一版 README 都要重写。

## 现状（2026-08-13 的体检）

一个陌生人点进来看到的东西：

| 项 | 当时状态 |
|---|---|
| 可见度 | public，0 star / 0 fork，建于 07-31 |
| LICENSE | **没有** → 法律上「保留所有权利」，任何公司都不能用、不能 fork |
| description / topics / homepage | 三个全空 → GitHub 搜索与推荐里等于隐形 |
| README 头条 | 那张 `AI activates one bundle` 流程图**描述的子系统已不存在**（决策 27 删的） |
| provider | 只有 DeepSeek / Kimi / GLM，但 README 是英文 |
| crates.io | 全部 `publish = false` |
| CI badge | `.github/` 空 |
| 可点的 demo | 没有 |

代码本身没问题：`cargo check --workspace` 干净通过，128k 行 Rust，167 个 issue 文档。
**问题全在门面。**

## 拍板

| # | 决策 | 理由 |
|---|---|---|
| **L1** | **主战场是英文社区** | 推论有三条，都不便宜：①README / demo / 文章英文优先；②[174](174-openai-compat-probe.md) 从「锦上添花」升级成**拉新前置**——海外读者手里没有 DeepSeek/Kimi/GLM 的 key，跑不起来就走；③关键文档要英译（[188](188-translate-invariants.md)） |
| **L2** | **定位不进「Rust agent 框架」品类** | 进去就跟 rig / swiftide / langchain-rust 挤，且比不过 star 惯性。**那些是「拼 LLM 应用的库」，这个是「能嵌进产品的 agent 运行时 + 一本真账本」**——不同品类。近的类比反而是 LangGraph 的 time-travel 和 Temporal 的 durable execution |
| ~~**L3**~~ | ~~**不擅自恢复 CI**~~ **已被用户推翻（2026-08-13），落地 [195](195-ci-revival.md)** | 原文：`9ae84d5`（08-05）是主动删除、连同五份文档一起改的，不是遗漏，加回来须用户拍板。**用户当天拍了「回来」**。事后看这个决定被数据证实了：[195](195-ci-revival.md) 发现 CI 缺席的 8 天里 clippy 漂了 **24 处**——理由不是 badge 好看，是没门禁的地方会漂 |

## 一句话定位（草案，[179](179-readme-rewrite.md) 定稿）

> The agent runtime with a real ledger: undo, redo, crash recovery and audit replay are
> the same mechanism, not four features. Runs on a server, in a desktop app, or entirely
> in your browser.

**三个可验证的钩子**，按传播力排序——每一条都要能当场演示，不能只是形容词：

1. `/undo` 之后那一轮**在模型记忆里真的不存在**（别人家的 chat UI 都是假 undo，只删 UI）
2. `kill -9` 之后接着聊（M18 真机验过）
3. 同一个核心跑在浏览器里，**没有任何服务端进程**（M13/M14）

## 验收

- 三条决策都有理由、都可被后续 issue 引用 ✅
- 后续每个 L 波 issue 的取舍能追溯到 L1/L2/L3 其中一条 ✅

## 未决（等用户拍板）

1. ~~**CI 要不要加回来**~~ **已定：回来**（2026-08-13，用户拍板）→ [195](195-ci-revival.md)。
2. ~~**LICENSE 署名**~~ **已定：保持 `einfach-agent contributors`**（2026-08-13，用户授权我定）。
   理由：不用猜用户的法律名/公司归属；是 Rust 生态通行写法；将来要换成真名是改两个文件的事，
   而现在写错一个归属主体，在别人 fork 之后就纠正不回来了。**取更容易改的那一边。**
3. ~~**crates.io 的名字**~~ **已定：先查 `agent-store`，被占则用 `einfach-`**（2026-08-13，用户授权我定）。
   判据写进 [180](180-crates-io-name-check.md)：`einfach-` 有血缘依据（上游 `einfach-core`），
   不是硬编的商标词，且天然给后面可能发的其他 crate 留了命名空间。**真查真定在 [180](180-crates-io-name-check.md)**——
   这里只是把取名判据先定死，免得到时候临时拍。
