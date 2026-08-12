# 160 `recover` 补 `limits` 入参：堵上恢复时的静默失配

**里程碑** M18 · **依赖** — · **模型** sonnet · **独测** ✅ · **状态** 完成（2026-08-12）

## 目标

让「恢复出来的会话」和「新建的会话」拿到**同一组** `AgentLimits`。今天做不到——
`agent-core/src/command/restore.rs:128` 硬写 `AgentLimits::default()`，而
`agent_runtime::recover` 没有对应入参，宿主连「载入后重调」的通道都没有。

**这条不等 [159](159-agent-limits-config-decision.md) 的决策**：不管配置面最后开在
哪一层，恢复路径都得能把一组非默认值带回来。

## 现状

- `agent_runtime::persist::recover.rs:58`：
  `recover(store, agent, history_cap, on_unknown_key) -> Result<Option<Session>, _>`
  ——`history_cap` 是入参，`limits` 不是。
- `agent-core/src/command/restore.rs:124-128` 的注释说 limits「和 `history_cap`
  一样是载入后重调」，**但那半句对 `history_cap` 才成立**（它真有入参）。注释要
  跟着改，别留着骗下一个人。
- `actor/body.rs:80-88` 的注释声称「恢复出来的会话带着它自己持久化过的配置，不被
  这一刻的服务端配置悄悄改写」——对 limits 是假的（它根本没持久化）。同样要改。
- 调用点共三处生产 + 若干测试：`agent-server/src/actor/body.rs:66`、
  `agent-cli/src/main.rs:147`、`agent-server/tests/it/recovered_pending_tools_fail_closed.rs:64`。

## 做什么

1. `agent-core`：`Session::restore`（`restore.rs` 里那个构造）收一个 `AgentLimits`，
   不再硬写 default。**它仍然不进快照、不进日志**——limits 是配置不是状态，
   这条既有拍板不动（`spawn.rs:47-51`）。
2. `agent-runtime`：`recover` 加 `limits: AgentLimits` 入参，**紧挨 `history_cap`**
   ——两个是同一类东西（「载入后由宿主重调的配置」），排在一起读得出这层意思。
3. 三处调用点跟随：server 传 `spec.tools.spawn_limits().unwrap_or_default()`，
   CLI 传它这一侧的那份（161/162 接上 flag 之前先传 `AgentLimits::default()`，
   行为一个字节不变）。
4. 改掉 `restore.rs` 与 `actor/body.rs` 那两段现在说假话的注释。

## 验收

- **主断言（今天会红的那条）**：用非默认 limits（如 `{max_depth: 2, max_children: 2}`）
  建会话 → 写盘 → `recover` 传同一组数 → 读出的 `session.agent_limits()` 等于那组数，
  **不是 default**。
- 传 default 时行为与本 issue 之前逐字节相同（回归闸）。
- 恢复出来的会话，`spawn_child` 真正拦人的闸用的是传进来那组数：恢复一个
  `max_children: 2` 的会话，spawn 第 3 个 → `SpawnRefused::TooManyChildren { max: 2 }`。
- limits **没有**混进快照/日志：恢复前后的日志条目数与字节不因这组数不同而变化。

## 注意

- **不要顺手把 limits 落盘**——那是 159 候选 C，被否的理由在那条 issue 里（不进
  undo 的 slot 是半吊子）。这条只补入参。
- 红线 3/4 不涉及（不新增可序列化状态）；红线 12 涉及：limits 是参数不是分支。
- `history_cap` 是这条的形状样板，签名、文档措辞、调用点写法都照它。
- **`command/restore.rs` 今天已经 303 行、红线 9 已在告警**（`check-invariants.sh --all`
  的存量提示之一）。这条要往里加东西 → **拆分是本次改动的一部分**，不留「下次再拆」
  （全局规则 + 红线 9）。怎么拆见 skill `one-file-one-thing`；那个文件里「重建
  `Session`」和「翻译落盘条目」是两件事，是天然的缝。
- 动到 `agent-server` 的公开面但**不进协议类型**，`--features ts` 那道不必跑
  （WORKFLOW §四第 4 步只在动过进协议面的类型时要求）；仍要 `cargo test --workspace`。

## 实做记录（2026-08-12，完成）

**改了什么**

| 位置 | |
|---|---|
| `agent-core/src/command/restore.rs` | `Session::restore` 加 `limits: AgentLimits`（紧挨 `history_cap`）；不再硬写 `default()`；补「`limits` 为什么必须是入参」一节 |
| `agent-runtime/src/persist/recover.rs` | `recover` 加同名入参并转发；文档说明两个参数为什么排在一起 |
| `agent-server/src/actor/body.rs` | 恢复路传 `spec.tools.spawn_limits().unwrap_or_default()`——**新建与恢复自此同一组数** |
| `agent-cli/src/main.rs`、`agent-wasm/src/assemble.rs` | 传 `AgentLimits::default()`，行为一字不变（CLI 那处 162 接 flag） |
| 其余 ~20 个测试文件 | 机械跟随签名 |

**拆分**（红线 9，本次改动的一部分）：`restore.rs` 开工时 303 行、已在告警，测试段
拆去 `restore_tests.rs`（`#[cfg(test)] #[path] mod tests;`，同 `spawn.rs`/`despawn.rs`
先例）→ **137 行，红线 9 告警从清单里消失**。

**测试**（6 条，两层）

- `restore_tests.rs` 3 条白盒：值带回来 / 闸真按它拦人（第 3 个子撞 `max: 2` 而非 8）
  / limits 不泄漏进日志（换一组上限，entry 数不变）。
- `tests/it/recover_limits_indep.rs` 3 条端到端：真 `Jsonl` 落盘 → 新 backend →
  `recover`，值带回来 / 闸按它拦 / 传默认档时消息数·日志长度·状态全不变。

**负向验证**（这条最重要）：把 `restore.rs` 临时改回硬写 `default()` 模拟 160 之前
的世界 → 两条主断言**确实变红**（`a_restored_session_carries_the_limits_the_host_passed_in`、
`the_gate_on_a_restored_session_uses_those_limits_not_the_defaults`），
`limits_do_not_leak_into_the_log` 照常绿（它是回归闸不是主断言，本该不红）。
「独测能把静默失败变成会红的断言」（WORKFLOW §二两步判据的第二步）就是这么验的。

**验证**：`cargo test --workspace` **2017 passed / 0 failed**；
`check-invariants.sh --all` exit 0，告警清单与开工前逐条比对**净减一条**。

## 踩到的坑

1. **批量改调用点的脚本插错位置**。在最后一个参数的逗号**前**插入 `lit,` 会产出
   `100 lit,, &mut ...`（多一个逗号）。46 处同一形状，一个正则修完。教训：机械改
   调用点先在一个文件上验证再铺开。
2. **`recover` 的调用是 4 参数，`Session::restore` 是 7**——用同一个「参数数 ≥ 7」
   的条件筛，会漏掉全部 `recover` 调用而只改 `restore`。两者要分开跑。
3. **`cargo fmt` 会波及别人在飞的文件**。`agent-wasm`/`agent-transport` 有另一会话
   未提交的改动，全量 fmt 会重排它们、污染那份 diff。改法：`git status` 取自己动过
   的文件清单，减去在飞目录，只对剩下的跑 `rustfmt`；`agent-wasm/src/assemble.rs`
   那一行手工核对格式（它本来就符合 rustfmt 风格）。

## 遗留

`agent-server/src/actor/body.rs` **328 行，红线 9 存量超限**（开工时 320，本次 +8：
一处 `let` + 收紧后的注释）。属于「路过存量超限文件的小改」，按规则指出而不顺手
大重构——它是 actor 主体、强内聚，拆它是一次独立重构，不该混进本条。161 若要再动
它，先读 skill `one-file-one-thing` 决定是否连同拆分。
