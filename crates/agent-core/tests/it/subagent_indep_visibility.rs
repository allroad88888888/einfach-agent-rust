//! 028 独立测试：红线 10 的集合性质（`Slot::ALL` 划分）+ 跨 agent 读的四类
//! 显式拒绝（兄弟、自己、方向传反、会话外 id），外加「读口无副作用」。
//!
//! 黑盒来源：docs/INVARIANTS.md 红线 10、docs/issues/028-multi-agent-graph.md
//! §2 与「注意」段、cargo doc 的 `graph::visibility` / `command::cross_read`
//! 模块文档。不读 `src/graph/visibility.rs` 与 `src/command/cross_read.rs` 源码。

use crate::support::session::new_session;
use agent_core::{AgentId, ChildConfig, ReadDenied, Session, Slot, Visibility};

/// root -> child -> grandchild 三层，够覆盖「隔代」和「兄弟」两类关系。
fn spawn_two_level(session: &mut Session) -> (AgentId, AgentId) {
    let root = session.agent().clone();
    let child = session
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("spawn child");
    let grandchild = session
        .spawn_child(&child, ChildConfig::default(), None)
        .expect("spawn grandchild");
    (child, grandchild)
}

/// 划分性质本身：对 `Slot::ALL` 穷举，每个槽位两个跨 agent 读口至多一个能
/// 成功——不是抽样几个槽位测几个用例，是覆盖全部（039 起是十一个）。
#[test]
fn for_every_slot_at_most_one_direction_can_succeed() {
    let mut session = new_session();
    let root = session.agent().clone();
    let (child, _) = spawn_two_level(&mut session);

    for slot in Slot::ALL {
        let up = session.read_ancestor(&child, &root, slot).is_ok();
        let down = session.read_descendant(&root, &child, slot).is_ok();
        assert!(!(up && down), "{slot:?} 两个方向都能读，U ∩ D 破了");
    }
}

/// 比上一条更强：不只是「不冲突」，是每个槽位的实际可读方向跟它自己声明的
/// `visibility()` 完全一致，而且三类合起来恰好是 `Slot::ALL` 的一个划分。
/// 当前具体归属（issue 实做记录判断 7，144 追加 PrefixAllowed 站
/// Downward）：Upward = {Messages, ...}，
/// Downward = {Status, ToolsAllowed, PrefixAllowed}，其余 Private。
#[test]
fn each_slot_behaves_exactly_as_its_declared_visibility_says() {
    let mut session = new_session();
    let root = session.agent().clone();
    let (child, _) = spawn_two_level(&mut session);

    let mut upward = Vec::new();
    let mut downward = Vec::new();
    let mut private = Vec::new();

    for slot in Slot::ALL {
        let up = session.read_ancestor(&child, &root, slot);
        let down = session.read_descendant(&root, &child, slot);
        match slot.visibility() {
            Visibility::Upward => {
                assert!(up.is_ok(), "{slot:?} 声明 Upward 但子读父失败: {up:?}");
                assert!(matches!(down, Err(ReadDenied::NotVisible { .. })));
                upward.push(slot);
            }
            Visibility::Downward => {
                assert!(
                    down.is_ok(),
                    "{slot:?} 声明 Downward 但父读子失败: {down:?}"
                );
                assert!(matches!(up, Err(ReadDenied::NotVisible { .. })));
                downward.push(slot);
            }
            Visibility::Private => {
                assert!(
                    matches!(up, Err(ReadDenied::NotVisible { .. })),
                    "{slot:?} 该是 Private"
                );
                assert!(
                    matches!(down, Err(ReadDenied::NotVisible { .. })),
                    "{slot:?} 该是 Private"
                );
                private.push(slot);
            }
        }
    }

    assert_eq!(
        upward,
        vec![
            Slot::Messages,
            Slot::SkillsActive,
            Slot::HostTools,
            Slot::HostSkills,
            Slot::DisabledBuiltins,
            Slot::PrefixChunks
        ]
    );
    // 144 追加 `PrefixAllowed`，跟 `ToolsAllowed` 同一边（理由见
    // `src/graph/visibility.rs` 的 match 分支注释：站队但不是活名单）。
    assert_eq!(
        downward,
        vec![Slot::Status, Slot::ToolsAllowed, Slot::PrefixAllowed]
    );
    assert_eq!(
        upward.len() + downward.len() + private.len(),
        Slot::ALL.len(),
        "三类合起来该恰好覆盖全部槽位一次"
    );
}

/// 第一类：兄弟读。API 面上不存在，两个方向都显式拒绝。
#[test]
fn siblings_cannot_read_each_other_and_the_attempt_has_no_side_effect() {
    let mut session = new_session();
    let root = session.agent().clone();
    let a1 = session
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("spawn a1");
    let a2 = session
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("spawn a2");

    let before = session.primitives();

    let r1 = session.read_ancestor(&a2, &a1, Slot::Messages);
    assert!(matches!(r1, Err(ReadDenied::NotAnAncestor { .. })));
    let r2 = session.read_descendant(&a1, &a2, Slot::Status);
    assert!(matches!(r2, Err(ReadDenied::NotADescendant { .. })));

    assert_eq!(
        session.primitives(),
        before,
        "被拒的跨 agent 读不该改变任何 primitive"
    );
}

/// 第二类：自读。`x` 严格意义上既不是自己的祖先也不是自己的后代，所以自读
/// 走的是跟兄弟读同一条拒绝路径，不是一个单独开的后门。
#[test]
fn reading_yourself_is_rejected_the_same_way_not_special_cased() {
    let mut session = new_session();
    let (child, _) = spawn_two_level(&mut session);

    let before = session.primitives();

    let r1 = session.read_ancestor(&child, &child, Slot::Messages);
    assert!(matches!(r1, Err(ReadDenied::NotAnAncestor { .. })));
    let r2 = session.read_descendant(&child, &child, Slot::Status);
    assert!(matches!(r2, Err(ReadDenied::NotADescendant { .. })));

    assert_eq!(session.primitives(), before);
}

/// 第三类：方向传反。两个 agent 之间确实存在祖先/后代关系，只是调用点把
/// `reader`/`target` 的角色搞反了。
#[test]
fn swapping_the_direction_is_rejected_even_for_real_relatives() {
    let mut session = new_session();
    let root = session.agent().clone();
    let (child, grandchild) = spawn_two_level(&mut session);

    let before = session.primitives();

    // child 是 grandchild 的祖先，但拿去问 read_descendant(reader=grandchild,
    // target=child)：target 不是 reader 的后代，方向反了。
    let r1 = session.read_descendant(&grandchild, &child, Slot::Status);
    assert!(matches!(r1, Err(ReadDenied::NotADescendant { .. })));

    // root 是 child 的祖先，但拿去问 read_ancestor(reader=root, target=child)：
    // target 不是 reader 的祖先，同样是方向反了。
    let r2 = session.read_ancestor(&root, &child, Slot::Messages);
    assert!(matches!(r2, Err(ReadDenied::NotAnAncestor { .. })));

    assert_eq!(session.primitives(), before);
}

/// 第四类：会话外 id。一棵完全不相干的路径（不共享 root 前缀），两个方向都
/// 不是祖先/后代关系，落进同一条拒绝路径，没有第三种「未知会话」的特判。
#[test]
fn an_id_from_nowhere_in_this_tree_is_rejected_the_same_way() {
    let mut session = new_session();
    let root = session.agent().clone();
    let (child, _) = spawn_two_level(&mut session);
    let alien = AgentId::new("some_other_tree/x1");

    let before = session.primitives();

    let r1 = session.read_ancestor(&child, &alien, Slot::Messages);
    assert!(matches!(r1, Err(ReadDenied::NotAnAncestor { .. })));
    let r2 = session.read_descendant(&root, &alien, Slot::Status);
    assert!(matches!(r2, Err(ReadDenied::NotADescendant { .. })));
    // 反过来，拿 alien 当 reader 一样被拒。
    let r3 = session.read_ancestor(&alien, &root, Slot::Messages);
    assert!(matches!(r3, Err(ReadDenied::NotAnAncestor { .. })));

    assert_eq!(
        session.primitives(),
        before,
        "拒绝的跨会话读不该在 family 里留下任何 atom"
    );
}
