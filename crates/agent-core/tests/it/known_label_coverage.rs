//! **每一条命令落下的 label 都必须能被 `known_label` 认回来。**
//!
//! 漏一条的症状是「**用过那个功能的会话恢复不回来**」：`recover` 撞
//! `InvalidHistory::UnknownLabel` 直接硬失败——不是退化、不是丢一条，是整个会话
//! 起不来。
//!
//! # 这条测试是补一个真出过事的缺口
//!
//! 206 落地时漏了 `"deliver"`（`srv:agent/send` 那条命令的 label），一直到 211 的
//! 独立测试 agent 造了一个「带留言的会话落盘再恢复」的场景才浮出来。当时**没有
//! 任何测试同时做过「用 send」和「持久化往返」这两件事**——`inbox_indep_*` 全在
//! 内存里跑，`jsonl_restart_*` 里没有人 send。两边各自全绿，中间那一格是空的。
//!
//! 所以这条测试的形状是刻意的：**跑一遍真的命令，逐条 entry 问 `known_label`**，
//! 而不是「维护一份跟 `KNOWN_LABELS` 平行的清单」——那种写法只会在有人两边一起
//! 忘记时同样全绿。

use std::sync::Arc;

use agent_core::value::inbox::Deliver;
use agent_core::{AgentId, ChildConfig, Session, known_label};

/// 把 M20 那一族命令连同几条老命令**真的跑一遍**，然后逐条 entry 问
/// 「这个 label 落盘之后还认得回来吗」。
///
/// 新增一条命令时这条会不会红，取决于有没有人在这里也调它一次——所以下面每一句
/// 都带着它对应的 issue 号，加命令的人照着补一行。
#[test]
fn every_label_a_real_session_produces_survives_a_persist_round_trip() {
    let root = AgentId::root();
    let mut session = Session::new(root.clone());

    // 老命令（026/028）：开一轮、改上限、长一个子 agent。
    session.begin_turn();
    session.set_max_turns(8);
    let child = session
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("spawn");

    // 205/206：投递两档 + 两个排空定点。
    session
        .deliver(&child, &root, Arc::from("下一轮再说"), Deliver::NextTurn)
        .expect("deliver next_turn");
    session
        .deliver(&root, &child, Arc::from("现在就听着"), Deliver::Now)
        .expect("deliver now");
    assert_eq!(session.drain_now(&child), 1, "夹具前提：真的搬了一条");
    assert_eq!(session.drain_next_turn(), 1, "夹具前提：真的搬了一条");

    // 209：草稿纸。
    session
        .set_note(&root, Arc::from("plan"), Some(Arc::from("先读 A")))
        .expect("set_note");

    // 逐条问。**报出是哪一条**——只说「有一条不认识」的话，下一个人还得自己
    // 二分找出来。
    let unknown: Vec<&str> = session
        .history()
        .entries()
        .map(|entry| entry.meta.label)
        .filter(|label| known_label(label).is_none())
        .collect();
    assert!(
        unknown.is_empty(),
        "这些 label 落盘之后 `known_label` 认不回来，用过它们的会话恢复不了：{unknown:?}\n\
         往 `agent-core/src/command/meta.rs` 的 KNOWN_LABELS 里补上。"
    );

    // 前提自检：上面真的产生了 entry。命令一条都没落账的话，上面那个断言会在
    // 一个空集合上通过——绿得毫无意义。
    assert!(
        session.history_len() >= 6,
        "夹具前提：上面那几条命令该各落一条 entry，实际只有 {}",
        session.history_len()
    );
}

/// 转移表那一侧同理：**每一种事件走一遍**，label 都得认得回来。
///
/// 跟上面分开是因为来源不同——那些是显式命令的字符串字面量，这些是
/// `transitions::label_of` 的返回值。两处各自都能漏，`Event::Wake`（214）
/// 就是最新的一个。
#[test]
fn every_transition_label_survives_the_round_trip() {
    // `label_of` 不是公开面，所以这里走它的产物：跑一段真的对话，
    // 逐条 entry 问同一个问题。
    use agent_core::Event;

    use crate::support::{provider_done_end_turn, user_input_event};

    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    let _ = session.step(user_input_event("你好"));
    let epoch = session.epoch();
    let _ = session.step(provider_done_end_turn(epoch, "答完了"));
    // 214：终态 → 唤醒。`Messages` 非空、预算没用完，所以它真的会动。
    let _ = session.step(Event::Wake {
        agent: root.clone(),
        epoch: session.epoch(),
    });

    let unknown: Vec<&str> = session
        .history()
        .entries()
        .map(|entry| entry.meta.label)
        .filter(|label| known_label(label).is_none())
        .collect();
    assert!(unknown.is_empty(), "转移表的 label 认不回来：{unknown:?}");
    assert!(
        session
            .history()
            .entries()
            .any(|entry| entry.meta.label == "wake"),
        "夹具前提：唤醒那一步真的落了 entry（没落的话这条什么都没测）"
    );
}
