# 180 crates.io 名字查重与取名

**里程碑** L · **依赖** [165](165-launch-positioning-decision.md) · **模型** sonnet · **估时** 15min · **状态** 完成（2026-08-13）

## 目标

在做任何发布准备之前，先确认**名字拿得到**。`agent-store` 这种通用词在 crates.io 上
大概率已被占——先查清楚，别等 [181](181-store-publish-prep.md) 做完了才发现要改名，
那时候改要连带动 README、文档链接和所有 `use` 路径。

## 做什么

1. 查这几个名字的占用情况：`agent-store`、`einfach-store`、`einfach-atom`、
   `agent-atoms`（`cargo search` 或直接看 crates.io）。
2. 如果 `agent-store` 被占：**倾向 `einfach-` 前缀**——它有血缘依据
   （上游是 `einfach-core`，见 [../ARCHITECTURE.md](../ARCHITECTURE.md) 与 CLAUDE.md §上游血缘），
   不是硬编的商标词，而且天然给后面可能发的其他 crate 留了命名空间。
3. **顺带查主 crate 名**：将来要不要发 `einfach-agent` 本体？现在只是查，不占。

## 验收

- 每个候选名有明确的「占用 / 可用」结论
- 定下 [181](181-store-publish-prep.md) 要用的名字**和理由**
- 如果要改名，列出改名波及的文件清单（`Cargo.toml` 的 members / 各处 `use` /
  文档里的 crate 名引用），交给 [181](181-store-publish-prep.md)

## 注意

**别在这一步真的注册占坑**。crates.io 不欢迎 name squatting，
而且发布是不可逆的（版本不能删只能 yank）。查清楚、定下来，
真发布在 [182](182-store-publish.md)，由用户执行。

---

## 实做记录（2026-08-13）

### 查的结果

| 名字 | 状态 |
|---|---|
| `agent-store` | ❌ **已被占用** — v0.1.2，95 下载，"Causal-ordered, backend-pluggable store substrate for the agent fleet" |
| `einfach-store` | ✅ 可用 |
| `einfach-agent` | ✅ 可用 |
| `einfach-core` | ✅ 可用 |
| `agent-atoms` | ✅ 可用 |
| `einfach-atom` | ✅ 可用 |

> 查法留一句：crates.io 的 API **不带 `User-Agent` 会 403**，不是 404 也不是 200。
> 第一遍六个名字全回 403，差点被读成「全都查不了」。

### 定：`einfach-store`

判据在 [165](165-launch-positioning-decision.md) §未决 #3 已经先定死了（免得到这一步临时拍）：
`einfach-` 有血缘依据（上游是 `einfach-core`，见 CLAUDE.md §上游血缘），不是硬编的
商标词，且天然给后面可能发的其他 crate 留了命名空间。

`agent-atoms` 也可用，但它是个描述性的通用词——同一类词已经被人占掉一个
（`agent-store`）就是信号：这类名字迟早还会撞。`einfach-` 是有主的前缀。

### 改名波及面（交给 [181](181-store-publish-prep.md)）

**只改 `[package] name`，不改目录名、不改 workspace 里的路径引用。**
Cargo 允许包名与目录名不同，改目录会把 git 历史打散、把所有文档里的
`crates/agent-store/...` 路径引用弄断，收益为零。

具体：

1. `crates/agent-store/Cargo.toml`：`name = "einfach-store"`
2. 依赖方改用重命名依赖，**保持 `use agent_store::…` 一个字节不动**：
   ```toml
   agent_store = { package = "einfach-store", path = "../agent-store" }
   ```
   **依赖方有四个**，不是一个：`agent-core` / `agent-runtime` / `agent-server` /
   `agent-tools`（`grep -n "agent-store" crates/*/Cargo.toml` 的结果；
   红线 7 说的 `agent-core → agent-store` 是**架构主线**，不是「唯一一条边」——
   别照着红线摘要想当然，`agent-tools` 的 `barrier_demo.rs` 就直接 `use agent_store::`）。
   四处都要改，漏一处 `cargo build` 会立刻红，不至于静默。
3. 文档里出现 crate 名的地方按需改；出现**路径** `crates/agent-store/` 的地方不动。

### 顺带一个发现，别踩

**`einfach-core` 可用，但不要发它。** 那是上游
[einfach](https://github.com/allroad88888888/einfach) 自己的 crate 名（CLAUDE.md
§上游血缘：本仓的 `agent-store` fork 自它的 `excel/rust/core`，crate 名就叫
`einfach-core`）。上游只是还没发布 —— 用本仓去占上游的名字是在给自己的上游挖坑，
哪怕是同一个作者的两个项目也不该这么干（将来上游要发就发不了了）。

`einfach-agent` 可用这条**记下不动**：将来主体要发时再说，现在占是 squatting。
