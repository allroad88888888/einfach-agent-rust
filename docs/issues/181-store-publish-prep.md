# 181 `agent-store` 发布前置补全

**里程碑** L · **依赖** [180](180-crates-io-name-check.md) · **模型** sonnet · **估时** 20min · **状态** 待开始

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
