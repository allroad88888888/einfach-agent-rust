//! 026 独立测试：cap 链路——`set_history_cap(Some(小值))` 之后多轮对话会让日志
//! 溢出、从最老一端丢，`take_drop_events()` 里能看到 `Oldest`；一路 undo 到头
//! 返回 `Nothing`，不 panic（cap 裁掉的是已生效区，不会把游标带到一个不存在的
//! 位置上）。

mod support;

use agent_store::DropEvent;

use agent_core::UndoReport;
use support::session::new_session;
use support::{provider_done_end_turn, user_input_event};

#[test]
fn overflowing_the_cap_reports_oldest_drop_events() {
    let mut session = new_session();
    session.set_history_cap(Some(3));

    for i in 0..6 {
        if i > 0 {
            session.begin_turn();
        }
        let _ = session.step(user_input_event(&format!("round {i}")));
        let _ = session.step(provider_done_end_turn(session.epoch(), &format!("answer {i}")));
    }

    assert!(session.history_len() <= 3, "日志长度不该超过设定的上限");

    let drops = session.take_drop_events();
    assert!(!drops.is_empty(), "六轮对话远超上限 3，必然有裁剪事件");
    assert!(
        drops.iter().any(|d| matches!(d, DropEvent::Oldest { .. })),
        "溢出裁剪该报 Oldest，不是别的变体：{drops:?}"
    );

    // 取走即清空，不取就一直攒着——不是每次都重复报告同一批。
    assert!(session.take_drop_events().is_empty(), "取走之后应该是空的");
}

#[test]
fn undoing_all_the_way_to_the_start_returns_nothing_without_panicking() {
    let mut session = new_session();
    session.set_history_cap(Some(3));

    for i in 0..6 {
        if i > 0 {
            session.begin_turn();
        }
        let _ = session.step(user_input_event(&format!("round {i}")));
        let _ = session.step(provider_done_end_turn(session.epoch(), &format!("answer {i}")));
    }
    let _ = session.take_drop_events();

    let mut reached_nothing = false;
    for _ in 0..64 {
        match session.undo_step() {
            UndoReport::Applied { .. } => continue,
            UndoReport::Blocked { .. } => panic!("这个场景里没有任何屏障，不该 Blocked"),
            UndoReport::Nothing => {
                reached_nothing = true;
                break;
            }
        }
    }

    assert!(reached_nothing, "应该能在有限步内退到日志的起点");
    // 端点上再退一次仍然是 Nothing，且不 panic——幂等。
    assert_eq!(session.undo_step(), UndoReport::Nothing);
}

#[test]
fn setting_a_smaller_cap_after_the_fact_trims_immediately() {
    let mut session = new_session();
    for i in 0..5 {
        if i > 0 {
            session.begin_turn();
        }
        let _ = session.step(user_input_event(&format!("round {i}")));
        let _ = session.step(provider_done_end_turn(session.epoch(), &format!("answer {i}")));
    }
    assert!(session.history_len() > 2);

    session.set_history_cap(Some(2));
    assert_eq!(session.history_len(), 2, "调小 cap 立即生效，不用等下一次 append");
}
