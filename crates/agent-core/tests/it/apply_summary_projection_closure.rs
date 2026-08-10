//! Issue 107 的收口：写进去的摘要真的能被 099 的 `project` 用起来——证明
//! 099（`SendPlan` / `project`）→ 104（`advance_boundary` 的边界机制）→
//! 107（`apply_summary` 的写回）三条接上了，不是三个互不相干的活。
//!
//! 场景：真跑两轮问答、`apply_summary` 一次、再长出第三轮，然后把「当前完整
//! 历史 + 当前 SendPlan + 从 `summary_text` 取回的正文」一起喂给 `project`，
//! 断言摘要在最前面、边界之前的消息不出现、边界之后的消息原样在。

use std::sync::Arc;

use agent_core::value::send_plan::project;
use agent_core::{AgentId, ContentBlock};

use crate::support;
use crate::support::session::new_session;

#[test]
fn applied_summary_flows_through_project_as_the_leading_message() {
    let mut s = new_session();
    let root = AgentId::root();

    // 两轮问答，边界之后要把这两轮盖住。
    let _ = s.step(support::user_input_event("第 0 轮问题"));
    let _ = s.step(support::provider_done_end_turn(s.epoch(), "第 0 轮回答"));
    s.begin_turn();
    let _ = s.step(support::user_input_event("第 1 轮问题"));
    let _ = s.step(support::provider_done_end_turn(s.epoch(), "第 1 轮回答"));

    let boundary = s.messages_of(&root).len();
    let summary_text: Arc<str> = Arc::from("摘要：讨论了第 0 轮和第 1 轮。");
    let id = s
        .apply_summary(&root, boundary, summary_text.clone())
        .unwrap();

    // 又长出一轮：证明投影用的是「当前完整历史」而不是压缩那一刻的快照。
    s.begin_turn();
    let _ = s.step(support::user_input_event("第 2 轮问题"));
    let _ = s.step(support::provider_done_end_turn(s.epoch(), "第 2 轮回答"));

    let plan = s.send_plan_of(&root);
    assert_eq!(plan.summary(), Some(&id));

    let stored_text = s
        .summary_text(&root, &id)
        .expect("apply_summary 写进去的正文该取得到");
    assert_eq!(stored_text, summary_text);

    let full_history = s.messages_of(&root);
    assert!(full_history.len() > boundary, "又长出了新消息");

    let projected = project(&full_history, &plan, Some(&stored_text));

    let first_is_summary = projected[0]
        .blocks
        .iter()
        .any(|b| matches!(b, ContentBlock::Text(t) if t.as_ref() == summary_text.as_ref()));
    assert!(first_is_summary, "摘要该作为最前面的一条消息出现");

    let pre_boundary: Vec<_> = full_history.iter().take(boundary).cloned().collect();
    for m in &pre_boundary {
        assert!(
            !projected.iter().any(|p| p.id == m.id),
            "边界之前的消息不该出现在投影里：{:?}",
            m.id
        );
    }

    let post_boundary: Vec<_> = full_history.iter().skip(boundary).cloned().collect();
    for m in &post_boundary {
        assert!(
            projected.iter().any(|p| p.id == m.id),
            "边界之后的消息该原样出现在投影里：{:?}",
            m.id
        );
    }
}
