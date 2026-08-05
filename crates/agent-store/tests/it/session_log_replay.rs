//! [`SessionLog`] 的精细场景：011 验收原文的三步流程，加上快照压实与 cap 驱逐交叉时
//! 的游标换算——`crates/agent-store/src/persist/log.rs` 模块文档里承诺的那条推导
//! （`record_drop_oldest` 不能对已经被快照压实的部分重复计数）在这里逐条钉死。
//!
//! 源文件只留两条最贴身的单元测试（红线 9 的取向：集成测试挪出源文件），这里是
//! 「新后端接入这套引擎之前应该先读的」那份行为清单——`Memory`/`Jsonl` 的验收测试
//! 断言的是端口的行为，这份测试断言的是端口行为**为什么**是这样。

use agent_store::history::{Change, Entry, Snapshot};
use agent_store::persist::SessionLog;

#[derive(Clone, Debug, PartialEq)]
struct V(i64);

fn entry(seq: u64) -> Entry<String, V, u32> {
    Entry {
        seq,
        meta: 1,
        changes: vec![Change {
            key: "a".to_string(),
            prev: V(seq as i64),
            next: V(seq as i64 + 1),
        }],
    }
}

fn snap() -> Snapshot<String, V> {
    Snapshot {
        values: vec![("a".to_string(), V(0))],
    }
}

type Log = SessionLog<String, V, u32>;

/// 011 验收原文：写 5 entry + 1 snapshot（第 3 条后）+ 2 entry
/// → load 得 snapshot + 之后 2 条 + cursor/next_seq 正确。
#[test]
fn three_entries_a_snapshot_then_two_more_only_the_tail_survives() {
    let mut log = Log::new();
    for i in 0..3 {
        log.record_append(&entry(i));
    }
    log.record_cursor(3);
    log.record_snapshot(&snap());
    for i in 3..5 {
        log.record_append(&entry(i));
    }
    log.record_cursor(5);

    let loaded = log.to_loaded().unwrap();
    assert!(loaded.snapshot.is_some());
    assert_eq!(
        loaded.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![3, 4]
    );
    assert_eq!(loaded.cursor, 2); // 顶：两条都生效
    assert_eq!(loaded.next_seq, 5);
}

#[test]
fn cursor_mid_history_translates_relative_to_the_compacted_tail() {
    let mut log = Log::new();
    for i in 0..3 {
        log.record_append(&entry(i));
    }
    log.record_snapshot(&snap());
    for i in 3..6 {
        log.record_append(&entry(i));
    }
    // 绝对游标 4：6 条里前 4 条生效——压实之后只剩 3 条（seq 3,4,5），
    // 相对游标 = 4 - 3(held_start) = 1。
    log.record_cursor(4);

    let loaded = log.to_loaded().unwrap();
    assert_eq!(
        loaded.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![3, 4, 5]
    );
    assert_eq!(loaded.cursor, 1);
}

#[test]
fn a_second_snapshot_only_captures_what_was_appended_since_the_first() {
    let mut log = Log::new();
    log.record_append(&entry(0));
    log.record_snapshot(&snap());
    log.record_append(&entry(1));
    log.record_append(&entry(2));
    log.record_snapshot(&snap());
    log.record_append(&entry(3));
    log.record_cursor(4);

    let loaded = log.to_loaded().unwrap();
    assert_eq!(
        loaded.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![3]
    );
    assert_eq!(loaded.cursor, 1);
    assert_eq!(loaded.next_seq, 4);
}

/// 3 条被快照压实（boundary=3），再 append 2 条（seq 3,4，held=[3,4]）。此时
/// `History.entries()` 自己其实还是全须全尾的 5 条（快照不 touch `History`）——
/// `drop_oldest(4)` 是 cap 从 `History` 当前的 5 条前端切 4 条，同一次调用里
/// `History::cursor` 也从 5 减到 1（`enforce_cap` 是一起做的，`history/cap.rs`）。
///
/// 落在 `held` 上的真实删除只有 1 条（seq 3）：0..3 早被快照吃过了。不能因为
/// count=4 就从 held 前面无脑切 2 条，那会把 seq 4 也删掉——不是游标算错这么轻，
/// 是把可恢复的历史真丢了。
#[test]
fn drop_oldest_after_a_snapshot_only_removes_what_is_still_held() {
    let mut log = Log::new();
    for i in 0..3 {
        log.record_append(&entry(i));
    }
    log.record_snapshot(&snap());
    log.record_append(&entry(3));
    log.record_append(&entry(4));
    log.record_drop_oldest(4);
    log.record_cursor(1); // History::cursor() 本身也被 enforce_cap 减到了 1

    let loaded = log.to_loaded().unwrap();
    assert_eq!(
        loaded.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![4]
    );
    assert_eq!(loaded.cursor, 1);
    assert_eq!(loaded.next_seq, 5);
}

#[test]
fn drop_oldest_before_any_snapshot_behaves_like_a_plain_front_trim() {
    let mut log = Log::new();
    for i in 0..5 {
        log.record_append(&entry(i));
    }
    log.record_drop_oldest(2); // 丢 seq 0,1
    log.record_cursor(3); // 剩 3 条全生效

    let loaded = log.to_loaded().unwrap();
    assert_eq!(
        loaded.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![2, 3, 4]
    );
    assert_eq!(loaded.cursor, 3);
}

#[test]
fn drop_after_only_trims_the_tail_and_leaves_the_front_offset_untouched() {
    let mut log = Log::new();
    for i in 0..3 {
        log.record_append(&entry(i));
    }
    log.record_cursor(1); // undo 两步：游标回到 1
    log.record_drop_after(1, 2); // 覆盖 redo 尾：seq 1,2 被丢
    log.record_append(&entry(9)); // 接一条新的（实际场景里 append 会带新 seq，这里借用 9 表意）
    log.record_cursor(2);

    let loaded = log.to_loaded().unwrap();
    assert_eq!(
        loaded.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![0, 9]
    );
    assert_eq!(loaded.cursor, 2);
}

#[test]
fn max_seq_never_regresses_even_when_entries_are_compacted_away() {
    let mut log = Log::new();
    for i in 0..3 {
        log.record_append(&entry(i));
    }
    log.record_snapshot(&snap()); // held 清空，但 seq 高水位得留着
    let loaded = log.to_loaded().unwrap();
    assert!(loaded.entries.is_empty());
    assert_eq!(loaded.next_seq, 3); // 不是 0
}
