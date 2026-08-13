//! 010 验收 5：`History` 的 `to_parts`/`from_parts` —— 写 5 条、undo 2 次
//! （cursor=3）→ `to_parts` → serde_json 往返 → `from_parts` → cursor/len/
//! can_redo 一致、redo 行为一致、append 续铸的 seq 不与历史重号。
//!
//! 不接触 `Store` —— 这个文件测的是 `History<K,V,M>` 本身的持久化契约，和写入
//! 路径无关（009 的 `history_serde.rs` 就是这个手法：K/V 用最简单的 String/i32）。

use einfach_store::{Change, Entry, History, UndoOutcome};

type Log = History<String, i32, String>;

fn change(prev: i32, next: i32) -> Change<String, i32> {
    Change {
        key: "a".to_string(),
        prev,
        next,
    }
}

fn build_five_entry_log() -> Log {
    let mut h = Log::new();
    for i in 0..5 {
        h.append(format!("e{i}"), vec![change(i, i + 1)]).unwrap();
    }
    h
}

#[test]
fn roundtrip_preserves_cursor_len_and_redo_after_two_undos() {
    let mut h = build_five_entry_log();
    let _ = h.undo_one(|_: &String| false);
    let _ = h.undo_one(|_: &String| false);
    assert_eq!((h.cursor(), h.len()), (3, 5));

    let (entries, cursor, next_seq) = h.to_parts();
    assert_eq!(cursor, 3);
    assert_eq!(next_seq, 5);
    assert_eq!(entries.len(), 5);

    let json = serde_json::to_string(&(entries.clone(), cursor, next_seq)).unwrap();
    let (entries2, cursor2, next_seq2): (Vec<Entry<String, i32, String>>, usize, u64) =
        serde_json::from_str(&json).unwrap();
    assert_eq!(entries2, entries);
    assert_eq!((cursor2, next_seq2), (cursor, next_seq));

    // —— cursor/len/can_redo 一致 ——
    let mut restored = Log::from_parts(entries2.clone(), cursor2, next_seq2)
        .expect("well-formed parts must reconstruct");
    assert_eq!(restored.cursor(), 3);
    assert_eq!(restored.len(), 5);
    assert!(restored.can_undo());
    assert!(restored.can_redo());

    // —— redo 行为一致：第 4 条（index 3, seq 3, a: 3->4）该被 redo 回来 ——
    let outcome = restored.redo_one();
    match outcome {
        UndoOutcome::Applied(applied) => {
            assert_eq!(applied.len(), 1);
            assert_eq!(applied[0].seq, 3);
            assert_eq!(applied[0].changes, vec![change(3, 4)]);
        }
        other => panic!("expected Applied, got {other:?}"),
    }
    assert_eq!(restored.cursor(), 4);

    // —— append 续铸的 seq 不与历史重号 ——
    // 另一份重建（游标仍在 3，没做过 redo）上写新内容：应丢弃 redo 尾（seq 3、4），
    // 新条目铸出 seq=5（next_seq 传下来的值），不回收。
    let mut restored2 =
        Log::from_parts(entries2, cursor, next_seq).expect("well-formed parts must reconstruct");
    let seq = restored2.append("after_restore".to_string(), vec![change(0, 1)]);
    assert_eq!(seq, Some(5));
    assert_eq!((restored2.cursor(), restored2.len()), (4, 4));
    assert!(!restored2.can_redo());
}
