# 182 `einfach-store` 首发 crates.io

**里程碑** L · **依赖** [181](181-store-publish-prep.md) · **谁做** **用户**（我做不了） · **估时** 15min · **状态** 前置全就绪，等你 `cargo login`

## 目标

发出去。

## 为什么必须用户做

crates.io 发布需要账号与 `cargo login` 的 token，且**发布不可逆**——
版本发出去只能 yank 不能删，名字一旦占用不能转让给别人。
这类动作要本人按下去。

## 做什么

1. 注册 crates.io（GitHub 账号登录）+ `cargo login`。
2. `cargo publish -p einfach-store --dry-run` 先跑，确认无误。
3. `cargo publish -p einfach-store`。
4. 发布后核对 crates.io 页面：README 渲染正常、license 显示 `MIT OR Apache-2.0`、
   docs.rs 构建成功（**docs.rs 构建失败是首发最常见的翻车点**，它跟本地
   `cargo doc` 的环境不一样）。

## 验收

- `cargo add einfach-store` 在一个空项目里可用
- docs.rs 上文档构建成功且能打开
- crates.io 页面的 license 显示正确

## 之后

- 主仓 README 加 crates.io badge
- 这条完成后 L1 波收官，可以进 L2 内容波

## 注意

**版本号定 `0.1.0` 还是 `0.0.1`？** 建议 `0.1.0`——`0.0.x` 传达「随时会崩」，
而这个 crate 的核心逻辑是 fork 自已在生产用的上游引擎、且本仓有完整测试覆盖。
但要在 README 里诚实写明 API 尚未稳定。
