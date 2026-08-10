//! Issue 107「定死的接口」`Session::apply_summary` 的**原子写回契约**：一次调用
//! 同时把边界推进、摘要引用填上、摘要正文存好，三件事落在**同一条 entry**里，
//! undo 一次三件事一起退回——不存在「边界推了摘要还没存」的中间态。
//!
//! 只看 107 给的公开签名，不读实现体：
//!
//! ```text
//! pub fn apply_summary(&mut self, agent: &AgentId, upto: usize, summary: Arc<str>)
//!     -> Result<SummaryId, BoundaryRejected>;
//! pub fn summary_text(&self, agent: &AgentId, id: &SummaryId) -> Option<Arc<str>>;
//! ```
//!
//! 「epoch 对得上时真的写进去了」「三者同一条 entry」的行为面在这里钉；
//! 「在飞时 /undo 或取消 → 不写入」这条**在飞**语义在 `apply_summary_epoch_gate.rs`。

use std::sync::Arc;

use agent_core::{AgentId, AtomKey, Slot, SummaryId, UndoReport};

use crate::support::session::new_session;

fn send_plan_key(agent: &AgentId) -> AtomKey {
    AtomKey::Agent(agent.clone(), Slot::SendPlan)
}

fn summaries_key(agent: &AgentId) -> AtomKey {
    AtomKey::Agent(agent.clone(), Slot::Summaries)
}

/// 一次 `apply_summary` 只留下一条 entry，且这条 entry 里 `SendPlan`（边界+引用）
/// 和 `Summaries`（正文）各被改恰好一次——两个槽位、一条 entry，不是两条。
#[test]
fn apply_summary_writes_boundary_reference_and_text_in_one_atomic_entry() {
    let mut s = new_session();
    let root = AgentId::root();
    let before_len = s.history_len();

    let id = s
        .apply_summary(&root, 5, Arc::from("摘要正文"))
        .expect("pristine 会话上从 0 推到 5 该成功");

    assert_eq!(
        s.history_len(),
        before_len + 1,
        "一次 apply_summary 只该留下一条 entry"
    );

    let plan = s.send_plan_of(&root);
    assert_eq!(plan.boundary(), 5);
    assert_eq!(plan.summary(), Some(&id));

    let entry = s.last_entry().expect("该有一条 entry");
    let sp_touched = entry
        .changes
        .iter()
        .filter(|c| c.key == send_plan_key(&root))
        .count();
    let sum_touched = entry
        .changes
        .iter()
        .filter(|c| c.key == summaries_key(&root))
        .count();
    assert_eq!(sp_touched, 1, "SendPlan 槽位该被改恰好一次");
    assert_eq!(sum_touched, 1, "Summaries 槽位该被改恰好一次");
}

/// `/undo` 一次，边界、摘要引用、摘要正文三者一起退回——没有「只退一部分」的
/// 中间态：`send_plan_of` 回到 pristine，`summary_text` 对那个刚写的 id 变回
/// `None`（正文跟着 `Summaries` 槽位的 change 一起被撤销）。
#[test]
fn undo_step_after_apply_summary_reverts_boundary_reference_and_text_together() {
    let mut s = new_session();
    let root = AgentId::root();

    let id = s.apply_summary(&root, 4, Arc::from("摘要正文")).unwrap();
    assert_eq!(s.summary_text(&root, &id), Some(Arc::from("摘要正文")));

    let report = s.undo_step();
    assert!(
        matches!(report, UndoReport::Applied { entries: 1, .. }),
        "{report:?}"
    );

    let plan = s.send_plan_of(&root);
    assert!(plan.is_pristine(), "退回到 apply_summary 之前——pristine 计划");
    assert_eq!(
        s.summary_text(&root, &id),
        None,
        "摘要正文跟边界、引用活在同一条 entry 里，undo 一次三者一起退回，\
         不存在『边界退了正文还留着』的中间态"
    );
}

#[test]
fn summary_text_resolves_the_exact_text_that_was_applied() {
    let mut s = new_session();
    let root = AgentId::root();
    let text: Arc<str> = Arc::from("这是一段摘要正文，带着一点长度和一点标点。");

    let id = s.apply_summary(&root, 3, text.clone()).unwrap();

    assert_eq!(s.summary_text(&root, &id), Some(text));
}

/// 取一个不存在的 id：`None`，不 panic。
#[test]
fn summary_text_for_an_unknown_id_returns_none_without_panicking() {
    let mut s = new_session();
    let root = AgentId::root();
    let _ = s.apply_summary(&root, 3, Arc::from("摘要正文")).unwrap();

    let unknown = SummaryId::new("does_not_exist");
    assert_eq!(s.summary_text(&root, &unknown), None);
}

/// 连一次 `apply_summary` 都没调用过的全新会话上查任意 id，同样 `None`。
#[test]
fn summary_text_on_a_pristine_session_is_always_none() {
    let s = new_session();
    let root = AgentId::root();

    assert_eq!(s.summary_text(&root, &SummaryId::new("anything")), None);
}
