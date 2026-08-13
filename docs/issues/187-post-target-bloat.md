# 187 文章：两天把 target 堆到 58GB

**里程碑** L · **依赖** [165](165-launch-positioning-decision.md) · **模型** sonnet · **估时** 20min · **状态** 初稿完成（2026-08-13）

## 目标

短文，讲那次构建事故。**每个 Rust 开发者都中过这个坑的某个版本**，
所以传播成本极低——它不需要读者关心你的项目，只需要他们关心自己的构建速度。

**用 sonnet 而不是 opus**：素材具体、结论明确、篇幅短，是执行活。

## 素材

- CLAUDE.md §Workspace：「267 个测试文件曾两天把 target 堆到 58GB/88 万文件，
  2026-08-05 已合并为每 crate 一个 harness」
- `Cargo.toml` 里 `[profile.dev] debug = "line-tables-only"` 那段注释
  （已经写得很好了，基本可以直接改写）

## 写什么

标题方向：*267 test files, 58GB of target/, and rustc spending minutes enumerating directories*

1. **症状**：构建越来越慢，慢到分钟级，但代码没变多少
2. **诊断**：`tests/` 顶层每个 `.rs` 都是**独立链接的二进制**。267 个文件 = 267 个二进制，
   每个都链接整个依赖树
3. **第二个因素**：全量类型调试信息是体积大头；文件数一过几十万，
   rustc 每次启动枚举 deps 目录本身就是分钟级——**构建自己拖慢自己**
4. **两个修法**：
   - 每 crate 一个 `tests/it/main.rs` harness，新测试加一行 `mod`
   - `[profile.dev] debug = "line-tables-only"` + 第三方依赖 `debug = false`
     （panic 栈里它们的帧有函数名就够）
5. 数字对比：前后的 target 体积、文件数、构建时间

## 验收

- 有**真实的前后数字**——没有数字这篇就不值得发
- 两个修法都给出可复制的配置片段
- 短。这篇的优势就是短，别写长

## 顺带

如果发之前能补测一次当前的 target 体积/构建时间，数字更有说服力。

---

## 实做记录（2026-08-13）

**初稿在 [`docs/posts/187-target-bloat.en.md`](../posts/187-target-bloat.en.md)**（英文，[165](165-launch-positioning-decision.md) L1）。

### 那次「补测一下当前数字」把文章整个换了个形状

原计划是标准的「事故 → 诊断 → 修复 → 前后对比」。我去量当前 target 只是想补一个
干净的「after」数字，结果量出来：

```
target/debug/incremental   16G     ← 最大的一块
target/debug/deps          14G
target/debug/examples     688M
──────────────────────────────
target                     31G / 794,507 文件
```

**31G。79 万文件。我准备当「after」用的数字，几乎就是当年的「before」。**

但位置变了：`deps`——08-05 那次修的对象——已经不是最大的一块了。**修是有效的，
是别的东西长进了腾出来的空间。** `incremental/` 里 708 个会话目录，最大的单个 151M。

原因不是又犯了一次错：incremental 状态按「crate × 编译配置」分，而本仓的组合不少
（`ts` feature、`--all-targets`、native/wasm32/x86_64-linux 三个目标），每个组合各留
一份历史。**没人做错什么，磁盘是在正常工作中被填满的。**

### 于是文章的论点变了

从「我修了个坑，你们别踩」变成——

> 表层原因是测试文件布局，修它是对的（`deps` 确实不再是问题）。但真正的性质是：
> **`target/` 是一个没有淘汰策略、没有预算的缓存**，而针对某一个增长来源的修复
> 不改变这个性质。第二次填满时它从别的地方填。它还会再来一次。
>
> 所以我真正需要的修复从来不是「重组测试」，而是**一个会被定期检查的数字**——
> 让「target 现在多大」变成有东西按时去问的问题，而不是我八天后写博客时才撞见的问题。
>
> 这是「修好一次事故」和「修好产生事故的东西」之间的差别。我只做到了前者，两次。

这个结尾比原计划诚实，也更有用——**它给读者的不是一个 checklist，是一个判据**。

### 顺带开了 [197](197-incremental-cache-bloat.md)

量出来的东西不该只进文章。[197](197-incremental-cache-bloat.md) 记了实测构成、
四条候选解法和它们的代价，并写明「**别只做一次性清理就关掉**」——58GB 那次
就是这么过去的。

顺带发现 `crates/agent-transport/tests/` 顶层还有 1 个 `.rs`（CLAUDE.md 写的是
不要在顶层建），其余五个 crate 都是 0，一并记进 [197](197-incremental-cache-bloat.md)。

### 待办

- [x] ~~文章里「builds got fast again」缺构建时间的真实数字~~ —— **量了**：
      `touch crates/agent-core/src/lib.rs && time cargo build --workspace` = **6 秒**
      （改最底层 crate 的一个文件，全 workspace 重建）。初稿已补进那句话。
      选这个口径而不是 `cargo clean` 后的全量重编，是因为**它才是日常迭代的体感**
      ——当年变慢的痛点就是「改一行等好久」，不是「第一次编译要多久」。
      同时量的 target 现状：**6.6G / deps 7,148 文件**（清理后又跑了几轮构建，
      从 3,227 涨上来，符合 [197](197-incremental-cache-bloat.md) 说的「会不断长回来」）
- [x] ~~中文版（L2 第二波）~~ —— **提前做了**（2026-08-13），在 [`docs/posts/187-target-bloat.zh-CN.md`](../posts/187-target-bloat.zh-CN.md)。
      L2 是我自己排的顺序，不是被什么挡着；「全部执行完」之下没有理由压着。
      **不是逐句直译**：英文版是先写的，中文版按中文的节奏重写，
      但**所有承重数字逐个核对过两版一致**（脚本比对，不靠眼看）。
