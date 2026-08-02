# 019 applier 对已 evict atom 的按需重建

**里程碑** M2 · **依赖** 017 · **模型** opus · **独立测试 agent** ✅ · **状态** 完成

## 目标

undo / redo 遇到**不存在的 atom** 时按需重建（默认值 create，再灌 `prev`）。

## 为什么必须有

子 agent 是短命的，一个 root 会话可能 spawn 上百个。atom 不回收就是泄漏；
但结束后 evict 掉它的 atom，用户再 undo 回它运行中的那一刻，目标就没了。

这是 `AtomKey` 是逻辑键而非 `AtomId` 带来的红利——逻辑键在，atom 就能重建。
上游 TS 的 applier 里 `resolve(op.scope)` 就是 family 的 get-or-create，已经这么干。

## 做什么

applier 拿到 `(AtomKey, prev, next)` 时：

1. 按 key 查 atom，存在就正常写
2. **不存在就按 key 的 Slot 类型创建一个默认值的 atom**，再写
3. 重建出来的 atom 要正确接进依赖图（它的下游 derived 要能重新订阅上）

## 验收

- evict 一个子 agent 的全部 atom，然后 undo 回它运行中的时刻——**状态完全恢复**
- 重建后该 atom 的下游 derived 能正确重算，不是停在旧值
- 重建走的是与正常创建同一条路径，不是一个特判分支

## 注意

**这是最容易漏的一条**：不写这段，代码在没有 evict 的测试里全绿，
只有真的跑长会话 + evict + undo 才炸——而那三个条件同时出现通常是在线上。

红线 4：整条链的前提是键必须是 `AtomKey`。如果哪里退化成了 `AtomId`，
重建就无从谈起（拿不到 Slot 类型，不知道该建什么）。

## 实做记录（实现 agent，2026-08-01）

### 落地的文件

| 文件 | 行数 | 职责 |
|------|------|------|
| `crates/agent-store/src/history/apply.rs` | 300 | applier：`apply_prev` / `apply_next`，缺席的 atom 由调用方的 get-or-create `resolve` 重建 |
| `crates/agent-store/src/history.rs` | 51（+3） | 门面：模块表一行、`mod apply`、`pub use apply::{apply_next, apply_prev}` |
| `crates/agent-store/src/lib.rs` | 28（+3） | 两个 re-export（`agent_store::apply_prev` 与 `agent_store::history::apply_prev` 都通） |

没有动 `store/` 下任何文件，也没有动 `log.rs` / `cursor.rs` / `record.rs` 的现有内容
（018 在并行改 `log.rs` / 新建 `cap.rs`，零重叠）。钉死的两个签名原样落地，一字未改。

### 设计判断

1. **重建长在 `resolve` 上，不长在 applier 里**。applier 若自己判「atom 还在不在」，
   就得知道「不在时按什么类型建、初值是什么」——那是上层 Slot 表的知识。塞进 applier
   等于在引擎里复刻一份上层 schema，而且是**只有 undo 路径才会走到**的那一份：它会和
   正常创建路径长期失同步，症状正是本 issue「注意」里说的那种线上才炸的静默错值。
   结构性后果：`apply.rs` 的实现区（非测试）**一个 `if` 都没有**，也没有 `has_atom`。
   自测里 applier 的 `resolve` 与 command 层写槽位、derived 现查槽位用的是**同一个函数**
   `slot()` ——「同一条创建路径」是字面意义上的同一行代码，不是纪律。
2. **整批包在一个 `store.batch` 里**。不批就是每写一个 primitive 冲一次：下游 derived
   在「一半旧一半新」的世界上重算若干次，而且那时后面的槽位可能还没被 `resolve` 建回来。
   批到最后一次 flush，下游只重算一次、且在全部值就位与全部缺席 atom 重建之后——
   自测断言了 `debug_recompute_count` 的增量**恰好是 1**。
3. **条目内方向由 applier 负责**（017 的顺序契约）：`apply_prev` 走 `.iter().rev()` 写
   `prev`，`apply_next` 走 `.iter()` 写 `next`。一次 batch 里同一槽位写两次时只有倒序
   才回得到最初值，自测两个方向各断言了一次。
4. **`resolve` 不返回「重建了什么」**。集成层想上报「本次 undo 重建了 N 个 atom」就在
   自己的闭包里数——闭包是天然的观察点，applier 不需要多一条返回通道。

### derived 重连语义的真相（本 issue 最重要的产出）

验收第二条「重建后下游 derived 正确重算，不是停在旧值」在本 store 里的**真实形状**与
issue 原文的直觉不同，逐条如实记下：

1. **「旧 derived 停在旧值」这个担心在本 store 里不成立——因为那个 derived 根本活不到
   重建那一刻。** `AtomFamily::evict` 在 `store.has_dependents(id) || has_subscribers(id)`
   时**拒绝**（返回 false、什么都不改），`Store::destroy_atom` 在还有反向边时**直接
   panic**。于是「逐出一个子图」必须**自叶向根**（拓扑序：先下游后上游），存量代码把
   这条写死在引擎里，不是约定。自测 `a_slot_the_derived_still_reads_cannot_be_evicted_at_all`
   钉的就是这个。
2. **一个槽位什么时候变得可逐出，由「谁还在读它」决定，而这条边的丢弃是 derived 重算的
   副产品**：derived 重算后不再 `get` 某个 atom 时，`commit_read` 的 dep diff 会摘掉那条
   反向边。所以真实形态是「子 agent 从活名单里移除 → 汇聚 derived 重算 → 边没了 →
   它的槽位这才可逐出」。**逐出策略必须由状态驱动，不能由外部计时器随手 evict**——
   计时器版本会撞上 `evict` 返回 false（好），或在别处 `destroy_atom` 上 panic（坏）。
3. **能重连的 derived 只有一种：read fn 按逻辑键现查 family 的那种。** 捕获了 `AtomId`
   的 derived 在依赖被逐出后是**当场 panic**（`read_dep` 的 `atom {id} not found in
   store`），不是静默错值——因为 `AtomId` 单调递增、槽位不复用，死 id 不会重指到别人身上。
   自测 `a_derived_that_captured_an_atom_id_does_not_reconnect` 是 `#[should_panic]`。
   **这是红线 4 在进程内的孪生条款**：红线 4 管「落盘的键不能是 `AtomId`」，这条管
   「derived 闭包里也不能焊 `AtomId`」。两条的理由是同一个——`AtomId` 是句柄不是身份。
   → 「状态搬进原子图」时，agent 侧所有汇聚型 derived（`turn.pending`、总 token 数、
   「等所有子 agent 完成」）必须以 `AtomKey` 为唯一寻址方式，family 现查。
4. **重连发生在 applier 那一次 flush 的重算里**：applier 在 batch 内把缺席的 atom 按键
   建回来（新 `AtomId`），flush 时下游 derived 重算 → 按同一个键从 family 拿到**新 id**
   → `args.get` → `commit_read` 重新装上反向边。自测断言了三件事：值恢复（12 / 4）、
   derived 重算到 16（不是停在 0）、`has_dependents(新 id)` 为真且再写一次能传播到下游
   ——「接回图里」是活边，不只是这一轮算对了。
5. **重建保证 atom 回来，不保证值回来。** applier 只写被 undo 的那些条目携带的值；
   **逐出本身不产生 `Change`**。所以「undo 回子 agent 运行中的那一刻」要真的拿回活值，
   前提是逐出前那条 teardown command 把活值记成了 `prev`（自测就是这么摆的：清零槽位
   + 移出名单是一条 command，`prev` 当场捕获到 12 / 4，然后才 evict）。
   若 teardown 不记日志、直接 evict，undo 拿回的是**默认值**——链是通的、值是错的，
   而且不报错。这条得进「状态搬进原子图」的 despawn 命令设计里。
6. 顺带的确权：`Inner::is_fresh` 对已销毁的依赖返回 false（`self.has(*dep) &&` 那一段），
   `dependencies_change` 与 `flush_pending` 都跳过不存在的 id。也就是说 store 对「依赖
   没了」是**安静地判不新鲜**，真正出声的是重算时的 `read_dep`——这正好使第 3 条成立。

### 自测（5 个，全部内联在 `apply.rs`，没建 `tests/` 文件）

形状：root 的活子 agent 名单（primitive）+ 一个「名单里所有子 agent 的 tokens + steps
之和」的 derived（按逻辑键现查槽位）+ 子 agent a1 的两个槽位（family 建）。

1. `an_evicted_subgraph_is_rebuilt_by_undo_and_the_derived_recomputes`（**验收 1 + 2**）：
   spawn → 干活 → teardown（记 prev）+ 逐出 a1 的**全部** atom（断言 `has_atom` 为假）
   → `undo_turn` + `apply_prev`（resolve = get-or-create）→ 新 id ≠ 旧 id、值 12 / 4
   完全恢复、derived 重算到 16、`has_dependents` 为真、再写一次能传播、重算恰好一次。
2. `apply_next_rebuilds_what_redo_needs_too`：undo 回子 agent 出生之前 → 把子图整个逐出
   → `redo_turn` + `apply_next` 同样把它建回来并灌 `next`。
3. `a_slot_the_derived_still_reads_cannot_be_evicted_at_all`：上面第 1 条真相。
4. `a_derived_that_captured_an_atom_id_does_not_reconnect`（`#[should_panic]`）：上面第 3 条。
5. `changes_inside_one_entry_unwind_in_reverse_and_redo_forward`：顺序契约两个方向。

### 验收结果

- `cargo test -p agent-store`：113 passed / 0 failed（lib 41 含本次 5 个 + 集成 + 1 doctest）
- `cargo test --workspace`：569 passed / 0 failed
  （018 / 020 在同一个工作树并行落地，总数是移动靶：本次收工前后测过两轮，
  545→569 全绿，本 issue 新增的是 `history::apply::tests` 那 5 个）
- `cargo clippy --workspace --all-targets -- -D warnings`：0 警告
- `bash scripts/check-invariants.sh --all`：红线检查通过（exit 0）
- 行数：`apply.rs` 300 / `history.rs` 51 / `lib.rs` 28
- 结构性验收：`apply.rs` 实现区 grep 不到 `if` / `has_atom` / `contains` / `is_none`
  ——唯一命中「if」的是那句「一个 `if` 都没有」的注释

### 推给别人的

- **teardown 必须记日志**（上面第 5 条）→ 「状态搬进原子图」的 despawn 命令设计。
- **汇聚型 derived 一律 family 现查、禁止闭包里焊 `AtomId`**（第 3 条）→ 同上。
  值得考虑把它加进 `scripts/check-invariants.sh`：`create_derived*` 的闭包里出现
  `move` 捕获的 `AtomId` 变量难以 grep，可能只能靠 review 或 skill。
- **`resolve` 闭包里不要读 store**：本测试把 family 放在 `Rc<RefCell<_>>` 里，applier 的
  `resolve`（batch 内）和 derived 的现查（flush 时）借的是同一个 `RefCell`，两者天然不
  重入；但若集成层在 `resolve` 里顺手 `store.get(某个 derived)`，那个 derived 又要借
  family，就会 `already mutably borrowed` panic。这是运行时借用的老问题，记一笔。

### 合并记录（主会话）

apply_prev 本体 8 行、全文件零 if——「重建走正常路径」的结构性验收字面达成。
双侧独立测出同一组引擎真相：逐出必须自叶向根且状态驱动；捕获 AtomId 的 derived
在依赖逐出后当场 panic（幸而不是静默错值），只有按逻辑键现查 family 的能重连
——已升格进红线 4 的孪生条款；重建保证 atom 不保证值，despawn 的 teardown 必须
把活值记成 prev——已记进 STATE-MODEL，「状态搬进原子图」issue 的硬输入。