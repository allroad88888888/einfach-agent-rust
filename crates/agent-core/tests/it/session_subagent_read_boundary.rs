//! 028：跨 agent 读边界（红线 10）。
//!
//! 验收对应：子读父 Messages ✓；父读子 Status ✓；兄弟读 → API 面不存在（编译期）
//! 加上 `read_ancestor`/`read_descendant` 对方向错误的调用显式拒绝；`visibility()`
//! 穷举且 Upward/Downward 集合不相交。
//!
//! **「兄弟读在 API 面上不存在」是编译期事实，测不出来**——`Session` 上只有
//! `read_ancestor` / `read_descendant` 两个跨 agent 读口，也没有 `store()`
//! 访问器。这里能测的是第二道保险：把兄弟当参数喂进这两个口，它们显式拒绝。

use std::sync::Arc;

use crate::support::{user_input_event, user_input_for};
use agent_core::{AgentId, AgentValue, ChildConfig, ReadDenied, Session, Slot, Visibility};

fn tree() -> (Session, AgentId, AgentId, AgentId) {
    let mut s = Session::new(AgentId::root());
    let root = AgentId::root();
    let a1 = s
        .spawn_child(
            &root,
            ChildConfig {
                tools_allowed: vec![Arc::from("srv:fs/read")],
                ..ChildConfig::default()
            },
            None,
        )
        .unwrap();
    let a2 = s.spawn_child(&root, ChildConfig::default(), None).unwrap();
    (s, root, a1, a2)
}

/// 子读父的消息历史——决策 3 承诺的「子读父是一次 `get`」。
#[test]
fn a_child_can_read_its_ancestors_messages() {
    let (mut s, root, a1, _) = tree();
    let _ = s.step(user_input_event("root 收到的一句话"));

    let value = s.read_ancestor(&a1, &root, Slot::Messages).unwrap();
    let messages = value.as_messages().expect("Messages 槽位持 Messages");
    assert_eq!(messages.len(), 1);
}

/// 隔一层的祖先同样可读——方向仍然朝树根，环的论证不受影响。
#[test]
fn a_grandchild_can_read_the_root() {
    let (mut s, root, a1, _) = tree();
    let a1_a1 = s.spawn_child(&a1, ChildConfig::default(), None).unwrap();
    let _ = s.step(user_input_event("hi"));

    assert!(s.read_ancestor(&a1_a1, &root, Slot::Messages).is_ok());
    assert!(s.read_ancestor(&a1_a1, &a1, Slot::Messages).is_ok());
}

/// 父读子的状态——029 的「等所有子完成」就长在这个方向上。
#[test]
fn a_parent_can_read_its_descendants_status() {
    let (mut s, root, a1, _) = tree();
    let _ = s.step(user_input_for(&a1, "子干活"));

    let value = s.read_descendant(&root, &a1, Slot::Status).unwrap();
    assert_eq!(
        value.as_status().unwrap(),
        &agent_core::TurnStatus::Thinking,
        "读到的是子 agent 自己的轮状态"
    );
    // 活名单也是往下读的：非 Null 就是活着。
    let tools = s.read_descendant(&root, &a1, Slot::ToolsAllowed).unwrap();
    assert!(!matches!(tools, AgentValue::Null));
}

/// **兄弟互读**：两个口各拒各的。横读在 API 面上不存在，这里是第二道保险。
#[test]
fn siblings_are_refused_in_both_directions() {
    let (s, _, a1, a2) = tree();

    assert_eq!(
        s.read_ancestor(&a1, &a2, Slot::Messages),
        Err(ReadDenied::NotAnAncestor {
            reader: a1.clone(),
            target: a2.clone()
        })
    );
    assert_eq!(
        s.read_descendant(&a1, &a2, Slot::Status),
        Err(ReadDenied::NotADescendant {
            reader: a1.clone(),
            target: a2.clone()
        })
    );
}

/// 方向传反了同样显式拒绝——`root/a1` 不是 root 的祖先。
#[test]
fn the_wrong_direction_is_refused() {
    let (s, root, a1, _) = tree();

    assert_eq!(
        s.read_ancestor(&root, &a1, Slot::Messages),
        Err(ReadDenied::NotAnAncestor {
            reader: root.clone(),
            target: a1.clone()
        })
    );
    assert_eq!(
        s.read_descendant(&a1, &root, Slot::Status),
        Err(ReadDenied::NotADescendant {
            reader: a1.clone(),
            target: root.clone()
        })
    );
}

/// 自己读自己也是拒绝：`is_ancestor_of` 是严格的，否则这两个口就成了绕过
/// visibility 的自读后门。
#[test]
fn nobody_reads_itself_through_these_two_doors() {
    let (s, root, a1, _) = tree();
    assert!(s.read_ancestor(&a1, &a1, Slot::Messages).is_err());
    assert!(s.read_descendant(&root, &root, Slot::Status).is_err());
}

/// 别的会话那棵树上的 id，两个口都不认。
#[test]
fn an_agent_from_another_tree_is_refused() {
    let (s, _, a1, _) = tree();
    let alien = AgentId::new("other/a1");
    assert!(s.read_ancestor(&a1, &alien, Slot::Messages).is_err());
    assert!(s.read_descendant(&alien, &a1, Slot::Status).is_err());
}

/// **集合性质，不是几个用例**：对每一个槽位，两个方向最多只有一个能读到。
/// 这就是 `U ∩ D = ∅` 在公开面上的形状——它是「环在结构上不可能」的前提。
#[test]
fn no_slot_is_readable_in_both_directions() {
    let (s, root, a1, _) = tree();

    for slot in Slot::ALL {
        let up = s.read_ancestor(&a1, &root, slot).is_ok();
        let down = s.read_descendant(&root, &a1, slot).is_ok();
        assert!(!(up && down), "{slot:?} 两个方向都能读——环就有可能了");

        // 而且能不能读完全由 visibility 决定，没有第三种可能。
        match slot.visibility() {
            Visibility::Upward => assert!(up && !down, "{slot:?}"),
            Visibility::Downward => assert!(down && !up, "{slot:?}"),
            Visibility::Private => assert!(!up && !down, "{slot:?}"),
        }
    }
}

/// 私有槽位两个方向都拒，且拒绝理由说得出是**可见性**不是方向。
#[test]
fn a_private_slot_is_refused_with_a_visibility_reason() {
    let (s, root, a1, _) = tree();
    assert_eq!(
        s.read_ancestor(&a1, &root, Slot::TurnsUsed),
        Err(ReadDenied::NotVisible {
            slot: Slot::TurnsUsed,
            visibility: Visibility::Private
        })
    );
    assert_eq!(
        s.read_descendant(&root, &a1, Slot::Messages),
        Err(ReadDenied::NotVisible {
            slot: Slot::Messages,
            visibility: Visibility::Upward
        })
    );
}

/// 被 despawn 的子 agent：方向和可见性都对，但 atom 已经不在图上——说「读不到」，
/// **不顺手建一个**。留下的墓碑（`ToolsAllowed`）还读得到，答的是 `Null`，
/// 也就是「它不在活名单上」。
#[test]
fn reading_a_despawned_child_says_so_instead_of_creating_atoms() {
    let (mut s, root, a1, _) = tree();
    let before = s.primitives().len();
    let _ = s.despawn_child(&a1).unwrap();

    assert!(matches!(
        s.read_descendant(&root, &a1, Slot::Status),
        Err(ReadDenied::NoSuchAtom { .. })
    ));
    assert_eq!(
        s.read_descendant(&root, &a1, Slot::ToolsAllowed),
        Ok(AgentValue::Null)
    );
    assert!(
        s.primitives().len() < before,
        "读取路径没有把逐出的 atom 建回来"
    );
}
