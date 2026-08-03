# probes —— 打真实 API 的探针

这里放的**不是测试**，是探针：用真实请求确认官方文档没写清楚（或写错）的行为。

**[PROVIDERS.md](PROVIDERS.md) 是唯一的结论文档。** 三家 provider 的全部实测差异都在
那里，`results/*.json` 是原始观测。**主线设计一个字都不该引用它** —— 模型差异全部由
adapter 层吸收（红线 12、[docs/ADAPTER.md](../docs/ADAPTER.md)），架构不该知道它们。

> 主线代码现在是空的（`crates/` 已删，见 [ROADMAP §二](../docs/ROADMAP.md)）。
> **本目录是仓库里仅存的代码**，因为它不是主线：它的产出是那份文档，不是可复用的实现。

## 为什么不放主 workspace 也不走 `cargo test`

- 要花钱、要网络、有速率限制，不能进 CI 的默认路径
- 结论是一次性的：验完写进文档，除非 provider 改协议才重跑
- 依赖 `ureq` 这类 IO 库，不该混进主 workspace 的依赖图

`probes/api/Cargo.toml` 自带 `[workspace]`，是独立 crate，根目录
`cargo build` / `cargo test` 不会碰它（[021](../docs/issues/021-skeleton.md)
建回根 workspace 之后也一样）。

## 跑

```bash
cd probes/api
cargo run --bin cache_prefix     # 前缀缓存：命中语义、块粒度、共享、压缩代价
cargo run --bin wire_shape       # wire 行为：流式分帧、tool_choice、错误形状、响应头
cargo run --bin system_inject    # 消息级 system 注入：收/听/前缀保持/对照差值（038）
cargo run --bin cache_prefix -- --help
```

需要仓库根的 `providers.toml`（已 gitignore）。每次运行用时间戳 nonce 保证从冷缓存
开始。全跑一轮三家共约 ¥2 以内。**探针任何时候都不打印 key。**

## 探针只记录，不下结论

每次调用的原始 `usage` 都会落盘。判读由人做，写进 PROVIDERS.md —— 因为同一组数字
常常有多种解释（比如「命中归零」到底是缓存被作废还是这个变体第一次见），
那需要设计对照实验，不是探针该干的事。
