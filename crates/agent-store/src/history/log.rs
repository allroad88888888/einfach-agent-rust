//! 日志结构本身：一处变更（`Change`）、一个 undo 步（`Entry`）、带游标的日志容器
//! （`History`）。这个文件对 store 一无所知 —— 不 import `AtomId`，键是泛型 `K`。
//! 「怎么把一次 store 写入变成一条 `Change`」在同目录的 `record`，
//! 「游标怎么动」在同目录的 `cursor`。

use serde::{Deserialize, Serialize};

use super::cap::DropEvent;

/// 一处源状态变更：`prev` 是**写入前当场捕获**的值，不是事后推算。
///
/// 当场捕获让每条 entry 自带完整逆操作。事后推算要回溯扫描前序日志才能找到某个键的
/// 上一个值 —— 日志一被截断（018 的 cap）就永久丢失可回滚性，正是本仓不选快照式的
/// 那个理由。
///
/// `K` 是**逻辑键**，语义由上层选择（红线 4：落盘的键不能是 `AtomId`，那是进程内的
/// 自增句柄，往构图函数中间插一行 `create_atom` 就会让所有旧记录静默错位）。本 crate
/// 对上层的键类型不可见，也不需要可见。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change<K, V> {
    pub key: K,
    pub prev: V,
    pub next: V,
}

/// 一个 undo 步。`seq` 由 [`History`] 铸造，严格递增；`meta` 由上层填。
///
/// `M` 是元数据的占位。agent 侧未来往里放 turn_id / epoch / owner / agent / label，
/// 但那些全是 agent 词汇，而 history 住在 agent-store，agent-store 不许 import
/// agent-core（`docs/ARCHITECTURE.md` §包结构）—— 007 已经为同一个依赖方向把 store
/// 泛型化过一次，这里同理，整组字段成为一个泛型参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry<K, V, M> {
    pub seq: u64,
    pub meta: M,
    pub changes: Vec<Change<K, V>>,
}

/// command log：一条扁平日志按时间排序，加一个游标。
///
/// 游标 = **已生效的条数**（`0..=len()`）。`[0, cursor)` 是当前世界里已经发生的，
/// `[cursor, len())` 是被 undo 掉、还能 redo 回来的尾巴。undo 不物理弹条目 —— 物理弹掉
/// 就没有 redo 了；「弹栈顶」弹的是「这一条还算不算数」。游标怎么动在同目录的
/// [`cursor`](super::cursor)（017），cap 与逐出在同目录的 [`cap`](super::cap)（018）。
///
/// 「一次 `store.batch` = 一个 undo 步」由调用方组装：一次 batch 里多个
/// [`record_set`](super::record_set) 产出的 `Change` 攒成一个 `Vec` 喂给
/// [`append`](History::append)。
#[derive(Debug, Clone)]
pub struct History<K, V, M> {
    pub(super) entries: Vec<Entry<K, V, M>>,
    /// 下一个要铸的 seq。**不是** `entries.len()`，也不是 `last().seq + 1`：018 的 cap
    /// 会从最老一端丢条目、分支覆盖会丢掉游标之后的条目，两种情况下由 `entries` 反推
    /// 出来的 seq 都会重复。seq 一重复，落盘的日志就无法定位「这一步是哪一步」。
    pub(super) next_seq: u64,
    /// 已生效条数。新日志与每次 `append` 之后都等于 `entries.len()`。
    pub(super) cursor: usize,
    /// 日志上限（018）。`None` = 无上限，与本字段加入之前的行为完全一致——旧调用方
    /// 不调 `set_cap` 就什么都不变。裁剪逻辑在同目录的 [`cap`](super::cap)。
    pub(super) cap: Option<usize>,
    /// 累积的裁剪事件，`take_drop_events` 取走即清空。见 [`cap::DropEvent`](super::cap::DropEvent)。
    pub(super) drop_events: Vec<DropEvent>,
}

impl<K, V, M> History<K, V, M> {
    /// 空日志，首条 entry 的 `seq` 是 0，游标在 0（= 栈顶，因为日志也是空的）。
    pub fn new() -> Self {
        History {
            entries: Vec::new(),
            next_seq: 0,
            cursor: 0,
            cap: None,
            drop_events: Vec::new(),
        }
    }

    /// 追加一步，返回新条目的 `seq`。
    ///
    /// `changes` 为空则**不落条目**，返回 `None`，且不消耗 seq：空步进日志会让 undo
    /// 出现「按一下没反应」的幽灵步 —— 用户按 undo，日志弹掉一条什么都没改的 entry，
    /// 屏幕上毫无变化。这类步在源头拦掉比在 undo 那侧跳过便宜，因为跳过要遍历。
    ///
    /// 上层组装 `changes` 的常见形态是 `record_set(..)` 的返回值过滤掉 `None`，
    /// 于是「一次 batch 里所有写入的值都没变」自然落成空 `Vec`，自然不产生 entry。
    ///
    /// # 游标不在栈顶时：**默认覆盖 redo 尾**
    ///
    /// undo 之后再写新内容，`[cursor, len())` 那段（已经被回滚掉的那些步）当场丢弃，
    /// 新条目接在游标处。这是 017 验收原文，也是 `docs/STATE-MODEL.md` §「cap 与分支」
    /// 的裁决：**从历史点开分支是显式操作，不是默认行为**（ROADMAP 决策 5 未做的那半
    /// —— 真要留分支得有第二条日志和「当前在哪条」的概念，那是另一个量级的东西）。
    /// 直觉理由：用户 undo 三步然后开始打新字，那三步在他心里已经不存在了。
    ///
    /// 丢了几条会入队一条 [`DropEvent::RedoTail`](super::cap::DropEvent::RedoTail)
    /// （018 开始报告；要不要通知谁、通知给谁归调用方，History 只记账）。
    /// 被丢条目的 seq **不回收**：`next_seq` 只增不减，于是「seq 5 的那一步」在整条会话
    /// 生命周期里指的永远是同一步，落盘日志和审计回放才能对得上。
    ///
    /// 空 `changes` 的早退发生在丢弃**之前**：什么都没写，就不该毁掉 redo 尾，也不该
    /// 报一条根本没发生的丢弃事件。
    ///
    /// 追加之后按当前 `cap` 裁剪一次（[`enforce_cap`](History::enforce_cap)，018）——
    /// 溢出裁剪只吃已生效区，不会把这一步刚写完就丢回去。
    pub fn append(&mut self, meta: M, changes: Vec<Change<K, V>>) -> Option<u64> {
        if changes.is_empty() {
            return None;
        }
        if let Some(dropped) = self.entries.get(self.cursor) {
            self.drop_events.push(DropEvent::RedoTail {
                first_seq: dropped.seq,
                count: self.entries.len() - self.cursor,
            });
        }
        self.entries.truncate(self.cursor);
        let seq = self.next_seq;
        self.next_seq += 1;
        self.entries.push(Entry { seq, meta, changes });
        self.cursor = self.entries.len();
        self.enforce_cap();
        Some(seq)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 只读遍历**全部**条目，按追加顺序（= `seq` 递增顺序），含游标之后还能 redo 的
    /// 那一段。010 的恢复从这里读整份日志。
    pub fn entries(&self) -> impl Iterator<Item = &Entry<K, V, M>> {
        self.entries.iter()
    }

    /// 物理最后一步。注意**不一定是 undo 要弹的那一条** —— 游标不在栈顶时，要弹的是
    /// `entries[cursor - 1]`，而 `last()` 是 redo 尾的末端。见
    /// [`undo_one`](History::undo_one)。
    pub fn last(&self) -> Option<&Entry<K, V, M>> {
        self.entries.last()
    }
}

impl<K, V, M> Default for History<K, V, M> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(key: &str, prev: i32, next: i32) -> Change<String, i32> {
        Change {
            key: key.to_string(),
            prev,
            next,
        }
    }

    type Log = History<String, i32, &'static str>;

    #[test]
    fn new_history_is_empty() {
        let h = Log::new();
        assert_eq!(h.len(), 0);
        assert!(h.is_empty());
        assert!(h.last().is_none());
        assert_eq!(h.entries().count(), 0);
    }

    #[test]
    fn seq_starts_at_zero_and_strictly_increases() {
        let mut h = Log::new();
        let seqs: Vec<u64> = (0..3)
            .map(|i| h.append("step", vec![change("a", i, i + 1)]).unwrap())
            .collect();
        assert_eq!(seqs, vec![0, 1, 2]);
        assert!(seqs.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(h.len(), 3);
    }

    #[test]
    fn empty_changes_records_nothing_and_burns_no_seq() {
        let mut h = Log::new();
        assert_eq!(h.append("first", vec![change("a", 1, 2)]), Some(0));
        // 幽灵步：没有 change 就没有 entry。
        assert_eq!(h.append("ghost", Vec::new()), None);
        assert_eq!(h.len(), 1);
        assert_eq!(h.last().unwrap().meta, "first");
        // 空步也不消耗 seq —— 下一条仍然是 1。
        assert_eq!(h.append("second", vec![change("a", 2, 3)]), Some(1));
    }

    #[test]
    fn entries_follow_append_order_and_carry_meta() {
        let mut h = Log::new();
        h.append("one", vec![change("a", 0, 1)]);
        h.append("two", vec![change("b", 0, 1), change("c", 0, 1)]);

        let seen: Vec<(u64, &str, usize)> = h
            .entries()
            .map(|e| (e.seq, e.meta, e.changes.len()))
            .collect();
        assert_eq!(seen, vec![(0, "one", 1), (1, "two", 2)]);
        assert_eq!(h.last().unwrap().seq, 1);
    }

    #[test]
    fn one_batch_is_one_entry_with_many_changes() {
        // 「一次 store.batch = 一个 undo 步」在日志这一侧的形状：多处变更、一条 entry。
        let mut h = Log::new();
        let seq = h
            .append("batch", vec![change("a", 1, 2), change("b", 3, 4)])
            .unwrap();
        assert_eq!(seq, 0);
        assert_eq!(h.len(), 1);
        assert_eq!(h.last().unwrap().changes.len(), 2);
    }

    #[test]
    fn appending_off_the_top_discards_the_redo_tail_and_never_recycles_seq() {
        // 游标不在栈顶时写新内容 → 丢弃 [cursor, len)。分支不是默认行为（017 验收）。
        let mut h = Log::new();
        for i in 0..3 {
            h.append("step", vec![change("a", i, i + 1)]);
        }
        let _ = h.undo_one(|_| false);
        let _ = h.undo_one(|_| false);
        assert_eq!((h.cursor(), h.len()), (1, 3));

        let seq = h.append("after_undo", vec![change("z", 0, 1)]);
        // seq 不回收：1 和 2 永久作废，不会被重发 —— 落盘日志里「seq 5」永远指同一步。
        assert_eq!(seq, Some(3));
        assert_eq!(h.entries().map(|e| e.seq).collect::<Vec<_>>(), vec![0, 3]);
        assert_eq!((h.cursor(), h.len()), (2, 2));
        assert!(!h.can_redo());
    }

    #[test]
    fn an_empty_step_does_not_destroy_the_redo_tail() {
        // 什么都没写就不该毁掉 redo 尾 —— 空 changes 的早退发生在丢弃之前。
        let mut h = Log::new();
        h.append("one", vec![change("a", 0, 1)]);
        h.append("two", vec![change("a", 1, 2)]);
        let _ = h.undo_one(|_| false);

        assert_eq!(h.append("ghost", Vec::new()), None);
        assert!(h.can_redo());
        assert_eq!((h.cursor(), h.len()), (1, 2));
    }

    #[test]
    fn entry_serde_roundtrip_with_logical_string_keys() {
        // 键是 String（逻辑键），不是 AtomId —— 红线 4。
        let entry = Entry {
            seq: 7,
            meta: "append_user_msg".to_string(),
            changes: vec![change("agent/root/messages", 1, 2), change("agent/root/status", 0, 1)],
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: Entry<String, i32, String> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, entry);
        assert!(json.contains("agent/root/messages"));
    }

    #[test]
    fn change_serde_roundtrip() {
        let c = change("k", -1, 42);
        let back: Change<String, i32> = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }
}
