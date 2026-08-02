# 030 session actor：store 独占线程，mpsc 进 broadcast 出

**里程碑** M3 · **依赖** 026 · **模型** sonnet · **独立测试 agent** 否 · **状态** 完成

## 目标

ARCHITECTURE 关键判断 1 落地：store 不是 Send 是刻意的——**每个 session 独占一个
线程**，外界只经 `mpsc<Command>` 进、`broadcast<Event>` 出。只有 `agent-server`
知道线程与 tokio 的存在。

## 做什么

新建 `crates/agent-server`（**库**，决策 12；红线 7 不辖它，tokio 可用）：

- `SessionActor`：专属 `std::thread` 里住 `Session` + `RunnerCtx`，循环收
  `Command::{Input(String), Undo { force: bool }, Redo, Cancel, Shutdown}`；
  `Cancel` **不排队**——旁路原子标志（复用 RunnerCtx 的 cancel flag），队列里
  的取消等轮次结束才生效就不叫取消
- `SessionEvent`：对外广播的事件（增量文本/思考、工具执行、Notice、GuardReport、
  轮终态、undo 结果），**全部可序列化**——032 要生成 TS 类型，这里就是协议雏形。
  `tokio::sync::broadcast`（有界，慢消费者丢帧要可检测——收到 `Lagged` 补一条
  显式的「你掉了 N 条」事件让下游知道自己瞎过）
- `SessionRegistry`：`SessionId → SessionHandle`（发送端 + 订阅入口 + join 句柄）；
  `open`（带 `--session` 同款持久化路径语义）/ `get` / `close`（优雅：Shutdown →
  join → 落最后快照）。M3 单副本内存表，Redis 版是 M4 后
- 崩溃隔离：actor 线程 panic 不拖垮进程——句柄侧能观测到「会话死了」并广播
  终态事件，registry 里标记为 dead（不静默移除，客户端要能问到死因）

## 验收

- 两个 session 并行各自对话（假 SSE），事件互不串台，store 线程互不共享
- 同一 session 两个订阅者收到**同一序列**事件
- 轮进行中发 `Cancel` → 数百 ms 内本轮 `Failed(Cancelled)`（复用 CLI 同款轮询）
- `Input` 在轮进行中到达 → 排队，当前轮结束后按序处理（不丢不并发）
- `close` 后线程 join、持久化文件完整可 `open` 恢复
- actor 内 panic → 进程活着、事件流收到终态、registry 报 dead
- 慢订阅者 Lagged → 收到显式掉帧事件

## 注意

`SessionEvent` 的形状别照抄 `RunnerEvent` 的借用结构——广播要 `Clone + Send +
'static`，该 owned 就 owned。028 正在并行改 `Session` 的公开面（step 长 agent
维度）——你只经 `agent-runtime::run_turn` 驱动，不直接调 `Session::step`，
接缝留给 029 统一对齐。红线 8 是 031 的（这里还没有网络面）。

## 实做与合并记录（主会话代笔，2026-08-02）

**代笔原因如实记录**：实现 agent 完成了全部代码与测试，但收尾时陷进
「起后台测试 → 等监视器 → 醒来再起一轮」的等待循环，三次催交未果，主会话按
磁盘现状验收。流程教训：**给 agent 的收工指令要求前台跑验证、禁止后台+监视器
自旋**——已知第一例，记此为鉴。

**落地**（`crates/agent-server`，src 共 1102 行，全部 ≤300）：
`Command{Input/Undo{force}/Redo/Cancel/Shutdown}` / `SessionEvent`（RunnerEvent
的 owned 可序列化翻译 + `SessionDied` + 掉帧显式事件，032 的协议雏形）/
`SessionHandle{send/cancel 旁路/subscribe}` / `SessionRegistry{open/get/close}`。
actor 用 `catch_unwind` 接 panic → 广播 SessionDied → registry 报 dead 不静默移除。
只驱动 `agent_runtime::run_turn` 不碰 `Session::step`（028 并行改造中，接缝留给 029）。

**验收对照**：7 个集成测试文件逐条压验收——两会话不串台 / 双订阅同序 /
轮中 Cancel 快速失败 / 排队输入按序 / close-reopen 恢复 / panic 报 dead /
并发 open 同 id 仅一个赢（超出验收的加项，收）。crate 自测 22/0；
其遗留验证轮跑到 476/0 零失败；主会话部分门禁 455/0。最终全量门禁与 028
合并时一并复核。
