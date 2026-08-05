//! `History` 的持久化边界：拆成可落盘的三元组，以及从落盘件重建时**唯一的受控入口**。
//!
//! 不做 IO（红线 7），只做「结构 ↔ 三元组」的转换与一次不变量校验。真正落盘的是
//! `docs/STATE-MODEL.md` §「持久化」里的 `SessionStore`，住在 agent-runtime。
//!
//! ## 为什么 `History` 不 derive `Deserialize`
//!
//! 009 的裁决（实做记录判断 8）：`next_seq` / `cursor` 是 `History` 自己维护的不变量，
//! 给它一个 `Deserialize` 就等于允许外部凭空构造出一份自相矛盾的日志 —— 游标指到条目
//! 之外（下一次 undo 直接下标越界 panic）、seq 重号（落盘日志再也无法定位「这一步是
//! 哪一步」，审计回放对不上）、`next_seq` 落在最后一条后面（下一次 append 铸一个已经
//! 用过的号，同上）。这三件事都不会在写入的当下报错，都要等到恢复之后的某一次 undo /
//! 某一次审计才浮出来。
//!
//! [`from_parts`](History::from_parts) 因此是外部数据进入 `History` 的**唯一**入口，
//! 它的职责就是不信任来的东西并当场拒绝。

use super::log::{Entry, History};

/// [`from_parts`](History::from_parts) 拒绝一份落盘件的三个理由。
///
/// 只有三个，因为**校验边界就是这三条**：它们各自对应一条会在恢复之后才发作的静默错误。
/// 别的可疑之处（比如某条 entry 的 `changes` 是空的 —— [`append`](History::append) 永远
/// 不会产出这种条目）不在这里拒：空条目回滚起来是无害的 no-op，而拒了会让旧版本写的
/// 日志在新版本里打不开，代价与收益反了。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidHistory {
    /// `cursor > entries.len()`：游标是「已生效条数」，越界的游标下一次 undo 就下标越界。
    CursorOutOfRange,
    /// 相邻两条的 `seq` 没有严格递增（含相等）。seq 是审计与落盘定位的唯一凭证。
    SeqNotIncreasing,
    /// `next_seq <= 最后一条的 seq`：下一次 `append` 会铸一个已经用过的号。
    NextSeqTooSmall,
}

impl std::fmt::Display for InvalidHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match self {
            InvalidHistory::CursorOutOfRange => "游标越过了条目末尾",
            InvalidHistory::SeqNotIncreasing => "seq 没有严格递增",
            InvalidHistory::NextSeqTooSmall => "next_seq 不大于最后一条的 seq",
        };
        write!(f, "落盘的日志不满足不变量：{what}")
    }
}

impl std::error::Error for InvalidHistory {}

impl<K, V, M> History<K, V, M> {
    /// 拆成可落盘的三元组：`(entries, cursor, next_seq)`。
    ///
    /// **`cap` 不在里面**：它是配置不是状态（会话层建 History 时自己
    /// [`set_cap`](History::set_cap)，见 `cap.rs` 的裁决）。存进落盘件就等于把「这个部署
    /// 现在允许多长的日志」冻进了历史数据里 —— 改配置之后旧会话还按旧上限跑。
    ///
    /// **`drop_events` 也不在里面**：那是「还没被取走的账」，不是日志内容。进程都换了，
    /// 上一次的裁剪事件没人再需要转发。
    ///
    /// 消费 `self` 而不是克隆：三元组是把内部字段整个搬出去，克隆一份整条日志只为满足
    /// 一个命名约定不值得（`to_*` 通常收 `&self`，这里的偏离是 issue 010 钉死的签名，
    /// 也是刻意的）。
    pub fn to_parts(self) -> (Vec<Entry<K, V, M>>, usize, u64) {
        (self.entries, self.cursor, self.next_seq)
    }

    /// 从落盘件重建，**校验不变量**，破坏不变量的输入拒绝。
    ///
    /// 三条校验（其余不查，理由见 [`InvalidHistory`]）：
    ///
    /// 1. `cursor <= entries.len()`。等于合法 —— 那是「游标在栈顶」，新日志的常态。
    /// 2. `seq` 严格递增。只查相邻对（O(n)）：严格递增按相邻可判，不需要全序扫描。
    /// 3. `next_seq > 最后一条的 seq`。**空 `entries` 时不设下限**：cap 把老条目全裁掉
    ///    之后 `next_seq` 必须留在高位（seq 不回收），此时没有任何东西能给它定下界。
    ///
    /// 重建出来的日志 `cap` 是 `None`、`drop_events` 是空的（见 [`to_parts`](History::to_parts)）。
    /// 会话层照常 `set_cap(Some(100))`，那一下会立刻裁剪一次 —— 正好是「载入一份比现在
    /// 的上限还长的旧日志」想要的行为。
    pub fn from_parts(
        entries: Vec<Entry<K, V, M>>,
        cursor: usize,
        next_seq: u64,
    ) -> Result<Self, InvalidHistory> {
        if cursor > entries.len() {
            return Err(InvalidHistory::CursorOutOfRange);
        }
        if entries.windows(2).any(|w| w[0].seq >= w[1].seq) {
            return Err(InvalidHistory::SeqNotIncreasing);
        }
        if entries.last().is_some_and(|last| next_seq <= last.seq) {
            return Err(InvalidHistory::NextSeqTooSmall);
        }
        Ok(History {
            entries,
            next_seq,
            cursor,
            cap: None,
            drop_events: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{Change, UndoOutcome};

    type Log = History<String, i32, u32>;

    fn change(key: &str, prev: i32, next: i32) -> Change<String, i32> {
        Change {
            key: key.to_string(),
            prev,
            next,
        }
    }

    fn entry(seq: u64) -> Entry<String, i32, u32> {
        Entry {
            seq,
            meta: 1,
            changes: vec![change("a", seq as i32, seq as i32 + 1)],
        }
    }

    /// 三步日志：seq 0/1/2，游标在顶。
    fn log_of(steps: u64) -> Log {
        let mut h = Log::new();
        for i in 0..steps {
            h.append(1, vec![change("a", i as i32, i as i32 + 1)]);
        }
        h
    }

    /// 拒绝的理由。`History` 没有 `PartialEq`（也不该有 —— 它是个可变容器），
    /// 所以断言拿 `unwrap_err` 而不是比 `Result`。
    fn rejected(
        entries: Vec<Entry<String, i32, u32>>,
        cursor: usize,
        next_seq: u64,
    ) -> InvalidHistory {
        Log::from_parts(entries, cursor, next_seq).unwrap_err()
    }

    fn applied_seqs(o: &UndoOutcome<String, i32, u32>) -> Vec<u64> {
        match o {
            UndoOutcome::Applied(es) | UndoOutcome::Blocked { applied: es, .. } => {
                es.iter().map(|e| e.seq).collect()
            }
            UndoOutcome::Nothing => Vec::new(),
        }
    }

    #[test]
    fn to_parts_then_from_parts_is_the_identity() {
        let mut h = log_of(3);
        let _ = h.undo_one(|_| false);
        let (entries, cursor, next_seq) = h.clone().to_parts();
        assert_eq!((cursor, next_seq), (2, 3));

        let back = Log::from_parts(entries.clone(), cursor, next_seq).unwrap();
        assert_eq!(back.entries().cloned().collect::<Vec<_>>(), entries);
        assert_eq!(back.cursor(), 2);
        assert_eq!(back.len(), 3);
    }

    #[test]
    fn a_cursor_past_the_end_is_rejected_but_the_top_is_fine() {
        assert_eq!(
            rejected(vec![entry(0)], 2, 1),
            InvalidHistory::CursorOutOfRange
        );
        assert!(Log::from_parts(vec![entry(0)], 1, 1).is_ok()); // 游标在栈顶
        assert!(Log::from_parts(Vec::new(), 0, 0).is_ok()); // 空日志
    }

    #[test]
    fn seq_that_repeats_or_goes_backwards_is_rejected() {
        assert_eq!(
            rejected(vec![entry(1), entry(1)], 2, 2),
            InvalidHistory::SeqNotIncreasing
        );
        assert_eq!(
            rejected(vec![entry(5), entry(2)], 2, 9),
            InvalidHistory::SeqNotIncreasing
        );
        // 有空洞（cap 裁掉过、分支覆盖过）是合法的 —— seq 不回收，跳号是常态。
        assert!(Log::from_parts(vec![entry(0), entry(7)], 2, 8).is_ok());
    }

    #[test]
    fn next_seq_must_be_past_the_last_entry() {
        assert_eq!(
            rejected(vec![entry(4)], 1, 4),
            InvalidHistory::NextSeqTooSmall
        );
        assert_eq!(
            rejected(vec![entry(4)], 1, 0),
            InvalidHistory::NextSeqTooSmall
        );
        assert!(Log::from_parts(vec![entry(4)], 1, 5).is_ok());
        // 空 entries 不设下限：cap 把老条目全裁光之后 next_seq 必须留在高位。
        assert!(Log::from_parts(Vec::new(), 0, 4096).is_ok());
    }

    #[test]
    fn a_restored_log_undoes_redoes_and_appends_exactly_like_the_original() {
        let mut original = log_of(3);
        let _ = original.undo_one(|_| false); // 游标 2，redo 尾一条

        let (entries, cursor, next_seq) = original.clone().to_parts();
        let mut restored = Log::from_parts(entries, cursor, next_seq).unwrap();

        // undo / redo 逐步同形。
        assert_eq!(
            applied_seqs(&original.undo_one(|_| false)),
            applied_seqs(&restored.undo_one(|_| false))
        );
        assert_eq!(
            applied_seqs(&original.redo_one()),
            applied_seqs(&restored.redo_one())
        );
        assert_eq!(original.cursor(), restored.cursor());

        // append 续铸不重号：两边都铸 3（0/1/2 用过，被丢掉的 2 也不回收）。
        let a = original.append(9, vec![change("z", 0, 1)]);
        let b = restored.append(9, vec![change("z", 0, 1)]);
        assert_eq!((a, b), (Some(3), Some(3)));
        assert_eq!(
            original.entries().map(|e| e.seq).collect::<Vec<_>>(),
            restored.entries().map(|e| e.seq).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_log_restored_from_a_capped_session_keeps_minting_past_the_dropped_entries() {
        // cap 裁掉最老的之后 next_seq 仍在高位；往返之后新条目接着高位铸号，
        // 而不是从 entries.len() 或 last().seq + 1 反推 —— 那两个都会重号。
        let mut h = Log::new();
        h.set_cap(Some(2));
        for i in 0..5 {
            h.append(1, vec![change("a", i, i + 1)]);
        }
        let (entries, cursor, next_seq) = h.to_parts();
        assert_eq!((entries.len(), cursor, next_seq), (2, 2, 5));

        let mut restored = Log::from_parts(entries, cursor, next_seq).unwrap();
        assert_eq!(restored.append(1, vec![change("a", 9, 10)]), Some(5));
        // cap 是配置不是状态：恢复出来的日志没有上限，直到会话层再设一次。
        assert_eq!(restored.len(), 3);
    }
}
