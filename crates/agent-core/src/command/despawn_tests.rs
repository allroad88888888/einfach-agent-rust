//! `despawn` 必须握住 `Session` 内部图结构的白盒测试。

use std::sync::Arc;

use crate::command::ChildConfig;
use crate::graph::{derived_atom, source_atom};

use super::*;

fn session_with_child() -> (Session, AgentId) {
    let mut session = Session::new(AgentId::root());
    let child = session
        .spawn_child(
            &AgentId::root(),
            ChildConfig {
                tools_allowed: vec![Arc::from("srv:fs/read")],
                ..ChildConfig::default()
            },
        )
        .unwrap();
    (session, child)
}

/// **019 硬约束 1 的反面**：derived 还活着时，它读的那个 primitive 逐不掉
/// ——引擎写死的顺序，不是我们的约定。`despawn_child` 之所以能成，
/// 正是因为它先销 derived。
#[test]
fn a_slot_the_childs_derived_still_reads_cannot_be_evicted_first() {
    let (session, child) = session_with_child();
    let converged = derived_atom(
        &session.store,
        &session.sources,
        &session.derived,
        &DerivedKey::ToolsConverged(child.clone()),
    );
    let _ = session.store.get(converged); // 算一次，把反向边装上

    let key = AtomKey::Agent(child, Slot::ToolSlots);
    assert!(
        !session.sources.borrow_mut().evict(&session.store, &key),
        "还有下游 derived 时逐出必须被拒绝"
    );
}

/// **019 硬约束 2**：子树之外还有人读时，整条命令拒绝，而且一个字节都没改。
///
/// 这个测试自己造了一个「外人」derived——它捕获了 `AtomId`（红线 4 的孪生条款
/// 明令 derived 不许这么干），这里是刻意的：它的唯一职责是**持有一条读边**，
/// 从建出来到测试结束都不会被重建。生产代码里的汇聚 derived 一律按逻辑键
/// 现查 family（`graph/build.rs`）。
#[test]
fn an_outside_reader_refuses_the_whole_despawn() {
    let (mut session, child) = session_with_child();
    let watched = source_atom(
        &session.store,
        &session.sources,
        &AtomKey::Agent(child.clone(), Slot::Status),
    );
    let watcher = session
        .store
        .create_derived_ctx(move |args| args.get(watched));
    let _ = session.store.get(watcher); // 装上反向边

    let before = session.primitives();
    let history_len = session.history_len();

    let Err(DespawnRefused::StillRead { key }) = session.despawn_child(&child) else {
        panic!("还有外部读者时必须拒绝");
    };
    assert_eq!(key, AtomKey::Agent(child.clone(), Slot::Status));
    assert_eq!(session.primitives(), before, "拒绝时状态一个字节都不该改");
    assert_eq!(session.history_len(), history_len, "拒绝不该留下一条 entry");
    assert!(session.is_live(&child), "拒绝之后子 agent 还活着");
}
