//! Issue 104：一次成功的 `advance_boundary` 之后**看得见的效果**——发送侧的
//! `project` 输出、库里记录的条数、日志条目的形状、undo 能不能退回去。
//! `advance_boundary` 自己的 `Result` 三分支契约见 `advance_boundary_command.rs`。
//!
//! 第 4 档「清窗口」= `next` 取历史长度、`summary` 传 `None`（104 目标一节），
//! 所以这里的测试全部走这一支；第 3 档「主动摘要」的投影行为已经在 099 的
//! `send_plan_project_boundary.rs` 独立验过，不在这里重复。

use agent_core::value::send_plan::project;
use agent_core::{AgentId, AtomKey, Session, Slot, SummaryId, UndoReport};

use crate::support;
use crate::support::session::new_session;

/// 造一个跑了 `rounds` 轮「用户问 + 模型纯文本答」的会话——不涉及工具，每轮
/// 稳定新增用户 + 助手各一条消息，边界数字因此能直接对上 `messages_of` 的
/// 长度，不需要猜测其它事件类型各添了几条消息。
fn session_with_rounds(rounds: usize) -> Session {
    let mut s = new_session();
    for i in 0..rounds {
        if i > 0 {
            s.begin_turn();
        }
        let _ = s.step(support::user_input_event(&format!("第 {i} 轮问题")));
        let _ = s.step(support::provider_done_end_turn(
            s.epoch(),
            &format!("第 {i} 轮回答"),
        ));
    }
    s
}

/// 验收第一条：清窗口（`next` = 当时的历史长度、`summary = None`）之后，
/// 再长出的新消息之上，`project` 的输出只剩边界之后的部分——边界之前那两轮
/// 问答不出现在投影里（它们还原样躺在 `messages_of` 里，见下一条验收）。
#[test]
fn clearing_the_window_leaves_only_messages_sent_after_the_boundary() {
    let mut s = session_with_rounds(2);
    let root = AgentId::root();
    let boundary = s.messages_of(&root).len();

    s.advance_boundary(&root, boundary, None)
        .expect("清窗口——边界推到当前历史长度——该成功");

    s.begin_turn();
    let _ = s.step(support::user_input_event("第三轮问题"));
    let _ = s.step(support::provider_done_end_turn(s.epoch(), "第三轮回答"));

    let full_history = s.messages_of(&root);
    assert!(full_history.len() > boundary, "又新长出了消息");

    let plan = s.send_plan_of(&root);
    let projected = project(&full_history, &plan, None);

    let expected: Vec<_> = full_history.iter().skip(boundary).cloned().collect();
    assert_eq!(
        projected, expected,
        "投影只剩边界之后新长出来的消息，边界之前的两轮问答不该出现"
    );
}

/// 验收第二条：`/undo` 一次——这里走 `undo_step`，因为一次 `advance_boundary`
/// 调用正是一条 entry（跟 `replace_send_plan` 同款，见既有先例
/// `send_plan_of_session_default.rs`）——边界退回，被盖住的消息全部重新出现。
#[test]
fn undo_step_after_clearing_the_window_brings_back_every_covered_message() {
    let mut s = session_with_rounds(2);
    let root = AgentId::root();
    let full_history = s.messages_of(&root);
    let boundary = full_history.len();

    s.advance_boundary(&root, boundary, None).unwrap();
    let projected_after_clear = project(&full_history, &s.send_plan_of(&root), None);
    assert!(
        projected_after_clear.is_empty(),
        "清窗口之后没有边界之后的消息可发"
    );

    let report = s.undo_step();
    assert!(
        matches!(report, UndoReport::Applied { entries: 1, .. }),
        "{report:?}"
    );

    let plan = s.send_plan_of(&root);
    assert!(plan.is_pristine(), "退回清窗口之前——pristine 计划");
    let projected_after_undo = project(&full_history, &plan, None);
    let expected: Vec<_> = full_history.iter().cloned().collect();
    assert_eq!(projected_after_undo, expected, "被盖住的消息全部重新出现");
}

/// 验收第三条：清窗口只改发送侧的坐标，不动库里的记录——`messages_of` 的长度
/// 一条没少。跟「空间管理」（`SessionStore::drop_oldest`，那是记录真的从库里
/// 丢了）是两套机制，095 的分界。
#[test]
fn clearing_the_window_does_not_remove_a_single_stored_message() {
    let mut s = session_with_rounds(3);
    let root = AgentId::root();
    let before = s.messages_of(&root).len();

    s.advance_boundary(&root, before, None).unwrap();

    assert_eq!(
        s.messages_of(&root).len(),
        before,
        "记录还在库里，一条没少——只是发送侧不发"
    );
}

/// 验收第四条：`advance_boundary` 落的那条 entry，`SendPlan` 槽位那个 change
/// 的 `prev` 序列化 < 1KB——它装的是一个 `SendPlan`（一个数 + 一个可选 id），
/// 不该大。
#[test]
fn the_journaled_entrys_prev_serializes_under_one_kilobyte() {
    let mut s = session_with_rounds(2);
    let root = AgentId::root();
    let boundary = s.messages_of(&root).len();

    s.advance_boundary(&root, boundary, None).unwrap();

    let entry = s.last_entry().expect("advance_boundary 该留下一条 entry");
    let send_plan_key = AtomKey::Agent(root.clone(), Slot::SendPlan);
    let change = entry
        .changes
        .iter()
        .find(|c| c.key == send_plan_key)
        .expect("entry 里该有一条改 SendPlan 槽位的 change");

    let bytes = serde_json::to_vec(&change.prev).expect("AgentValue 全部可序列化（红线 3）");
    assert!(
        bytes.len() < 1024,
        "prev 装的是个数，不该到 1KB：实际 {} bytes",
        bytes.len()
    );
}

/// 额外一：边界与摘要引用是**同一条 entry** 改的——`SendPlan` 是一个原子，
/// 两个字段活在同一个值里，一次 `advance_boundary` 只产生一条 `Change`
/// （对 `Slot::SendPlan` 这一个键），所以 undo 一次两个字段**一起**退回，
/// 结构上就不存在「只退回一个」的中间态。
#[test]
fn undo_reverts_boundary_and_summary_together_never_partially() {
    let mut s = new_session();
    let root = AgentId::root();

    s.advance_boundary(&root, 3, Some(SummaryId::new("sum_1")))
        .unwrap();
    let after_first = s.send_plan_of(&root);
    assert_eq!(after_first.boundary(), 3);
    assert_eq!(after_first.summary(), Some(&SummaryId::new("sum_1")));

    s.advance_boundary(&root, 7, Some(SummaryId::new("sum_2")))
        .unwrap();

    let entry = s.last_entry().expect("第二次推进该留下一条 entry");
    let send_plan_key = AtomKey::Agent(root.clone(), Slot::SendPlan);
    let touched: Vec<_> = entry
        .changes
        .iter()
        .filter(|c| c.key == send_plan_key)
        .collect();
    assert_eq!(
        touched.len(),
        1,
        "边界和摘要引用活在同一个 SendPlan 原子里，一次推进只该产生一条 change"
    );

    let report = s.undo_step();
    assert!(
        matches!(report, UndoReport::Applied { entries: 1, .. }),
        "{report:?}"
    );

    let reverted = s.send_plan_of(&root);
    assert_eq!(
        reverted, after_first,
        "两个字段一起退回上一条推进的样子，没有中间态"
    );
    assert_eq!(reverted.boundary(), 3);
    assert_eq!(reverted.summary(), Some(&SummaryId::new("sum_1")));
}

/// 额外二：`next` 远大于历史长度不 panic，投影退化成「一条正文都不发」——
/// 099 的既有裁决（`SendPlan` 不知道历史多长，边界不跟历史长度校验）在命令层
/// 原样成立，这里从 `advance_boundary` 的入口验一次。
#[test]
fn a_boundary_far_beyond_history_length_does_not_panic_and_sends_nothing() {
    let mut s = session_with_rounds(1);
    let root = AgentId::root();
    let real_len = s.messages_of(&root).len();

    let result = s.advance_boundary(&root, real_len + 1_000_000, None);
    assert_eq!(result, Ok(()), "越界边界照样接受，不该 panic");

    let full_history = s.messages_of(&root);
    let projected = project(&full_history, &s.send_plan_of(&root), None);
    assert!(projected.is_empty(), "边界越界之后，投影退化成一条正文都不发");
}

/// 额外三：连续两次推进（0→5→9）之后连续 undo 两次，边界依次回到 5、再回到
/// 0——两次推进各是一条独立 entry（跟 `replace_send_plan` 的既有先例同一条
/// 纪律），`undo_step` 一次退一条，所以两次 undo 落在两个不同的中间点上，
/// 不是一步退到底。
#[test]
fn two_advances_then_two_undos_land_on_each_intermediate_boundary() {
    let mut s = new_session();
    let root = AgentId::root();
    assert_eq!(s.send_plan_of(&root).boundary(), 0);

    s.advance_boundary(&root, 5, None).unwrap();
    assert_eq!(s.send_plan_of(&root).boundary(), 5);

    s.advance_boundary(&root, 9, None).unwrap();
    assert_eq!(s.send_plan_of(&root).boundary(), 9);

    let first_undo = s.undo_step();
    assert!(
        matches!(first_undo, UndoReport::Applied { entries: 1, .. }),
        "{first_undo:?}"
    );
    assert_eq!(
        s.send_plan_of(&root).boundary(),
        5,
        "第一次 undo 退到 5，不是一步到 0"
    );

    let second_undo = s.undo_step();
    assert!(
        matches!(second_undo, UndoReport::Applied { entries: 1, .. }),
        "{second_undo:?}"
    );
    assert_eq!(s.send_plan_of(&root).boundary(), 0);
    assert!(s.send_plan_of(&root).is_pristine());
}
