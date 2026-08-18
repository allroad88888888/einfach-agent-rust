//! 205 独立测试（二）：`deliver` 的**拒绝面**——每一种都是显式变体，不是
//! `Option`，也不是投完静默丢。
//!
//! 黑盒来源：docs/issues/205-core-peek-and-inbox.md「做什么」3 与「验收」、
//! docs/issues/204-agent-mesh-decision.md §二（`next_turn` 只能投给 root 的理由：
//! 子 agent 不跨 turn）。**实现体一行没读**（见 `inbox_indep.rs` 顶部）。
//!
//! 这一份跟送达行为分开，是因为它测的是**另一件事**：不是「消息怎么到」，
//! 而是「哪些投递压根不该发生」。

use std::sync::Arc;

use crate::inbox_indep::tree;
use agent_core::{AgentId, Deliver, DeliverDenied};

/// **静默丢消息的唯一入口**：`NextTurn` 投给一个下一轮不存在的收件箱。
/// 子 agent 不跨 turn（孤儿在 turn 收尾被拆掉），所以下一轮只有 root 还在。
#[test]
fn a_next_turn_delivery_to_a_non_root_target_is_refused_explicitly() {
    let (mut session, root, a1, a2) = tree();
    let history_before = session.history_len();

    match session.deliver(&a2, &a1, Arc::from("下一轮见"), Deliver::NextTurn) {
        Err(DeliverDenied::NextTurnMustTargetRoot { target, root: r }) => {
            assert_eq!(target, a1);
            assert_eq!(r, root, "拒绝理由要说清合法目标是谁");
        }
        other => panic!("投给一个下一轮不存在的收件箱，该当场显式拒，得到 {other:?}"),
    }

    assert!(session.inbox_of(&a1).is_empty(), "拒了就不该留下条目");
    assert_eq!(session.history_len(), history_before, "被拒的投递不落 entry");
}

/// 同一句话改成 `Now` 就该通过——上一条拒的是**时机**，不是这对收发人。
#[test]
fn the_same_pair_is_fine_with_the_now_mark() {
    let (mut session, _root, a1, a2) = tree();
    session
        .deliver(&a2, &a1, Arc::from("兄弟这就说"), Deliver::Now)
        .expect("Now 档的合法目标是本轮任意活 agent，含兄弟");
    assert_eq!(session.inbox_of(&a1).len(), 1);
}

/// 目标已经 despawn：显式拒，不静默丢。
#[test]
fn delivering_to_a_despawned_agent_is_refused_explicitly() {
    let (mut session, root, a1, _a2) = tree();
    let _ = session.despawn_child(&a1).expect("despawn a1");
    assert!(!session.is_live(&a1), "fixture 没能把 a1 弄死");

    match session.deliver(&root, &a1, Arc::from("还在吗"), Deliver::Now) {
        Err(DeliverDenied::TargetNotLive { target }) => assert_eq!(target, a1),
        other => panic!("投给已经死掉的 agent 该显式拒，得到 {other:?}"),
    }
}

/// 发送方自己不活着，同样拒——`from` 只是个路径 id，谁都写得出来。
#[test]
fn a_dead_sender_is_refused_explicitly() {
    let (mut session, _root, a1, a2) = tree();
    let _ = session.despawn_child(&a1).expect("despawn a1");

    match session.deliver(&a1, &a2, Arc::from("我还在"), Deliver::Now) {
        Err(DeliverDenied::SenderNotLive { from }) => assert_eq!(from, a1),
        other => panic!("死掉的发送方该显式拒，得到 {other:?}"),
    }
    assert!(session.inbox_of(&a2).is_empty(), "拒了就不该留下条目");
}

/// 别的树上的 id：跨会话不共享 store，一律不认（跟 `read_agent` 对会话外 id
/// 的处理同一条理由）。
#[test]
fn an_id_from_another_tree_is_refused_as_not_in_session() {
    let (mut session, root, _a1, _a2) = tree();
    let alien = AgentId::new("some_other_tree/a1");

    match session.deliver(&root, &alien, Arc::from("你好"), Deliver::Now) {
        Err(DeliverDenied::NotInSession { target }) => assert_eq!(target, alien),
        other => panic!("会话外的 id 该按「不在这棵树上」拒，得到 {other:?}"),
    }
}

/// 空正文：一条什么都没说的消息进对方 prompt 是纯浪费。
#[test]
fn an_empty_text_is_refused() {
    let (mut session, root, a1, _a2) = tree();
    let history_before = session.history_len();

    assert_eq!(
        session.deliver(&root, &a1, Arc::from(""), Deliver::Now),
        Err(DeliverDenied::EmptyText)
    );

    assert!(session.inbox_of(&a1).is_empty());
    assert_eq!(session.history_len(), history_before, "被拒的投递不落 entry");
}

/// 发给自己：自己给自己写条子有 `Notes`（209），不是走收件箱。
#[test]
fn delivering_to_yourself_is_refused() {
    let (mut session, root, a1, _a2) = tree();

    match session.deliver(&a1, &a1, Arc::from("自言自语"), Deliver::Now) {
        Err(DeliverDenied::ToYourself { agent }) => assert_eq!(agent, a1),
        other => panic!("发给自己该显式拒，得到 {other:?}"),
    }
    match session.deliver(&root, &root, Arc::from("自言自语"), Deliver::NextTurn) {
        Err(DeliverDenied::ToYourself { agent }) => assert_eq!(agent, root),
        other => panic!("root 给自己留条也是发给自己，得到 {other:?}"),
    }

    assert!(session.inbox_of(&a1).is_empty());
    assert!(session.inbox_of(&root).is_empty());
}
