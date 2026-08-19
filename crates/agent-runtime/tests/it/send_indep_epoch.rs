//! 206 · 红线 6：**投递之后、收信人的回执落地之前世代被推走 → 那条回执被丢掉，
//! 不写进任何人的历史**；而**被投递的那条本身不是在飞凭据**，它照旧躺在收件箱里。
//!
//! 骨架照 `spawn_bg_epoch_writeback.rs`：在飞的那一笔是一次 Web 宿主回传
//! （`Location::Web` 的工具），它的落地时刻由测试自己决定，不靠 sleep 赌毫秒。
//!
//! ```text
//! 1. root 一跳吐两个调用：spawn(background=true) + 一个远端工具
//!    → root 停在 `ToolsPending`（**它就是收信人**，手上有一笔在飞的凭据）
//! 2. 后台子一跳吐两个调用：send(to=root) + 一个远端工具
//!    → 消息当场投进 root 的收件箱；两张在飞表都空了，run_turn 返回 ToolsPending
//! 3. 测试在这里推世代：一次真的 Cancel（`/undo` 走的是同一个 bump）
//! 4. 测试把 root 那次远端调用的结果回传进来 → ToolExecuted 发出来了
//!    （**证明回执真的回来了**），但 `Session::step` 入口的 epoch 闸把它丢掉
//! ```
//!
//! 下面第二条是**孪生对照**：同一份脚本、同一次回传，只是不推世代，回执就该
//! 老老实实落进 root 的历史（顺带证明第一条不是空跑的）。
//!
//! 黑盒来源与「实现体没读」的声明见 `send_indep_support/mod.rs` 顶部。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, Deliver, Event, Failure, Session, ToolCallId, TurnStatus};
use agent_runtime::{RemoteToolOutput, RunnerEvent, resolve_remote_tool, run_turn};

use crate::send_indep_support::{
    Route, RoutedServer, SEND_WIRE, build_ctx, index_of, sse_text, sse_tool_calls, temp_dir,
    wire_tool_name,
};

/// Web 宿主执行的交互工具之一（`ToolTable::standard` 注册，`Location::Web`）。
const REMOTE_TOOL: &str = "ask_user_question";

fn no_delay(needle: &'static str, lines: Vec<String>) -> Route {
    Route {
        needle,
        delay: Duration::ZERO,
        status: 200,
        lines,
    }
}

type Parked = (
    Session,
    agent_runtime::RunnerCtx,
    std::rc::Rc<std::cell::RefCell<Vec<agent_runtime::AgentEvent>>>,
    RoutedServer,
);

/// 起一轮，停在「root 等远端回传、收件箱里已经躺着后台子投来的一条」那个状态上。
fn park(tag: &str) -> Parked {
    let dir = temp_dir(tag);
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);
    let remote_wire = wire_tool_name(REMOTE_TOOL);

    let server = RoutedServer::start(vec![
        no_delay("GHOSTRECEIPT", sse_text("ROOTFINISHED")),
        no_delay(
            "GHOSTTASK",
            sse_tool_calls(&[
                (
                    "call_child_send",
                    SEND_WIRE,
                    r#"{"to":"root","text":"MESHNOTE 后台那半边先报一句"}"#,
                ),
                (
                    "call_child_ask",
                    &remote_wire,
                    r#"{"question":"要我继续吗"}"#,
                ),
            ]),
        ),
        no_delay(
            "kickoff-epoch",
            sse_tool_calls(&[
                (
                    "call_bg",
                    &spawn_wire,
                    r#"{"task":"GHOSTTASK 后台干活","background":true}"#,
                ),
                (
                    "call_root_ask",
                    &remote_wire,
                    r#"{"question":"顺便问一句"}"#,
                ),
            ]),
        ),
    ]);

    let tools = agent_runtime::ToolTable::standard()
        .with_spawn(AgentLimits::default())
        .with_send();
    let (mut ctx, events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(
        &mut session,
        &mut ctx,
        "kickoff-epoch 一个后台子 + 一个远端工具",
    )
    .expect("远端派发不是 source failure");
    assert_eq!(
        status,
        TurnStatus::ToolsPending,
        "root 该停在等远端回传的非终态上（这条测试的前提）：{status:?}"
    );

    let root = AgentId::root();
    let inbox = session.inbox_of(&root);
    assert_eq!(inbox.len(), 1, "后台子该已经投进来一条：{inbox:?}");
    assert_eq!(inbox[0].when, Deliver::Now);
    assert!(
        index_of(&session, &root, "MESHNOTE").is_none(),
        "root 还没组装下一次请求，那条不该已经进对话"
    );
    (session, ctx, events, server)
}

/// 世代被推走之后回来的回执 —— **丢弃**，一个字节都不进任何人的历史。
#[test]
fn a_receipt_that_lands_after_the_epoch_moved_is_dropped_for_the_recipient_too() {
    let (mut session, mut ctx, events, _server) = park("send-epoch-ghost");
    let root = AgentId::root();
    let child = root.child(1);

    let before = session.epoch();
    let _ = session.step(Event::Cancel {
        agent: root.clone(),
    });
    assert_ne!(session.epoch(), before, "推世代失败，这条测试是空跑的");
    assert_eq!(session.status(), TurnStatus::Failed(Failure::Cancelled));

    let _ = resolve_remote_tool(
        &mut session,
        &mut ctx,
        root.clone(),
        ToolCallId::new("call_root_ask"),
        RemoteToolOutput::Success("GHOSTRECEIPT 迟到的回执".to_string()),
    )
    .expect("这次回传本身是合法的——被挡掉的是它的落地，不是它的受理");

    // 它**确实回来了**：回传路照常发过一条 ToolExecuted。
    let seen = events.borrow();
    assert!(
        seen.iter().any(|e| matches!(
            &e.event,
            RunnerEvent::ToolExecuted { tool, .. } if &**tool == REMOTE_TOOL
        )),
        "远端回传该真的走完（发过 ToolExecuted）——否则没测到闸"
    );
    drop(seen);

    for agent in [&root, &child] {
        assert!(
            index_of(&session, agent, "GHOSTRECEIPT").is_none(),
            "迟到的回执被写进了已经推走世代的世界（红线 6）：{:#?}",
            session.messages_of(agent)
        );
    }

    // 而**被投递的那条不是在飞凭据**：`send` 当场回写、不产生任何要过闸的东西，
    // 所以推世代动不到它，它照旧躺在收件箱里等下一次组装请求。
    let inbox = session.inbox_of(&root);
    assert_eq!(
        inbox.len(),
        1,
        "投递是纯写命令，不该被 epoch 闸连坐：{inbox:?}"
    );
    assert_eq!(&*inbox[0].text, "MESHNOTE 后台那半边先报一句");
}

/// 孪生对照：**不**推世代，同一次回传该老老实实落进 root 的历史。
#[test]
fn and_the_very_same_receipt_lands_when_the_epoch_still_matches() {
    let (mut session, mut ctx, _events, _server) = park("send-epoch-control");
    let root = AgentId::root();

    let _ = resolve_remote_tool(
        &mut session,
        &mut ctx,
        root.clone(),
        ToolCallId::new("call_root_ask"),
        RemoteToolOutput::Success("GHOSTRECEIPT 迟到的回执".to_string()),
    )
    .expect("回传该被受理");

    assert!(
        index_of(&session, &root, "GHOSTRECEIPT").is_some(),
        "世代没变时同一次回传该落地（否则上一条测试是空跑的）：{:#?}",
        session.messages_of(&root)
    );
    // 顺带：回执落地之后 root 又组装了一次请求，那条投递也就在这时被排空了。
    assert!(
        index_of(&session, &root, "MESHNOTE").is_some(),
        "root 下一次组装请求之前该把收件箱排空：{:#?}",
        session.messages_of(&root)
    );
}
