//! 010 验收 6：`from_parts` 的三种校验失败，各自拒绝且变体正确。三个子测试各自
//! 只违反一条不变量，避免检查顺序这类实现细节混进断言里；另加两个边界内的
//! 「本该放行」对照，确认边界判定不是失之毫厘的另一套规则。

use agent_store::{Change, Entry, History, InvalidHistory};

type Log = History<String, i32, String>;

fn entry(seq: u64, prev: i32, next: i32) -> Entry<String, i32, String> {
    Entry {
        seq,
        meta: format!("e{seq}"),
        changes: vec![Change {
            key: "a".to_string(),
            prev,
            next,
        }],
    }
}

#[test]
fn cursor_past_the_end_is_rejected() {
    // 2 条合法递增的 entry，cursor 给到 5（> len=2）。
    let entries = vec![entry(0, 0, 1), entry(1, 1, 2)];
    let result = Log::from_parts(entries, 5, 2);
    assert!(matches!(result, Err(InvalidHistory::CursorOutOfRange)));
}

#[test]
fn cursor_exactly_at_len_is_accepted() {
    let entries = vec![entry(0, 0, 1), entry(1, 1, 2)];
    let h = Log::from_parts(entries, 2, 2).expect("cursor == len is the top of the stack, legal");
    assert_eq!(h.cursor(), 2);
    assert!(!h.can_redo());
}

#[test]
fn seq_not_increasing_is_rejected() {
    // entries 本身乱序（5 在 3 前面），cursor/next_seq 单看都合法。
    let entries = vec![entry(5, 0, 1), entry(3, 1, 2)];
    let result = Log::from_parts(entries, 2, 6);
    assert!(matches!(result, Err(InvalidHistory::SeqNotIncreasing)));
}

#[test]
fn next_seq_not_past_the_last_entry_is_rejected() {
    // entries 递增合法（0,1,2），cursor 合法，但 next_seq 落在最后一条 seq 上——
    // 下一次 append 会铸出 seq=2，和已有的 entries[2].seq 撞号。
    let entries = vec![entry(0, 0, 1), entry(1, 1, 2), entry(2, 2, 3)];
    let result = Log::from_parts(entries, 3, 2);
    assert!(matches!(result, Err(InvalidHistory::NextSeqTooSmall)));
}

#[test]
fn next_seq_exactly_one_past_the_last_entry_is_accepted() {
    let entries = vec![entry(0, 0, 1), entry(1, 1, 2), entry(2, 2, 3)];
    let h = Log::from_parts(entries, 3, 3).expect("next_seq == last_seq + 1 must not collide");
    assert_eq!(h.len(), 3);
}
