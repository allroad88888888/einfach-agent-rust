# 195 CI 复活（推翻 L3）

**里程碑** L · **依赖** [165](165-launch-positioning-decision.md) · **模型** sonnet · **估时** 20min · **状态** ✅ 完成（2026-08-13，线上三 job 全绿）

## 目标

把 08-05 删掉的门禁 workflow 加回来，并**补上删除之后才出现的第三道门**。

## 决策：L3 被推翻（2026-08-13，用户拍板）

[165](165-launch-positioning-decision.md) 的 L3 是「不擅自恢复 CI」——因为 `9ae84d5`（08-05）
是**主动删除**、连同五份文档一起改的。现在用户明确要回来，L3 作废。

**理由记在这里，别再翻案**：删的时候仓库是自用的，本地四道门禁 + PostToolUse hook
已经够；现在要对外，README 顶部那个绿勾是**陌生人判断可信度的第一个信号**——
128k 行 Rust 一个绿勾都没有，在 [165](165-launch-positioning-decision.md) L1 定的英文社区里是硬伤。
**变的不是工程判断，是受众。**

## 现状

`git show a75caea:.github/workflows/ci.yml` 是被删的原文，两个 job：

- `rust`：红线检查 → clippy `-D warnings` → `cargo test --workspace` → `cargo test -p agent-server --features ts`
- `ts`：`pnpm -r typecheck`

**它有一个洞**：写于 08-05，而 **wasm 目标是 08-10 才恢复的**（决策 26）。
所以原文里没有 `build-wasm.sh`——而今天本仓的收工标准是**三门禁**
（`cargo test --workspace` / `check-invariants.sh --all` / `build-wasm.sh`，
[153](153-timed-run-session.md)、[178](178-openai-compat-dogfood.md) 都是这个口径）。

直接 `git revert` 会把这个洞一起带回来。

## 做什么

1. 恢复 `.github/workflows/ci.yml`，两个 job 照原文（它写得是对的，注释也在）。
2. **加第三个 job `wasm`**：装 `wasm32-unknown-unknown` + `wasm-pack`，跑 `scripts/build-wasm.sh`。
   - 这道门同时**保护 [170](170-pages-workflow.md) 的 demo**——demo 编不出来 = 首发的地基塌了，
     必须在合并前就红，不能等部署时才发现
3. 把 08-05 那次删除时**同步改掉的五份文档**改回来（`README.md` 那句
   "This repository intentionally has no hosted build pipeline."、`docs/ARCHITECTURE.md`、
   `docs/DOC-AUDIT.md`、`docs/INVARIANTS.md`、`docs/ROADMAP.md`、`docs/TOOLS.md`、
   `docs/WORKFLOW.md`——以 `git show 9ae84d5` 的 diff 为准，逐处核对**当时改成了什么**，
   不是简单反向）
4. README 加 CI badge。

## 验收

- **推之前本地先跑一遍全部门禁**，确认不是推上去才发现红：
  - `bash scripts/check-invariants.sh --all`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - `cargo test -p agent-server --features ts`
  - `pnpm -r typecheck`
  - `scripts/build-wasm.sh`
- 推上去后 Actions 三个 job 全绿
- README badge 显示 passing
- `grep -rn "no hosted build pipeline" .` 零命中（文档回滚干净）

## 注意

- **CI 缓存**：`Swatinem/rust-cache@v2` 原文就有，留着。wasm job 也要加，
  否则每次都全量编 128k 行。
- 与 [170](170-pages-workflow.md) 是**两个 workflow 文件**（`ci.yml` / `pages.yml`），
  别合并——一个是门禁一个是部署，触发条件和权限都不一样。

---

## 实做记录（2026-08-13）

**本地六道门全绿。** 但过程里发现了这件事真正的价值所在：

### clippy 在这 8 天里漂了 24 处

CI 08-05 删掉之后没人再跑 `clippy -D warnings`，到今天累计 **24 个 error**
（`-D warnings` 口径），横跨 `agent-runtime` / `agent-cli` / `agent-server` 三个 crate
的生产代码和测试。**这就是「CI 该不该回来」的实证答案**——不是badge好看，
是没有门禁的地方八天就能漂这么多。

分三类处理：

**一、等价改写（16 处）**，语义一个字节没变：

| 类型 | 处数 | 改法 |
|---|---|---|
| `clone_on_copy` | 2 | 去掉 `.clone()` |
| `iter_cloned_collect` | 1 | → `.to_vec()` |
| `question_mark` | 1 | `let...else { return None }` → `?` |
| `double_ended_iterator_last` | 5 | `.last()` → `.next_back()` |
| `manual_split_once` | 1 | `splitn(2,..).nth(1)` → `split_once` |
| `collapsible_if` | 3 | 折成 let-chain（edition 2024 支持） |
| `doc_list_item_without_indentation` | 3 | 调整换行位置 / `1)` → `①` |

**二、类型别名（3 处）** —— `type_complexity`。给已有类型起名字，不拆结构：
`InterceptEntries` / `TimedEntries`（`extension_pack.rs`）、`EventSink` / `EventObserver`
（测试 `support/mod.rs`）、测试局部的 `Probe`。

**三、带理由的 `allow`（5 处）** —— 这几条 clippy 说的没错，但**按它说的改是设计变更**，
不该混进「让 CI 变绿」：

- `too_many_arguments` ×2（`intercept_registry::dispatch` 10 个、`actor::body::run` 8 个）：
  参数多是「四个可变借用来自四个不同所有者」和「actor 入口拿全部依赖」的直接后果，
  合并成 struct 要么要求同源要么背一堆生命周期，更难读。
- `result_large_err`（`provider_call::start`）：`StartFailure` 大是因为内嵌 `Event`；
  起飞失败要原样变成一个进 loop 的事件，Box 一层换不到任何东西，且不在热路。
- `duplicate_mod` ×2（测试夹具 `#[path]` 复用）：去重会让两边的 `RoutedServer` 等类型
  合并成同一个，是夹具结构的改动。allow 落在 `tests/it/main.rs` 的 crate 级
  （落在 `#[path]` 那一项上对 `spawn_bg_support` 那条不生效）。

每条 allow 都写了理由注释——**allow 不写理由，下一个人只会当成噪音删掉或跟着抄**。

### 中途改坏两处，已修

折 let-chain 时在 `actor/body.rs` 少删一层花括号（`unexpected closing delimiter`）；
`split_once` 那处闭包里 `line` 是 `&&str`，`map_or(line, ..)` 类型对不上，改成 `*line`。
两处都是本地跑门禁当场发现的——**这正是「推之前先在本地跑一遍」这条验收的意义**。

### 六道门的结果

| 门 | 结果 |
|---|---|
| `check-invariants.sh --all` | ✅ |
| `clippy --workspace --all-targets -- -D warnings` | ✅ 退出码 0 |
| `cargo test --workspace` | ✅ 0 个 suite 失败（最大一个 410 passed） |
| `cargo test -p agent-server --features ts` | ✅ 134 passed |
| `pnpm -r typecheck` | ✅ protocol + web 都过 |
| `scripts/build-wasm.sh` | ✅ |

### 文档回滚

`README.md` 那句 "This repository intentionally has no hosted build pipeline."
与中文版对应句已改成描述今天这六道门；README 顶部加了 CI badge 与 License badge。
`grep -rn "no hosted build pipeline"` 只剩本 issue 自身的引用。

### 顺带记一笔（不在本 issue 范围）

`crates/agent-server/src/actor/body.rs` **332 行，超红线 9 的 300**。本次只是路过小改
（折一个 if + 加一条 allow 注释），按全局规矩指出但不擅自重构。要拆的话是另一个 issue。

## 推送后确认（2026-08-13，三条全部验完）

- [x] **Actions 三个 job 全绿**（run `31680061584`，`fbc2240`）：
      `rust (test + clippy + 红线)` / `typescript (协议 + web typecheck)` /
      `wasm (浏览器宿主构建)`，三个 `conclusion: success`
- [x] **README badge 返回 HTTP 200**（`actions/workflows/ci.yml/badge.svg`）
- [x] **`jetli/wasm-pack-action@v0.4.0` 在 CI 上真能装**——装出来的是 **wasm-pack
      v0.15.0**，比本地的 0.14.0 还新。这条是三条里唯一有真实风险的：
      本地跑绿只证明「已装好的 0.14.0 能编」，证明不了「CI 上装得上」，
      所以当时特意把它单列成一条待确认，而不是跟着另外两条一起打勾。

**顺带一条判据（这次踩出来的）**：中途我用 `grep -c "test result: FAILED"` 判过一次
「测试全绿」，报了「0 失败」——**但那个 grep 看不见编译失败**（`agent-store` 改名后
33 个测试根本没编出来，一条 `test result:` 都不会打印）。后来全部门禁改成看**退出码**。
`grep` 判绿的问题不是不准，是**它在最该报警的那种失败上恰好静音**。
