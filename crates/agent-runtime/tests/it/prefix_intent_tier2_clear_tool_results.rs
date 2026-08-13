//! 103 验收第一条：第 2 档（清工具返回）开火那一轮，第 1 层判读要落
//! `DriftVerdict::Expected { segment: History }`，不是 `Unexpected`。
//!
//! 走真实 `run_turn`：第一轮产出一个真实 `ToolCallId`，第二轮
//! `replace_send_plan` 清掉它的结果（跟 `send_plan_wiring_clears_tool_results.rs`
//! 同一条生产路径），触发 History 段真的漂一次。第 1 层怎么判读这次漂移，
//! 由 `PrefixIntent` 决定——本轮 core 是有意改前缀的，漂了不该算事故。
//!
//! 顺带证明「Tools / System 两段在压缩轮不漂」：`DriftVerdict::Expected`
//! 只携带**一个** `Segment`，断言它恰好是 `History` 就排除了 Tools/System
//! 被误判成漂移的可能——真漂的话，报出来的会是那一段，不是 History。

use agent_core::{AgentId, ContentBlock, DriftVerdict, SendPlan, Segment, Session, ToolCallId};
use agent_runtime::{RunnerEvent, run_turn};

use crate::support;

#[test]
fn tier2_clear_tool_results_round_is_expected_drift_not_unexpected() {
    let dir = support::temp_dir("prefix-intent-tier2");
    std::fs::write(dir.join("alpha.txt"), b"ALPHA-CONTENT").unwrap();

    let port = support::spawn_scripted_server(vec![
        support::sse_tool_call("call_alpha", "srv_3Afs_2Fread", r#"{\"path\": \"alpha.txt\"}"#),
        support::sse_text("读完了"),
        support::sse_text("已清掉，继续"),
    ]);
    let (mut ctx, events) = support::build_ctx(port, &dir);
    let mut session = Session::new(AgentId::root());

    run_turn(&mut session, &mut ctx, "读一下 alpha.txt")
        .expect("第一轮不该是 source failure");

    let root = AgentId::root();
    let call_id = session
        .messages_of(&root)
        .iter()
        .flat_map(|m| m.blocks.iter())
        .find_map(|block| match block {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .expect("第一轮该产出一个真实 ToolCallId");
    assert_eq!(call_id, ToolCallId::new("call_alpha"));

    // 第 2 档开火：清掉这个工具调用的结果。
    let mut plan = SendPlan::new();
    plan.clear_tool_results([call_id]);
    session.begin_turn();
    session.replace_send_plan(&root, plan);

    run_turn(&mut session, &mut ctx, "总结一下")
        .expect("第二轮（压缩轮）不该是 source failure");

    let events = events.borrow();
    let last_guard = events
        .iter()
        .filter_map(|e| match e {
            RunnerEvent::TurnGuard { report, .. } => Some(report),
            _ => None,
        })
        .next_back()
        .unwrap_or_else(|| panic!("压缩轮该有一份 GuardReport：{events:#?}"));

    assert_eq!(
        last_guard.drift,
        DriftVerdict::Expected {
            segment: Segment::History
        },
        "压缩轮本来就打算改前缀，History 漂了不是事故：{events:#?}"
    );
}
