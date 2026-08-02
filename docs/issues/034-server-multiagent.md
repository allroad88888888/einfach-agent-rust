# 034 server 接满多 agent —— M3 终点

**里程碑** M3 · **依赖** 033 · **模型** sonnet · **独立测试 agent** 否（终局由主会话真浏览器验收）· **状态** 完成

## 目标

把 029 给 runtime 的多 agent 能力接满到 HTTP 协议面，清掉 033 上报的三条缺口。
之后 M3 终局验收（真浏览器）才有東西可看。

## 三条缺口与修法

1. **SSE 帧带 agent 归属**：actor 改用 `RunnerCtx::with_agent_events`（029 就为
   这个准备的），`SessionEvent` 的携带方式**信封化**——SSE 帧 data 变
   `Frame { agent: string, event: SessionEvent }`（029 判断 5 的同款理由：归属是
   元数据不是任何变体的载荷，包一层是类型事实）。协议变更 → 032 的机制走一遍
   （重生成 + fixtures + 一致性测试 + web 端 dispatch 适配）
2. **spawn 经 HTTP 开闸**：`ToolTableSpec` 加档（如 `Full { spawn_limits }`），
   `examples/serve.rs` 用它——浏览器会话的模型能拿到 `srv:agent/spawn` 与
   `srv:shell/exec`（屏障 UI 在 3 修好之前先由 web 的 confirm 撑着）
3. **`undo_blocked` 带详情**：actor 侧（同进程持有 session）把 `Blocked` 富化——
   `barrier_seq` 对应 entry 的 label 与（若是工具屏障）工具名/call_id 读出来，
   随 undo outcome 帧下发；web 的 confirm 显示「越过的是什么」（027 的原则：
   让人明白自己在确认什么）。CLI 的 describe_barrier 逻辑若可搬进
   agent-core/session 公共处，搬（一份实现两处用），搬不动就 server 侧对齐语义

## web 侧跟进

- 真分栏替掉「疑似子 agent 活动」近似（帧归属现在是真的）
- confirm 文案带工具名/call_id
- typecheck/build 照绿

## 验收

- 重生成后 `cargo test -p agent-server --features ts` 全绿（一致性含 Frame）
- 假上游集成测试：spawn 轮经 HTTP → SSE 帧里两个子 agent 归属交错出现
- undo 撞屏障 → SSE 出的 blocked 帧含工具名与 call_id
- `pnpm -r typecheck` + `pnpm --filter web build` 绿
- 全量门禁四件套

## 注意

协议此刻改动成本最低（033 是唯一消费者）——这正是把 034 排在真浏览器验收前的
理由。`Frame` 信封同时是将来 M4 多路复用（一条 SSE 多 session？不做，但别堵死）
的自然位置。红线照常。

### 合并记录（主会话，含代收尾）

三条缺口修法全落地：Frame{agent,event} 信封（重生成走 032 机制、web dispatch
跟进）、ToolTableSpec 加满档 + serve.rs 开 spawn/shell、Blocked 富化（公共读口，
CLI/server 一份语义）。核心验证 agent-server(ts) 110/0、workspace 940/0、
typecheck/build 绿、红线过。实现 agent 第四次收尾自旋病发（433 次调用后坐等
监视器），主会话代收尾：修掉它没跑完的 clippy 尾巴（while_let 一处）。
收尾自旋已成模式病——记 WORKFLOW 待改进：收工模板需要结构性防自旋条款。