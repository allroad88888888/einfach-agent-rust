//! `Entry` / `Change` 可 serde 往返，且日志的键是逻辑键（`String`），不是 `AtomId`
//! —— 红线 4「快照与日志落盘用 AtomKey 不用 AtomId」在 009 这里的形状：history 对
//! `AtomId` 不可见，这个测试文件里也完全不出现它（编译期就证明了这一点：这里没有
//! `use einfach_store::AtomId`，也没有任何 `Store`）。
//!
//! 验收 4：整个 History 的 entries 序列化 -> 反序列化 -> 逐条相等。
//!
//! 这个文件不接触 `Store`：serde 往返测的是 `Entry<K,V,M>` / `Change<K,V>` 这两个
//! 类型本身的契约，和写入路径无关，所以 `K`/`V` 用最简单的 `String`，不复用
//! `tests/common` 的 `TestValue`（那是 store 行为夹具，跟 serde 无关，用它只会多一层
//! 不必要的耦合）。

use serde::{Deserialize, Serialize};

use einfach_store::{Change, Entry, History};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Meta {
    label: String,
}

fn assert_change_eq(a: &Change<String, String>, b: &Change<String, String>) {
    assert_eq!(a.key, b.key);
    assert_eq!(a.prev, b.prev);
    assert_eq!(a.next, b.next);
}

fn assert_entry_eq(a: &Entry<String, String, Meta>, b: &Entry<String, String, Meta>) {
    assert_eq!(a.seq, b.seq);
    assert_eq!(a.meta, b.meta);
    assert_eq!(a.changes.len(), b.changes.len());
    for (ca, cb) in a.changes.iter().zip(b.changes.iter()) {
        assert_change_eq(ca, cb);
    }
}

#[test]
fn entries_roundtrip_through_json_with_string_keys() {
    let mut history: History<String, String, Meta> = History::new();

    history
        .append(
            Meta {
                label: "append_user_msg".to_string(),
            },
            vec![Change {
                key: "root/config".to_string(),
                prev: "gpt-4".to_string(),
                next: "gpt-5".to_string(),
            }],
        )
        .expect("non-empty batch appends");

    history
        .append(
            Meta {
                label: "tool_result".to_string(),
            },
            vec![
                Change {
                    key: "root/messages".to_string(),
                    prev: "[]".to_string(),
                    next: "[hello]".to_string(),
                },
                Change {
                    key: "root/a1/turn_status".to_string(),
                    prev: "Idle".to_string(),
                    next: "Done".to_string(),
                },
            ],
        )
        .expect("non-empty batch appends");

    let original: Vec<&Entry<String, String, Meta>> = history.entries().collect();
    assert_eq!(original.len(), 2);

    let json = serde_json::to_string(&original).expect("Entry must serialize");
    let restored: Vec<Entry<String, String, Meta>> =
        serde_json::from_str(&json).expect("Entry must deserialize");

    assert_eq!(restored.len(), original.len());
    for (orig, back) in original.iter().zip(restored.iter()) {
        assert_entry_eq(orig, back);
    }
}
