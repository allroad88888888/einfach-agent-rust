# 197 target 又胀回来了：incremental 只是最显眼的，不是最大的

**里程碑** L（[187](187-post-target-bloat.md) 写文章时量出来的） · **模型** sonnet · **估时** 20min · **状态** 完成（2026-08-13，脚本 + 首次执行）

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

## 顺带发现（**后来查明是我看错了**，见实做记录末尾）

`crates/agent-transport/tests/` 顶层有 **1 个 `.rs` 文件**——CLAUDE.md §Workspace
写的是「**不要在 `tests/` 顶层建 `.rs`**」，其余五个 crate 都是 0。

---

## 实做记录（2026-08-13）

### 产出的是脚本，不是一次清理

`scripts/clean-build-cache.sh`，清三样**可再生的中间产物**：
`incremental/` + `deps/**/*.rcgu.o` + 非原生非 wasm 的目标目录。这是本 issue「别只做一次性 `rm -rf`」那条要求的兑现——
58GB 那次（08-05）就是按一次性处理的：清了、改了测试组织、写进文档，然后八天后
从另一个口子长回 31G。

**不动 `.rlib`/`.rmeta`**（不是 `cargo clean`），所以下次构建不会从零开始。
三个开关：`--dry`（只报不删）/ `--all`（连产物一起，等价 `cargo clean`）/
`--files`（顺带数文件数）。

报告的除法修过一次：整数除法把「释放了 900M」印成「释放 0G」——
**一个说谎的报告比没有报告糟**，现在小于 1G 时按 M 报。

**默认不数文件数**是有理由的：`find | wc -l` 在八十万文件上要跑好几分钟——
我第一版脚本就卡在这里超时了。**这个讽刺本身就是问题的一部分**：文件多到连
「有多少文件」都变成一次慢操作，而那正是当初构建变慢的机制（rustc 启动要枚举
deps 目录）。

### 只清 incremental 之后，用户问了一句「target 怎么还有 15 个 G」

这一问把诊断推深了一层。**incremental 不是最大的那块，只是最显眼的那块。**

清完 incremental 还剩 15G，其中 `deps/` 占 14G。拆开：

| | |
|---|---|
| `.rlib`（真正的产物） | 1.7G |
| `.rmeta` | 906M |
| **`.rcgu.o`（中间产物）** | **≈ 11.4G，631,526 个文件** |

`.rcgu.o` 是**每个 codegen unit 一个目标文件**（dev profile 默认
`codegen-units=256`），**按构建 hash 分开存，而 cargo 从不回收旧 hash 的那些**。

实测 `agent_cli` 一个 crate：**40 个不同构建 hash、42,732 个 `.o`**。
只有当前那一份有用，其余 39 份是历史垃圾。

`.o` 是链接的中间产物，`.rlib` 已经链好了——删掉最坏就是下次重新生成。

### 顺带：一个非原生目标的孤儿

`target/x86_64-unknown-linux-gnu` 274M，**仓库里没有任何东西引用它**
（`grep` 过 `.sh`/`.toml`/`.yml`），是某次一次性交叉编译留下的。删。

**`wasm32-unknown-unknown` 不在此列，必须保留**：它是 `build-wasm.sh` 的产物，
浏览器宿主（M13/M14 的第三种形态、[170](170-pages-workflow.md) 的 Pages demo）
靠它。它不是「另一个平台的顺带产物」，是产品的一个面。
脚本里对这条做了显式豁免，别让后来人误删。

### 最终数字

```
             清理前    清理后
target        31G  →   6.2G
agent-wasm   2.2G  →   1.7G
probes/api   498M  →   346M
desktop      1.1G  →   944M
────────────────────────────
合计          35G  →     9G
```

**`deps/` 的文件数：631,526 → 3,227（约 200 倍）。**
这个数字比体积更值钱——rustc 启动要枚举 `deps/`，几十万条目就是分钟级，
**构建自己拖慢自己**，那正是 58GB 那次的真正机制。

**代价量过了**：清理后六道门全跑一遍（红线 / clippy / test / ts feature /
typecheck / build-wasm）**全绿**——删的都是可再生的中间产物，没有伤到任何东西。
清完第一次 `cargo check --workspace` 31 秒。

> 注意 `incremental` 在我这一轮工作里从 16G 涨到 20G——**一个开发会话就 +4G**。
> 这个增速说明「定期」不能太稀疏。

### 触发时机：搭已经在跑的那趟车

`scripts/check-build-cache.sh`（51 行）。三个决定：

**① 独立脚本，不塞进 `check-invariants.sh`。** 磁盘胀不是架构错误——那个文件管的是
「能被 grep 判定的架构红线」，混进来是概念放错位置。（我第一版就是直接加进去的，
被行数 hook 顶破 300 才回头想清楚这件事：**行数规则替我抓到了一个职责错误**。）

**② 不进 PostToolUse hook，挂在 `--all` 上。** `du` 在 target 胀起来的时候恰好最慢，
装进每次 Edit 就成了「最需要它的时候最碍事」。`--all` 是收工 / CI 路径，本来就不频繁。

**③ 退出码恒 0——这是提示不是门禁。** 让 CI 因为开发机磁盘胀而变红是错的信号；
而且 CI 的 target 每次都是新的，它永远不会触发这条。

阈值 20G：清理后 9G，日常涨几 G 正常（实测一个会话 +4G），到 20G 说明该清了。
**定得太低会天天响，响多了就没人看——那跟没有一样。**

验过它真的会响：把阈值临时调到 5G，`--quiet` 模式如期打出提示、退出码 0，
串在 `--all` 里也正常。**「低于阈值时静默」意味着平时看不见它工作，
所以必须专门验一次它响的样子**，否则不知道装了个哑炮。

### 还没做的

- [x] ~~「历史版本 vs 不同配置」的构成比没量~~ —— 用户追问「怎么还有 15G」之后量了，
      答案是**历史版本压倒性**：`agent_cli` 40 个构建 hash 里只有 1 个当前有效
- [x] ~~触发时机没定~~ —— `scripts/check-build-cache.sh`，阈值 20G，
      挂在 `check-invariants.sh --all`（收工 / CI）上，见下
- [x] ~~`crates/agent-transport/tests/` 顶层那 1 个 `.rs` 没归位~~
      —— **是我看错了，它是合法例外**，见下

### 更正：那个顶层 `.rs` 不是漏网的

`crates/agent-transport/tests/wasm_smoke.rs` **不能**并进 `tests/it/main.rs`：

- 它整个文件被 `#![cfg(target_arch = "wasm32")]` 门着，native 上编译成空 crate
- 它靠 `wasm-pack test --node` 跑（见 `tests/wasm_node/run.sh`），连的是 Node 的
  真实 `fetch`/`AbortController`，对着真实 HTTP 服务器发请求——**不走 `cargo test`**
- `tests/it/main.rs` 是 native harness，把一个 wasm-only 的 mod 挂进去只会让两条
  平台路径混在一个入口里

**我上一版只数了文件个数就下结论「顺手归位」，没读文件内容。** 记在这里当个提醒：
「顶层不许有 `.rs`」这条规则的理由是「每个顶层文件是一个独立链接的二进制」，
而这个文件在 native 上**根本不产生二进制**——它不占那个成本，所以不在规则要防的
那类里。**判据是理由，不是字面。**
