//! 208 验收第 8 条：上下文压过没有只回布尔，不回摘要正文——把摘要正文塞进
//! `self` 的 tool_result 等于让同一段文字在 prompt 里出现两次（issue 208 §注意）。
//!
//! # 怎么造一个「压过的会话」而不用真的走一遍摘要子 agent
//!
//! `Session::apply_summary`（`agent-core`，109）是压缩记录**唯一**的写口，
//! 真正触发压缩的三档阶梯（106/107/108）最终也是调它落地——直接调用它是在用
//! 公开命令层的正门，不是绕过实现抄近道。`compaction_record.rs` 的既有单测
//! 已经证明它在一个全新 `Session` 上不需要先跑过任何轮次就能生效。
//!
//! # 怎么证明那个布尔是活的、不是「反正不回摘要文本所以永远是同一句话」
//!
//! root 在压缩前后各调一次 `self`，两次都是各自新轮次的第 1 跳（`turns_used`/
//! depth/工具表/重试次数全部相同，跟 `self_indep_limits_not_hardcoded.rs`
//! 同一个隔离手法）——唯一变了的状态是「压过了没有」。两段正文必须不相等，
//! 否则说明这个字段压根没被读，是句写死的话。

use std::sync::Arc;
use std::time::Duration;

use agent_core::{AgentId, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::self_indep_support::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_call, temp_dir, tool_result, wire_tool_name,
};

/// 摘要正文里一段不会凑巧出现在别处的标记文本——`self` 的正文如果不小心把
/// 摘要原文带出来了，这个标记会在断言里现形。
const SUMMARY_MARKER: &str = "SUMMARYMARKERXYZ 这段摘要原文不该出现在 self 的正文里";

#[test]
fn self_reports_a_boolean_for_compaction_never_the_summary_text() {
    let dir = temp_dir("self-compaction-boolean");
    let self_wire = wire_tool_name(agent_runtime::SELF_TOOL);

    let server = RoutedServer::start(vec![
        Route {
            needle: "call_after",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("turn two done"),
        },
        Route {
            needle: "kickoff-compaction-after",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_after", &self_wire, "{}"),
        },
        Route {
            needle: "call_before",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("turn one done"),
        },
        Route {
            needle: "kickoff-compaction-before",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_before", &self_wire, "{}"),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin().with_self();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();

    assert!(
        session.summary_library(&root).is_empty(),
        "前提：还没压缩过"
    );
    let status = run_turn(&mut session, &mut ctx, "kickoff-compaction-before 压缩前问一次自己")
        .expect("第一轮不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });
    let (before, before_error) = tool_result(&session, &root, "call_before");
    assert!(!before_error, "纯读不该失败：{before}");
    assert!(
        !before.contains(SUMMARY_MARKER),
        "压缩前的正文本来就不该有摘要文本：{before}"
    );

    // 直接调公开命令层的正门造一个「压过的会话」，不需要真的跑一遍摘要子 agent。
    session
        .apply_summary(&root, 1, Arc::from(SUMMARY_MARKER))
        .expect("在一个全新 Session 上 apply_summary 不该失败");
    assert_eq!(session.summary_library(&root).len(), 1, "压缩记录该落了一条");
    session.begin_turn();

    let status = run_turn(&mut session, &mut ctx, "kickoff-compaction-after 压缩后再问一次自己")
        .expect("第二轮不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });
    let (after, after_error) = tool_result(&session, &root, "call_after");
    assert!(!after_error, "纯读不该失败：{after}");

    assert!(
        !after.contains(SUMMARY_MARKER),
        "压缩后 self 的正文里出现了摘要原文——这个字段该是布尔，不是正文：{after}"
    );
    assert_ne!(
        before, after,
        "压缩前后正文逐字节相同——像是「压过没有」这个字段压根没被读，是句写死的话"
    );
}
