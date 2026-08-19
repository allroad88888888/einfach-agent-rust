//! 206 验收的**行为核心**：两个兄弟隔着 `srv:agent/send` 对上话。
//!
//! ```text
//! root
//! ├── root/a1   status 看见兄弟 → send 一条中间结论给它 → 后来收到回信
//! └── root/a2   下一次请求的 prompt 里带上那条 → 回一条给 root/a1
//! ```
//!
//! 两边**本来就还要再说话**——这正是 `drain_now` 那个定点（收信人下一次组装
//! provider 请求之前）服务的场景，全程不需要任何人被唤醒。这条断言写成
//! 「provider 调用次数恰好等于脚本里的跳数」：偷偷造一条唤醒边，这里立刻多一次。
//!
//! 黑盒来源与「实现体没读」的声明见 `send_indep_support/mod.rs` 顶部。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::send_indep_support::{
    Route, RoutedServer, SEND_WIRE, build_ctx, calls_matching, index_of, injected, sse_text,
    sse_tool_call, sse_tool_calls, temp_dir, tool_result, unread_warnings, wire_tool_name,
};

/// root/a2 的第一跳慢这么久：只要明显长过「root/a1 读树 + 发信」那一小段就够，
/// 下面的断言比的是服务器记下的真实请求体，不是这个数字本身。
const B_FIRST_HOP: Duration = Duration::from_millis(600);
/// root/a1 第三跳的滞留时间：它要活到 root/a2 的回信送到之后。
const A_THIRD_HOP: Duration = Duration::from_millis(1200);

#[test]
fn two_running_siblings_exchange_conclusions_without_waking_anybody() {
    let dir = temp_dir("send-siblings");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);
    let status_wire = wire_tool_name(agent_runtime::STATUS_TOOL);
    assert_eq!(
        wire_tool_name(agent_runtime::SEND_TOOL),
        SEND_WIRE,
        "send 的 wire 名跟脚本里焊死的那个对不上，下面整份脚本都是空跑的"
    );

    // 按「越具体越靠前」排：后一跳的请求体里必然含前一跳的 call_id。
    let server = RoutedServer::start(vec![
        Route {
            needle: "call_a_probe",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("AAAFINISHED"),
        },
        Route {
            needle: "call_b_send",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("BBBFINISHED"),
        },
        // root/a1 第三跳：**停在这儿 1.2s**，等 root/a2 的回信。
        Route {
            needle: "call_a_send",
            delay: A_THIRD_HOP,
            status: 200,
            lines: sse_tool_call("call_a_probe", &status_wire, "{}"),
        },
        // root/a2 第二跳：这一跳的请求体里该带上兄弟投来的那条。
        Route {
            needle: "call_b_probe",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call(
                "call_b_send",
                SEND_WIRE,
                r#"{"to":"root/a1","text":"REPLYFROMB 右路量到 42"}"#,
            ),
        },
        // root/a1 第二跳：status 看见了兄弟，给它发一条中间结论。
        Route {
            needle: "call_a_status",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call(
                "call_a_send",
                SEND_WIRE,
                r#"{"to":"root/a2","text":"MIDFROMA 左路的中间结论是 7"}"#,
            ),
        },
        Route {
            needle: "call_r1",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("ALLDONE"),
        },
        Route {
            needle: "TASKAAA",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_a_status", &status_wire, "{}"),
        },
        Route {
            needle: "TASKBBB",
            delay: B_FIRST_HOP,
            status: 200,
            lines: sse_tool_call("call_b_probe", &status_wire, "{}"),
        },
        Route {
            needle: "kickoff-mesh",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_calls(&[
                ("call_r1", &spawn_wire, r#"{"task":"TASKAAA 左路"}"#),
                ("call_r2", &spawn_wire, r#"{"task":"TASKBBB 右路"}"#),
            ]),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_status()
        .with_send();
    let (mut ctx, events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff-mesh 两路并行，互相通气")
        .expect("兄弟互发消息不是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();
    let a1 = root.child(1);
    let a2 = root.child(2);

    // --- ① root/a1 先 status 看见了兄弟 ---
    let (seen, is_error) = tool_result(&session, &a1, "call_a_status");
    assert!(!is_error, "{seen}");
    assert!(
        seen.contains(a2.as_str()),
        "root/a1 该在树上看见 root/a2（不然它根本拿不到发给谁的 id）：{seen}"
    );

    // --- ② 发信成功，且 send **不等回复**：当场就是一条非错结果 ---
    let (sent, is_error) = tool_result(&session, &a1, "call_a_send");
    assert!(!is_error, "给一个活着的兄弟发消息该成功：{sent}");

    // --- ③ root/a2 的**下一次请求**里有那条（这才是「进 prompt」）---
    let b_hop2 = server
        .call("call_b_probe")
        .expect("root/a2 该发过第二跳请求");
    assert!(
        b_hop2.body.contains("MIDFROMA"),
        "投来的那条该出现在 root/a2 下一次请求的 prompt 里：{}",
        b_hop2.body
    );
    assert!(
        b_hop2.body.contains(a1.as_str()),
        "prompt 里那条该认得出发信人是 root/a1：{}",
        b_hop2.body
    );

    // 历史里的形状：`Role::User` 的一条消息、单个 `Text` 块、含发信人路径 id + 原文。
    let (_, text) = injected(&session, &a2, "MIDFROMA");
    assert!(text.contains(a1.as_str()), "含发信人的路径 id：{text}");
    assert!(
        text.ends_with("MIDFROMA 左路的中间结论是 7"),
        "原文原样：{text}"
    );

    // --- ④ 回信同理：root/a1 的下一次请求里有 root/a2 的那条 ---
    let a_hop4 = server
        .call("call_a_probe")
        .expect("root/a1 该在收到回信之后又发过一跳");
    assert!(
        a_hop4.body.contains("REPLYFROMB"),
        "回信该进 root/a1 下一次请求的 prompt：{}",
        a_hop4.body
    );
    let (_, reply) = injected(&session, &a1, "REPLYFROMB");
    assert!(
        reply.contains(a2.as_str()),
        "回信认得出是 root/a2 发的：{reply}"
    );

    // 回信真的是「后来」才到的：root/a1 第三跳发出去的时候还没有它。
    let a_hop3 = server.call("call_a_send").expect("root/a1 该发过第三跳");
    assert!(
        !a_hop3.body.contains("REPLYFROMB"),
        "第三跳发出去时回信还没写出来，出现在这里说明时序假设塌了：{}",
        a_hop3.body
    );

    // --- ⑤ 全程没有任何一方被唤醒 ---
    // 九跳 = root 2 + root/a1 4 + root/a2 3，一次不多。唤醒边会在这里多出一跳。
    assert_eq!(
        server.calls().len(),
        9,
        "provider 调用次数该恰好等于脚本里的跳数：{:?}",
        server.calls().iter().map(|c| c.needle).collect::<Vec<_>>()
    );
    for needle in [
        "kickoff-mesh",
        "TASKAAA",
        "TASKBBB",
        "call_a_status",
        "call_a_send",
        "call_a_probe",
        "call_b_probe",
        "call_b_send",
        "call_r1",
    ] {
        assert_eq!(
            calls_matching(&server, needle),
            1,
            "{needle} 该恰好被走一次"
        );
    }

    // --- ⑥ 两条都被读到了：收件箱空、轮末不告警 ---
    assert!(session.inbox_of(&a1).is_empty(), "root/a1 的收件箱该被读空");
    assert!(session.inbox_of(&a2).is_empty(), "root/a2 的收件箱该被读空");
    assert!(
        unread_warnings(&events.borrow()).is_empty(),
        "两条都被读到了，不该有未读告警：{:?}",
        unread_warnings(&events.borrow())
    );

    // 顺带：投递没有把话直接塞到对方 `Messages` 的任意位置——它排在收信人那一跳
    // 的 assistant 回复之后（严格的顺序断言在 `send_indep_injection_order.rs`）。
    assert!(
        index_of(&session, &root, "MIDFROMA").is_none(),
        "root 不该收到这条"
    );
}
