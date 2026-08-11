# 135 开局驱动：新建会话跑 `SessionStart` 工具

**里程碑** M15 · **依赖** [133](133-call-timing-field.md) + [134](134-prefix-chunk-state.md) · **模型** sonnet · **独测** ✅ · **状态** 完成（2026-08-11）

## 目标

会话**新建**（不是恢复）完成装表之后、第一轮之前：按注册顺序执行
`timed(SessionStart)` 的每个工具，结果拼成 `SystemChunk` 列表一次写入 134 的状态；
**任一失败 = 会话创建失败**，向创建方返回错误。组料时前缀块排在基础
system 之后。

## 现状

- 组料点：`provider_call.rs` 每轮现组 `Ingredients`。
- server 的创建是幂等三态（决策 25②：活着接上 / 磁盘有则恢复 / 都没有才建）——
  **只有第三态跑驱动**。

## 做什么

1. `agent-runtime` 新驱动（如 `run_session_start`）：逐个调 timed 条目
   **自带的执行体**（133 修订后的形状：本地同步函数，不走 dispatch/executor/
   远端槽——创建时 SSE 还没接上，远端时机工具结构上不存在这条路）→ 文本结果 →
   `SystemChunk { label: "init:<name>", text }`。
   空文本不产块（不白占一段前缀）。任一执行体返回 `Err` → 整个会话不建。
2. 全部成功后一次 `set_prefix_chunks`。
3. `provider_call` 组 `ing.system`：基础 system 块之后追加 `prefix_chunks()`
   （顺序即状态顺序）。
4. 宿主接线：
   - `agent-cli`：创建路调驱动，`Err` = 启动失败，打印后退出。
   - `agent-server`：三态里只有「新建」调；`Err` → HTTP 错误响应，**会话不落盘、
     不留半个状态**。
   - `agent-wasm`：装配不开 timed 档（skills 档本来就不开），不接线、零改动。

## 验收

- fake init 工具（测试表）：首轮 encode body 的 system 段含其输出；两个 init 工具
  **交换注册顺序 → body 中的先后跟着换**，且同料两次 encode 逐字节相同。
- 执行计数器断言：新建 = 1 次；快照恢复后再跑一轮 = **仍 1 次**（恢复不重跑，
  值从 134 的状态来）。
- fake 失败工具：创建返回 `Err`；无 journal 文件、无任何状态残留。
- 无 timed 工具的装配：创建路径与首轮 body 与本条落地前**逐字节相同**（红线 11）。

## 注意

- **红线 11**：前缀块进 system，顺序与内容必须逐字节确定——顺序即注册顺序，
  已由 133 的 `timed()` 保证，这里别用任何无序容器中转。
- 「恢复不重跑」的一半在 134（值是状态），一半在这里（驱动只挂新建路径）——
  两条验收合起来才闭环。
- 原稿里「Web 位置 + deadline」那条已删：会话创建时 SSE 还没接上，远端时机
  工具的回写通道结构上不存在，133 修订后的注册签名让这条路无法表达，
  比运行期校验更早。

## 实做记录（2026-08-11）

- 落点：`session_start.rs`（194 行）+ agent-cli/agent-server 各一个薄壳模块（恢复路 no-op）。
  全有或全无：任一执行体 Err → 一个前缀块都不写。组料在 `subagent::system_for`：
  `ctx.system` → 前缀块 → 子 agent 模板，root 与子同路。
- **139 期间修了本条接线的时序 bug**：两个宿主原先把 `maybe_run` 放在
  `persist::seed_after_recover` 之前，`prefix_init` entry 被当成「已在盘上」永不落盘，
  重启静默丢索引。已改为 seed 之后跑，`http_capabilities_skills_survive_restart` 钉住。
- 独测 8 条全绿，含线级断言（录制服务器抓真实请求体：init 块在两轮 system 段各恰一次）。
