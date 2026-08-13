# 197 target 又胀回来了：这次是 incremental，不是 deps

**里程碑** L（[187](187-post-target-bloat.md) 写文章时量出来的） · **模型** sonnet · **估时** 20min · **状态** 待开始

## 目标

08-05 那次「267 个测试二进制 → 每 crate 一个 harness」修的是 **`deps`**。修好了。
但今天（2026-08-13）实测 target **31G / 79.4 万文件**，最大的一块是别的东西。

## 实测（2026-08-13）

```
target/debug/incremental   16G     ← 最大
target/debug/deps          14G
target/debug/examples     688M
其余                        ~1G
──────────────────────────────
target 合计                 31G / 794,507 文件

crates/agent-wasm/target   2.2G    ← 独立 workspace
probes/api/target          498M    ← 独立 workspace
──────────────────────────────
三个 workspace 合计        ~33.7G
```

`incremental/` 里有 **708 个会话目录**，最大的单个 **151M**（都是 `agent_server-*`）。

## 为什么会这样（待确认，别当结论）

**猜测**（动手前先验）：incremental 目录是按「crate × 编译配置」分的，而本仓的
配置组合不少——`--features ts`、`--all-targets`、三个编译目标
（native / `wasm32-unknown-unknown` / `x86_64-unknown-linux-gnu`）、
test/bench 各自的 profile。组合多 ⇒ 目录多；cargo 的 GC 是按每份保留最近若干个，
组合一多，总量就上去了。

**要先量的**：这 708 个里有多少是「同一 crate 同一配置的历史版本」（该被 GC 掉的），
有多少是「不同配置各自的一份」（合理存在的）。两个数不一样，解法完全不同。

## 几条可能的解法（先量再选，别直接上）

| 解法 | 代价 |
|---|---|
| `.cargo/config.toml` 里关 incremental | **增量编译整个没了**，日常迭代变慢——这是本仓最不该牺牲的东西（58GB 那次的教训恰恰是「构建自己拖慢自己」） |
| 定期清 `incremental/`（脚本 / 收工检查） | 下一次编译慢一轮，之后照常。**倾向这个** |
| 给 `incremental` 设保留上限 | cargo 目前没有这个旋钮（要确认） |
| 只在 CI 关（`CARGO_INCREMENTAL=0`） | CI 上 `Swatinem/rust-cache` 已经设了 0，**这条已经是现状**，对本地无帮助 |

## 验收

- 量出「历史版本 vs 不同配置」的构成比
- 定一条能长期执行的做法，写进 CLAUDE.md §Workspace（那里已经有 58GB 那段，
  这条是它的续集）
- 执行后 target 回到一个可说得出的量级，并**把数字记下来**——
  下一次胀回来时才有对照

## 注意

**别只做一次性清理就关掉这个 issue。** 58GB 那次就是这么过去的：清了、改了测试组织、
写进文档，然后从另一个口子长回来。这一条要产出的是**一个会被反复执行的做法**，
不是一次 `rm -rf`。

## 顺带发现

`crates/agent-transport/tests/` 顶层有 **1 个 `.rs` 文件**——CLAUDE.md §Workspace
写的是「**不要在 `tests/` 顶层建 `.rs`**」，其余五个 crate 都是 0。
不阻塞（一个文件多一个链接产物而已），但既然量到了就记上，顺手归位。
