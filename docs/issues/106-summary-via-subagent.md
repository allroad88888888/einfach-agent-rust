# 106 摘要生成走子 agent

**里程碑** M12 · **依赖** [105](105-effect-compact.md) · **模型** sonnet · **独立测试 agent** 是 · **状态** 完成

## 目标

用第 5 档实现第 3 档：摘要这活派给一个子 agent 干，它自己的上下文里烧掉那堆原文，
回来只给一份摘要。

## 为什么不在父 agent 里直接调

三条好处是白捡的：

1. **中间过程天然不污染父上下文**——摘要那次请求要把整段历史发过去，
   如果在父的会话里做，这一来一回本身就进了父的历史
2. **用哪个模型摘要由 `ChildConfig` 说了算**，不是 core 在 `match provider`
   （红线 12 干净）。想用便宜模型摘要就配一个便宜的子 agent
3. **M8 的编排现成**，不用新机制

## 做什么

宿主收到 `Effect::Compact` → spawn 一个窄范围子 agent → 它读边界之前那段历史、
产出摘要 → 回一个带同一个 `epoch` 的结果事件。

摘要的提示词是产品判断，不是模型判断——写在宿主侧，别下沉进 adapter。

## 接线点与契约（2026-08-10 主会话定）

**本条不引入任何新的公开 API**——可观测契约已经被 105 的类型定死了：

```
Effect::Compact { agent, upto, epoch }   进
        ↓（宿主 spawn 一个窄范围子 agent，让它读父的 [0, upto) 那段历史）
Event::CompactDone { agent, summary, epoch }   出（成功）
Event::CompactFailed { agent, epoch }          出（失败 / 超时）
```

接线点是 `crates/agent-runtime/src/dispatch.rs` 里 `Effect::Compact` 那个分支
——**105 把它留成了「立刻回 `CompactFailed`」的桩**，本条把桩换成真的。
`upto` 就是给这里取那段历史用的。

四条硬契约：

1. **摘要提示词写在宿主侧**，不下沉进 adapter——那是产品判断不是模型判断
2. **子 agent 用哪个模型由它自己的 `ChildConfig` 定**，core 里不许多出任何
   provider 分支（红线 12）
3. **失败 / 超时是正常事件**：回 `Event::CompactFailed`，压缩这一次作废、边界不动、
   下一轮照常跑。**不许卡死父 agent**
4. 回执**原样带回 `Effect::Compact` 给的那个 `epoch`**（红线 6）——闸在
   `Session::step`（105 已落地），你只负责别把 epoch 弄丢

## 验收

- 父的 `encode` 输出里**不含**被摘要的原文
- 父的 `encode` 输出里**不含**子 agent 的摘要过程（请求、中间轮次都不进父的历史）
- 摘要子 agent 用的模型来自它自己的 `ChildConfig`；core 里搜不到任何
  为摘要新增的 provider 分支
- 摘要正文以 `Arc` 存在 `SendPlan` 之外，`SendPlan` 里只有引用
  （[099](099-send-plan.md) 已经锁死，这里复验一次）
- 子 agent 失败 / 超时：父不卡死，压缩这一次作废，边界不动，下一轮照常跑

最后一条容易漏。摘要失败是正常事件，不是异常路径。

## 注意

- **红线 10**（agent 之间只允许上下读）——摘要子 agent 读父的历史是向上读，允许
- **红线 12**——摘要用哪个模型是配置，不是分支
- 这条跟 [097](097-subagent-ingredient-audit.md) 是一回事的两面：
  097 确认「父不吃子的过程」，本条依赖那个性质成立。097 要是发现取错了，
  先修 097

## ⚠️ 行为验收移交 [108](108-tier-ladder.md)（2026-08-10 裁决）

独测 agent 用编译探针查实：**`Effect::Compact` 目前对外完全不可达**——

1. `dispatch::run_effect` / `compact_spawn::intercept` / `CompactSlots` 全是
   `pub(crate)`，`dispatch` 模块本身是私有 `mod`
2. 唯一公开入口 `run_turn` 固定从 `Event::UserInput` 起步
3. **`agent-core` 里没有任何命令、任何一格转移会产出 `Effect::Compact`**
   ——产出它是 [108](108-tier-ladder.md) 的事，106 明确「不引入任何新的公开 API」

**这是我排序时造成的依赖倒置**：106 排在 108 之前，但 106 的可观测行为要等 108
接上阶梯才第一次变得可达。

**裁决：不为测试新开 API，把行为验收挪到 108。** 理由：108 的阶梯在 turn 结束时开火，
届时 `run_turn` 这个既有公开入口天然就能驱动整条链，不需要任何 test-only 的口子。
现在加一个 `pub fn compact_now` 只为让测试跑起来，是拿公开 API 面换测试便利。

**移交的四条**（已加进 108 的验收）：

- 父的 `encode` 不含被摘要原文
- 父的 `encode` 不含子 agent 的摘要过程（每轮打 `CHILD_STEP_NN` 标记逐一断言）
- 失败 / 超时 → `CompactFailed` → 父不卡死、边界不动、下一轮照常跑完
- 反向锁：成功路径回执确实是 `CompactDone` 而不是 `CompactFailed`

**106 现在实际被什么盖住**：实现侧 5 条内联单测（`compact_spawn_tests.rs`）+
独测 4 条「core 里没有为摘要新增的 provider 分支」的 grep 断言
（`compact_subagent_no_provider_branch.rs`，151 行，含对 grep 逻辑本身做的变异检验）。
**行为路径在 108 落地前是没有集成测试的**，动 `compact_spawn.rs` / `compact_slot.rs`
的人要知道这点。

## 实做记录（实现 agent + 独立测试 agent 并行，2026-08-10）

与 [103](103-prefix-intent-wiring.md) 同时开工（`dispatch.rs` 归本条、
`provider_call.rs` 归 103，不重叠）。

### 落地的文件

| 文件 | 行数 | 职责 |
|---|---|---|
| `agent-runtime/src/compact_spawn.rs`（新建） | 150 | `Effect::Compact` → spawn 摘要子：建 `ChildConfig`、把 `[0, upto)` 渲成子的第一条 user 消息、记在飞槽位 |
| `agent-runtime/src/compact_slot.rs`（新建） | 178 | 在飞摘要子的收割：终态翻成 `CompactDone`/`CompactFailed`，复用 `child_outcome::outcome` 的翻译规则 |
| `agent-runtime/src/compact_spawn_tests.rs`（新建） | 238 | 5 条单测 |
| `agent-runtime/src/{dispatch,runner,ctx,execution_binding,lib}.rs` | +7~20 各 | 接线：`CompactSlots` 穿过泵循环，在 `subtree.harvest` 之后收割 |

### 四条契约怎么保证的

1. **提示词在宿主侧**：`SUMMARY_INSTRUCTIONS` 是 `agent-runtime` 里的 `const &str`，
   不碰 `agent-providers`
2. **模型由 `ChildConfig` 定**：`execution_profile` 取自 `ctx.compaction_execution_profile`
   ——**`agent-core` 一个文件都没动**，零新增 provider 分支
3. **失败/超时是正常路径**：spawn 被拒（深度/子数上限）当场回 `CompactFailed` 零副作用；
   子后来失败/被取消/超时由 `CompactSlots::harvest` 翻成 `CompactFailed`。
   摘要子**零工具 → 结构上单轮**，provider 超时兜底，不可能卡死父
4. **epoch 原样透传**：`intercept` 记下 effect 自己的 epoch，`harvest` 直接抄进事件，
   不重算。闸仍在 `Session::step`（105）

### 一处刻意的判断：摘要读的是**原始**历史不是投影后的

如果拿第 2 档已经打过占位的视图去摘要，摘出来的东西更空洞。多出来的那点长度，
正是 `compaction_execution_profile` 存在的意义——宿主可以给摘要子配一个便宜模型抵掉。

### 路过的存量超限（只指出，未重构）

`agent-runtime/src/runner.rs` 改动前就是 **343 行**（已超 300，属那 17 个存量之一），
接 `CompactSlots` 之后 **356 行**。是外科式接线不是重构。它本身是「单一职责的事件泵
/ 状态机」，够得上「复杂文件 ≤500」那一档，但**理由要有人正式写下来**，
不能一直靠默认。

### ⚠️ 它报出来的一个真问题，已并进 108

**摘要子 agent 收割完从不 despawn**，`max_children` 默认 8 → 长会话压 8 次之后
自动压缩永久失效。没红任何红线（每次响亮地报 `Notice::CompactionFailed`），
但正好打在 M12 的要害上。裁决与验收见
[108 §摘要子 agent 必须回收](108-tier-ladder.md)。

### 命令输出

```
$ cargo test --workspace                                     全绿
$ cargo clippy -p agent-runtime --all-targets -- -D warnings  5 个存量错误，零新增
$ bash scripts/check-invariants.sh --all                      exit 0
```
