//! 019 验收覆盖第五条："resolve 是 get-or-create，无特判"的行为面证据：
//! 每个 change 的 key 都必须经过 resolve，不管这个 atom 是不是已经在 family 里
//! ——已存在的 atom 不能因为"反正找得到"就被走另一条路径跳过 resolve。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use einfach_store::{AtomFamily, AtomId, Change, Entry, Store, apply_prev};
use crate::common::{TestValue as V, num};

#[test]
fn every_change_key_goes_through_resolve_no_special_casing() {
    let store: Store<V> = Store::new();
    let family = Rc::new(RefCell::new(AtomFamily::<String>::new()));

    // "a" 一直都在（正常路径）；"b" 从未被创建过，必须靠 resolve 的 get-or-create 现建。
    let a = family
        .borrow_mut()
        .get_or_create("a".to_string(), || store.create_atom(num(0.0)));

    let entry1: Entry<String, V, ()> = Entry {
        seq: 0,
        meta: (),
        changes: vec![
            Change {
                key: "a".to_string(),
                prev: num(0.0),
                next: num(1.0),
            },
            Change {
                key: "b".to_string(),
                prev: num(0.0),
                next: num(1.0),
            },
        ],
    };
    let entry2: Entry<String, V, ()> = Entry {
        seq: 1,
        meta: (),
        changes: vec![Change {
            key: "a".to_string(),
            prev: num(1.0),
            next: num(2.0),
        }],
    };
    let entries = [entry1, entry2];

    let calls: Rc<RefCell<HashMap<String, u32>>> = Rc::new(RefCell::new(HashMap::new()));
    let calls_for_resolve = calls.clone();
    let family_for_resolve = family.clone();
    let store_for_resolve = store.clone();
    let mut resolve = move |k: &String| -> AtomId {
        *calls_for_resolve.borrow_mut().entry(k.clone()).or_insert(0) += 1;
        family_for_resolve
            .borrow_mut()
            .get_or_create(k.clone(), || store_for_resolve.create_atom(num(0.0)))
    };

    apply_prev(&store, &mut resolve, &entries);

    {
        let calls = calls.borrow();
        // "a" 在两条 entry 里各出现一次 change —— 已经存在的 atom 也一样经过
        // resolve，不因为 family 里已经有它就被跳过。
        assert_eq!(
            calls.get("a"),
            Some(&2),
            "已存在的 atom 也必须每次都经过 resolve"
        );
        // "b" 从未存在过，走的是 get-or-create 里创建的那一支，同样经过同一个 resolve。
        assert_eq!(calls.get("b"), Some(&1), "需要重建的 atom 必须经过 resolve");
    }

    // resolve 确实把 b 建出来了，且两次写入按 apply_prev 给定顺序落地：
    // a 先被 entry1 的 change 写回 0，又被 entry2 的 change 写回 1（entry2 在
    // entries 里排在 entry1 之后，同一 atom 后写的生效）。
    assert_eq!(store.get(a), num(1.0));
    let b_id = family
        .borrow()
        .get(&"b".to_string())
        .expect("b 应该被 resolve 现建出来");
    assert_eq!(store.get(b_id), num(0.0));
}
