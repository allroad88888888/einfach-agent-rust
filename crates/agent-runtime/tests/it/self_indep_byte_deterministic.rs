//! 208 验收第 4 条（红线 11）：同一状态下连调两次 `self`，两段正文**逐字节
//! 相同**（不带时间戳、不带调用序号）。
//!
//! # 怎么保证「同一状态」而不是「凑巧差不多」
//!
//! 两次调用放进**同一跳**的并行 `tool_calls`（同一条 assistant 消息里一次给
//! 两个调用，跟 `status_indep_lists_descendants.rs` 的手法一致）：两次调用之间
//! 没有发生任何新的 provider 往返，`turns_used`/`retries_used`/存活子数/工具表/
//! 是否压缩过全都没有变过——这是能做到的最强的「同一状态」。
//!
//! 这份正文会原样进下一轮 prompt（`srv:agent/status` 模块文档点名的同一条红线），
//! 带时间戳或调用序号的实现会在这里现出原形：`call_id` 不同，如果实现把
//! `call_id` 掺进了正文，两段就不会相等。

use std::time::Duration;

use agent_core::{AgentId, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::self_indep_support::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_calls, temp_dir, tool_result, wire_tool_name,
};

#[test]
fn two_calls_in_the_same_hop_produce_byte_identical_bodies() {
    let dir = temp_dir("self-byte-det");
    let self_wire = wire_tool_name(agent_runtime::SELF_TOOL);

    let server = RoutedServer::start(vec![
        Route {
            needle: "call_y",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("both answered"),
        },
        Route {
            needle: "kickoff-byte-det",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_calls(&[("call_x", &self_wire, "{}"), ("call_y", &self_wire, "{}")]),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin().with_self();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff-byte-det 同一跳里并列问两次自己")
        .expect("这条流程不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();
    let (x_body, x_error) = tool_result(&session, &root, "call_x");
    assert!(!x_error, "纯读不该失败：{x_body}");
    let (y_body, y_error) = tool_result(&session, &root, "call_y");
    assert!(!y_error, "纯读不该失败：{y_body}");

    assert_eq!(
        x_body, y_body,
        "红线 11：同一状态下连调两次，正文必须逐字节相同——不相同说明混进了\
         时间戳、call_id 或别的不确定性"
    );
}
