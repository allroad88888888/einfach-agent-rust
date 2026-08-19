//! 212 验收 6：`/undo` 掉建立 `await` 的那一轮 → 等待边消失，反向 `await`
//! 从此放行。

use agent_core::value::awaiting::AwaitUntil;
use agent_core::{AgentId, AwaitDenied, ChildConfig, Session, UndoReport};

#[test]
fn undoing_the_turn_that_established_an_await_removes_the_edge_and_unblocks_the_reverse() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    let a = session
        .spawn_child(&root, ChildConfig::default(), None)
        .unwrap();
    let b = session
        .spawn_child(&root, ChildConfig::default(), None)
        .unwrap();

    // 建边这件事自己独占一轮，undo_turn 才只退它，不连带 spawn。
    session.begin_turn();
    session
        .await_agent(&a, &b, AwaitUntil::Settled)
        .expect("A await B 该成功");
    assert_eq!(
        session.awaiting_on(&a),
        vec![(b.clone(), AwaitUntil::Settled)]
    );

    // 反向此刻确实被挡——先把「挡住」这件事本身钉住，undo 之后的「放行」
    // 才有对照。
    assert!(matches!(
        session.await_agent(&b, &a, AwaitUntil::Settled),
        Err(AwaitDenied::WouldCycle { .. })
    ));

    let report = session.undo_turn();
    assert!(matches!(report, UndoReport::Applied { .. }), "{report:?}");

    // 等待边消失。
    assert!(
        session.awaiting_on(&a).is_empty(),
        "undo 之后 A 的等待边该没了：{:?}",
        session.awaiting_on(&a)
    );

    // 反向 await 从此放行——不再是环，因为原来那条边已经不在了。
    session
        .await_agent(&b, &a, AwaitUntil::Settled)
        .expect("原来那条边被 undo 掉之后，反向 await 该放行");
    assert_eq!(
        session.awaiting_on(&b),
        vec![(a.clone(), AwaitUntil::Settled)]
    );
}

/// `redo` 把这条边带回来——undo/redo 是同一套机制的两个投影，不是只有
/// undo 半边被测过。
#[test]
fn redo_brings_the_edge_back_and_the_reverse_is_blocked_again() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    let a = session
        .spawn_child(&root, ChildConfig::default(), None)
        .unwrap();
    let b = session
        .spawn_child(&root, ChildConfig::default(), None)
        .unwrap();

    session.begin_turn();
    session.await_agent(&a, &b, AwaitUntil::Settled).unwrap();

    let _ = session.undo_turn();
    assert!(session.awaiting_on(&a).is_empty());

    let report = session.redo_turn();
    assert!(matches!(report, UndoReport::Applied { .. }), "{report:?}");

    assert_eq!(
        session.awaiting_on(&a),
        vec![(b.clone(), AwaitUntil::Settled)],
        "redo 之后边该原样回来"
    );
    assert!(matches!(
        session.await_agent(&b, &a, AwaitUntil::Settled),
        Err(AwaitDenied::WouldCycle { .. })
    ));
}

/// `stop_awaiting` 是另一条独立的撤销路径（不经 undo，直接命令），同样让边
/// 消失、反向放行——两条路径各自成立，互不依赖。
#[test]
fn stop_awaiting_also_clears_the_edge_without_going_through_undo() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    let a = session
        .spawn_child(&root, ChildConfig::default(), None)
        .unwrap();
    let b = session
        .spawn_child(&root, ChildConfig::default(), None)
        .unwrap();

    session.await_agent(&a, &b, AwaitUntil::Settled).unwrap();
    session.stop_awaiting(&a, &b);

    assert!(session.awaiting_on(&a).is_empty());
    session
        .await_agent(&b, &a, AwaitUntil::Settled)
        .expect("stop_awaiting 之后反向该放行");
}
