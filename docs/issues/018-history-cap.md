# 018 日志上限与分支覆盖

**里程碑** M2 · **依赖** 017 · **模型** sonnet · **独立测试 agent** 否 · **状态** 完成

## 目标

日志不能无限增长。cap 默认 100 条，溢出从最老一端丢。

## 为什么事务日志能截断而快照式不能

每条 entry 自带完整逆操作，所以丢掉最老的**不影响剩余条目回滚**。
快照式必须回溯扫描前序历史才能找到某个 atom 的上一个值，截断即永久丢失。

这是选事务日志的三个理由之一（另两个：键是字符串所以可序列化；一次 undo 是
O(本条 ops 数) 而非 O(历史长度)）。

## 做什么

- cap 默认 100，可配置
- 溢出时从最老一端丢，并通知 `SessionStore::drop_oldest`
- 游标不在栈顶时写入新 entry → 丢弃下标 ≥ cursor 的条目，通知 `drop_after`

## 验收

- 写满 150 条后仍能连续 undo 100 次，第 101 次明确报「已到日志尽头」而不是 panic
- 溢出后剩余条目的回滚结果与未溢出时逐值相同
- redo 尾被覆盖后不能再 redo 到被丢弃的分支

## 注意

cap 与压缩策略有冲突，这是 ROADMAP 第四节记的未决问题之一：`ExtensionOnly` 的
provider 上压缩损失 100%，被迫压得又晚又狠，单次 `prev` 特别大，cap 100 条下能 undo
回去的窗口就特别短。**本 issue 不解决它**，只把 cap 做对；取舍到 M3 定。

## 实做记录（实现 agent，2026-08-01）

### 落地的文件

| 文件 | 行数 | 职责 |
|------|------|------|
| `crates/agent-store/src/history/cap.rs`（新建） | 292 | 日志上限：`set_cap` / `take_drop_events` / `DropEvent` / `enforce_cap` |
| `crates/agent-store/src/history/log.rs` | 275（+21） | `History` 加 `cap` / `drop_events` 两个字段；`append` 在丢弃 redo 尾时入队 `DropEvent::RedoTail`，追加完毕后调 `enforce_cap` |
| `crates/agent-store/src/history.rs` | 51 | 门面：注册 `mod cap`，`pub use cap::DropEvent` |
| `crates/agent-store/src/lib.rs` | 28 | re-export 多一个 `DropEvent` |

没有动 `cursor.rs` / `record.rs` / `apply_roundtrip.rs`，也没有动任何已有函数的签名——
`append` 的参数和返回值不变，只在函数体内多了两步记账。019（并行落地的
`history/apply.rs`）与本 issue 互不相关，两边各自往 `history.rs` / `lib.rs` 这两个共享
门面文件追加了自己的一行，合并时零冲突。

### 钉死的公开 API：字面落地

```rust
pub enum DropEvent {
    Oldest { count: usize },
    RedoTail { first_seq: u64, count: usize },
}
impl<K, V, M> History<K, V, M> {
    pub fn set_cap(&mut self, cap: Option<usize>);
    pub fn take_drop_events(&mut self) -> Vec<DropEvent>;
}
```

一字不差；`enforce_cap` 是第三个方法，`pub(super)`，只给 `log.rs` 的 `append` 内部调用，
不对外。`History::new()` 的 `cap` 仍是 `None`——**不**在这里硬编码「默认 100」，理由见
下面第 1 条判断。

### 设计判断

1. **`History` 自己不带「默认 100」，`cap` 字段默认 `None`**。issue 原文「cap 默认 100」
   是会话层的策略，不是日志结构的常量：`History` 对「一个会话该有多大」和对
   `AtomId`、`turn_id` 一样一无所知。真正的默认值由建 `History` 的调用方在会话层
   显式调一次 `set_cap(Some(100))` 落地——这也是「与现状兼容」的字面意思：017 落地时
   已经有调用方在用不设 cap 的 `History`（本 crate 自己的测试就是），`History::new()`
   突然自带上限会让他们的日志在没人要求的情况下开始被裁。
2. **cap 溢出裁剪只吃 `[0, cursor)` 的已生效区，绝不动 redo 尾 `[cursor, len)`**——这是
   本 issue 唯一需要裁决的地方（cap 与 undo 交互）。理由：已生效的旧条目丢了，世界不变
   （游标同步左移，`cursor` 之后一切照旧）；redo 尾是用户明确 undo 出来、还没决定要不要
   走回去的**未来分支**，被 cap 静默吃掉和被新写入显式覆盖不是一回事——后者是「打了
   新字，旧分支不要了」的显式动作（017 的 `append` 行为），前者只是「日志太长该瘦身
   了」，两件事的因果链不一样，不该共用一条「丢弃」的理由。代价：如果 redo 尾本身就比
   cap 还长，裁剪之后 `len()` 仍可能超过 cap——这是故意的，不是漏洞；等用户 redo 回顶或
   者打字覆盖掉这段 redo 尾（无论哪种，游标都会回到顶），下一次 `append` 触发的裁剪会把
   欠的账一次性补上（`cap_shrunk_mid_undo_only_evicts_the_effective_region_and_spares_the_redo_tail`
   测试把这整条链路走了一遍：先证明降 cap 时只丢 3/4 条并保住 redo 尾，再证明 redo 尾
   原样能 redo 回去，最后证明回到顶之后再写一步会把剩下的 2 条一次吃掉）。
3. **`enforce_cap` 只在两处触发：`append` 结束时、`set_cap` 调用时**。undo/redo 只挪
   游标不改变 `entries.len()`，天然不需要触发；这也是判断 2 成立的结构性原因——`append`
   触发时 `cursor` 总是等于 `entries.len()`（因为 `append` 自己在 truncate + push 之后
   把游标顶到栈顶），所以「只吃 `[0, cursor)`」在 `append` 路径上从不构成限制，真正会被
   限制住的只有「游标停在中位时调 `set_cap`」这一种情况——也正是 018 要我裁决的那种。
4. **`Oldest` 事件不带 `first_seq`，只带 `count`**——完全对齐
   `docs/STATE-MODEL.md` 里 `SessionStore::drop_oldest(&self, id, count: usize)` 的
   签名，宿主直接把 `count` 转发过去，不需要反查 seq。`RedoTail` 带 `first_seq` 是因为
   它记的是「被丢的第一条是谁」，给 UI 提示或审计用；它和 `SessionStore::drop_after`
   的 `cursor: usize` 参数不是同一个数——把 `first_seq` 换算成持久化那边的游标位置是
   011（`SessionStore` 端口）的事，018 只给 History 能给出的那部分。
5. **`RedoTail` 事件的入队点在 `append` 里，用 `self.entries.get(self.cursor)` 判断
   「有没有东西可丢」**，而不是比较长度——`Some` 就说明 `[cursor, len)` 非空，`first_seq`
   直接从这条条目上取，比「算出长度再回头找第一条的 seq」更直接，也避免了长度为 0
   和「本来就没有 redo 尾」两种情况混在一起判断。
6. **`take_drop_events` 没有标 `#[must_use]`**——和 `record_set`/`UndoOutcome` 不同，
   不取走 `drop_events` 不会造成状态自相矛盾（cap 的裁剪已经真实发生，只是没人被通知），
   顶多是宿主没转发通知、`Vec` 会一直长。这是调用方的责任（模块文档写了：「History 不
   做 IO 不发通知，只记账，红线 7」），不是本文件要用编译器兜底的洞。
7. **`enforce_cap` 不要求 `K/V/M: Clone`**，比 `cursor.rs` 的 impl 块更宽——裁剪只是
   `Vec::drain`，不需要克隆任何条目。`set_cap` / `take_drop_events` 因此对任何
   `History<K, V, M>` 都可用，不用等调用方的类型满足 `Clone`。

### 验收结果（逐条对应 issue 原文）

- 「写满 150、cap=100：len==100、连续 undo 100 次成功、第 101 次 `Nothing` 不
  panic」—— `overflow_drops_from_the_oldest_end_and_caps_len_at_cap` +
  `one_hundred_undos_succeed_then_the_101st_is_nothing_not_a_panic`。
- 「溢出后剩余条目的回滚结果与未溢出时逐值相同」——
  `surviving_entries_undo_to_the_same_values_whether_or_not_the_log_ever_overflowed`：
  两份平行 History（一份 cap=100 会溢出、一份不设 cap）写同一批命令，capped 那份撤到底
  拿到的 100 个 `prev` 值，逐个等于 uncapped 那份撤 100 步拿到的后 100 个。
- 「redo 尾被覆盖后报 `DropEvent::RedoTail`，`first_seq`/`count` 正确，且不能 redo 回去」
  —— `overwriting_the_redo_tail_reports_first_seq_and_count`。
- 「`take_drop_events` 取走即清空，多次裁剪事件按序累积」——
  `take_drop_events_drains_fifo_and_clears`：`Oldest` 与 `RedoTail` 混在一起触发时，
  FIFO 顺序与实际发生顺序一致。
- 「cap 与 undo 交互的裁决」——判断 2 的理由 + 上面提到的
  `cap_shrunk_mid_undo_only_evicts_the_effective_region_and_spares_the_redo_tail`。

另加两个边界测试：`no_cap_never_drops_anything`（`cap=None` 写 500 条不裁剪，`drop_events`
始终为空）、`set_cap_none_stops_future_eviction_but_does_not_undo_past_drops`（`set_cap`
调小再调回 `None`：过去已经丢的不会因为解除上限而恢复，之后的写入不再受限）。

### 命令输出

```
$ cargo test -p agent-store
running 36 tests（history::cap 8 个新增，全部内联，没建 tests/ 新文件）
test result: ok. 36 passed; 0 failed
+ 集成测试与 doctest 全部 ok（合计 49 passed，0 failed，涵盖 019 并行落地的
  history::apply 4 个新单测）

$ cargo test --workspace
565 passed; 0 failed（`--no-fail-fast` 复核，exit=0；期间与 019/020 两个并行 agent
的落地存在过几次瞬时的编译期竞态——019 往同一个 `history.rs`/`lib.rs` 门面追加
`apply` 模块、020 的 `agent-tools` 中途缺 `libc` 依赖 / 有红线 2 违规 / 有一次生命周期
编译错误——都是对方仍在修改中的正常瞬态，收工前最后一轮复核时均已由对方自行修好，
与本 issue 改动无关，未做任何越界修复）

$ cargo clippy --workspace --all-targets -- -D warnings
0 警告

$ bash scripts/check-invariants.sh --all
红线检查通过

$ wc -l crates/agent-store/src/history/*.rs crates/agent-store/src/history.rs crates/agent-store/src/lib.rs
250 apply_roundtrip.rs / 300 apply.rs / 292 cap.rs / 300 cursor.rs / 275 log.rs /
248 record.rs / 51 history.rs / 28 lib.rs —— 全部 ≤300，无需拆分。
```

### 合并记录（主会话）

cap 只吃已生效区、永不碰 redo 尾的裁决——收，理由站得住（空间管理 ≠ 分支覆盖，
两种丢失不混）。默认 None 不烧死 100，策略归会话层（011/集成）。DropEvent 队列
由调用方 take，History 保持零 IO。三 agent 并行编辑 history 门面零冲突。