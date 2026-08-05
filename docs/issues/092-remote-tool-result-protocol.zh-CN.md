# 宿主原生 Tools 与 Skills

> 本文是辅助中文摘要。规范主文档为英文版：
> [092-remote-tool-result-protocol.md](./092-remote-tool-result-protocol.md)。

## 核心价值

前端不只是聊天 UI。它可以在创建会话时，把自己实现的 tools 和 skills 交给 Rust agent；
Rust 则负责按需加载、唯一执行、强确认和完整状态追踪。

```text
前端自定义 tools / skills
          ↓
只把精简 skill 索引交给 AI
          ↓
AI 主动选择并加载完整详情
          ↓
宿主原子认领并执行
          ↓
Rust 强确认最终结果
```

### 1. 能力由宿主动态扩展

浏览器、桌面端或 Java 宿主都能声明当前会话专属的工具与技能，不需要把每个业务系统的能力
硬编码进 Rust。声明经过校验、稳定排序、会话持久化和恢复，历史会话不会因部署环境变化而漂移。

### 2. Skill 能力包真正延迟加载

大规模能力可以组织成 skills。会话开始时只给 AI skill 名称、简述和激活入口，不把 skill
正文及其携带的全部 tool schema 塞进每次请求。AI 决定需要哪个 skill 后，才加载对应的完整
instructions 和 tool definitions。

少量始终可用的顶层 host tools 仍是立即可见路径；大目录则走 skill 能力包的延迟路径。因此
能力数量可以持续增长，而 prompt 不会线性膨胀；无关业务知识不会干扰模型，稳定索引也更利于
provider prompt cache。

### 3. 多个前端只有一个能执行

多个浏览器标签页或宿主进程看到同一个调用时，必须先在 Rust actor 内原子认领。只有收到
`claimed` 的执行端可以产生副作用，其他执行端得到 `tool_claimed_by_other`，只能观察。

这不是前端约定，而是服务端执行前的硬闸，可防止重复下单、重复发消息或重复修改数据。

### 4. HTTP 200 代表真的提交成功

结果回传不是“扔进队列就算成功”。Rust 只有在校验认领凭据、通过会话 epoch 闸并把终态提交到
当前工具槽后，才返回 `committed`。

相同结果重试返回 `duplicate`，不会再次推进模型；同一提交被篡改则返回 `result_conflict`。
即使 HTTP 响应丢失，前端也只需重投结果，不需要重新执行外部工具。

### 5. 对无法确认的副作用绝不撒谎

- 从未有人认领：`unclaimed_timeout`
- 已认领但执行端失联：`outcome_unknown`
- 宿主明确失败：`failed`
- 会话取消：`cancelled`
- 结果成功提交：`succeeded`

`outcome_unknown` 明确表示副作用可能已经发生，系统禁止静默自动重试。若业务要求跨崩溃的
exactly-once，外部业务系统应使用 `tool_call_id` 作为幂等键。

## 真实验证

- 100 轮真实 TCP 双客户端竞争，每轮严格一个成功、一个 409；
- 两个真实浏览器经 Java 网关连接同一 `chatid`，副作用计数分别为 1 和 0；
- 模拟响应丢失后的重复提交得到 `duplicate`，revision 不再增加；
- 获胜端认领后断线，状态正确进入 `outcome_unknown`，观察端仍保持连接且会话继续。
