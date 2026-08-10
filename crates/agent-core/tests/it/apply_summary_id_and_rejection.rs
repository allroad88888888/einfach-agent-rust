//! Issue 107：`SummaryId` **由 `upto` 派生，不需要调用方给**——这条决定了两件事：
//! 同一个 `upto` 无论正文是什么都该出同一个 id（这也是「同一份历史重放两次，
//! 摘要 id 逐字节相同」的前提，红线 1 的落点）；不同 `upto` 该出不同 id。
//!
//! 顺带验 `apply_summary` 复用 104 的 `BoundaryRejected`——边界只能前进，
//! 拒绝不留痕，跟 `advance_boundary` 同一条纪律（`Result<SummaryId, BoundaryRejected>`
//! 这个签名本身就是这条纪律的证据：107 没有另开一个错误类型）。

use std::sync::Arc;

use agent_core::{AgentId, BoundaryRejected};

use crate::support::session::new_session;

/// 两个完全独立的会话，用同一个 `upto`、不同的摘要正文——该出同一个
/// `SummaryId`。这条区分开「id 由 upto 派生」和「id 由正文内容派生」两种可能
/// 的实现：正确答案只认 upto。
#[test]
fn the_same_upto_derives_the_same_summary_id_regardless_of_summary_text() {
    let root = AgentId::root();

    let mut a = new_session();
    let id_a = a.apply_summary(&root, 7, Arc::from("摘要文本 A")).unwrap();

    let mut b = new_session();
    let id_b = b
        .apply_summary(&root, 7, Arc::from("完全不同的摘要文本 B，长度也不一样"))
        .unwrap();

    assert_eq!(
        id_a, id_b,
        "SummaryId 只由 upto 派生，两个独立会话用同一个 upto 该出同一个 id，\
         不管正文写了什么"
    );
}

#[test]
fn different_upto_derive_different_summary_ids() {
    let mut s = new_session();
    let root = AgentId::root();

    let id_1 = s.apply_summary(&root, 5, Arc::from("摘要 1")).unwrap();
    let id_2 = s.apply_summary(&root, 9, Arc::from("摘要 2")).unwrap();

    assert_ne!(id_1, id_2, "不同的 upto 该派生出不同的 SummaryId");
}

/// 边界只能前进：比当前更小的 `upto` 被拒，状态原地不动，不留痕——
/// `advance_boundary_command.rs`「拒绝不留痕」同款断言。
#[test]
fn a_smaller_upto_is_rejected_without_leaving_a_trace() {
    let mut s = new_session();
    let root = AgentId::root();

    s.apply_summary(&root, 5, Arc::from("摘要正文")).unwrap();
    let before_plan = s.send_plan_of(&root);
    let before_len = s.history_len();

    let result = s.apply_summary(&root, 3, Arc::from("迟到或过期的摘要"));

    assert_eq!(
        result,
        Err(BoundaryRejected::NotAdvancing {
            current: 5,
            requested: 3
        }),
        "跟 advance_boundary 同一条纪律：边界只能前进，复用同一个 BoundaryRejected"
    );
    assert_eq!(s.send_plan_of(&root), before_plan, "拒绝之后状态原地不动");
    assert_eq!(s.history_len(), before_len, "拒绝不该产生新 entry");
}

/// 同一个 `upto`、同一份正文再调一次：跟已经生效的那份逐字相同——幂等，
/// `Ok` 同一个 id，**不产生新 entry**（同 104 的第二种情况：`next == current`
/// 且摘要引用相同 → 幂等无操作）。
#[test]
fn re_applying_the_same_upto_with_the_same_text_is_idempotent() {
    let mut s = new_session();
    let root = AgentId::root();

    let id_first = s.apply_summary(&root, 5, Arc::from("摘要正文")).unwrap();
    let before_len = s.history_len();

    let result = s.apply_summary(&root, 5, Arc::from("摘要正文"));

    assert_eq!(result, Ok(id_first), "重复同一份摘要该是幂等 Ok，id 不变");
    assert_eq!(s.send_plan_of(&root).boundary(), 5, "边界不该因为幂等调用而变化");
    assert_eq!(s.history_len(), before_len, "幂等不该留下第二条 entry");
}

/// 同一个 `upto`、**不同**正文再调一次：这是 104「同边界换摘要」定成拒绝的
/// 那件事换了个入口——`upto` 相同意味着派生出的 `SummaryId` 也相同，判定点从
/// 「比 id」挪到「比正文」，拒绝的语义没变：`Err(SameBoundaryDifferentSummary)`，
/// 不留痕。
#[test]
fn re_applying_the_same_upto_with_different_text_is_rejected() {
    let mut s = new_session();
    let root = AgentId::root();

    s.apply_summary(&root, 5, Arc::from("摘要正文")).unwrap();
    let before_plan = s.send_plan_of(&root);
    let before_len = s.history_len();

    let result = s.apply_summary(&root, 5, Arc::from("换了一份不一样的正文"));

    assert_eq!(
        result,
        Err(BoundaryRejected::SameBoundaryDifferentSummary),
        "同一个 upto 换了正文，就是『同边界换摘要』的另一种表现形式"
    );
    assert_eq!(s.send_plan_of(&root), before_plan, "拒绝之后状态原地不动");
    assert_eq!(s.history_len(), before_len, "拒绝不该产生新 entry");
}
