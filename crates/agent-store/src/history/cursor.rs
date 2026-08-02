//! 游标：在日志上往回走和往前走，两层粒度（一条 / 一个 turn）。
//!
//! 和 `log.rs` 一样**对 store 一无所知** —— 这里只挪游标、把该应用的条目克隆出来交给
//! 调用方。「把 `prev` / `next` 写回状态」是上层 applier 的事（019 处理已 evict 的
//! atom 怎么重建），全链路长什么样见同目录的 `apply_roundtrip`（仅测试）。
//!
//! undo **不物理弹条目**，只把游标往回挪：物理弹掉就没有 redo 了。「undo 就是弹栈顶」
//! （`docs/STATE-MODEL.md` §「Command log」）弹的是「这一条还算不算数」。

use super::log::{Entry, History};
/// undo/redo 的产物：要**应用**的条目（克隆件）。
///
/// `Applied` 里的顺序**就是应用顺序**：undo 按 `seq` 倒序（新的先回滚）、redo 按正序。
/// 每条 entry 内部还要再拆一层 —— undo 时 `changes` 也**倒序**逐条写 `prev`，redo 时
/// 正序写 `next`。一次 batch 里同一个 atom 被写两次时（`1→2` 然后 `2→3`），只有倒序
/// 回滚才回得到 1；正序回滚会停在 2，因为第二条的 `prev` 是第一条的 `next`。
///
/// 克隆而不是借引用：undo 要 `&mut self`（游标动了），借出去调用方在应用期间就再也
/// 碰不了这份日志 —— 而应用过程里往往要接着记录（比如 019 的重建）。
#[derive(Debug, Clone, PartialEq)]
#[must_use = "丢弃 UndoOutcome = 该应用的条目没人应用，undo 静默失效"]
pub enum UndoOutcome<K, V, M> {
    /// 走完了。undo：对每条 entry 把 `changes` **倒序**逐条写 `prev`；redo：正序写 `next`。
    Applied(Vec<Entry<K, V, M>>),
    /// 撞上屏障：`applied` 里的已经该应用（比屏障新的那些），`barrier_seq` 停在门口
    /// **没被越过**，游标就停在它后面一格。
    ///
    /// 上层拿它去问用户（`docs/TOOLS.md`：undo 越过 `Irreversible` 要停下问）。用户说
    /// 「继续，副作用不回滚」就再调一次，传一个放行这一条的谓词
    /// （`|m| barrier(m) && seq_of(m) != barrier_seq`）—— 「越过」因此永远是上层的一次
    /// 显式决定，History 自己不记「这条已经确认过了」的状态位。
    Blocked {
        applied: Vec<Entry<K, V, M>>,
        barrier_seq: u64,
    },
    /// 无可做（游标已在端点）。
    Nothing,
}

impl<K: Clone, V: Clone, M: Clone> History<K, V, M> {
    /// 游标 = 已生效条数，`0..=len()`。新日志游标 == `len()`。
    ///
    /// 它和 `seq` 是两码事：游标是 `entries` 的下标计数，`seq` 是铸出来的号。undo 之后
    /// 再 append 会丢掉 redo 尾（游标退回去了，条目数变少），但 `seq` 只增不减。
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor < self.entries.len()
    }

    /// batch 粒度：回退一条（开发者模式那条可展开的时间线）。
    ///
    /// `barrier(meta)` 为真的条目是**不可越过**的屏障（agent 侧填的是
    /// `Reversibility::Irreversible`）。游标前第一条就是屏障时返回
    /// `Blocked { applied: vec![], .. }`，游标**一动不动**。
    pub fn undo_one(&mut self, barrier: impl Fn(&M) -> bool) -> UndoOutcome<K, V, M> {
        self.undo_while(barrier, |_| false)
    }

    /// turn 粒度（UI 默认）：从游标处连续回退所有 `same_turn` 判为同一 turn 的条目 ——
    /// 与**游标前第一条**比较（调用形如 `same_turn(候选, 第一条)`），跨过 turn 边界即停。
    /// 途中撞屏障 → `Blocked`。第一条无条件取（它就是这个 turn 的定义者），于是
    /// `|_, _| false`（每条自成一个 turn）自然退化成 [`undo_one`](History::undo_one)，
    /// `|_, _| true` 自然一路退到底。
    ///
    /// agent 侧的 `same_turn` 是比 `turn_id`。子 agent 的 entry 继承所在 root turn 的
    /// `turn_id`、不产生新边界，所以一次 `undo_turn` 会连带退掉那一轮里所有子 agent 的
    /// 工作 —— 这正是「整棵树共用一个 store」应有的语义（`docs/STATE-MODEL.md`）。
    pub fn undo_turn(
        &mut self,
        same_turn: impl Fn(&M, &M) -> bool,
        barrier: impl Fn(&M) -> bool,
    ) -> UndoOutcome<K, V, M> {
        let Some(first) = self.cursor.checked_sub(1).map(|i| self.entries[i].meta.clone()) else {
            return UndoOutcome::Nothing;
        };
        self.undo_while(barrier, |m| same_turn(m, &first))
    }

    /// redo **没有屏障**：它只是把 `next` 值写回状态，不重放外部副作用。undo 挡的是
    /// 「状态回滚到副作用发生之前、和已经发生的外部世界对不上」，redo 是把状态追回到
    /// 外部世界已经在的那个点上 —— 方向反了，理由不成立。
    pub fn redo_one(&mut self) -> UndoOutcome<K, V, M> {
        self.redo_while(|_| false)
    }

    /// turn 粒度的 redo，判据与 [`undo_turn`](History::undo_turn) 对称（与**游标处第一条**
    /// 比较）。同一个 `same_turn` 喂进去，`undo_turn` 之后紧跟 `redo_turn` 恰好回到原
    /// 游标，值全部还原。
    pub fn redo_turn(&mut self, same_turn: impl Fn(&M, &M) -> bool) -> UndoOutcome<K, V, M> {
        let Some(first) = self.entries.get(self.cursor).map(|e| e.meta.clone()) else {
            return UndoOutcome::Nothing;
        };
        self.redo_while(|m| same_turn(m, &first))
    }

    /// 往回走，直到 `more` 说停 / 到底 / 撞屏障。第一条不问 `more`。
    fn undo_while(
        &mut self,
        barrier: impl Fn(&M) -> bool,
        more: impl Fn(&M) -> bool,
    ) -> UndoOutcome<K, V, M> {
        if self.cursor == 0 {
            return UndoOutcome::Nothing;
        }
        let mut applied = Vec::new();
        while self.cursor > 0 {
            let entry = &self.entries[self.cursor - 1];
            if !applied.is_empty() && !more(&entry.meta) {
                break;
            }
            if barrier(&entry.meta) {
                let barrier_seq = entry.seq;
                return UndoOutcome::Blocked {
                    applied,
                    barrier_seq,
                };
            }
            let entry = entry.clone();
            applied.push(entry);
            self.cursor -= 1;
        }
        UndoOutcome::Applied(applied)
    }

    /// 往前走，直到 `more` 说停 / 到顶。第一条不问 `more`。
    fn redo_while(&mut self, more: impl Fn(&M) -> bool) -> UndoOutcome<K, V, M> {
        if !self.can_redo() {
            return UndoOutcome::Nothing;
        }
        let mut applied = Vec::new();
        while let Some(entry) = self.entries.get(self.cursor) {
            if !applied.is_empty() && !more(&entry.meta) {
                break;
            }
            let entry = entry.clone();
            applied.push(entry);
            self.cursor += 1;
        }
        UndoOutcome::Applied(applied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Change;

    /// 最小的 meta：一个 turn 号 + 一个「不可逆」标记。agent 侧未来是 `turn_id` 与
    /// `Reversibility`，判定逻辑由调用方以闭包喂进来，History 不认识这两个词。
    #[derive(Debug, Clone, PartialEq)]
    struct Meta {
        turn: u32,
        irreversible: bool,
    }

    type Log = History<String, i32, Meta>;

    /// 普通一步 / 不可逆一步 / 判同 turn / 判屏障 / 全放行。
    fn m(turn: u32) -> Meta { Meta { turn, irreversible: false } }
    fn boom(turn: u32) -> Meta { Meta { turn, irreversible: true } }
    fn same_turn(a: &Meta, b: &Meta) -> bool { a.turn == b.turn }
    fn barrier(m: &Meta) -> bool { m.irreversible }
    fn open(_: &Meta) -> bool { false }

    /// 每条 entry 一处变更，值无关紧要 —— 这个文件只测游标怎么动。
    fn log_of(metas: &[Meta]) -> Log {
        let mut h = Log::new();
        for (i, meta) in metas.iter().enumerate() {
            let i = i as i32;
            h.append(meta.clone(), vec![Change { key: "a".to_string(), prev: i, next: i + 1 }]);
        }
        h
    }

    fn applied_seqs(outcome: &UndoOutcome<String, i32, Meta>) -> Vec<u64> {
        match outcome {
            UndoOutcome::Applied(es) | UndoOutcome::Blocked { applied: es, .. } => {
                es.iter().map(|e| e.seq).collect()
            }
            UndoOutcome::Nothing => Vec::new(),
        }
    }

    #[test]
    fn fresh_log_has_the_cursor_at_the_top() {
        let h = Log::new();
        assert_eq!(h.cursor(), 0);
        assert!(!h.can_undo() && !h.can_redo());

        let h = log_of(&[m(1), m(1)]);
        assert_eq!((h.cursor(), h.len()), (2, 2));
        assert!(h.can_undo() && !h.can_redo());
    }

    #[test]
    fn undo_one_pops_the_top_and_redo_one_puts_it_back() {
        let mut h = log_of(&[m(1), m(2), m(3)]);
        assert_eq!(applied_seqs(&h.undo_one(open)), vec![2]);
        assert_eq!(h.cursor(), 2);
        assert!(h.can_redo());
        // 条目没被物理弹掉 —— 否则 redo 无从谈起。
        assert_eq!(h.len(), 3);

        assert_eq!(applied_seqs(&h.redo_one()), vec![2]);
        assert_eq!(h.cursor(), 3);
        assert!(!h.can_redo());
    }

    #[test]
    fn both_ends_report_nothing() {
        let mut h = Log::new();
        assert_eq!(h.undo_one(open), UndoOutcome::Nothing);
        assert_eq!(h.undo_turn(same_turn, open), UndoOutcome::Nothing);
        assert_eq!(h.redo_one(), UndoOutcome::Nothing);
        assert_eq!(h.redo_turn(same_turn), UndoOutcome::Nothing);

        let mut h = log_of(&[m(1)]);
        assert_eq!(h.redo_one(), UndoOutcome::Nothing); // 已在栈顶
        let _ = h.undo_one(open);
        assert_eq!(h.undo_one(open), UndoOutcome::Nothing); // 已在底
    }

    #[test]
    fn a_barrier_right_under_the_cursor_blocks_without_moving_it() {
        let mut h = log_of(&[m(1), boom(1)]);
        let blocked = UndoOutcome::Blocked { applied: vec![], barrier_seq: 1 };

        assert_eq!(h.undo_one(barrier), blocked);
        assert_eq!(h.cursor(), 2); // 一动不动
        // 幂等：再问一次还是同样的答案，History 不记「已经问过了」。
        assert_eq!(h.undo_one(barrier), blocked);
        // 用户确认「继续，副作用不回滚」= 上层换一个放行这一条的谓词再调一次。
        assert_eq!(applied_seqs(&h.undo_one(open)), vec![1]);
        assert_eq!(h.cursor(), 1);
    }

    #[test]
    fn undo_turn_stops_at_the_turn_boundary_and_walks_turn_by_turn() {
        // 两个 turn：seq 0,1 属于 turn 1；seq 2,3,4 属于 turn 2。
        let mut h = log_of(&[m(1), m(1), m(2), m(2), m(2)]);

        // 新的先回滚 —— Applied 的顺序就是应用顺序。
        assert_eq!(applied_seqs(&h.undo_turn(same_turn, open)), vec![4, 3, 2]);
        assert_eq!(h.cursor(), 2);
        assert_eq!(applied_seqs(&h.undo_turn(same_turn, open)), vec![1, 0]);
        assert_eq!(h.cursor(), 0);
        assert_eq!(h.undo_turn(same_turn, open), UndoOutcome::Nothing);
    }

    #[test]
    fn undo_turn_blocked_midway_keeps_what_it_already_popped() {
        // 同一个 turn 里第二条不可逆：seq 2 该退，seq 1 是门，seq 0 在门后面退不了。
        let mut h = log_of(&[m(7), boom(7), m(7)]);

        let outcome = h.undo_turn(same_turn, barrier);
        assert_eq!(applied_seqs(&outcome), vec![2]);
        assert!(matches!(outcome, UndoOutcome::Blocked { barrier_seq: 1, .. }));
        assert_eq!(h.cursor(), 2); // 停在屏障后一格：屏障本身没被越过

        // 被挡住那一半仍然能 redo 回去。
        assert_eq!(applied_seqs(&h.redo_turn(same_turn)), vec![2]);
        assert_eq!(h.cursor(), 3);
    }

    #[test]
    fn redo_turn_is_the_exact_inverse_of_undo_turn() {
        let mut h = log_of(&[m(1), m(1), m(2), m(2)]);
        let before = h.cursor();

        let undone = applied_seqs(&h.undo_turn(same_turn, open));
        assert_eq!(undone, vec![3, 2]);

        let mut redone = applied_seqs(&h.redo_turn(same_turn));
        assert_eq!(h.cursor(), before);
        redone.reverse();
        assert_eq!(redone, undone); // 同一批条目，顺序恰好相反
    }

    #[test]
    fn a_never_same_turn_predicate_degenerates_to_one_entry() {
        // 「每条自成一个 turn」是合法的粒度配置，不是「什么都不做」。
        let mut h = log_of(&[m(1), m(1), m(1)]);
        assert_eq!(applied_seqs(&h.undo_turn(|_, _| false, open)), vec![2]);
        assert_eq!(h.cursor(), 2);
        assert_eq!(applied_seqs(&h.redo_turn(|_, _| false)), vec![2]);
        assert_eq!(h.cursor(), 3);

        // 反过来，「全在一个 turn」一路退到底。
        assert_eq!(applied_seqs(&h.undo_turn(|_, _| true, open)), vec![2, 1, 0]);
        assert_eq!(h.cursor(), 0);
    }

}
