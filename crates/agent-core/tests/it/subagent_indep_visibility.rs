//! 028 独立测试（**决策 35 改写**）：跨 agent 可见性的集合性质 + 三个读口的
//! 显式拒绝 + 「读口无副作用」。
//!
//! 黑盒来源：docs/INVARIANTS.md 红线 10、docs/issues/204-agent-mesh-decision.md §一、
//! cargo doc 的 `graph::visibility` / `command::cross_read` 模块文档。
//! 不读 `src/graph/visibility.rs` 与 `src/command/cross_read.rs` 源码。
//!
//! **这个文件原本测的性质被决策 35 删掉了。** 028 时代它断言 `U ∩ D = ∅`
//! （每个槽位至多一个方向可读）和「兄弟互读在 API 面上不存在」，因为那是
//! 「环在结构上不可能」的前提。决策 35 查实：这三个口走的是命令层的非追踪读，
//! **一条依赖边都不建**，方向约束防的是一类当时还不存在的边。无环的判据因此
//! 换了地方（跨 agent 的边只许指向 primitive，断言在 issue 212 的 `await`
//! derived 上），这里剩下的性质是**可见性说了算，方向不说话**。

use crate::support::session::new_session;
use agent_core::{AgentId, ChildConfig, ReadDenied, Session, Slot, Visibility};

/// root -> child -> grandchild 三层 + 一个兄弟，够覆盖「隔代」「兄弟」两类关系。
fn spawn_tree(session: &mut Session) -> (AgentId, AgentId, AgentId) {
    let root = session.agent().clone();
    let child = session
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("spawn child");
    let sibling = session
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("spawn sibling");
    let grandchild = session
        .spawn_child(&child, ChildConfig::default(), None)
        .expect("spawn grandchild");
    (child, sibling, grandchild)
}

/// **划分性质本身**：对 `Slot::ALL` 穷举，每个槽位跨 agent 读的结果只有两种
/// ——`Shared` 一律读得到、`Private` 一律 `NotVisible`——而且**三个方向上答案
/// 完全相同**。不是抽样几个槽位测几个用例，是覆盖全部。
///
/// 这条替代 028 的 `for_every_slot_at_most_one_direction_can_succeed`：那条断言的
/// 「两个方向不能都成功」现在**必须失败**，因为方向已经不是判据了。
#[test]
fn every_slot_reads_the_same_in_every_direction_and_visibility_decides() {
    let mut session = new_session();
    let root = session.agent().clone();
    let (child, sibling, _) = spawn_tree(&mut session);

    for slot in Slot::ALL {
        let expected = slot.visibility() == Visibility::Shared;
        for (label, target) in [("祖先", &root), ("后代", &child), ("兄弟", &sibling)] {
            let got = session.read_agent(target, slot);
            assert_eq!(
                got.is_ok(),
                expected,
                "{slot:?} 读{label}：visibility() 说 {:?}，实际 {got:?}",
                slot.visibility()
            );
            if !expected {
                assert!(
                    matches!(got, Err(ReadDenied::NotVisible { .. })),
                    "{slot:?} 该以可见性为由被拒，而不是别的理由：{got:?}"
                );
            }
        }
    }
}

/// 比上一条更强：两类的**具体成员**跟 `visibility()` 声明的完全一致，
/// 而且合起来恰好覆盖 `Slot::ALL` 一次。
#[test]
fn the_two_classes_partition_all_slots_exactly_once() {
    let mut session = new_session();
    let (_, sibling, _) = spawn_tree(&mut session);

    let mut shared = Vec::new();
    let mut private = Vec::new();
    for slot in Slot::ALL {
        match session.read_agent(&sibling, slot) {
            Ok(_) => shared.push(slot),
            Err(ReadDenied::NotVisible { .. }) => private.push(slot),
            other => panic!("{slot:?} 落进了第三种结果：{other:?}"),
        }
    }

    assert_eq!(
        shared.len() + private.len(),
        Slot::ALL.len(),
        "两类合起来该恰好覆盖全部槽位一次"
    );
    assert!(!shared.is_empty() && !private.is_empty(), "两类都该非空");

    // 钉住内部账本那几样——它们被读出去会长出「把别人的预算算进自己的」这类
    // 账目错误，或者「读得到摘要引用、查不到正文」这类静默降级。
    for internal in [
        Slot::TurnsUsed,
        Slot::MaxTurns,
        Slot::RetriesUsed,
        Slot::MaxRetries,
        Slot::NextMessageId,
        Slot::ToolSlots,
        Slot::PrevPrefix,
        Slot::SendPlan,
        Slot::PrevSendPlan,
        Slot::Summaries,
        Slot::ExecutionProfile,
    ] {
        assert!(private.contains(&internal), "{internal:?} 该是别人读不到的");
    }
}

/// **兄弟互读现在是放行的**——决策 35 的行为核心，028 时代这里断言的是相反的事。
/// 顺带钉住「读口无副作用」：成功的读也不该改变任何 primitive。
#[test]
fn siblings_can_read_each_other_and_the_read_has_no_side_effect() {
    let mut session = new_session();
    let (child, sibling, _) = spawn_tree(&mut session);

    let before = session.primitives();

    assert!(session.read_agent(&sibling, Slot::Status).is_ok());
    assert!(session.read_agent(&child, Slot::Status).is_ok());

    assert_eq!(session.primitives(), before, "读不该改变任何 primitive");
}

/// 两个带亲缘断言的封装仍然拒兄弟——它们断言的是「你俩是不是祖先/后代」，
/// 而兄弟两样都不是。要横读走 `read_agent`。
#[test]
fn the_kinship_wrappers_still_refuse_siblings() {
    let mut session = new_session();
    let (child, sibling, _) = spawn_tree(&mut session);

    let before = session.primitives();

    assert!(matches!(
        session.read_ancestor(&child, &sibling, Slot::Messages),
        Err(ReadDenied::NotAnAncestor { .. })
    ));
    assert!(matches!(
        session.read_descendant(&child, &sibling, Slot::Status),
        Err(ReadDenied::NotADescendant { .. })
    ));

    assert_eq!(session.primitives(), before);
}

/// 方向传反：两个 agent 之间确实有亲缘关系，只是调用点把 `reader`/`target`
/// 的角色搞反了。封装照拒。
#[test]
fn swapping_the_direction_is_rejected_even_for_real_relatives() {
    let mut session = new_session();
    let root = session.agent().clone();
    let (child, _, grandchild) = spawn_tree(&mut session);

    let before = session.primitives();

    assert!(matches!(
        session.read_descendant(&grandchild, &child, Slot::Status),
        Err(ReadDenied::NotADescendant { .. })
    ));
    assert!(matches!(
        session.read_ancestor(&root, &child, Slot::Messages),
        Err(ReadDenied::NotAnAncestor { .. })
    ));

    assert_eq!(session.primitives(), before);
}

/// 会话外的 id：一棵完全不相干的路径。封装按亲缘拒，`read_agent` 按「图上没有
/// 这个 atom」拒——**没有第三种「未知会话」的特判**，而且都不建 atom。
#[test]
fn an_id_from_nowhere_in_this_tree_is_rejected_without_creating_atoms() {
    let mut session = new_session();
    let root = session.agent().clone();
    let (child, _, _) = spawn_tree(&mut session);
    let alien = AgentId::new("some_other_tree/x1");

    let before = session.primitives();

    assert!(matches!(
        session.read_ancestor(&child, &alien, Slot::Messages),
        Err(ReadDenied::NotAnAncestor { .. })
    ));
    assert!(matches!(
        session.read_descendant(&root, &alien, Slot::Status),
        Err(ReadDenied::NotADescendant { .. })
    ));
    assert!(matches!(
        session.read_agent(&alien, Slot::Messages),
        Err(ReadDenied::NoSuchAtom { .. })
    ));

    assert_eq!(
        session.primitives(),
        before,
        "拒绝的跨会话读不该在 family 里留下任何 atom"
    );
}
