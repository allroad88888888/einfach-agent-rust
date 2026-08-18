//! 206 的 `when="next_turn"` 端到端，两条：
//!
//! 1. 子 agent 在轮末给 root 留一张条 → 这一轮**照常结束、不被延长、不告警** →
//!    下一轮用户随便说句话 → 那条留言**在这一轮的 prompt 里**，而且排在用户
//!    这句新话**前面**（排空的定点在 `begin_turn` 之后、第一次组装请求之前）。
//! 2. `/undo` 的归属（206「做什么」§2 那条脚注）：下一轮排空之后 `/undo` 掉
//!    **新**这一轮 → 那条留言**退回收件箱**，既不是消失，也不是留在 `Messages`
//!    里。排空点放在 `begin_turn` 之前的话，它会挂在上一轮尾巴上，这条就红。
//!
//! 黑盒来源与「实现体没读」的声明见 `send_indep_support/mod.rs` 顶部。

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use agent_core::{AgentId, AgentLimits, Deliver, Session, TurnStatus, UndoReport};
use agent_runtime::{AgentEvent, RunnerCtx, run_turn};

use crate::send_indep_support::{
    Route, RoutedServer, SEND_WIRE, build_ctx, index_of, injected, sse_text, sse_tool_call,
    temp_dir, tool_result, unread_warnings, wire_tool_name,
};

/// 第二轮用户说的那句话——也是它那一跳的路由 needle。
const SECOND_ASK: &str = "SECONDASK 这一轮我另有一问";

fn no_delay(needle: &'static str, lines: Vec<String>) -> Route {
    Route {
        needle,
        delay: Duration::ZERO,
        status: 200,
        lines,
    }
}

/// 跑完第一轮，停在「root 的收件箱里躺着一张 `NextTurn` 的条」这个状态上。
fn after_the_note_was_left(
    tag: &str,
) -> (
    Session,
    RunnerCtx,
    Rc<RefCell<Vec<AgentEvent>>>,
    RoutedServer,
) {
    let dir = temp_dir(tag);
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        no_delay("SECONDASK", sse_text("ROUND2DONE")),
        no_delay("call_a_note", sse_text("AAADONE")),
        no_delay("call_r1", sse_text("ROUND1DONE")),
        no_delay(
            "TASKNOTE",
            sse_tool_call(
                "call_a_note",
                SEND_WIRE,
                r#"{"to":"root","text":"LEFTNOTE 后台那半边量到 42","when":"next_turn"}"#,
            ),
        ),
        no_delay(
            "kickoff-note",
            sse_tool_call("call_r1", &spawn_wire, r#"{"task":"TASKNOTE 干完留张条"}"#),
        ),
    ]);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_send();
    let (mut ctx, events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status =
        run_turn(&mut session, &mut ctx, "kickoff-note 派个活").expect("留言不是 source failure");
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "留了张条不该改变这一轮的结果"
    );

    let root = AgentId::root();
    let a1 = root.child(1);
    let (sent, is_error) = tool_result(&session, &a1, "call_a_note");
    assert!(!is_error, "给 root 留 next_turn 的条该成功：{sent}");

    // 这一轮**没被延长**：脚本里就四跳，留言没有把任何人重新拉起来。
    assert_eq!(
        server.calls().len(),
        4,
        "四跳 = root 2 + 子 2：{:?}",
        server.calls().iter().map(|c| c.needle).collect::<Vec<_>>()
    );
    // 而且**不告警**——`NextTurn` 留在收件箱里是正常，不是未读。
    assert!(
        unread_warnings(&events.borrow()).is_empty(),
        "`NextTurn` 条目不该被算成未读：{:?}",
        unread_warnings(&events.borrow())
    );

    let inbox = session.inbox_of(&root);
    assert_eq!(inbox.len(), 1, "留言该在 root 的收件箱里等着：{inbox:?}");
    assert_eq!(inbox[0].when, Deliver::NextTurn);
    assert_eq!(inbox[0].from, a1);
    assert!(
        index_of(&session, &root, "LEFTNOTE").is_none(),
        "这一轮还不该看到它：{:#?}",
        session.messages_of(&root)
    );

    (session, ctx, events, server)
}

/// 下一轮开始时送达，而且排在用户这一轮的第一句话**前面**。
#[test]
fn a_next_turn_note_reaches_the_prompt_of_the_following_turn_before_the_user_line() {
    let (mut session, mut ctx, _events, server) = after_the_note_was_left("send-next-turn");
    let root = AgentId::root();

    session.begin_turn();
    let status = run_turn(&mut session, &mut ctx, SECOND_ASK).expect("第二轮不是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    // ① 收件箱空了，正文进了对话。
    assert!(
        session.inbox_of(&root).is_empty(),
        "该被 drain_next_turn 收走"
    );
    let (note_idx, note) = injected(&session, &root, "LEFTNOTE");
    assert!(note.contains("root/a1"), "认得出是谁留的：{note}");
    assert!(
        note.ends_with("LEFTNOTE 后台那半边量到 42"),
        "原文原样：{note}"
    );

    // ② 排在用户这一轮的第一句话**前面**。
    let ask_idx = index_of(&session, &root, "SECONDASK").expect("用户这句该在历史里");
    assert!(
        note_idx < ask_idx,
        "留言该排在用户这句新话之前：note@{note_idx} ask@{ask_idx}\n{:#?}",
        session.messages_of(&root)
    );

    // ③ 真的进了这一轮的 prompt——断的是发出去的请求体，不只是本地历史。
    let body = &server.call("SECONDASK").expect("第二轮该发过一次请求").body;
    let note_pos = body.find("LEFTNOTE").expect("留言该进 prompt");
    let ask_pos = body.find("SECONDASK").expect("用户这句该进 prompt");
    assert!(
        note_pos < ask_pos,
        "请求体里也得是「先留言、后用户这句」：{body}"
    );
}

/// `/undo` 掉**新**这一轮 → 留言退回收件箱（不是消失，也不是留在 `Messages` 里）。
#[test]
fn undoing_the_new_turn_puts_the_note_back_into_the_inbox() {
    let (mut session, mut ctx, _events, _server) = after_the_note_was_left("send-next-turn-undo");
    let root = AgentId::root();
    let queued_before = session.inbox_of(&root);
    assert_eq!(queued_before.len(), 1);

    session.begin_turn();
    let status = run_turn(&mut session, &mut ctx, SECOND_ASK).expect("第二轮不是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });
    assert!(session.inbox_of(&root).is_empty(), "先确认它真的被收走了");
    assert!(index_of(&session, &root, "LEFTNOTE").is_some());

    match agent_runtime::undo::undo_turn(&mut session, &mut ctx) {
        UndoReport::Applied { .. } => {}
        other => panic!("排空是纯状态，不该拦住 undo：{other:?}"),
    }

    assert_eq!(
        session.inbox_of(&root),
        queued_before,
        "留言该**退回收件箱**（`from` / `text` / `when` 逐字相同），而不是消失"
    );
    assert!(
        index_of(&session, &root, "LEFTNOTE").is_none(),
        "也不该继续留在 `Messages` 里：{:#?}",
        session.messages_of(&root)
    );
    assert!(
        index_of(&session, &root, "SECONDASK").is_none(),
        "新这一轮整个退掉了"
    );
}
