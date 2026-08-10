//! Issue 104「定死的接口」`Session::advance_boundary` 的 **`Result` 契约**：
//! 三种情况分别落在哪一支，别的什么都不碰——不需要真实消息历史，全部在一个
//! 空会话上用纯数字驱动。「推进之后发送侧真的变了什么」是另一件事，见
//! `advance_boundary_window_clear.rs`。
//!
//! 只看 104 给的公开签名，不读 `command/advance_boundary.rs` 的实现体。
//!
//! ```text
//! pub fn advance_boundary(
//!     &mut self,
//!     agent: &AgentId,
//!     next: usize,
//!     summary: Option<SummaryId>,
//! ) -> Result<(), BoundaryRejected>;
//! ```
//!
//! - `next > 当前边界` → `Ok`，产生一条 entry
//! - `next == 当前边界` 且摘要引用相同 → `Ok`，**不产生 entry**（幂等）
//! - 其余（`next` 更小，或 `next` 相等但摘要引用不同）→ `Err`，**状态不变、
//!   不留痕**（先校验再写——不是「先写再看要不要回滚」）

use agent_core::{AgentId, BoundaryRejected, SummaryId};

use crate::support::session::new_session;

/// `next` 比当前边界小：拒绝，不是静默忽略。状态原地不动，也不产生新 entry
/// ——「拒绝不留痕」，先校验再写。
#[test]
fn a_smaller_boundary_is_rejected_without_leaving_a_trace() {
    let mut s = new_session();
    let root = AgentId::root();

    s.advance_boundary(&root, 5, None)
        .expect("从 pristine（边界 0）推到 5 该成功");
    let before_plan = s.send_plan_of(&root);
    let before_len = s.history_len();

    let result = s.advance_boundary(&root, 3, None);
    assert_eq!(
        result,
        Err(BoundaryRejected::NotAdvancing {
            current: 5,
            requested: 3
        })
    );
    assert_eq!(s.send_plan_of(&root), before_plan, "拒绝之后状态原地不动");
    assert_eq!(s.history_len(), before_len, "拒绝不该产生新 entry");
}

/// `next == 当前边界` 且摘要引用相同（都是 `None`，第 4 档「清窗口」按两次
/// 的场景）：幂等，`Ok`，不产生新 entry。
#[test]
fn advancing_to_the_same_boundary_with_no_summary_is_idempotent() {
    let mut s = new_session();
    let root = AgentId::root();

    let total = 4;
    s.advance_boundary(&root, total, None).unwrap();
    let before_plan = s.send_plan_of(&root);
    let before_len = s.history_len();

    let result = s.advance_boundary(&root, total, None);
    assert_eq!(result, Ok(()), "同一个边界再清一次窗口该是幂等 Ok");
    assert_eq!(s.history_len(), before_len, "幂等不该留下第二条 entry");
    assert_eq!(s.send_plan_of(&root), before_plan);
}

/// `next == 当前边界` 且摘要引用相同（都指向同一个 `SummaryId`）：同样幂等。
#[test]
fn advancing_to_the_same_boundary_with_the_same_summary_reference_is_idempotent() {
    let mut s = new_session();
    let root = AgentId::root();
    let sid = SummaryId::new("sum_1");

    s.advance_boundary(&root, 5, Some(sid.clone())).unwrap();
    let before_plan = s.send_plan_of(&root);
    let before_len = s.history_len();

    let result = s.advance_boundary(&root, 5, Some(sid));
    assert_eq!(result, Ok(()));
    assert_eq!(s.history_len(), before_len, "幂等不该留下第二条 entry");
    assert_eq!(s.send_plan_of(&root), before_plan);
}

/// `next == 当前边界` 但摘要引用不同：`Err(SameBoundaryDifferentSummary)`——
/// 「重新摘要同一段」不在本条支持范围内，不是静默接受也不是静默忽略。
#[test]
fn advancing_to_the_same_boundary_with_a_different_summary_is_rejected() {
    let mut s = new_session();
    let root = AgentId::root();

    s.advance_boundary(&root, 5, Some(SummaryId::new("sum_1")))
        .unwrap();
    let before_plan = s.send_plan_of(&root);
    let before_len = s.history_len();

    let result = s.advance_boundary(&root, 5, Some(SummaryId::new("sum_2")));
    assert_eq!(result, Err(BoundaryRejected::SameBoundaryDifferentSummary));
    assert_eq!(s.history_len(), before_len, "拒绝不该产生新 entry");
    assert_eq!(s.send_plan_of(&root), before_plan, "状态不变");
}

/// 同一个边界，摘要引用从「没有」变成「有」：同样落在「摘要引用不同」那一支
/// ——`None` 和 `Some(_)` 不相同，不能因为「以前没有摘要」就放行。
#[test]
fn advancing_from_no_summary_to_a_summary_at_the_same_boundary_is_also_rejected() {
    let mut s = new_session();
    let root = AgentId::root();

    s.advance_boundary(&root, 4, None).unwrap();
    let before_plan = s.send_plan_of(&root);
    let before_len = s.history_len();

    let result = s.advance_boundary(&root, 4, Some(SummaryId::new("sum_1")));
    assert_eq!(result, Err(BoundaryRejected::SameBoundaryDifferentSummary));
    assert_eq!(s.history_len(), before_len);
    assert_eq!(s.send_plan_of(&root), before_plan);
}
