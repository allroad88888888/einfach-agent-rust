# 033 web 最小客户端 —— M3 终点

**里程碑** M3 · **依赖** 031 + 032 · **模型** sonnet · **独立测试 agent** 否（终局验收由主会话真浏览器跑）· **状态** 完成

## 目标

真浏览器连上 agent-server：看流、看子 agent 并行、undo。M3 的「能用了」。

## 做什么

`packages/web`（vite + vanilla TS，**不引框架**——M3 要的是验收面不是 UI 资产；
组件化是 `packages/ui` 的事，未排期）：

- 类型**只从 `@einfach-agent/protocol` 导入**（032 的生成物）——手写一个协议
  接口即违背决策 2，review 一票否
- 连接：`POST /sessions` 建会话 → `EventSource('/sessions/:id/events')`；
  vite dev proxy 把 `/sessions` 代到 `AGENT_SERVER`（默认 127.0.0.1:该端口）
  ——**不动 server 加 CORS**（企业本来就走网关同源，dev 用代理模拟同一形状）
- 渲染：按 `SessionEvent` 帧渲染——增量文本流式追加（thinking 弱化显示）、
  工具调用卡片（名字/参数/结果长度/reversibility）、**agent 归属分栏或前缀**
  （root 主栏，子 agent 缩进/标色——「并行干活」要肉眼可见：两个子的增量
  交错出现）、GuardReport 一行小字、`gap` 帧显示「掉了 N 帧」
- 控件：输入框 + 发送、Cancel、Undo/Redo、`undo_blocked` 时弹出确认（显示
  工具名与 call_id，确认即发 `force: true`——027 的 `/undo!` 语义搬到点击）
- 断线重连：`EventSource` 自动重连自带 `Last-Event-ID`（031 已支持精确补发）
  ——断网恢复后界面不重复渲染已见帧（按帧 id 去重）
- `pnpm -r typecheck` 覆盖本包；`pnpm --filter web dev` 起本地

## 验收（构建面；行为面主会话真浏览器跑）

- `pnpm -r typecheck` 全绿；`pnpm --filter web build` 产出静态物
- 源码 grep 不到任何手写的 `type SessionEvent`/`interface Command`（决策 2 实检）
- README（包内）写清启动三步：起 server（临时 bin 或 cargo run 示例）、
  `pnpm --filter web dev`、浏览器开 vite 地址

## 主会话终局验收（真浏览器，Playwright 驱动）

- 十轮真实对话流式可见；一轮触发 spawn，两个子 agent 的输出**交错**出现
- 断开（关标签页）→ 5s 后 server 侧在飞取消（日志/下一连接验证）
- Undo 撞 shell 屏障 → 确认弹层 → force 越过；undo 后界面与会话状态一致
- 刷新页面 → Last-Event-ID 补发，历史完整不重复

## 注意

server 还没有 bin：给 `crates/agent-server/examples/serve.rs`（二十行，读
providers.toml 起在随机或指定端口）——example 不是 bin，不违背「bin 是 M4」；
红线 8 照常（默认 loopback）。前端代码里不出现 api key（它只跟 server 说话）。

### 合并记录（主会话）

构建面验收全过（933/0、typecheck/build 绿、决策 2 实检 grep 为空），冒烟超纲
验证了六条 HTTP 行为 + vite 代理贯通。protocol/index.ts 扩 re-export 与
.gitignore 两处越界改动合规（前者是协议包唯一手写入口，后者是运行时产物）。
**三条协议缺口如实上报**（agent 归属 / spawn 经 HTTP / undo_blocked 详情），
同根因：029 的多 agent 能力未被 agent-server 接满——立 034 补桥，M3 终局
验收（真浏览器）等 034。