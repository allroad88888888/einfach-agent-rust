//! 010 验收 7：`Snapshot` 本身 serde 往返（K=String）；红线 4 在这个文件里的
//! 编译期证明——`AtomId` 只出现在喂给 `capture` 的枚举参数里（定位当前进程里的
//! 槽位），`Snapshot` 的键类型参数处处是 `String`。`AtomId`（`src/ids.rs`）本身
//! 不 derive `Serialize`/`Deserialize`，所以哪怕有人手滑把它塞进 `K`，
//! `Snapshot<AtomId, _>` 也序列化不出来——这不是靠人记得别写，是类型系统直接
//! 不给编译（试图给 `Snapshot<AtomId, Tv>` 调 `serde_json::to_string` 会在这个
//! 文件里编译失败，所以不写那个反例，写在这里当注释存档）。

use serde::{Deserialize, Serialize};

use einfach_store::{AtomFamily, AtomId, AtomValue, Snapshot, Store, capture};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Tv(i64);

impl AtomValue for Tv {
    fn null() -> Self {
        Tv(0)
    }
}

#[test]
fn snapshot_survives_a_json_roundtrip_with_string_keys() {
    let store: Store<Tv> = Store::new();
    let mut family: AtomFamily<String> = AtomFamily::new();
    let a = family.get_or_create("a".to_string(), || store.create_atom(Tv(1)));
    let b = family.get_or_create("b".to_string(), || store.create_atom(Tv(2)));
    store.set(a, Tv(11));
    store.set(b, Tv(22));

    // capture 的入参里 AtomId 只出现在这一个元组的第二个位置 —— 陪 String 键定位
    // 当前进程里的槽位，不会跟着快照走。
    let atoms: Vec<(String, AtomId)> = vec![("a".to_string(), a), ("b".to_string(), b)];
    let snap: Snapshot<String, Tv> = capture(&store, atoms.into_iter());

    let json = serde_json::to_string(&snap).expect("Snapshot<String, Tv> must serialize");
    assert!(json.contains("\"a\""));
    assert!(!json.contains("AtomId"));

    let restored: Snapshot<String, Tv> =
        serde_json::from_str(&json).expect("Snapshot<String, Tv> must deserialize");
    assert_eq!(restored, snap);
    assert_eq!(
        restored
            .values
            .iter()
            .find(|(k, _)| k == "a")
            .map(|(_, v)| v.clone()),
        Some(Tv(11))
    );
}
