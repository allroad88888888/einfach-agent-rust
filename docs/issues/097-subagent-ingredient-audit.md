# 097 核查：父 agent 取料取的是子结论还是子 history

**里程碑** M12 · **依赖** 无 · **模型** sonnet · **独立测试 agent** 是 · **状态** 完成

## 目标

确认第 5 档（子 agent 隔离）**真的在压缩**：父 agent 出料单时取的是子 agent 的
**结论**，不是它的完整 history。确认成立就用测试锁死；不成立就修。

跟 095/096 无依赖，第一天就能开工，可与决策并行。

## 为什么单列一条

整棵 agent 树共用一个 store（CLAUDE.md），子 agent 的完整历史**物理上就在父能读到的
地方**。取错了不报错、功能完全正常——只是父的每一轮 prompt 里都混进子的全部中间轮次，
一个跑 20 轮的子 agent 能让父的 prompt 直接翻几倍。

这是典型的「只在账单上浮出来」。M7/M8 落地时没有人从压缩视角检查过这条路径。

## 做什么

1. 读父 agent 的取料路径，确认它读的是哪个 atom
2. 写测试锁死（不管结论如何，这个测试都要留下）
3. 取的是完整 history → 改成只取结论，并在 issue 里记下原来为什么会那样

## 验收

核查已完成（下节），现状正确，所以本 issue 的实做 = **只写锁死测试，不改实现**。
四条断言，前两条是集成级、后两条钉在性质真正住的地方：

1. **数**：同一份 fixture 跑两次，子分别做 5 轮和 20 轮工具调用、
   **最后一条 assistant 文本固定为同一个字符串**；捕获父 harvest 之后那一跳的请求体，
   断言 `body_5 == body_20`（逐字节）。
   ——「子的终答固定」这个前提不能漏，漏了测试必红且红得没有意义。
2. **痕迹**：子的每一轮中间产物打唯一标记（`CHILD_STEP_00`…`CHILD_STEP_19`，
   工具入参和中间文本各一份），断言父那一跳请求体对 20 个标记全部
   `!body.contains(marker)`，且 `body.matches(ANSWER).count() == 1`。
3. **单元级（跑得快）**：给子造 20 条 assistant 消息（每条含 `Text` + `ToolUse` +
   `ToolResult` 块），断言 `child_outcome::final_text` 等于最后一条 assistant 的
   `Text` 块拼接，且返回长度对 5 轮 / 20 轮完全相同。
   ——这条挡的是「风险点」第二项。
4. **M8 形态 + 核心不变量**：三个后台子（轮数 3 / 10 / 20）全部 `collect` 之后，
   断言父的 `messages_of(&root)` 里提到子的 `ContentBlock` **恰好 3 个 `ToolResult`**、
   每个 `content` 逐字节等于对应的 `child_outcome::final_text`，
   且根历史的**消息条数与三个子的轮数无关**；再补一条结构断言
   `read_descendant(&root, &child, Slot::Messages)` 必须是
   `Err(ReadDenied::NotVisible { .. })`——把「core 层父读不到子正文」
   从注释变成会红的测试。

## 核查记录（2026-08-10，只读核查，未改代码）

**结论：父的料单不含子 agent 的任何中间消息。** 第 5 档（子 agent 隔离）真的在压缩，
[106](106-summary-via-subagent.md) 可以依赖它。

证据链：

1. `agent-runtime/src/dispatch.rs:87` → `provider_call.rs:117`：
   `Effect::CallProvider { agent, epoch }` 把 agent 原样交给 `provider_call::start`，
   取料那一句是 `session.messages_of(&agent)`——参数就是本次 `CallProvider` 归属的
   agent，没有任何拼接祖先/后代历史的分支。全仓生产代码只有 `provider_call.rs:157`
   （与 161 的 one-shot 安全重编码）调 `provider.encode`，**取料路径唯一**。
2. `agent-core/src/command/read.rs:101` + `:68`：`messages_of(agent)` →
   `AtomKey::Agent(agent.clone(), Slot::Messages)` 的 `peek`。
   历史 atom 是按 `AgentId` 参数化的 family 键（`graph/slot.rs:155`），
   父用自己的 id 取，**物理上取不到子的那一格**。
3. `agent-providers/src/lib.rs:42`：`Ingredients` 是纯引用结构，
   **不持有 `Session` 也不持有 store**，`encode`（`:93`）拿不到别的 agent 的状态
   ——adapter 那一侧是类型级封死的。
4. `agent-runtime/src/child_outcome.rs:56`：子的产出 = 最后一条
   `role == Assistant && has_text` 消息里的 `Text` 块拼接，
   `Thinking`/`ToolUse`/`ToolResult` **显式过滤**。**O(1) 于子的轮数。**
   回写经 `child_slot.rs:56` → `transitions/tool_outcome.rs:63` 的
   `push_message`，写进**父**的 Messages 槽，一条消息，
   且经 `DEFAULT_TOOL_OUTPUT_BYTES = 32KiB` 截断封顶。
5. M8 `collect` 走同一套：`subtree.rs:216` 调同一个 `child_outcome::outcome`，
   已有测试 `tests/it/collect_matches_blocking_spawn.rs:202` 断言两条路交给父的正文
   **逐字节相同**。
6. `srv:agent/status`：`status_tool.rs:182` 每个后代只渲染
   `id / depth / activity / task` 四个字段；`task` 是**父自己写的** spawn 任务文本，
   再按 `TASK_CHARS = 100` 截断。**零条子的消息正文。**

### 风险点：保证是弱的

**不是类型系统保证，是约定。** `Session::messages_of(&self, agent: &AgentId)` 是 `pub`、
只做 `peek`、**没有方向校验也没有可见性校验**（`read.rs:68`）。
core 那道真正的闸（`Slot::Messages = Upward`，`graph/visibility.rs:65`；
`read_descendant` 返回 `ReadDenied::NotVisible`，`cross_read.rs:98`）
**这条路径根本不经过**——`child_outcome.rs:9` 的模块文档自己写了它是
「运行时侧读，绕开 core 跨读 API」。

三行改动能静默破坏它，现有测试一条都不会红：

| 位置 | 怎么破 |
|---|---|
| `provider_call.rs:118` | 换个 id，或 `extend` 一段子的历史（「让父看到子的过程」听起来很合理） |
| `child_outcome.rs:62` | `.rev().find(...)` 改成 `.filter(...).collect()`，或放开块过滤 |
| `subagent.rs:29` | `system_for` 里塞一段「子 agent 进展摘要」 |

代价量级：一个跑 20 轮带工具的子 agent 约 40 条消息（20 assistant + 20 tool_result，
单条上限 32KiB），取代现在的**1 条** ≤32KiB `tool_result`；而且它躺在父的历史里，
父**后续每一跳都重发**，还会把前缀镜像从中段推走 → 前缀缓存全断
（DeepSeek 上一次值 ~120 轮命中）。三个后台子并行再乘 3。

## 注意

- **红线 10**（agent 之间只允许上下读）——现状**没走** core 的跨读闸，
  验收第 4 条那句 `read_descendant` 断言就是把这道闸补上一半的锁
- **红线 12**——写测试时别引入任何按 provider 的分支
- 本 issue 不改实现。真要把「弱保证」升级成类型级保证（比如给取料换一个
  只能传自己 id 的窄接口），那是另一条 issue，别夹带

## 实做记录（独立测试 agent，2026-08-10）

**只加测试，零实现改动**——`git diff --stat crates/agent-runtime/src/child_outcome.rs`
是 `124 insertions(+), 0 deletions`。

### 落地的文件

| 文件 | 行数 | 职责 |
|---|---|---|
| `agent-runtime/src/child_outcome.rs` | 208（+124） | 内联 `#[cfg(test)] mod tests`，2 个单元测试 |
| `agent-runtime/tests/it/harvest_omits_child_turns_support/mod.rs`（新建） | 110 | 共享 fixture：N 轮工具调用，每轮打 `CHILD_STEP_NN` 标记，末轮固定终答 |
| `agent-runtime/tests/it/blocking_spawn_omits_child_turns.rs`（新建） | 82 | 断言 1、2（M7 阻塞路径） |
| `agent-runtime/tests/it/collect_omits_child_turns.rs`（新建） | 143 | 断言 4（M8 后台 + `read_descendant` 结构断言） |
| `agent-runtime/tests/it/main.rs` | +3 | 三行 `mod` |

`final_text` 保持 `pub(crate)` 未放开——单元测试内联，不为测试改可见性。

### 四条断言逐条对应

1. `blocking_spawn_omits_child_turns::five_and_twenty_round_children_yield_byte_identical_harvest_bodies_given_the_same_final_answer`
2. `blocking_spawn_omits_child_turns::none_of_the_twenty_intermediate_markers_leak_into_the_harvest_body`
3. `child_outcome::tests::final_text_is_only_the_last_assistant_texts_not_any_tool_use_or_tool_result`
   \+ `final_text_length_does_not_grow_with_the_number_of_rounds`
4. `collect_omits_child_turns::three_background_children_collect_into_a_fixed_shape_no_matter_their_round_counts`

### 变异检验（主会话复核，不是 agent 自评）

锁死测试不会红就是废的。注入「风险点」表里点名的那个真实改动
——`.rev().find(...)` 改成 `.filter().collect()`（「让父看到子的全过程」）：

```
单元测试   2 个全 FAILED
集成测试   3 个全 FAILED
```

另单独试只去掉 `.rev()`（取第一条而非最后一条）：单元测试红一个。**锁是实的。**
已还原，复跑 5 个全绿。

### 两处偏离 issue 原文的 fixture 决定（均成立）

1. **断言 4 把 `spawn(background)` 与 `collect` 绑进同一跳**。理由：`spawn(bg)` 立刻
   resolve，`collect` 会 Pending 到子收敛，同一跳就让三个子严格串行，于是能用
   按连接顺序的录制服务器，不需要 052/053 那套按内容路由的并发服务器
   （3+10+20=33 轮子调用的路由竞态面很大）。代价是模型脚本略合成
   ——真实模型不会 collect 一个同轮刚 spawn 的 id。测的是数据形状不是模型行为，可接受。
2. **断言 4 没有直接调 `final_text`**（`pub(crate)`，`tests/it` 是独立二进制够不着）。
   改用构造性等价：三个固定 `ANSWER_A/B/C` 是各子历史里**唯一**的文本内容，
   所以与字面量逐字节相等 ⇔ 与 `final_text` 输出相等；`final_text` 本身的行为
   由断言 3 的单元测试钉住。

### 命令输出

```
$ cargo test -p agent-runtime
145 lib + 102 integration + 0 doctest，全过，0 failed

$ bash scripts/check-invariants.sh --all
exit 0；17 个存量红线 9（行数）违规，新增/改动的文件一个都不在其中

$ cargo clippy -p agent-runtime --all-targets -- -D warnings
❌ baseline 就是红的，见下
```

### ⚠️ 顺带发现的存量问题（与本 issue 无关，不在此修）

`cargo clippy -p agent-runtime --all-targets -- -D warnings` **在 baseline 上就失败**，
5 个错误全在本 issue 没碰过的文件里（`git status` 确认这些文件干净）：

| 文件 | 问题 |
|---|---|
| `provider_call.rs:99` | `Err` 变体过大 |
| `provider_message.rs:85` | `let...else` 可以用 `?` |
| `subagent.rs:48` | `iter().cloned().collect()` 应为 `to_vec()` |
| `execution_binding_tests.rs:102` | `Copy` 类型上调 `clone` |
| `provider_call_tests.rs:35` | 同上 |

前三个是 **lib 级**，会挡住 cargo 编译 `tests/it` 目标——也就是说
[WORKFLOW](../WORKFLOW.md) §四第 4 步那句 `cargo clippy --workspace --all-targets -- -D warnings`
**在这个 crate 上现在根本走不通**。下一个照 WORKFLOW 收工的 agent 会一头撞上。
要么 CI 没跑 clippy，要么跑了没人管——值得单开一条。
