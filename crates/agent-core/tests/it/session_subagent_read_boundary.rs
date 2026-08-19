//! 028 + 决策 35：跨 agent 读边界（红线 10）。
//!
//! **决策 35 改写了这个文件测的性质。** 028 时代的红线 10 是「只上下读、禁横读」，
//! 这里断言的是 `U ∩ D = ∅`（每个槽位至多一个方向可读）和「兄弟两个口各拒各的」。
//! 决策 35 之后横读全开，判据只剩一条：**这个槽位是不是别人的内部账本**
//! （`Visibility::Private`）。
//!
//! 所以本文件现在断言三组：
//!
//! 1. `read_agent` 不限方向——**兄弟读得到兄弟**，这是这一波的行为核心；
//! 2. `Private` 仍然挡得住，且拒绝理由说得出是可见性；
//! 3. `read_ancestor` / `read_descendant` 作为**带亲缘断言的封装**行为不变
//!    ——它们断言的是「你俩是不是祖先/后代」，不再是槽位的方向。

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

// ——— 一、横读全开（决策 35 的行为核心）———————————————————

/// **兄弟读得到兄弟**。028 起这条一直是被拒的，决策 35 把它打开——
/// `srv:agent/status`（207）看得见整棵树、`srv:agent/send`（206）发得到兄弟，
/// 都建立在这一条上。
#[test]
fn a_sibling_can_now_read_a_sibling() {
    let (mut s, _, _a1, a2) = tree();
    let _ = s.step(user_input_for(&a2, "a2 在干活"));

    let value = s.read_agent(&a2, Slot::Status).unwrap();
    assert_eq!(
        value.as_status().unwrap(),
        &agent_core::TurnStatus::Thinking,
        "a1 读到的是兄弟 a2 自己的轮状态"
    );
}

/// 三个方向对 `read_agent` 是同一件事：兄弟、祖先、后代都读得到。
/// **它一条方向断言都没有**——那是两个封装的事。
#[test]
fn read_agent_does_not_care_about_direction() {
    let (mut s, root, a1, a2) = tree();
    let _ = s.step(user_input_event("root 收到的一句话"));

    assert!(s.read_agent(&root, Slot::Messages).is_ok(), "往上");
    assert!(s.read_agent(&a1, Slot::Status).is_ok(), "往下");
    assert!(s.read_agent(&a2, Slot::Status).is_ok(), "横着");
}

/// 决策 35 之前 `Messages` 是 Upward-only，父读子在 core 那层就拿不到；现在
/// `Shared`，**父和兄弟都读得到别人的完整正文**。
///
/// 这条是决策 35 §一 点名的两样真代价之一：一次读进自己的 prompt 就能把一轮成本
/// 翻几倍。core 这一层放行，**工具层不给模型开按槽位读它的入口**（模型侧要正文有
/// `collect` 和 `send`）。这里断言的是 core 的行为，不是模型能做的事。
#[test]
fn messages_are_now_readable_across_agents_at_the_core_layer() {
    let (mut s, root, a1, _) = tree();
    let _ = s.step(user_input_for(&a1, "子说的话"));

    assert!(s.read_descendant(&root, &a1, Slot::Messages).is_ok());
    assert!(s.read_agent(&a1, Slot::Messages).is_ok());
}

// ——— 二、`Private` 仍然挡得住 ———————————————————————

/// 私有槽位任何方向都拒，且拒绝理由说得出是**可见性**。
#[test]
fn a_private_slot_is_refused_with_a_visibility_reason() {
    let (s, root, a1, _) = tree();

    for (label, target) in [("祖先", &root), ("后代", &a1)] {
        assert_eq!(
            s.read_agent(target, Slot::TurnsUsed),
            Err(ReadDenied::NotVisible {
                slot: Slot::TurnsUsed
            }),
            "{label}的 TurnsUsed 该被可见性挡住"
        );
    }
    assert_eq!(
        s.read_ancestor(&a1, &root, Slot::Summaries),
        Err(ReadDenied::NotVisible {
            slot: Slot::Summaries
        }),
        "封装也走同一道可见性闸"
    );
}

/// **集合性质，不是几个用例**：对每一个槽位，能不能跨 agent 读到完全由
/// `visibility()` 决定，没有第三种可能，也不随方向变。
///
/// 决策 35 之前这里断言的是 `U ∩ D = ∅`（「环在结构上不可能」的前提）。方向去掉
/// 之后那个前提换了地方——跨 agent 的读**一条边都不建**，第一条真的跨 agent 边由
/// `srv:agent/await` 的 derived 建出来（issue 212），无环的断言在那里。
#[test]
fn readability_is_decided_by_visibility_alone_in_every_direction() {
    let (s, root, a1, a2) = tree();

    for slot in Slot::ALL {
        let expected = slot.visibility() == Visibility::Shared;
        for (label, target) in [("祖先", &root), ("后代", &a1), ("兄弟", &a2)] {
            assert_eq!(
                s.read_agent(target, slot).is_ok(),
                expected,
                "{slot:?} 读{label}：可见性说 {:?}",
                slot.visibility()
            );
        }
    }
}

// ——— 三、两个封装：断言的是亲缘，不是槽位方向 ————————————

/// 子读父的消息历史——决策 3 承诺的「子读父是一次 `get`」。
#[test]
fn a_child_can_read_its_ancestors_messages() {
    let (mut s, root, a1, _) = tree();
    let _ = s.step(user_input_event("root 收到的一句话"));

    let value = s.read_ancestor(&a1, &root, Slot::Messages).unwrap();
    let messages = value.as_messages().expect("Messages 槽位持 Messages");
    assert_eq!(messages.len(), 1);
}

/// 隔一层的祖先同样可读。
#[test]
fn a_grandchild_can_read_the_root() {
    let (mut s, root, a1, _) = tree();
    let a1_a1 = s.spawn_child(&a1, ChildConfig::default(), None).unwrap();
    let _ = s.step(user_input_event("hi"));

    assert!(s.read_ancestor(&a1_a1, &root, Slot::Messages).is_ok());
    assert!(s.read_ancestor(&a1_a1, &a1, Slot::Messages).is_ok());
}

/// 父读子的状态。
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
    // 活名单也读得到：非 Null 就是活着。
    let tools = s.read_descendant(&root, &a1, Slot::ToolsAllowed).unwrap();
    assert!(!matches!(tools, AgentValue::Null));
}

/// **两个封装仍然拒兄弟**——它们断言的是亲缘关系，而兄弟不是祖先也不是后代。
/// 要横读走 `read_agent`（上面第一组）。这条是刻意保留的：调用点写
/// `read_ancestor` 就是在说「我读的是我祖先」，说错了该被拦住。
#[test]
fn the_two_wrappers_still_refuse_siblings_because_they_assert_kinship() {
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

/// 自己读自己经两个封装是拒绝：`is_ancestor_of` 是严格的。
///
/// 但 `read_agent(自己, ...)` 是**放行**的——它压根不问「你是谁」，只问「这个槽位
/// 别人读不读得到」。自读本来就不需要许可（`Private` 的含义是「**别的** agent
/// 读不到」），所以这不是后门，是这个口本来的语义。
#[test]
fn self_read_is_refused_by_the_wrappers_but_read_agent_has_no_such_notion() {
    let (s, root, a1, _) = tree();

    assert!(s.read_ancestor(&a1, &a1, Slot::Messages).is_err());
    assert!(s.read_descendant(&root, &root, Slot::Status).is_err());
    assert!(s.read_agent(&a1, Slot::Messages).is_ok());
}

/// 别的会话那棵树上的 id：封装按亲缘拒，`read_agent` 按「图上没有这个 atom」拒。
/// **两条都不该顺手建一个 atom。**
#[test]
fn an_agent_from_another_tree_is_refused_without_creating_anything() {
    let (s, _, a1, _) = tree();
    let alien = AgentId::new("other/a1");
    let before = s.primitives();

    assert!(s.read_ancestor(&a1, &alien, Slot::Messages).is_err());
    assert!(s.read_descendant(&alien, &a1, Slot::Status).is_err());
    assert!(matches!(
        s.read_agent(&alien, Slot::Messages),
        Err(ReadDenied::NoSuchAtom { .. })
    ));

    assert_eq!(s.primitives(), before, "被拒的跨 agent 读不该建出任何 atom");
}

/// 被 despawn 的子 agent：可见性没问题，但 atom 已经不在图上——说「读不到」，
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
