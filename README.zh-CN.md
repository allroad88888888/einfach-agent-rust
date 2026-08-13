# einfach-agent

> [English README](README.md) 是主文档；本文只提供中文摘要。

一个可嵌入业务产品的 Agent Runtime。它最核心的能力不是再做一个聊天界面，而是让浏览器、
桌面端和 Java 宿主把自己的 tools 与 skills 动态交给同一个 Rust agent core。

## 核心能力

### 宿主可以动态扩展 Agent

每个会话可以声明自己的 `web:` / `desk:` tools、skills 和内置工具开关。因此同一个 agent core
可以进入财务系统、管理后台、设计工具或桌面应用，而不需要把所有业务集成都写死在 Rust 中。

能力声明经过校验、稳定排序、会话持久化和恢复；部署环境后来发生变化，也不会静默改写历史
会话当时拥有的能力。

### 大规模能力按需加载

大量 tools 可以组织成 skills。会话开始时，AI 只看到 skill 名称和描述组成的精简索引；正文由
AI 按需经一次普通工具调用取回，以 tool result 进入**对话消息**。

全程不往 system 段中途注入任何东西——正文走消息尾部追加，那正是 prompt 缓存本来就为之设计
的路径，因此每次读取都不破坏已缓存前缀。DeepSeek 上十轮实测（含发生正文读取的轮）：缓存命中
97.5%–99.8%，均值 98.5%。

这让能力目录可以持续增长，而 prompt 不会随全部能力线性膨胀。少量始终可用的工具仍可直接
声明为顶层 host tools。

### 状态是唯一事实来源

Agent 的全部状态位于一张原子依赖图中。Undo、redo、崩溃恢复和审计回放使用同一套机制。
被 `/undo` 撤销的轮次在模型恢复出的记忆中真正不存在，可逆性屏障则保护不可逆操作。

## 本地运行

```bash
cp providers.example.toml providers.toml
# 在 providers.toml 中填写 DeepSeek、Kimi 或 GLM 的 API key。

cargo run -p agent-cli
```

独立 HTTP/SSE server：

```bash
cargo run -p agent-server-bin -- --sessions-dir ./sessions
```

每次推送与 PR 都跑与本地同一套门禁：红线检查、`clippy -D warnings`、workspace 测试、
协议一致性测试（重新生成 TS 类型）、前端 typecheck，以及浏览器宿主的 wasm 构建。

## 许可

[Apache License 2.0](LICENSE-APACHE) 或 [MIT](LICENSE-MIT) 双许可，采用方任选其一。
