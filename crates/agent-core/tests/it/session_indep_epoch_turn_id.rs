//! 026 独立测试：`epoch` / `turn_id` 不进原子图——世代只增不减。
//!
//! undo 之后 `epoch()` 是 bump 过的新值（undo 不会、也不可能把它回滚：它根本
//! 不是一个 primitive，没有 `prev` 可写回）；`turn_id()` 同理不回退——哪怕
//! `undo_turn` 把整个新轮的 primitive 全部退回上一轮结束时的样子，`turn_id()`
//! 仍然报告新轮的号码，因为它是日志的分组依据，不是被日志记录的状态。

use agent_core::UndoReport;
use crate::support::session::new_session;
use crate::support::{provider_done_end_turn, user_input_event};

#[test]
fn epoch_only_moves_forward_across_undo_and_redo() {
    let mut session = new_session();
    let initial_epoch = session.epoch();

    let _ = session.step(user_input_event("hi"));
    let _ = session.step(provider_done_end_turn(session.epoch(), "bye"));
    assert_eq!(session.epoch(), initial_epoch, "普通转移不碰 epoch");

    let _ = session.undo_step();
    let epoch_after_first_undo = session.epoch();
    assert_ne!(epoch_after_first_undo, initial_epoch, "undo 必须 bump");

    let _ = session.undo_step();
    let epoch_after_second_undo = session.epoch();
    assert_ne!(
        epoch_after_second_undo, epoch_after_first_undo,
        "每次 undo 各自 bump 一次"
    );

    let redo1 = session.redo_step();
    assert!(matches!(redo1, UndoReport::Applied { .. }));
    assert_eq!(session.epoch(), epoch_after_second_undo, "redo 不 bump");

    let redo2 = session.redo_step();
    assert!(matches!(redo2, UndoReport::Applied { .. }));
    assert_eq!(
        session.epoch(),
        epoch_after_second_undo,
        "redo 到底也不会把 epoch 还原成更早的值"
    );
    assert_ne!(session.epoch(), initial_epoch, "epoch 永远回不到 undo 之前");
}

#[test]
fn turn_id_survives_an_undo_turn_that_rolls_back_the_whole_turn_it_belongs_to() {
    let mut session = new_session();
    let _ = session.step(user_input_event("turn one"));
    let _ = session.step(provider_done_end_turn(session.epoch(), "answer one"));
    assert_eq!(session.turn_id(), 1);

    session.begin_turn();
    assert_eq!(session.turn_id(), 2);
    let _ = session.step(user_input_event("turn two"));
    let _ = session.step(provider_done_end_turn(session.epoch(), "answer two"));
    assert_eq!(session.turn_id(), 2);

    // undo_turn 回退的是整个第二轮——包括 begin_turn 自己写下的那条 entry，
    // primitive 状态会回到第一轮结束时的样子，但 turn_id() 不是 primitive，
    // undo 没有任何路径能把它写回去。
    let report = session.undo_turn();
    assert!(matches!(report, UndoReport::Applied { turn_id: 2, .. }));
    assert_eq!(session.turn_id(), 2, "turn_id 不回退：日志分组依据只增不减");
    assert_eq!(
        session.status(),
        agent_core::TurnStatus::Done { truncated: false },
        "primitive 确实回到了第一轮结束时的样子"
    );

    // 再开一轮拿到的是全新的号码，不是被退掉的那个 2。
    session.begin_turn();
    assert_eq!(session.turn_id(), 3, "新一轮拿到的是没被用过的号");
}
