//! 206 **最硬的一条**：收信人有一个 provider 请求在飞时给它投一条 → 它的
//! `Messages` 里，**那次在飞请求的 assistant 回复排在被投递的那条之前**。
//!
//! 为什么这条能抓住真的 bug（204 §二）：投递如果写成「直接往对方 `Messages`
//! 追加」，那条 user 消息会插在在飞请求的回复**前面**，于是历史里长出一段
//! 「答非所问」——**这不报错**，下一轮模型读到的就是错的。
//!
//! 所以这份用例做三件事：
//!
//! 1. 用服务器记下的真实时间窗，把「投递发生在那次请求还在飞的时候」钉死
//!    （手法照 `status_indep_whole_tree.rs`）；
//! 2. 断 `Messages` 里两条的**相对下标**；
//! 3. 断收信人**下一次请求体**里两段文字的先后——进 prompt 的那一份也得是对的。
//!
//! 黑盒来源与「实现体没读」的声明见 `send_indep_support/mod.rs` 顶部。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::send_indep_support::{
    Route, RoutedServer, SEND_WIRE, build_ctx, injected, sse_text, sse_tool_call, sse_tool_calls,
    temp_dir, tool_use_index, unread_warnings, wire_tool_name,
};

/// 收信人第一跳滞留这么久——投递就发生在这段里。
const IN_FLIGHT: Duration = Duration::from_millis(900);

#[test]
fn a_message_delivered_mid_flight_lands_after_that_flights_assistant_reply() {
    let dir = temp_dir("send-injection-order");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);
    let status_wire = wire_tool_name(agent_runtime::STATUS_TOOL);

    let server = RoutedServer::start(vec![
        // 发信人第二跳：这一跳的到达时刻就是「投递已经发生」的证据。
        Route {
            needle: "call_a_send",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("AAAFINISHED"),
        },
        // 收信人第二跳：投来的那条该在这次请求体里，而且排在自己上一跳回复之后。
        Route {
            needle: "call_b_probe",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("BBBFINISHED"),
        },
        Route {
            needle: "call_r1",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("ALLDONE"),
        },
        Route {
            needle: "TASKSENDER",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call(
                "call_a_send",
                SEND_WIRE,
                r#"{"to":"root/a2","text":"INJECTEDLINE 顺带一句"}"#,
            ),
        },
        // **停在这儿 900ms**：投递落在这段窗口里。
        Route {
            needle: "TASKINFLIGHT",
            delay: IN_FLIGHT,
            status: 200,
            lines: sse_tool_call("call_b_probe", &status_wire, "{}"),
        },
        Route {
            needle: "kickoff-order",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_calls(&[
                ("call_r1", &spawn_wire, r#"{"task":"TASKSENDER 快的那路"}"#),
                (
                    "call_r2",
                    &spawn_wire,
                    r#"{"task":"TASKINFLIGHT 慢的那路"}"#,
                ),
            ]),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_status()
        .with_send();
    let (mut ctx, events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff-order 一快一慢")
        .expect("投递不是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let b = AgentId::root().child(2);

    // --- ① 投递那一刻，收信人那次请求**确实在飞** ---
    // 发信人第二跳的请求是在 send 写回之后才发出去的，它比收信人第一跳的应答写完
    // 还早 → 投递发生在收信人那次请求的在飞窗口里。没有这条，下面两条顺序断言
    // 测的是「恰好收敛之后才投」，什么都没证明。
    let sender_next = server
        .call("call_a_send")
        .expect("发信人该在 send 之后又发过一跳");
    let in_flight = server.call("TASKINFLIGHT").expect("收信人该发过第一跳");
    assert!(
        sender_next.start < in_flight.end,
        "投递该发生在收信人那次请求还在飞的时候：sender_next.start={:?} in_flight.end={:?}",
        sender_next.start,
        in_flight.end,
    );

    // --- ② `Messages` 里的相对下标 ---
    let reply_idx = tool_use_index(&session, &b, "call_b_probe");
    let (injected_idx, text) = injected(&session, &b, "INJECTEDLINE");
    assert!(
        reply_idx < injected_idx,
        "在飞那次请求的 assistant 回复该排在被投递的那条**之前**\
         （写成直接往 Messages 追加，这里就是反的）：reply@{reply_idx} injected@{injected_idx}\n{:#?}",
        session.messages_of(&b)
    );
    assert!(text.contains("INJECTEDLINE"), "投的就是这条：{text}");

    // --- ③ 进 prompt 的那一份同样是对的 ---
    let next_body = &server
        .call("call_b_probe")
        .expect("收信人该在收到之后又发过一跳")
        .body;
    let reply_pos = next_body
        .find("call_b_probe")
        .expect("下一次请求里该有上一跳的工具调用");
    let injected_pos = next_body
        .find("INJECTEDLINE")
        .expect("投来的那条该进 prompt");
    assert!(
        reply_pos < injected_pos,
        "请求体里也得是「先回复、后投递」：{next_body}"
    );

    // --- ④ 这一轮照常收尾，没有人被唤醒，也没有未读 ---
    assert!(session.inbox_of(&b).is_empty(), "那条被读到了，收件箱该空");
    assert!(
        unread_warnings(&events.borrow()).is_empty(),
        "被读到了就不该告警"
    );
    assert_eq!(
        server.calls().len(),
        6,
        "六跳 = root 2 + 发信人 2 + 收信人 2，多一跳就是多了一条唤醒边：{:?}",
        server.calls().iter().map(|c| c.needle).collect::<Vec<_>>()
    );
}
