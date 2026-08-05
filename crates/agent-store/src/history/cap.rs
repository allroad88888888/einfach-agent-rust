//! 日志上限与裁剪事件（018）：`set_cap` / `take_drop_events` / [`DropEvent`]。
//!
//! 和 `cursor.rs` 同一条纪律：**这里不做 IO，不发通知**（红线 7）。裁剪只是记账——
//! 把「刚刚丢了什么」攒进一个队列，调用方什么时候来取、取了转发给谁（`docs/STATE-MODEL.md`
//! 的 `SessionStore::drop_oldest` / `drop_after`）是上层的事。攒着不取也不是 bug，是
//! 调用方还没来处理；但长期不取 `drop_events` 会无限增长，这是留给调用方的责任，不是
//! 本文件要兜底的东西——兜底就是自己发通知，那就是 IO 了。
//!
//! `History::new()` 的 `cap` 仍然是 `None`（无上限），**不**在这里硬编码「默认 100」。
//! issue 原文的「默认 100」是会话层的策略，不是日志结构本身的常量：`History` 对「一个
//! 会话该有多大」一无所知，就像它对 `AtomId`、`turn_id` 一无所知一样。会话层建 History
//! 时自己调一次 `set_cap(Some(100))`。这也是「与现状兼容」的字面意思——017 落地时
//! 已经有调用方在用不设 cap 的 `History`，这里不能让他们的日志突然被裁。

use super::log::History;

/// 日志的裁剪事件。宿主拿去转发 `SessionStore::drop_oldest` / `drop_after`
/// （`docs/STATE-MODEL.md` §「持久化」）。
///
/// 两种事件不是同一件事的两种严重程度，是两类不同的丢弃：
/// - `Oldest` 丢的是**已经生效、不再是任何人的「未来」**的旧条目——纯粹的空间管理。
/// - `RedoTail` 丢的是**用户可能还想 redo 回去**的分支——017 就有的行为（游标不在栈顶
///   时写入默认覆盖），018 开始报告它，好让 UI 能提示「redo 不可用了」。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DropEvent {
    /// cap 溢出，从最老端丢了 `count` 条。
    Oldest { count: usize },
    /// 游标不在顶时写入，丢弃了 redo 尾（017 的既有行为，这次开始报告）。
    /// `first_seq` 是被丢的第一条（也是 seq 最小的一条）的 `seq`。
    RedoTail { first_seq: u64, count: usize },
}

impl<K, V, M> History<K, V, M> {
    /// 设置日志上限。`None` = 无上限（默认，与现状兼容）。`Some(n)` 立即裁剪一次——
    /// 调小 cap 时不用等下一次 `append` 才生效，此刻就能把内存要回去。
    ///
    /// 裁剪产生的 [`DropEvent`] 入队，见 [`take_drop_events`](History::take_drop_events)。
    pub fn set_cap(&mut self, cap: Option<usize>) {
        self.cap = cap;
        self.enforce_cap();
    }

    /// 取走积累的裁剪事件（FIFO，按发生顺序）。取走即清空——下一次调用只会看到新产生的。
    ///
    /// 不取就一直攒着：History 不做 IO 不发通知，只记账（红线 7）。
    pub fn take_drop_events(&mut self) -> Vec<DropEvent> {
        std::mem::take(&mut self.drop_events)
    }

    /// 溢出裁剪。**只吃 `[0, cursor)` 的已生效区，绝不动 redo 尾**（`[cursor, len)`）。
    ///
    /// 这是本 issue 唯一需要裁决的地方：undo 之后游标停在中位时，`cap` 触发的裁剪要不要
    /// 连 redo 尾一起吃？答案是不。理由是 redo 尾和「已经生效的旧条目」不是同一种东西——
    /// 已生效的旧条目丢了，世界不变（游标同步左移，`[0, cursor)` 少了几条，`cursor` 之后
    /// 一切照旧）；redo 尾是用户明确 undo 出来、还没决定要不要走回去的**未来分支**，被
    /// cap 静默吃掉和被新写入显式覆盖不是一回事——后者是「打了新字，旧分支不要了」的
    /// 显式动作（017 的 `append`），前者只是「日志太长该瘦身了」，两件事的因果链完全不同，
    /// 不该共用一条「丢弃」的理由。
    ///
    /// 代价：如果 redo 尾本身就比 cap 还长，这一次裁剪之后 `len()` 仍然可能超过 cap——
    /// 这是故意的（宁可暂时超限也不吃 redo 尾），不是漏洞。等用户 redo 回顶或者打字覆盖掉
    /// 这段 redo 尾（无论哪种，游标都会回到顶），下一次 `append` 触发的裁剪会把账补上。
    pub(super) fn enforce_cap(&mut self) {
        let Some(cap) = self.cap else { return };
        let overflow = self.entries.len().saturating_sub(cap);
        let count = overflow.min(self.cursor);
        if count == 0 {
            return;
        }
        self.entries.drain(0..count);
        self.cursor -= count;
        self.drop_events.push(DropEvent::Oldest { count });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{Change, Entry};

    fn change(key: &str, prev: i32, next: i32) -> Change<String, i32> {
        Change {
            key: key.to_string(),
            prev,
            next,
        }
    }

    type Log = History<String, i32, &'static str>;

    /// 写第 `i` 条：`a` 从 `i` 变到 `i+1`。和 `log.rs`/`cursor.rs` 的测试同一手法——
    /// 值本身不重要，重要的是能倒推出「这条把 a 变回了几」。
    fn push(h: &mut Log, i: i32) -> u64 {
        h.append("step", vec![change("a", i, i + 1)]).unwrap()
    }

    fn applied_of<'a>(
        outcome: &'a crate::history::UndoOutcome<String, i32, &'static str>,
    ) -> &'a [Entry<String, i32, &'static str>] {
        use crate::history::UndoOutcome::*;
        match outcome {
            Applied(es) | Blocked { applied: es, .. } => es,
            Nothing => &[],
        }
    }

    // —— 验收：写满 150、cap=100 ——————————————————————————————

    #[test]
    fn overflow_drops_from_the_oldest_end_and_caps_len_at_cap() {
        let mut h = Log::new();
        h.set_cap(Some(100));
        for i in 0..150 {
            push(&mut h, i);
        }
        assert_eq!(h.len(), 100);
        assert_eq!(h.cursor(), 100);
        // 剩下的是最新的 100 条：seq 50..150（0..50 被丢）。
        let seqs: Vec<u64> = h.entries().map(|e| e.seq).collect();
        assert_eq!(seqs.first(), Some(&50));
        assert_eq!(seqs.last(), Some(&149));
    }

    #[test]
    fn one_hundred_undos_succeed_then_the_101st_is_nothing_not_a_panic() {
        let mut h = Log::new();
        h.set_cap(Some(100));
        for i in 0..150 {
            push(&mut h, i);
        }
        for _ in 0..100 {
            let outcome = h.undo_one(|_| false);
            assert!(
                !applied_of(&outcome).is_empty(),
                "前 100 次 undo 必须都有效"
            );
        }
        assert_eq!(h.cursor(), 0);
        // 第 101 次：明确报「到头」，不 panic。
        assert_eq!(h.undo_one(|_| false), crate::history::UndoOutcome::Nothing);
    }

    // —— 验收：溢出后剩余条目的回滚结果与未溢出时逐值相同 ————————————

    #[test]
    fn surviving_entries_undo_to_the_same_values_whether_or_not_the_log_ever_overflowed() {
        // 两份平行日志，写同一批命令：一份 cap=100（会溢出丢掉前 50 条），
        // 一份不设 cap（全须全尾留着）。溢出那份剩下的 100 条，逐条 undo 出来的
        // `prev` 值必须和不限量那份对应的 100 条一字不差——截断不该改变剩余条目的
        // 回滚结果，这正是事务日志（每条自带完整逆操作）相对快照式的那个优势。
        let mut capped = Log::new();
        capped.set_cap(Some(100));
        let mut uncapped = Log::new();
        for i in 0..150 {
            push(&mut capped, i);
            push(&mut uncapped, i);
        }
        assert_eq!(capped.len(), 100);
        assert_eq!(uncapped.len(), 150);

        // capped 撤到底：应该拿到 100 个 prev 值，是 uncapped 撤 100 步的后 100 个。
        let mut capped_prevs = Vec::new();
        loop {
            let outcome = capped.undo_one(|_| false);
            let batch = applied_of(&outcome);
            if batch.is_empty() {
                break;
            }
            for e in batch {
                capped_prevs.push(e.changes[0].prev);
            }
        }
        assert_eq!(capped_prevs.len(), 100);

        let mut uncapped_prevs = Vec::new();
        for _ in 0..100 {
            let outcome = uncapped.undo_one(|_| false);
            for e in applied_of(&outcome) {
                uncapped_prevs.push(e.changes[0].prev);
            }
        }
        assert_eq!(capped_prevs, uncapped_prevs);
    }

    // —— 验收：redo 尾被覆盖后报 DropEvent::RedoTail，且不能 redo 回去 ————————

    #[test]
    fn overwriting_the_redo_tail_reports_first_seq_and_count() {
        let mut h = Log::new();
        for i in 0..3 {
            push(&mut h, i);
        }
        let _ = h.undo_one(|_| false);
        let _ = h.undo_one(|_| false);
        assert_eq!((h.cursor(), h.len()), (1, 3));
        assert!(h.take_drop_events().is_empty()); // undo 本身不产生裁剪事件

        h.append("after_undo", vec![change("z", 0, 1)]);
        assert!(!h.can_redo()); // 被丢的分支回不去了

        let events = h.take_drop_events();
        assert_eq!(
            events,
            vec![DropEvent::RedoTail {
                first_seq: 1,
                count: 2
            }]
        );
    }

    // —— 验收：take_drop_events 取走即清空，多次事件按序累积 ————————————

    #[test]
    fn take_drop_events_drains_fifo_and_clears() {
        let mut h = Log::new();
        h.set_cap(Some(2));
        push(&mut h, 0);
        push(&mut h, 1);
        push(&mut h, 2); // 第三条把第一条挤掉 → 一个 Oldest 事件（entries=[seq1,seq2]）
        let _ = h.undo_one(|_| false); // 弹掉 seq2，cursor=1
        h.append("branch", vec![change("z", 0, 1)]); // 覆盖 redo 尾（seq2）→ RedoTail；
        // 覆盖之后 entries=[seq1, seq3]，len=2 == cap，这次 append 不再触发 Oldest。

        let events = h.take_drop_events();
        assert_eq!(
            events,
            vec![
                DropEvent::Oldest { count: 1 },
                DropEvent::RedoTail {
                    first_seq: 2,
                    count: 1
                },
            ]
        );
        // 取走即清空。
        assert!(h.take_drop_events().is_empty());
        push(&mut h, 9);
        assert_eq!(h.take_drop_events(), vec![DropEvent::Oldest { count: 1 }]);
    }

    // —— cap 与 undo 交互的裁决：裁剪只吃 [0, cursor)，绝不吃 redo 尾 ————————

    #[test]
    fn cap_shrunk_mid_undo_only_evicts_the_effective_region_and_spares_the_redo_tail() {
        // 5 条，undo 2 次 → 游标在 3（已生效区 [0,3)，redo 尾是 [3,5) 两条）。
        let mut h = Log::new();
        for i in 0..5 {
            push(&mut h, i);
        }
        let _ = h.undo_one(|_| false);
        let _ = h.undo_one(|_| false);
        assert_eq!((h.cursor(), h.len()), (3, 5));

        // 把 cap 降到 1：期望丢 4 条，但已生效区只有 3 条可丢 —— 裁决是不动 redo 尾，
        // 所以只丢 3 条，len 降到 2（>1，超过新 cap，但那是故意的）。
        h.set_cap(Some(1));
        assert_eq!(h.cursor(), 0);
        assert_eq!(h.len(), 2); // 5 - 3，仍然 > cap(1)：redo 尾原样留着
        assert_eq!(h.take_drop_events(), vec![DropEvent::Oldest { count: 3 }]);

        // redo 尾没被动过：还能把之前 undo 掉的两条 redo 回去，值不变。
        let outcome = h.redo_one();
        assert_eq!(applied_of(&outcome).len(), 1);
        let outcome = h.redo_one();
        assert_eq!(applied_of(&outcome).len(), 1);
        assert!(!h.can_redo());
        assert_eq!(h.len(), 2);

        // 一路 redo 到顶之后再写一步：游标回到顶，append 之后 cursor == len，之前被
        // 保护的两条 redo 尾条目不再有「未来」这层身份，这次裁剪把它们和新写的一条一起
        // 吃到只剩 cap(1) 条 —— 欠的账在这里一次性补上，不再有特殊保护。
        h.append("after_redo_to_top", vec![change("z", 0, 1)]);
        assert_eq!(h.len(), 1);
        assert_eq!(h.take_drop_events(), vec![DropEvent::Oldest { count: 2 }]);
    }

    #[test]
    fn no_cap_never_drops_anything() {
        let mut h = Log::new();
        for i in 0..500 {
            push(&mut h, i);
        }
        assert_eq!(h.len(), 500);
        assert!(h.take_drop_events().is_empty());
    }

    #[test]
    fn set_cap_none_stops_future_eviction_but_does_not_undo_past_drops() {
        let mut h = Log::new();
        h.set_cap(Some(3));
        for i in 0..10 {
            push(&mut h, i);
        }
        assert_eq!(h.len(), 3);
        let _ = h.take_drop_events();

        h.set_cap(None);
        for i in 100..110 {
            push(&mut h, i);
        }
        assert_eq!(h.len(), 13); // 3 旧 + 10 新，不再受限
        assert!(h.take_drop_events().is_empty());
    }
}
