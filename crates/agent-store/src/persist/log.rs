//! [`SessionLog`]：`SessionStore` 两个实现共用的记账引擎。零 IO（红线 7）——只回答
//! 「收到这个操作之后，状态该是什么样」，不碰文件、不碰锁。`Memory`（同目录）把它包一层
//! `Mutex`；`Jsonl`（agent-runtime）在 IO 线程里养一份同样的引擎决定该往文件里写什么、
//! 崩溃恢复时把读回来的记录重新喂给一份新引擎重放。
//!
//! ## 要解决的问题：`held` 比 `History.entries()` 短了一截
//!
//! [`SessionStore::set_cursor`](super::SessionStore::set_cursor) 收到的是
//! [`History::cursor()`](crate::History::cursor) 的原样值——**相对 `History` 自己当前
//! 那份 `entries`**，包括它已经被 cap 驱逐缩短过的那部分（`enforce_cap` 驱逐时
//! `cursor` 和 `entries.len()` 是一起减的，见 `history/cap.rs`）。但
//! [`LoadedSession::entries`](super::LoadedSession::entries) 只是**快照点之后**的那一段
//! （之前的被压实丢了，`snapshot()` 不touch `History`本体，只影响这个引擎自己的副本）
//! ——`History::from_parts` 要求 `cursor <= entries.len()`。
//!
//! `boundary` 就是这两者的差：`held` 逻辑上等于 `History.entries()[boundary..]`。
//! `record_snapshot` 让它前进（多少条被这一张快照吃掉）；`record_drop_oldest` 让它
//! 后退（`History` 自己的 `entries` 缩短了，`held` 相对它的起点也要跟着往前挪）。
//! 两条路径**必须共用同一个 `boundary`**，不能分开记：`set_cursor` 给的值本来就已经
//! 把 cap 驱逐的效果算在内了，如果这个引擎自己再额外记一份「cap 吃到哪」，会对 cap
//! 的效果重复计数——这正是本文件第一版踩过的坑（`record_drop_oldest` 的推导见下）。
//!
//! ## 为什么这值得单独抽出来，而不是各写一份
//!
//! 011 验收要求「Memory 与 Jsonl 都过同一套端口行为测试（写→load→重放语义一致）」——
//! 上面这段推导不简单，两个实现各自独立推一遍，迟早在某个边界情况上分岔，而且是
//! 「测试各自都过、行为却不一致」的那种分岔。写一遍、在这里测清楚，两个后端复用。

use crate::history::{Entry, Snapshot};

use super::LoadedSession;

/// 见模块文档。字段全私有——外部只能通过 `record_*` 方法推进状态、`to_loaded` 读结果，
/// 不能直接摆弄内部坐标系。
pub struct SessionLog<K, V, M> {
    snapshot: Option<Snapshot<K, V>>,
    held: Vec<Entry<K, V, M>>,
    /// `held` 相对 `History.entries()` 当前起点的偏移：`held == History.entries()[boundary..]`。
    boundary: usize,
    /// seq 高水位，只增不减：即使 `held` 因为压实变空，下一个 seq 也不能跌回去。
    max_seq: Option<u64>,
    last_cursor: usize,
    /// 有没有收到过任何一次 `record_*`——`to_loaded` 用它区分「全新会话」（`None`）
    /// 和「写过东西但目前恰好是空的」（`Some` 的退化形态）。
    written: bool,
}

impl<K, V, M> SessionLog<K, V, M> {
    pub fn new() -> Self {
        SessionLog {
            snapshot: None,
            held: Vec::new(),
            boundary: 0,
            max_seq: None,
            last_cursor: 0,
            written: false,
        }
    }
}

impl<K, V, M> Default for SessionLog<K, V, M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Clone, V: Clone, M: Clone> SessionLog<K, V, M> {
    pub fn record_append(&mut self, entry: &Entry<K, V, M>) {
        self.written = true;
        self.max_seq = Some(self.max_seq.map_or(entry.seq, |m| m.max(entry.seq)));
        self.held.push(entry.clone());
    }

    /// cap 驱逐：`History::enforce_cap` 每次都是从 `History` 自己**当前**的 `entries`
    /// 前端切掉 `count` 条，`cursor` 跟着减 `count`——两件事在 `History` 那一侧是同一次
    /// 调用做的（`history/cap.rs::enforce_cap`）。
    ///
    /// 对这个引擎来说，`History.entries()` 缩短了 `count`，`held` 相对它新起点的偏移
    /// `boundary` 就该跟着退 `count`：如果 `count <= boundary`（被切掉的这些本来就在
    /// `held` 之前，早被快照吃过了），`held` 不用动，`boundary -= count` 就够；如果
    /// `count > boundary`，多出来的 `count - boundary` 条是 `held` 里真正还活着的，
    /// 要物理删掉，`boundary` 归零。
    ///
    /// 这条推导是本文件存在的理由：如果直接「不管三七二十一从 `held` 前面切 `count`
    /// 条」，快照压实之后再来一次 cap 驱逐会把**还没过时的 entries**误删——不是游标
    /// 算错这么轻，是真的把可恢复的历史丢了。
    ///
    /// 返回**这一次真正从 `held` 前端切掉的条数**（`<= count`，`count` 有多少被
    /// `boundary` 吸收就少多少）。`Jsonl` 落盘这一侧要用它——不能照抄调用方给的原始
    /// `count` 写进文件，见 `agent-runtime/src/jsonl/io_thread.rs` 模块文档「压实之后
    /// 为什么要落这一份『已经算过的』量」那一节：压实截断文件之后，重放是从
    /// `boundary = 0` 起步的一份全新 `SessionLog`，喂给它「相对旧 boundary 算出来的
    /// 原始 count」会重新按 0 起步的 boundary 再吸收一遍，两次吸收对不上。返回值就是
    /// 「已经吸收过一次之后，对 `held` 的净效果」，写这个数字，重放端从 0 起步再套
    /// 同一条公式，效果和这里物理发生的完全一致。
    pub fn record_drop_oldest(&mut self, count: usize) -> usize {
        self.written = true;
        let remove_from_held = count.saturating_sub(self.boundary).min(self.held.len());
        self.held.drain(0..remove_from_held);
        self.boundary = self.boundary.saturating_sub(count);
        remove_from_held
    }

    /// 分支覆盖：`first_seq` 起的 redo 尾被丢弃。这是**尾部**操作——不移动 `boundary`，
    /// 那个字段记的是「前端已经不在了多少」，跟尾巴无关。
    pub fn record_drop_after(&mut self, first_seq: u64, _count: usize) {
        self.written = true;
        self.held.retain(|e| e.seq < first_seq);
    }

    /// `cursor` 是 [`History::cursor()`](crate::History::cursor) 的原样值，调用方不用
    /// 为这个端口另做换算——已经把 cap 驱逐的效果算在内了（见模块文档），换算是这个
    /// 引擎自己的事（[`to_loaded`](Self::to_loaded) / [`relative_cursor`](Self::relative_cursor)）。
    pub fn record_cursor(&mut self, cursor: usize) {
        self.written = true;
        self.last_cursor = cursor;
    }

    /// 当前的相对游标——[`to_loaded`](Self::to_loaded) 里那个换算单拎出来，不必为了
    /// 读一个 `usize` 就克隆整份 `held`/`snapshot`。`Jsonl` 落盘 `Cursor` 记录时用它：
    /// 跟 [`record_drop_oldest`](Self::record_drop_oldest) 的返回值同一个理由——落盘的
    /// 必须是「已经换算过」的值，重放端从 `boundary = 0` 起步直接消费，不需要（也没有
    /// 能力）知道压实之前的真实 `boundary` 有多大。
    pub fn relative_cursor(&self) -> usize {
        self.last_cursor.saturating_sub(self.boundary).min(self.held.len())
    }

    /// 落一张快照：`held` 里现在这些全部被它代表了，标记为压实——`boundary` 前进
    /// `held.len()`，`held` 清空。
    pub fn record_snapshot(&mut self, snap: &Snapshot<K, V>) {
        self.written = true;
        self.snapshot = Some(snap.clone());
        self.boundary += self.held.len();
        self.held.clear();
    }

    /// `None` = 从来没收到过任何一次 `record_*`（全新会话）。
    ///
    /// 返回的 `cursor` 保证 `<= entries.len()`，`next_seq` 保证大于 `entries` 里最后一条
    /// 的 `seq`（`entries` 为空则不设下限）——`History::from_parts` 的三条不变量在这里
    /// 就已经满足，调用方不用另外校验。
    ///
    /// **已知的精度损失**：如果崩溃发生在「快照之后又 undo 回快照点之前」——理论上
    /// 允许，`History` 自己完全有能力 undo 到那么远——`last_cursor < boundary`，
    /// `saturating_sub` 会把它钳到 0。持久化这一侧没有能力精确表达「回到已经被压实掉
    /// 的那一步」，钳到 0（相当于「把 held 里的都退干净」）是能给出的最接近的答案，
    /// 不是 bug；真要精确，得放弃压实，那正是这个引擎存在的意义相反面。
    pub fn to_loaded(&self) -> Option<LoadedSession<K, V, M>> {
        if !self.written {
            return None;
        }
        let next_seq = self.max_seq.map_or(0, |s| s + 1);
        Some(LoadedSession {
            snapshot: self.snapshot.clone(),
            entries: self.held.clone(),
            cursor: self.relative_cursor(),
            next_seq,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct V(i64);

    fn entry(seq: u64) -> Entry<String, V, u32> {
        Entry {
            seq,
            meta: 1,
            changes: vec![crate::history::Change {
                key: "a".to_string(),
                prev: V(seq as i64),
                next: V(seq as i64 + 1),
            }],
        }
    }

    fn snap() -> Snapshot<String, V> {
        Snapshot { values: vec![("a".to_string(), V(0))] }
    }

    type Log = SessionLog<String, V, u32>;

    #[test]
    fn a_fresh_log_has_nothing_to_load() {
        assert!(Log::new().to_loaded().is_none());
    }

    #[test]
    fn plain_append_and_cursor_round_trip_with_no_snapshot() {
        let mut log = Log::new();
        for i in 0..3 {
            log.record_append(&entry(i));
        }
        log.record_cursor(3);

        let loaded = log.to_loaded().unwrap();
        assert!(loaded.snapshot.is_none());
        assert_eq!(loaded.entries.len(), 3);
        assert_eq!(loaded.cursor, 3);
        assert_eq!(loaded.next_seq, 3);
    }

    /// 更精细的场景（快照压实与 cap 驱逐交叉、游标停在压实边界中间……）挪到
    /// `agent-store/tests/session_log_replay.rs`——红线 9 的取向是集成测试挪出源文件，
    /// 这里只留「新写一个方法时最贴身」的两条。
    #[test]
    fn a_snapshot_compacts_everything_held_so_far_but_the_seq_high_water_mark_survives() {
        let mut log = Log::new();
        log.record_append(&entry(0));
        log.record_snapshot(&snap());
        let loaded = log.to_loaded().unwrap();
        assert!(loaded.entries.is_empty()); // 被这一张快照整个压实
        assert_eq!(loaded.next_seq, 1); // 但 seq 高水位没有跌回 0
    }
}
