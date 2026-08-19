//! 206 §3 / §4：**turn 收尾时两档的命运不同**。
//!
//! **214 把这个文件里的一条翻了面**（那份 issue §注意 点名只翻这一条）：
//! 投给一个已经落终态的 agent，206 时代是「没有人被唤醒、条目留在收件箱、轮末
//! 告警」，214 之后是「**把它叫醒**，它在同一个 turn 里接着干」。这里守的因此
//! 从「206 没有偷偷造一条唤醒边」变成了「214 那条边真的通了，而且没顺手重置
//! 别人的轮次预算」。
//!
//! 另一条（`Now` + `NextTurn` 共存）留下来守 §4 那个直觉陷阱：孤儿收尾
//! 「收件箱非空就告警」的写法会把 `NextTurn` 这种**正常情况报成异常**，
//! 接着有人顺手清干净。214 之后 `Now` 那条不会再剩下（它把人叫醒了），
//! 所以「只有 Now 算未读」的对比那一半移交给 214 自己的「撞顶不唤醒」用例
//! ——那时 `Now` 才会真的剩在收件箱里。
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

/// 目标已经 `Done` → `send` 成功，**并且把它叫醒**：它又发了一次 provider 调用、
/// 读到了那条话、`TurnsUsed` 接着往上数而不是从头开始（214 最硬的一条）。
#[test]
fn sending_to_a_terminal_agent_wakes_it_without_resetting_its_turn_budget() {
    let dir = temp_dir("send-terminal");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        no_delay("call_r_send", sse_text("ROOTFINISHED")),
        no_delay(
            "call_r1",
            sse_tool_call(
                "call_r_send",
                SEND_WIRE,
                r#"{"to":"root/a1","text":"WAKEMEUP 你还在吗"}"#,
            ),
        ),
        // 被叫醒之后那一次请求：**排在 `TASKDONE` 之前**，不然它会撞回子的
        // 第一条路由（首次匹配，见 `support/routed.rs`），两次回同一句话，
        // 「它真的读到了那条话」就测不出来了。
        no_delay("WAKEMEUP", sse_text("AAAWOKE 读到了")),
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
    assert!(session.is_live(&a1), "它还在树上——挡住唤醒的不该是活性闸");
    assert_eq!(
        session.status_of(&a1),
        TurnStatus::Done { truncated: false },
        "被叫醒、答完之后它该又落回终态"
    );

    // ① `send` 仍然成功：它只负责投递。
    let (sent, is_error) = tool_result(&session, &root, "call_r_send");
    assert!(
        !is_error,
        "投给一个终态的 agent 该成功——send 只负责投递：{sent}"
    );

    // ② 它真的又跑了一次（214：唤醒边通了）。
    assert_eq!(
        calls_matching(&server, "WAKEMEUP"),
        1,
        "子该被唤醒后再请求一次，且那次请求里带着投给它的那句话"
    );
    assert_eq!(
        server.calls().len(),
        5,
        "五跳 = root 3 + 子 2（第二次是被叫醒的）：{:?}",
        server.calls().iter().map(|c| c.needle).collect::<Vec<_>>()
    );

    // ③ **`TurnsUsed` 接着往上数，不是从头开始**——214 §三 点名这一波唯一会
    //    静默出错的地方：写成重置，两个 agent 互相喊话就是真无界。
    assert_eq!(
        session.turns_used_of(&a1),
        2,
        "唤醒走的是跟别的调用同一条 `try_call_provider` 出口，照常计数"
    );

    // ④ 那条话进了它的对话，而且**恰好一次**：排空的定点只有一处，
    //    唤醒那条转移自己不 `push_message`（214 §做什么.1）。
    assert_eq!(
        session
            .messages_of(&a1)
            .iter()
            .filter(|m| crate::send_indep_support::message_contains(m, "WAKEMEUP"))
            .count(),
        1,
        "被投递的正文该恰好进一次历史：{:#?}",
        session.messages_of(&a1)
    );

    // ⑤ 收件箱空了，轮末一条告警都没有——没人漏读。
    assert!(
        session.inbox_of(&a1).is_empty(),
        "读过了就该排空：{:?}",
        session.inbox_of(&a1)
    );
    assert_eq!(
        unread_warnings(&events.borrow()),
        Vec::<(String, usize)>::new(),
        "被叫醒并读到了，就不该再报未读"
    );

    // ⑤ 轮次结果仍是 root 本来的终态。
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "未读消息不是失败——判成 Failed(Cancelled) 说明走了会话级取消"
    );
}

/// 收尾时一条 `Now` + 一条 `NextTurn`：前者把人叫醒并被读到（214），
/// 后者**一个字不少地原地不动**，而且从头到尾没被当成未读报出来。
#[test]
fn a_next_turn_note_survives_turn_end_untouched_and_is_never_called_unread() {
    let dir = temp_dir("send-turn-end-both");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        no_delay("call_r_now", sse_text("ROOTFINISHED")),
        // 被叫醒之后那一次请求（214）。**排在 `TASKBOTH` 之前**：首次匹配，
        // 落回那条路由的话它会再发一遍 `call_a_note`，同一个 call_id 出现两次。
        no_delay("LOSTNOW", sse_text("AAAWOKE 读到了")),
        no_delay("call_a_note", sse_text("AAADONE")),
        no_delay(
            "call_r1",
            sse_tool_call(
                "call_r_now",
                SEND_WIRE,
                r#"{"to":"root/a1","text":"LOSTNOW 本轮就该被读到"}"#,
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

    // ① 一条未读告警都没有。`Now` 那条把 a1 叫醒、被它读掉了（214）；
    //    `NextTurn` 那条**本来就不该算未读**——它正等着下一轮，把它报出来就是
    //    把正常情况报成异常，接着下一个人会「顺手清干净」（206 §4 的陷阱）。
    assert_eq!(
        unread_warnings(&events.borrow()),
        Vec::<(String, usize)>::new(),
        "`NextTurn` 不算未读；`Now` 那条已经被唤醒后读掉了"
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

    // ③ `Now` 那条被读掉了：收件箱空，正文进了 a1 的对话。
    assert!(
        session.inbox_of(&a1).is_empty(),
        "被叫醒读过之后该排空：{:?}",
        session.inbox_of(&a1)
    );
    assert!(
        index_of(&session, &a1, "LOSTNOW").is_some(),
        "读到了就该在它的 `Messages` 里：{:#?}",
        session.messages_of(&a1)
    );
}
