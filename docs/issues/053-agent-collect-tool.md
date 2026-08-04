# 053 `srv:agent/collect` 工具——领取后台子 agent 结果

**里程碑** M8 · **依赖** 052（detached 集 + stash） · **模型** opus · **独测** ✅（碰红线 6）

异步核心的**收割半边**。052 让父发后台子并观测，053 让父**择时领结果**。052+053 合起来 =
「发后台子 → observe → collect」闭环，本里程碑的「能用」终点。接缝见
[ORCHESTRATION.md](../ORCHESTRATION.md) §三/四/五。

## 范围

1. **工具声明**（`tool_table.rs` + 新 `collect_tool.rs`）：`srv:agent/collect`，Server 位置。
   参数 `id`（必填，必须是调用者的**后代**，否则 `is_error`——红线 10）。可逆性 `Pure`：collect
   本身只读子结果、无副作用；**子自己的不可逆操作带自己的屏障位**（在子的日志、同 `turn_id`），
   undo 撞到那些先停，collect 不需要额外屏障。
2. **dispatch 截获 + 复用 harvest**（`dispatch.rs` 截获，`subtree.rs` 逻辑）：
   - 子已在 052 的 stash（已完成未领取）→ **立刻**把 stash 里的结果回写 collect 槽、从 stash
     移除（领取即消费）。
   - 子仍在 detached 集里跑 → `subtree.record(child=id, parent, call_id=collect_call, epoch)`
     绑定，走**现有 `harvest`→`Event::ToolResult` 回写路**（`subtree.rs:65-93`）：collect 槽
     `Pending` → 父 `ToolsPending` → pump 驱动子 → 子终态 harvest 回写 collect 槽 → 父恢复。
   - 子正文用现有 `subtree::final_text`（`:134`）——运行时侧读，非 core 跨读（`Messages` 是
     Upward-only，core 跨读拿不到；harvest 是宿主既有合法回写路，见 ORCHESTRATION §五）。
3. **双重 collect / collect 已取消的子**：同一 id 被 collect 两次 → 第二次 `is_error`（已消费）。
   collect 一个已被孤儿取消/undo 掉的 id → `is_error`，不 panic。

## 验收（可判定）

- 父 `spawn(bg) A` → `status` 看到 A Done → `collect(A)` **立刻**返回 A 的最终结果；结果内容
  == 同任务用阻塞 spawn 得到的结果（后台=前台拆开，结果一致）。
- 父 `spawn(bg) B`（慢）→ 立刻 `collect(B)`：collect 槽 `Pending`、父 `ToolsPending`、pump 驱动
  B、B 终态后 collect 回写、父恢复——**等价于老阻塞 spawn，只是显式**。
- 父 `spawn(bg) A,B,C` → `collect` 顺序可任意（先收先完成的）→ 三个结果都拿到；全 collect 完
  detached 集空、无孤儿取消触发（正常收尾，无告警）。
- 子失败（is_error）→ `collect` 拿到 `is_error` 的 tool_result，loop 继续。
- **红线 6**：collect 绑定的在飞子在飞时 bump epoch → 回写被 `step.rs` 门丢弃，collect 槽不被
  幽灵结果填。断言。
- 双重 collect 第二次 `is_error`；collect 不存在/已取消的 id `is_error`。

## 注意

- **红线 6**（静默失败）：collect 绑定复用 052/现有的 `ChildSlot.epoch` + `step.rs:69` 门，
  **不新造**。派独测断言在飞 bump epoch → 结果丢弃。
- **红线 10**：`id` 过 `is_descendant_of(caller)`。collect 读子正文经 harvest（运行时），不经
  core 跨读 API——不违反 `Messages` 的 Upward-only 可见性（ORCHESTRATION §五说清）。
- **前台 spawn 不受影响**：collect 只作用于后台子；阻塞 spawn 仍自己 harvest 回 spawn 槽。跑
  既有 spawn 测试证明无回归。
- **不碰 `agent-tools/`**；与 052 同改 `subtree.rs`/`dispatch.rs`，**排 052 之后**做，避冲突。
- 收工验证前台跑完（WORKFLOW §四 -1）。

## 实做记录（完成 · 2026-08-04）

收割半边落地，M8 闭环合拢。跟 052 一样，**一个 atom / primitive / `Effect` /
`Notice` 都没加，`agent-core` 一行没改**——而且这一次连 `runner.rs` 都一行没改：
collect 走的是 029 就有的那条槽位收割路，它要做的全部事情就是**在那张表上补一笔**。

### 建了什么

| 文件 | 行 | 干什么 |
|---|---|---|
| `agent-runtime/src/collect_tool.rs` | 229 | 新：`COLLECT_TOOL` / `collect_spec` / `parse` / `intercept`（两条出路）/ 三种拒绝文案 |
| `agent-runtime/src/collect_tool_tests.rs` | 244 | 新：入参、红线 10 的清单边界、领取即消费、**红线 6 的确定性孪生**（`#[path]` 子模块） |
| `agent-runtime/src/reply.rs` | 80 | 新（前置重构）：截获类工具「当场有结果」的收尾——通报 + 收敛槽位一对成套 |
| `agent-runtime/src/child_outcome.rs` | 66 | 新（前置重构）：子终态 → 父读得懂的正文（从 `subtree.rs` 拆出，红线 9） |
| `agent-runtime/src/spawn_tool.rs` | 282（229→，+53） | 改（前置重构）：`intercept`/`detach` 从 `dispatch.rs` 搬回来，`refuse` 换成 `reply::refuse` |
| `agent-runtime/src/spawn_tool_tests.rs` | 74 | 新（前置重构）：原 inline `mod tests` 原样挪成 `#[path]` 子文件 |
| `agent-runtime/src/dispatch.rs` | 181（293→，−112） | 改：spawn 三函数搬走、加 collect 截获臂——回到纯分派器的形状 |
| `agent-runtime/src/subtree.rs` | 288（277→，+11） | 改：`ChildSlot.tool`、`record` 收工具名、`is_detached`/`is_awaited`/`take_stashed`/`collectable`、`harvest_slots` 里那行 `detached.retain` |
| `agent-runtime/src/status_tool.rs` | 263（288→，−25） | 改：成功/拒绝两条尾巴换成 `reply::ok`/`reply::refuse` |
| `agent-runtime/src/orphan.rs` | 126（111→，+15） | 改：**只有文档**——把第三条判据从「恒真的占位」改写成两条真实路径 |
| `agent-runtime/src/tool_table.rs` | 286（262→，+24） | 改：`with_collect()`、`COLLECT_TOOL => Pure` |
| `agent-runtime/src/lib.rs` | 103 | 改：三个新 `mod`、`pub use collect_tool::{COLLECT_TOOL, collect_spec}`、模块文档补「领结果」一节 |
| `agent-cli/src/main.rs` | +6 | 改：`.with_collect()` 接在 `.with_status()` 之后（开了 bg 不开 collect 是陷阱） |
| `agent-runtime/tests/collect_matches_blocking_spawn.rs` | 163 | 新：验收 1 + **后台 = 前台拆开**（逐字节相同） |
| `agent-runtime/tests/collect_waits_for_a_running_child.rs` | 101 | 新：验收 2（时序 + 「父一跳都没多发」） |
| `agent-runtime/tests/collect_three_out_of_order.rs` | 118 | 新：验收 3（乱序领 + **一句告警都没有**） |
| `agent-runtime/tests/collect_refusals.rs` | 113 | 新：双重 collect / 不存在的 id / 非后代 |
| `agent-runtime/tests/collect_child_failure.rs` | 75 | 新：子失败 → `is_error` 的 tool_result，loop 继续 |

`runner.rs` 299 行、`agent-core` / `agent-tools` / `agent-server`：**一行没改**。

### 前置重构：三刀，全是被行数逼出来的真拆分

052 交接时 `dispatch.rs` 293 / `runner.rs` 299，离红线 9 只剩个位数。按 052 的建议
先拆再加，实做时发现一刀不够——**每一刀都在原地量过行数**，不是照单执行：

1. **`spawn`/`detach`/`refuse` 从 `dispatch.rs` 搬进 `spawn_tool.rs`**（052 点名的
   那一刀）。搬完 `dispatch.rs` 只回答「这个名字归谁执行」，跟 051 立的
   `status_tool::intercept` 规矩一致。inline `mod tests` 同时挪进
   `spawn_tool_tests.rs`（`#[path]`，043/051 同款处置）。
2. **`reply.rs`**：搬完一量，`spawn_tool.rs` 落在 **298** 行——过是没过，但那是
   天花板不是目标。真正的问题也不是行数：`spawn`/`status`/`collect` 三个截获工具
   各自写一遍「发一条 `ToolExecuted` + 产一条 `ToolResult`/`ToolFailed`」，就是
   三处各自可能漏一半，而漏了**不报错**（只是面板上那次调用永远停在 executing）。
   抽成 `reply::{ok, refuse, settle}`，`is_error` ⟺ `ToolFailed` 这条对应关系也
   只定死一次。三个调用方一起换，`spawn_tool.rs` 落到 282、`status_tool.rs`
   从 288 降到 263。
3. **`child_outcome.rs`**：给 `subtree.rs` 加完 collect 那几个方法它到了 **339**。
   拆的是 `outcome`/`final_text`——`subtree.rs` 管**记账**（谁在等谁、谁还在跑、
   谁跑完没人领），这几个函数管**翻译**（子那边发生的事怎么变成父历史里的一段
   正文）。两件事会往两个方向长：记账跟着编排走，翻译跟着「父想读到什么」走。
   拆完 288。

改动后的全部文件 ≤300（最大的是 `runner.rs` 299，本 issue 没碰它）。

### collect 的两条路：都不是新机械

```rust
if let Some(done) = subtree.take_stashed(&child) { /* 一：当场端走 */ }
if subtree.is_detached(&child) && session.is_live(&child) { /* 二：补一笔，Pending */ }
/* 三：领不了 */
```

**一（stash 命中）**：`take_stashed` 按 `AgentId` 从 stash 里 `remove` 掉那条，
`reply::settle` 当场收敛父的槽（`is_error` 由**子**成没成决定，不由 collect 这次
调用决定）。**领取即消费就是这个 `remove`**——第二次 collect 同一个 id 落到第三条
路上拿 `is_error`，不需要为「领过了」再记一张表。

**二（绑定等待）**：`subtree.record(child, parent, collect_call_id, epoch, COLLECT_TOOL)`
——**跟前台阻塞 spawn 调的是同一个方法、进的是同一张表**，然后返回
`Dispatched::Nothing`（不产出任何事件），于是父那个槽保持 `Pending`、父停在
`ToolsPending`、泵接着把子驱动到终态、`harvest_slots` 回写。ORCHESTRATION §三 那句
「前台 spawn ≡ spawn(bg) 紧跟 collect」因此不是修辞：两条路共用同一张槽位表，
差别只在模型什么时候把那一笔记上。`tests/collect_matches_blocking_spawn.rs` 用
**同一个任务两条路跑、断言回到父历史里的字节完全相同**把这条钉死。

`is_live` 那一半是防死等的：detached 名单上一个已经被撤销/拆掉的子永远落不了终态，
绑上去就是让父等一个不会来的结果，而 `run_turn` 里没有人能救它。

**三（领不了）**：领过了 / 不是 `background=true` 开的 / 已经被撤销或拆掉——
**三种合成一句话**，不分三条。它们在这一刻是真的分不开（`Subtree` 里都表现为
「两张表里都没有」），硬要分就得为「已经领过」再留一张只为措辞而存在的表，
而那张表自己也会跟真相不同步。诚实地把三种可能都列出来，比精确地猜错一种好。

### 红线 10：清单那一半才是危险的一半

`id` 过 `is_descendant_of(caller)`，这是明面上的一道。真正容易漏的是拒绝文本里那句
「你现在能领的是：…」——`Subtree` 的三张表是**全 turn 共享**的（root 的后台子和
`root/a1` 的后台子住在同一张 `detached` 里），照单列出去就是一次横读，而且不会有
任何东西报错。所以那句话走 `mine()`：`collectable()` 再过一次
`is_descendant_of(caller)`，跟 051 `status_tool::descendants` 同一个「由构造保证」
的形状。`a_refusal_never_names_agents_outside_the_callers_subtree` 专门断言兄弟
子树的 id 一个字都不出现在清单里。

`collectable()` 自己 `sort()`：那段文本会进下一轮 prompt（红线 11），而两张表的
插入顺序取决于「谁先落终态」这种运行期时序——不排的话同一个世界能渲染出两种字节。

### 孤儿判据第三条：怎么从「恒真的占位」变成真的在挡人

052 把 `take_orphans` 的判据写成「detached 且 `is_live` 且**没有 collect 绑定**」，
第三条当时恒真。053 接上之后它有了两条真实路径，**都不需要改 `orphan.rs` 一行代码**
（那个文件本次只改了文档）：

- **正常路径**：collect 绑定 → 父那个槽 `Pending` → root 进不了终态 → `reap` 一开头
  就返回。子被领走时结果回到父那里，**并从 detached 名单里划掉**（见下），两张表
  干净，收工那一圈的 `reap` 无事可做。
- **取消路径**：Ctrl-C 把 root 推成 `Failed(Cancelled)`（终态）时 collect 还绑着 →
  第三条判据把这个子挡在 `despawn_child` 之外，它以活着的状态跨过这一轮。
  **这是刻意跟前台 spawn 保持一致**（诚实标注：这不是「更好」，是「一致」）——
  前台 spawn 的子从 029 起就只住在 `slots` 里、从来不在 detached 名单上，`reap`
  一直看不见它们；绑了 collect 的子就是一个前台子，只是记账时刻晚一点。它烧不了
  token：取消 bump 过世代，它接下来每一条回执都撞 epoch 闸。

**实做里发现的一个真 bug（052 留下的接缝）**：绑定期间子**要留在** detached 名单上
（`take_orphans` 的第三条判据要靠它认出「有人在等它」），但收割回写之后就必须划掉
——不划的话，同一次 `harvest` 里紧接着跑的 `harvest_detached` 会看到「终态 + 没人等
（槽刚被上面摘掉）」，把**同一份结果再塞进 stash 一次**，轮末再报一句「跑完没人领」：
领了，还报。修在 `harvest_slots` 里一行 `self.detached.retain(...)`（前台 spawn 的子
从不在这张名单上，对它是空操作）。这条有突变验证，见下。

### 红线 6：校验点在哪、为什么对抗测试是确定性的而不是 e2e 的

| 路径 | 谁把门 | 新的吗 |
|---|---|---|
| collect 绑定的子，落终态时回写父 | `harvest_slots` 产出的事件带 **`slot.epoch`**（= 那次 collect 调用的世代），`Session::step` 入口的 epoch 闸比对 | **没新造**——用的就是前台 spawn 那条路的同一道门 |
| stash 里躺着的结果 | 进 stash 时已经过了 `harvest_detached` 的比对（052 新加的那处）；collect 领它时经 `Event::ToolResult` 再过 `step` 的闸 | 没新造，不给 `Stashed` 加世代字段 |

053 唯一能搞砸的地方是**用错世代**：`epoch: session.epoch()` 而不是 `slot.epoch`，
= 拿「现在的世代」交差、绕过闸。所以对抗测试瞄的就是这一点。

**为什么是确定性单测而不是 e2e**（跟 052 的取舍相反，如实记录理由）：这条断言要求
「世代被推走**之后**子仍能落终态」，而世代一旦推走，所有带 epoch 的事件（provider
回执、MCP 回执、远端回传）全部撞闸——子再也进不了终态，e2e 里根本走不到那次回写。
唯一不带 epoch 的事件是 `UserInput` 和 `Cancel`，而 `Cancel` 自己就 bump 世代。
硬凑一个 e2e 只会得到一条「因为别的原因而绿」的测试。

确定性版本（`collect_tool_tests.rs`）用**诱饵子 agent**解决同一个问题：

```
1. root 一条真的 ToolUse 进转移表 → ToolsPending，槽位是 core 记的（不是给字段赋值）
2. 后台子 child 还在跑；另有一个诱饵子 decoy 也在跑
3. subtree.detach(child) + subtree.record(child, ..., COLLECT_TOOL)  ← collect 绑定
4. session.step(Cancel{decoy}) → 世代 0→1。**root 和 child 一个字节没变**
5. child 落终态 → subtree.harvest() → 产出一条 ToolResult{epoch:0}
6. session.step(那条事件) → 闸 0 != 1 → 返回空 effect、什么都没写
```

取消**诱饵**而不是取消 root 或 child 是这条测试的关键：取消 root 会顺手清空它的
槽位、取消 child 会改掉它的终态，两者都会让「结果没落地」有第二种解释。取消诱饵
只动世代，于是没落地就**只可能**是闸干的。孪生对照（`and_the_very_same_write_back_
lands_when_the_epoch_still_matches`）同一份夹具、同一次收割，只是不推世代，结果就
老老实实落进父的历史、父的槽收敛。

#### 突变验证（真实红/绿输出）

**突变一**：`subtree.rs` 的 `harvest_slots` 里两处 `epoch: slot.epoch,` →
`epoch: session.epoch(),`（= 用现在的世代交差、绕过闸）。

```
$ cargo test -p agent-runtime --lib collect_tool
test collect_tool::tests::a_collect_binding_write_back_is_dropped_when_the_epoch_moved_on ... FAILED

thread '...a_collect_binding_write_back_is_dropped_when_the_epoch_moved_on' panicked at
crates/agent-runtime/src/collect_tool_tests.rs:115:5:
assertion `left == right` failed: 回写该带绑定那一刻的世代，不是现在的
  left: Some(Epoch(1))
 right: Some(Epoch(0))

test result: FAILED. 6 passed; 1 failed; 0 ignored; 0 measured; 76 filtered out
```

把那条结构断言也临时删掉、只留行为断言（证明红不是只红在一句 `assert_eq!` 上）：

```
thread '...a_collect_binding_write_back_is_dropped_when_the_epoch_moved_on' panicked at
crates/agent-runtime/src/collect_tool_tests.rs:117:5:
过期世代的回写该被闸整条丢掉（红线 6）

test result: FAILED. 6 passed; 1 failed; 0 ignored; 0 measured; 76 filtered out
```

改回原样：

```
test collect_tool::tests::a_collect_binding_write_back_is_dropped_when_the_epoch_moved_on ... ok
test collect_tool::tests::and_the_very_same_write_back_lands_when_the_epoch_still_matches ... ok
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 76 filtered out; finished in 0.00s
```

**突变二**（上面那个 052 接缝 bug）：删掉 `harvest_slots` 里那行
`self.detached.retain(|entry| entry.child != slot.child);`。

```
$ cargo test -p agent-runtime --lib collect_tool
test collect_tool::tests::and_the_very_same_write_back_lands_when_the_epoch_still_matches ... FAILED
thread '...' panicked at crates/agent-runtime/src/collect_tool_tests.rs:137:5:
领到的结果不该再进一次 stash（轮末会误报「没人领」）

$ cargo test -p agent-runtime --test collect_three_out_of_order
test three_background_children_are_collected_fastest_first_and_nothing_is_left_to_reap ... FAILED
thread '...' panicked at crates/agent-runtime/tests/collect_three_out_of_order.rs:113:5:
全部领完就不该有孤儿、也不该有「跑完没人领」——`take_orphans` 的第三条判据这一下才真的在挡人：[
    "后台子 agent root/a3 已经干完了，但这一轮结束前没有人领它的结果（15 字节），结果被丢弃。",
    "后台子 agent root/a1 已经干完了，但这一轮结束前没有人领它的结果（13 字节），结果被丢弃。",
]
```

改回原样后两条都绿（见下面的完整验证）。

### 决策 20 兼容 / 前台无回归

`background=false` 那条路在本次重构里**只换了住址，没换一个字节的行为**：解析、
子集校验、`spawn_child`、`persist::sync`、`subtree.record` 顺序原样，唯一的差别是
`refuse` 现在叫 `reply::refuse`（同样是「通报 + `ToolFailed`」）。九个
`spawn_indep_*` 与四个 `spawn_bg_*` 独测全绿，是操作证据。

### 没做（照 issue §不做 与硬约束）

跨 turn 后台子、per-child cancel（ORCHESTRATION §六 明确延后）。
**`agent-server` 没接 `with_collect`**——硬约束点名不碰那个 crate（并发会话 WIP），
它归 054（面板呈现 bg/collect + 真机 dogfood）一次做完。CLI 这边已经接上，
`cargo run -p agent-cli` 就能跑完整闭环。

### 验证（前台跑完，真实输出）

```
$ cargo test -p agent-runtime -p agent-core
exit=0
binaries=102 passed=557 failed=0 ignored=0

tests/collect_child_failure.rs             test result: ok. 4 passed; 0 failed; ... 0.26s
tests/collect_matches_blocking_spawn.rs    test result: ok. 5 passed; 0 failed; ... 0.32s
tests/collect_refusals.rs                  test result: ok. 4 passed; 0 failed; ... 0.27s
tests/collect_three_out_of_order.rs        test result: ok. 4 passed; 0 failed; ... 0.42s
tests/collect_waits_for_a_running_child.rs test result: ok. 4 passed; 0 failed; ... 0.42s
tests/spawn_bg_epoch_writeback.rs          test result: ok. 5 passed; 0 failed; ... 0.01s
tests/spawn_bg_orphan_reaped.rs            test result: ok. 4 passed; 0 failed; ... 0.92s
tests/spawn_bg_tail_cut.rs                 test result: ok. 5 passed; 0 failed; ... 1.51s
tests/spawn_bg_two_children_no_block.rs    test result: ok. 4 passed; 0 failed; ... 0.71s
tests/spawn_indep_cancel_tree.rs           test result: ok. 4 passed; 0 failed; ... 0.75s
tests/spawn_indep_depth_chain.rs           test result: ok. 4 passed; 0 failed; ... 0.01s
tests/spawn_indep_failure_propagation.rs   test result: ok. 4 passed; 0 failed; ... 0.01s
tests/spawn_indep_no_ghost_events.rs       test result: ok. 4 passed; 0 failed; ... 0.52s
tests/spawn_indep_parallel3.rs             test result: ok. 4 passed; 0 failed; ... 0.38s
tests/spawn_indep_privilege_refusal.rs     test result: ok. 4 passed; 0 failed; ... 0.01s
tests/spawn_indep_sibling_prefix.rs        test result: ok. 4 passed; 0 failed; ... 0.01s
tests/spawn_indep_undeclared_tool.rs       test result: ok. 4 passed; 0 failed; ... 0.00s
tests/spawn_indep_undo_subtree.rs          test result: ok. 4 passed; 0 failed; ... 0.01s

$ cargo clippy -p agent-runtime --all-targets -- -D warnings
    Checking agent-runtime v0.1.0 (/Volumes/work/self/einfach-agent-rust/crates/agent-runtime)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.53s
exit=0

$ bash scripts/check-invariants.sh --all
红线检查通过
规则与理由：docs/INVARIANTS.md
exit=0

$ cargo check --workspace --all-targets   # 顺带确认没碰坏 server/cli/desktop
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 15.36s
exit=0
```

052 那一轮是 528 个测试，本轮 557（+29）。
