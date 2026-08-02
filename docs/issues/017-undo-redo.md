# 017 undo / redo 的两层粒度

**里程碑** M2 · **依赖** 009 · **模型** opus · **独立测试 agent** ✅ · **状态** 完成

## 目标

在 009 的日志之上实现两层粒度的回退与前进。

## 做什么

一条扁平日志、一个游标。**undo 就是弹栈顶**——日志按时间排序，弹掉的是最近发生的
那一步，不管哪个 agent 干的。

- `undo(turn)` —— 从栈顶弹到 `turn_id` 变化处（UI 默认粒度）
- `undo(batch)` —— 弹一条（开发者模式的可展开时间线）
- `redo` —— 反向重放 `next`

`turn_id` 由 root agent 分配，子 agent 的 entry 继承所在 root turn 的值，**不产生新的
turn 边界**。于是 `undo(turn)` 一次退回一整个 root turn，连带那轮里所有子 agent 的工作。

## 为什么不做选择性 undo

「只回滚某个 agent 的条目」= 跳过日志中间的条目，而中间条目的 `prev` 是在当时的世界
状态下捕获的，跳着回滚就不成立。那是另一个量级的问题，本仓不做（ROADMAP 决策 4）。

## 验收

- undo → redo 往返后，**所有 primitive 逐值相等，所有 derived 重算一致**
- 连续多次 undo 跨越多个 turn 边界，每次都停在正确的位置
- 新写入时游标不在栈顶 → redo 尾被丢弃（默认覆盖，分支是显式操作）
- 撞上 `Reversibility::Irreversible` 的 entry → 停下并推 `undo_blocked`，不越过

## 注意

**红线 4**：日志里的键是 `AtomKey` 不是 `AtomId`。`AtomId` 是自增 u64，
往构图函数中间插一行 `create_atom` 就会让所有旧记录静默错位。

**红线 6**：undo 时要 bump session epoch，否则在飞的 effect 回来会写进一个已经被
回滚掉的世界。

## 实做记录（实现 agent，2026-08-01）

### 落地的文件

| 文件 | 行数 | 职责 |
|------|------|------|
| `crates/agent-store/src/history/cursor.rs` | 300 | 游标：两层粒度的 undo/redo，产出 `UndoOutcome`。**对 store 一无所知** |
| `crates/agent-store/src/history/apply_roundtrip.rs` | 250 | 仅测试：全链路（图 → `record_set` → `undo_turn` → applier → `redo_turn`） |
| `crates/agent-store/src/history/log.rs` | 254（+55） | 加 `cursor` 字段；`append` 的覆盖语义；两个 append 自测 |
| `crates/agent-store/src/history/record.rs` | 248（+9） | `record_set` 补上 `#[must_use]`（009 挂的账） |
| `crates/agent-store/src/history.rs` | 45 | 门面：多一行模块表，说明「History 不碰 store」这条分界 |

`lib.rs` 多 re-export 一个 `UndoOutcome`。**没有动 store/ 下任何文件**，也没有动
`log.rs` / `record.rs` 已有函数的签名（只有 `append` 按验收更新了语义、`History` 的
私有字段改成 `pub(super)` 让同目录的 `cursor` 能动游标）。

### 设计判断

1. **undo 不物理弹条目，只挪游标**。「undo 就是弹栈顶」弹的是「这一条还算不算数」，
   不是把它从 `Vec` 里 pop 掉 —— 真 pop 了就没有 redo 可言。于是游标 = 已生效条数，
   `[0, cursor)` 是当前世界，`[cursor, len)` 是能 redo 回来的尾巴。
2. **游标字段放在 `log.rs` 的 `History` 里，而不是 cursor.rs 造一个包装类型**。
   `append` 必须看得见游标（丢 redo 尾是 append 的动作），而 append 属于 log.rs。
   包装类型会让「日志」和「日志上的位置」变成两个可以不同步的对象。
3. **`cursor.rs` 保持对 store 一无所知**，和 `log.rs` 同一条纪律：这个文件里没有
   `AtomId`、没有 `Store`。于是 undo/redo 的全部逻辑不需要一个 store 就能测，
   红线 4 在日志这一侧仍然是结构事实而不是纪律。代价是「产物怎么落回状态」得有个地方
   演示 —— 那就是 `apply_roundtrip.rs`（`#[cfg(test)]`，唯一同时看见 `Store` 和日志的
   文件）。放进 cursor.rs 会把两件事糊在一个文件里，也会顶破 300 行。
4. **`UndoOutcome` 返回克隆件而不是引用**。undo 要 `&mut self`，借出 `&Entry` 会让调用方
   在整个应用期间碰不了这份日志 —— 而 applier 往往要接着记录（019 的按需重建就在那一步）。
   钉死的签名要的也是克隆。
5. **`undo_turn` 的第一条无条件取**，之后才拿它当基准问 `same_turn`。这让判据退化时
   语义仍然正确：`|_, _| false`（每条自成一个 turn）= `undo_one`，`|_, _| true` = 一路
   到底。若改成「第一条也要 `same_turn(m, m)` 为真」，一个不自反的判据会让 `undo_turn`
   什么都不做，那是个没人想要的静默行为。
6. **屏障检查发生在「即将跨过这一条」之前，屏障条目自己不进 `applied`**。`Blocked` 之后
   游标停在屏障后一格 —— 屏障没被越过。History **不记「这条已经问过了」**：用户确认
   「继续、副作用不回滚」在上层表达为「换一个放行这一条的谓词再调一次」，于是「越过」
   永远是一次显式决定，不会因为某个状态位而在下一次 undo 里被静默继承。
7. **`Blocked` 也带 `applied`，而不是全有或全无**。撞屏障时比屏障新的那些条目已经该回滚
   （它们的 `prev` 链在屏障之上，自洽），退回去问用户期间把它们留在新值上，等于状态处于
   一个日志里不存在的中间态。全有或全无还要求先扫一遍再动手，那是两趟。
8. **turn 边界上的屏障不看**。`undo_turn` 停在 turn 边界时返回 `Applied` 而不是 `Blocked`，
   即使边界外那一条是 `Irreversible` —— 我们压根没打算退它，没有「越过」发生。
9. **`append` 的空 `changes` 早退发生在丢弃 redo 尾之前**。什么都没写就不该毁掉 redo 尾；
   009 的「幽灵步不落条目」在这里长出第二条推论。
10. **被丢弃条目的 `seq` 不回收**（`next_seq` 只增不减）。「seq 5 那一步」在整条会话生命
    周期里必须指同一步，否则落盘日志和审计回放对不上。游标是 `entries` 的下标计数、
    seq 是铸出来的号，两者从这里开始正式分家。
11. **`record_set` 补 `#[must_use]`**（009 裁决推到本 issue）。丢弃返回值 = 值写进了 store
    却没进日志，正是红线 2 要挡的洞，而 `record_set(..);` 一行长得像普通写入语句，
    编译器是唯一能出声的人。仓内现有调用方（含 009 独测留下的 4 个集成测试文件）无一
    裸调用，`-D warnings` 下零新增警告。
12. **`UndoOutcome` 没有加 `#[must_use]`**，尽管它同样丢不得。理由与 009 当时一致：本
    issue 有并行的独立测试 agent，他们被告知的只有 `record_set` 那一笔；`h.undo_one(..);`
    这种「只想挪游标」的写法在 `-D warnings` 下会当场炸掉他们的构建。**建议合并时补上** ——
    已核对他们落地的 6 个测试文件里没有一处裸调用（产物一律 `match` 或按值传给 applier），
    现在加是零破坏，但那是合并时的决定，不是并行期的。
13. **`UndoOutcome` 不 derive serde**。它是进程内的产物，不落盘；`Entry` 那一侧已经可
    序列化，够 010 用。

### 推给别人的

- **红线 6（undo 时 bump session epoch）** 是集成层的事 —— History 对 epoch 不可见
  （epoch 是 `M` 里的一个字段，本 crate 连这个词都不认识）。推给「状态搬进原子图」issue：
  在 applier 那一层，写回 `prev` 之前先 bump epoch，在飞的 effect 回来才会被拒。
- **`undo_blocked` 事件**（验收原文的「推事件」）也在集成层：`Blocked { barrier_seq }`
  是 History 能给出的全部，谁来推、推给谁归 loop / server。
- **丢了几条 redo 尾要不要通知谁** 归 018（这里只做丢）。
- **已 evict atom 的重建** 归 019：`apply_roundtrip.rs` 里的 `resolve` 现在是个 `match`，
  真实实现是 `AtomFamily` 的按键查找，找不到时的重建策略在那个 issue。

### 自测（13 个新单测，全部内联/同目录，没建 `tests/` 新文件）

`cursor.rs` 8 个：新日志游标在栈顶、undo_one/redo_one 一进一出且条目不被物理弹掉、
两端返回 `Nothing`、屏障就在游标下时游标一动不动且幂等、`undo_turn` 逐个 turn 走并停在
边界、中途撞屏障保留已弹出的部分且能 redo 回去、`redo_turn` 恰好反演、退化判据
（恒假 = 一条、恒真 = 到底）。
`log.rs` 2 个：游标不在栈顶时 append 丢 redo 尾且 seq 不回收、空步不毁 redo 尾。
`apply_roundtrip.rs` 3 个：**undo→redo 往返后所有 primitive 逐值相等、derived 重算一致**
（验收第一条，且断言 `debug_recompute_count` 确实涨了 —— 是重算不是缓存）、
一次 batch 里同一 atom 写两次必须倒序回滚（正序回滚会停在中间值，测试把这个反例也
断言了）、撞屏障时只应用弹出的那部分且状态自洽。

### 验收结果

- `cargo test -p agent-store`：93 全绿 = 28 unit（15 + 本次 13）+ 64 集成 + 1 doctest。
  其中 `tests/undo_redo_*.rs` 6 个文件 10 个测试是**独立测试 agent 的**，落地时零改动
  一次通过 —— 屏障门口游标不动、`undo_turn` 停在屏障后一格、连续 undo 的每个 turn 边界、
  `redo_turn` 无 barrier 参数，逐条与本实现对上，双侧零分歧。
- `cargo test --workspace`：515 passed / 0 failed（本次 +13；009 记录里的 492 是当时的数）
- `cargo clippy --workspace --all-targets -- -D warnings`：0 警告
- `bash scripts/check-invariants.sh --all`：红线检查通过
- 行数：`cursor.rs` 300 / `apply_roundtrip.rs` 250 / `log.rs` 254 / `record.rs` 248 / `history.rs` 45

### 合并记录（主会话）

双侧零分歧：独测 10 测试对并行实现一次全过（屏障两态、prev 链倒序、redo 无屏障
全部吻合）。workspace 515/0。合并时补 UndoOutcome 的 #[must_use]（实现方推的账，
已核对无裸调用）。红线 6（undo bump epoch）与 undo_blocked 事件按实做记录推给
「状态搬进原子图」集成 issue——History 对 epoch 不可见是对的，它就该只管日志。
