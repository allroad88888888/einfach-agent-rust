//! 206 §3 / §4：**turn 收尾时两档的命运不同**。
//!
//! - 投给一个已经落终态的 agent：`send` 照样成功（它只负责投递），但**没有人被
//!   唤醒**——条目原地留在收件箱里，轮末报一条 `RunnerEvent::UnreadMessages`。
//!   214 落地之后这条会变成「唤醒了」；在此之前它守的是「206 没有偷偷造一条唤醒边」。
//! - 收尾时同时有一条 `Now` 和一条 `NextTurn`：只有 `Now` 那条算未读，
//!   `NextTurn` 那条**一个字不少地还在收件箱里**（§4 那条直觉陷阱：孤儿收尾
//!   「收件箱非空就告警」的写法会把正常情况报成异常，接着有人顺手清干净）。
//!
//! 两条共用的一条：**轮次结果仍是 root 本来的终态**，不是 `Failed(Cancelled)`
//! ——未读消息是编排失误的信号，不是错误（ORCHESTRATION §四.4 的既有结论）。
//!
//! 黑盒来源与「实现体没读」的声明见 `send_indep_support/mod.rs` 顶部。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, Deliver, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::send_indep_support::{
    Route, RoutedServer, SEND_WIRE, build_ctx, calls_matching, index_of, sse_text, sse_tool_call,
    temp_dir, tool_result, unread_warnings, wire_tool_name,
};

fn no_delay(needle: &'static str, lines: Vec<String>) -> Route {
    Route {
        needle,
        delay: Duration::ZERO,
        status: 200,
        lines,
    }
}

/// 目标已经 `Done` → `send` 成功、没有新的 provider 调用、条目留着、轮末告警。
#[test]
fn sending_to_a_terminal_agent_wakes_nobody_and_warns_once_at_turn_end() {
    let dir = temp_dir("send-terminal");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        no_delay("call_r_send", sse_text("ROOTFINISHED")),
        no_delay(
            "call_r1",
            sse_tool_call(
                "call_r_send",
                SEND_WIRE,
                r#"{"to":"root/a1","text":"TOODEADTOREAD 你还在吗"}"#,
            ),
        ),
        no_delay("TASKDONE", sse_text("AAADONE")),
        no_delay(
            "kickoff-terminal",
            sse_tool_call(
                "call_r1",
                &spawn_wire,
                r#"{"task":"TASKDONE 一句话就答完"}"#,
            ),
        ),
    ]);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_send();
    let (mut ctx, events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff-terminal 先派活，再补一句")
        .expect("投给终态的 agent 不是 source failure");

    let root = AgentId::root();
    let a1 = root.child(1);

    // 前提：投的时候它真的已经答完了（否则这条测的是别的事）。
    assert_eq!(
        session.status_of(&a1),
        TurnStatus::Done { truncated: false },
        "子该已经落终态"
    );
    assert!(session.is_live(&a1), "它还在树上——挡住唤醒的不该是活性闸");

    // ① `send` 仍然成功：它只负责投递。
    let (sent, is_error) = tool_result(&session, &root, "call_r_send");
    assert!(
        !is_error,
        "投给一个终态的 agent 该成功——send 只负责投递：{sent}"
    );

    // ② 没有新的 provider 调用发生。
    assert_eq!(
        calls_matching(&server, "TASKDONE"),
        1,
        "子只该被调用过一次；多一次就是有人偷偷把它唤醒了"
    );
    assert_eq!(
        server.calls().len(),
        4,
        "四跳 = root 3 + 子 1：{:?}",
        server.calls().iter().map(|c| c.needle).collect::<Vec<_>>()
    );

    // ③ 条目原地留在收件箱里，一个字没进它的对话。
    let inbox = session.inbox_of(&a1);
    assert_eq!(inbox.len(), 1, "没被读到的条目该留着：{inbox:?}");
    assert_eq!(inbox[0].when, Deliver::Now);
    assert_eq!(inbox[0].from, root);
    assert_eq!(&*inbox[0].text, "TOODEADTOREAD 你还在吗");
    assert!(
        index_of(&session, &a1, "TOODEADTOREAD").is_none(),
        "没排空就不该出现在它的 `Messages` 里：{:#?}",
        session.messages_of(&a1)
    );

    // ④ 轮末告警一条，说清是谁的几条。
    assert_eq!(
        unread_warnings(&events.borrow()),
        vec![(a1.as_str().to_string(), 1)],
        "该恰好报一条「root/a1 有 1 条没被读到」"
    );

    // ⑤ 轮次结果仍是 root 本来的终态。
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "未读消息不是失败——判成 Failed(Cancelled) 说明走了会话级取消"
    );
}

/// 收尾时一条 `Now` + 一条 `NextTurn`：只有前者算未读，后者原地不动。
#[test]
fn turn_end_warns_about_the_now_item_and_keeps_the_next_turn_one_untouched() {
    let dir = temp_dir("send-turn-end-both");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        no_delay("call_r_now", sse_text("ROOTFINISHED")),
        no_delay("call_a_note", sse_text("AAADONE")),
        no_delay(
            "call_r1",
            sse_tool_call(
                "call_r_now",
                SEND_WIRE,
                r#"{"to":"root/a1","text":"LOSTNOW 没人读得到"}"#,
            ),
        ),
        no_delay(
            "TASKBOTH",
            sse_tool_call(
                "call_a_note",
                SEND_WIRE,
                r#"{"to":"root","text":"KEEPNOTE 给下一轮的留言","when":"next_turn"}"#,
            ),
        ),
        no_delay(
            "kickoff-both",
            sse_tool_call(
                "call_r1",
                &spawn_wire,
                r#"{"task":"TASKBOTH 留个条就收工"}"#,
            ),
        ),
    ]);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_send();
    let (mut ctx, events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff-both 一条本轮一条下轮")
        .expect("两档共存不是 source failure");
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "轮次结果仍是 root 本来的终态"
    );

    let root = AgentId::root();
    let a1 = root.child(1);

    // 两条都成功投出去了。
    for call in ["call_a_note", "call_r_now"] {
        let owner = if call == "call_a_note" { &a1 } else { &root };
        let (body, is_error) = tool_result(&session, owner, call);
        assert!(!is_error, "{call} 该成功：{body}");
    }

    // ① `Now` 那条：告警，且只告警它。
    assert_eq!(
        unread_warnings(&events.borrow()),
        vec![(a1.as_str().to_string(), 1)],
        "只有 `Now` 那条算未读——`NextTurn` 一起报就是把正常情况报成异常"
    );

    // ② `NextTurn` 那条：**一个字不少地还在收件箱里**。
    let kept = session.inbox_of(&root);
    assert_eq!(kept.len(), 1, "留言该原地还在：{kept:?}");
    assert_eq!(kept[0].when, Deliver::NextTurn, "时机标记也得原样");
    assert_eq!(kept[0].from, a1, "发信人原样");
    assert_eq!(&*kept[0].text, "KEEPNOTE 给下一轮的留言", "正文一个字不少");
    assert!(
        index_of(&session, &root, "KEEPNOTE").is_none(),
        "它还没被排空，不该已经在 root 的对话里：{:#?}",
        session.messages_of(&root)
    );

    // ③ 没被读到的 `Now` 条目同样留在它自己的收件箱里。
    let unread = session.inbox_of(&a1);
    assert_eq!(unread.len(), 1);
    assert_eq!(unread[0].when, Deliver::Now);
    assert_eq!(&*unread[0].text, "LOSTNOW 没人读得到");
}
