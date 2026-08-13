# 181 `agent-store` 发布前置补全

**里程碑** L · **依赖** [180](180-crates-io-name-check.md) · **模型** sonnet · **估时** 20min · **状态** 完成（2026-08-13）

## 目标

`agent-store` 自己就是个**独立有用的东西**——一个 Rust 原子依赖图 + command log
（同步可重入、glitch-free 传播、256 深度预算、`AtomFamily`）。它能独立带一波人：
对「Rust 版 jotai / signals」感兴趣的人远多于对「agent 运行时」感兴趣的人，
而他们进来之后会顺着看到主项目。

## 做什么

1. **补 [166](166-license.md) 留的尾巴**：`crates/agent-store/Cargo.toml` 加
   `license.workspace = true`，去掉 `publish = false`。
2. 补 `description`、`repository`、`keywords`、`categories`
   （`categories` 建议 `data-structures` / `caching`；`keywords` 上限 5 个）。
3. **写独立 README**（`crates/agent-store/README.md`）并在 Cargo.toml 里 `readme = `：
   - **一个不依赖任何 agent 概念的最小例子**——这是本 issue 最关键的产出。
     进来的人不关心 agent，例子里出现 agent 就劝退了
   - 说清跟上游 [einfach](https://github.com/allroad88888888/einfach) 的血缘与分叉
     （CLAUDE.md §上游血缘那段可以直接改写：fork 时移除了什么、保留了什么）
4. `cargo package --list` 看进包的文件对不对（别把测试固件、大文件带进去）。

## 验收

- `cargo package -p agent-store` 成功、零 warning
- README 里的最小例子**复制出去能编过**（真跑一次，别目测）
- `cargo package --list` 的内容清点过，没有意外文件
- 主 workspace `cargo test --workspace` 仍绿（去掉 `publish = false` 不该影响任何东西）

## 不做

不执行 `cargo publish`——那是 [182](182-store-publish.md)，且**不可逆**。

---

## 实做记录（2026-08-13）

### 改了什么

1. **包名 `agent-store` → `einfach-store`**（[180](180-crates-io-name-check.md) 定的），
   **目录名不动**。四个依赖方（`agent-core`/`agent-runtime`/`agent-server`/`agent-tools`）
   改成重命名依赖：
   ```toml
   agent_store = { package = "einfach-store", path = "../agent-store" }
   ```
   于是四个 crate 里的 `use agent_store::…` **一个字节没改**。
2. 补齐发布元信息：`license.workspace = true`（[166](166-license.md) 留的尾巴，
   这次还上了）、`description`、`repository`、`readme`、`keywords`（5 个上限用满）、
   `categories`（`data-structures` + `rust-patterns`）。
3. **写了独立 README**（`crates/agent-store/README.md`）。

### README 那个最小例子

本 issue 最关键的产出是「**一个不依赖任何 agent 概念的最小例子**」——进来的人不关心
agent，例子里出现 agent 就劝退了。写的是一个 counter + 一个 doubled 派生：
定义自己的 `Val` 枚举、`create_atom` / `create_derived` / `get` / `set`，二十行。

顺带把「为什么 read fn 必须纯」用一句能吓到人的话写进去了——不是风格偏好：
undo 会重放日志并重算 derived，read fn 里放个时钟，重放时算出来就是另一个值，
**而且什么都不报错**，几周后以「undo 之后值悄悄错了」的形式浮出来。
这正是红线 1 的理由，对外讲反而比对内更有说服力。

**例子是真验过的，不是目测**：抽出代码块 → 单独建一个只依赖
`einfach-store` 的项目 → `cargo build` 通过 → `cargo run` 退出码 0（断言真跑过）。

### 打包核对

`cargo package -p einfach-store`（**带验证构建**，不是 `--no-verify`）通过：
**66 个文件，338.5 KiB（压缩后 99.6 KiB）**。清点过内容：`src/` 31 个 + `tests/` 34 个
+ README + 三份 manifest，**没有意外文件、没有大文件、没有测试固件**。

### 踩了个坑，值得记

改完包名跑门禁，我先报了「`cargo test --workspace` 失败 suite: 0」——**那是假绿**。

真相是 `crates/agent-store` **自己的** 33 个集成测试全都 `use agent_store::`，
改名后整个 test target **编译失败**。而我当时的检查方式是
`grep -c "test result: FAILED"`——编译失败根本不产生那一行，所以数出来是 0。

> **判据**：验门禁**看退出码，不要 grep 输出里的成功/失败字样**。
> 编译失败、link 失败、harness 崩溃都不会印出那句 `test result:`，
> 一 grep 就全成了「通过」。这次是当场发现的（clippy 那道立刻红了），
> 但如果只跑 test 一道，这个假绿会一路带到 CI。

修法：store 自己的测试与 doctest 里 `agent_store::` → `einfach_store::`（33 个文件），
外加三处散文里的 crate 名。依赖方那边因为有重命名别名，**一处都不用改**。

### 验收（全部看退出码）

| 门 | 退出码 |
|---|---|
| `check-invariants.sh --all` | 0 |
| `clippy --workspace --all-targets -- -D warnings` | 0 |
| `cargo test --workspace` | 0 |
| `cargo test -p agent-server --features ts` | 0 |
| `pnpm -r typecheck` | 0 |
| `scripts/build-wasm.sh` | 0 |
| `cargo package -p einfach-store`（带验证构建） | 0 |

`scripts/check-invariants.sh` 不受影响——它按**路径**匹配红线 7 的白名单
（`crates/agent-store/*`），不认包名。这是「只改包名不改目录」的又一条理由。
