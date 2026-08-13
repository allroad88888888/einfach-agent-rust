//! 103 验收第二条：第 4 档（边界推进，`Session::advance_boundary`）开火那一轮，
//! 第 1 层判读同样要落 `DriftVerdict::Expected { segment: History }`。
//!
//! 跟第 2 档（`prefix_intent_tier2_clear_tool_results.rs`）是同一条接线路径
//! （103「第 2、3 档共用同一条路径，不各写一份」，第 4 档跟第 3 档又共用
//! `advance_boundary`，104 已定），这里用第 4 档单独走一遍是因为它不需要真实
//! 工具调用就能让 History 段整段消失一截——比第 2 档的漂移更彻底（不是某个
//! 块换成占位，是这些消息干脆不再发出去）。
//!
//! 边界推进不依赖模型调用（105–107 的摘要机制不在本 issue 范围内），走的是
//! 「清窗口」那条子路径：`summary` 传 `None`。

use agent_core::{AgentId, DriftVerdict, Segment, Session};
use agent_runtime::{RunnerEvent, run_turn};

use crate::support;

#[test]
fn tier4_advance_boundary_round_is_expected_drift_not_unexpected() {
    let dir = support::temp_dir("prefix-intent-tier4");

    let port = support::spawn_scripted_server(vec![
        support::sse_text("第一轮回复"),
        support::sse_text("边界推进之后的回复"),
    ]);
    let (mut ctx, events) = support::build_ctx(port, &dir);
    let mut session = Session::new(AgentId::root());

    run_turn(&mut session, &mut ctx, "第一句话")
        .expect("第一轮不该是 source failure");

    let root = AgentId::root();
    let history_len = session.messages_of(&root).len();
    assert!(history_len > 0, "第一轮之后历史该有内容可推");

    // 第 4 档开火：边界推到历史末尾，不留摘要——「清窗口」那条子路径。
    session.begin_turn();
    session
        .advance_boundary(&root, history_len, None)
        .expect("边界只增不减，从 0 推到 history_len 该被接受");

    run_turn(&mut session, &mut ctx, "继续")
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
        "边界推进本来就打算改前缀，History 漂了不是事故：{events:#?}"
    );
}
