# 166 加 LICENSE（双许可 MIT OR Apache-2.0）

**里程碑** L · **依赖** [165](165-launch-positioning-decision.md) · **谁做** claude · **状态** 完成（2026-08-13）

## 目标

仓库 public 了但没有 LICENSE——法律上等于「保留所有权利」，**任何公司都不能用、不能 fork**。
这是推广的 0 号阻塞：前面所有工作在它没做之前都是白干。

## 做了什么

1. `LICENSE-APACHE` —— 全文 `curl` 自 apache.org 官方 txt（202 行）。**没有手抄**：
   法律文本逐字节错一个词都可能改变含义，手写是不必要的风险。
2. `LICENSE-MIT` —— 版权行 `Copyright (c) 2026 einfach-agent contributors`。
3. `Cargo.toml` 的 `[workspace.package]` 加 `license = "MIT OR Apache-2.0"`。
4. 两份 README 各加 License 段；英文版按惯例带上 contribution 双许可声明
   （「你提交的贡献同样按上述双许可」）。

**为什么是双许可而不是单 MIT**：这是 Rust 生态的通行形状（serde / rand / tokio 一系），
也是 crates.io 期待的形状。Apache-2.0 带专利授权条款，企业法务过得更顺——而本项目的
定位就是**嵌进企业产品**（[165](165-launch-positioning-decision.md) L2），这条不是可选项。
采用方任选其一，等于给对方最大回旋余地。

## 验收

- [x] 根目录有 `LICENSE-MIT` + `LICENSE-APACHE`
- [x] `cargo metadata --no-deps` 通过（`license` 字段没写坏 manifest）
- [x] 两份 README 都有 License 段
- [x] `check-invariants.sh --all` 退出码 0，且输出不点名本次任何文件

## 留的尾巴（不阻塞，但别忘）

- **各 crate 的 `Cargo.toml` 还没加 `license.workspace = true`。** 今天全部
  `publish = false`，不加无影响；**[181](181-store-publish-prep.md) 发 crates.io 时必须补**，
  否则 `cargo package` 会因缺 license 字段直接报错。
- **GitHub 侧栏对「多份 `LICENSE-*` 文件」的识别**：API 的 `licenseInfo` 返回 `null`，
  但网页侧栏会显示 "Apache-2.0, MIT licenses found"（serde / rand 同款）。
  **这是双许可的已知代价，不是配错了**——别看到 API 返回 null 就去"修"它。
