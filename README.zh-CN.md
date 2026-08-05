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

大量 tools 可以组织成 skills。对话开始时，AI 只看到 skill 名称和描述组成的精简索引；只有
AI 主动激活某个 skill 后，它的完整 instructions 和 tool schemas 才进入请求。

这让能力目录可以持续增长，而 prompt 不会随全部能力线性膨胀。少量始终可用的工具仍可直接
声明为顶层 host tools。

### 多个前端只允许一个执行

多个浏览器或宿主进程看到同一工具调用时，必须先由 Rust actor 原子认领。只有获胜者能产生
副作用，其他客户端得到明确冲突，因此不会重复下单、重复发消息或重复修改数据。

结果回传也有强确认：

- `committed` 表示结果已经由 actor 校验并提交，不只是进入队列；
- 相同结果重试返回 `duplicate`，不会再次推进模型；
- 冲突提交返回 `result_conflict`；
- 已认领后失联进入 `outcome_unknown`，不会被伪装成普通超时并自动重试。

### 状态是唯一事实来源

Agent 的全部状态位于一张原子依赖图中。Undo、redo、崩溃恢复和审计回放使用同一套机制。
被 `/undo` 撤销的轮次在模型恢复出的记忆中真正不存在，可逆性屏障则保护不可逆操作。

## 真实验证

- 100 轮真实 TCP 并发认领，每轮严格一个获胜者；
- 两个真实浏览器经 Java 网关接入同一 `chatid`，只有一个执行副作用；
- 模拟 HTTP 响应丢失后重试得到 `duplicate`；
- 认领端断线后正确进入 `outcome_unknown`，观察端与会话继续运行。

完整英文说明见
[Host-Native Tools and Skills](docs/issues/092-remote-tool-result-protocol.md)。

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

本仓库不配置托管构建流水线；相关测试和红线检查在本地修改对应组件时执行。
