//! 052 验收「红线 6 对抗测试」：**后台子在飞时世代被推走（取消/undo 那一下），
//! 它的结果回来时进不了这个世界**。写法上跟 043 的 `mcp_epoch_writeback.rs` 同
//! 一个骨架——把「结果确实回来了」和「世界里没有它」两件事钉在一起，闸拆了就红
//! ——但**去掉了时序**：这里的在飞调用是一次 Web 宿主回传（`Location::Web` 的
//! 工具），它的落地时刻由测试自己决定，不靠 sleep 赌毫秒。
//!
//! # 怎么把静默失败变红
//!
//! ```text
//! 1. root 一跳吐两个调用：spawn(background=true) + 一个远端工具
//!    → 后台子当场开工；root 那个远端槽 Pending，于是 root **不落终态**
//!      （轮末的孤儿收尾因此不触发，这个子会活着等在那儿 —— 见下面为什么重要）
//! 2. 后台子自己也吐一个远端工具调用 → 它的凭据记下**起飞那一刻的 epoch=0**
//!    → 两张在飞表都空了，run_turn 返回 ToolsPending（非终态）
//! 3. 测试在这里 bump epoch：一次真的 Cancel（undo 走的是同一个 bump）→ epoch=1
//! 4. 测试把子那次远端调用的结果回传进来：`resolve_remote_tool` 照常发一条
//!    ToolExecuted（**证明结果真的回来了**），组出 ToolResult{epoch:0} 喂回泵
//! 5. `Session::step` 入口的 epoch 闸 0 != 1 → 丢弃、不写消息历史
//! ```
//!
//! **为什么第 1 步要让 root 停在非终态**：这样后台子在整个过程里**一直活着**，
//! 于是「结果没落地」就只可能是 epoch 闸干的——活性闸（`step.rs` 里 epoch 闸后
//! 面那一道）在这条路上是被排除掉的。断言 `is_live(child)` 把这个排除写死。
//!
//! 下面第二条用例是**孪生对照**：同一份脚本、同一次回传，只是不 bump epoch，
//! 结果就该老老实实落进子的历史。一个进一个不进，闸的存在才是被测出来的。

mod spawn_bg_support;

use std::time::Duration;

use agent_core::{AgentId, Event, Failure, Session, ToolCallId, TurnStatus};
use agent_runtime::{RemoteToolOutput, RunnerEvent, resolve_remote_tool, run_turn};

use spawn_bg_support::{
    Route, RoutedServer, any_message_mentions, build_ctx, sse_text, sse_tool_call, sse_tool_calls,
    temp_dir, wire_tool_name,
};

/// Web 宿主执行的交互工具之一（`ToolTable::standard` 注册，`Location::Web`）。
const REMOTE_TOOL: &str = "ask_user_question";

fn routes() -> Vec<Route> {
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);
    let remote_wire = wire_tool_name(REMOTE_TOOL);
    vec![
        // 子在拿到远端回传之后的第二跳（只有对照组会走到）。
        Route { needle: "call_child_ask", delay: Duration::ZERO, status: 200, lines: sse_text("子答完了") },
        // 子的第一跳：吐一个远端工具调用 —— 它会**停在那儿**等宿主回传。
        Route {
            needle: "GHOSTTASK",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_child_ask", &remote_wire, r#"{"question":"要我继续吗"}"#),
        },
        // root 的第一跳：一个后台 spawn + 一个远端工具调用（后者让 root 停在
        // `ToolsPending`，这一轮不落终态）。
        Route {
            needle: "kickoff",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_calls(&[
                ("call_bg", &spawn_wire, r#"{"task":"GHOSTTASK 后台干活","background":true}"#),
                ("call_root_ask", &remote_wire, r#"{"question":"顺便问一句"}"#),
            ]),
        },
    ]
}

/// 起一轮，停在「root 等远端回传、后台子也等远端回传」那个状态上。
fn park(tag: &str) -> (Session, agent_runtime::RunnerCtx, std::rc::Rc<std::cell::RefCell<Vec<agent_runtime::AgentEvent>>>, RoutedServer) {
    let dir = temp_dir(tag);
    let server = RoutedServer::start(routes());
    let tools = agent_runtime::ToolTable::standard().with_spawn(agent_core::AgentLimits::default());
    let (mut ctx, events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff 一个后台子 + 一个远端工具");
    assert_eq!(
        status,
        TurnStatus::ToolsPending,
        "root 该停在等远端回传的非终态上（这条测试的前提）：{status:?}"
    );
    assert!(session.is_live(&AgentId::new("root/a1")), "后台子该活着");
    (session, ctx, events, server)
}

/// 世代被推走之后回来的后台子结果 —— **丢弃**，一个字节都不进消息历史。
#[test]
fn a_background_childs_late_result_is_dropped_by_the_epoch_gate() {
    let (mut session, mut ctx, events, _server) = park("bg-epoch-ghost");
    let child = AgentId::new("root/a1");

    // bump epoch：一次真的取消（undo 走的是同一个 bump，`undo.rs` 与 `commit.rs`）。
    let before = session.epoch();
    let _ = session.step(Event::Cancel { agent: AgentId::root() });
    assert_ne!(session.epoch(), before, "取消该推走世代，否则这条测试是空跑的");
    assert_eq!(session.status(), TurnStatus::Failed(Failure::Cancelled));

    // 幽灵结果回来了。
    let status = resolve_remote_tool(
        &mut session,
        &mut ctx,
        child.clone(),
        ToolCallId::new("call_child_ask"),
        RemoteToolOutput::Success("GHOSTANSWER 幽灵答案".to_string()),
    )
    .expect("这次回传本身是合法的（槽位还在等它）—— 被挡掉的是它的落地，不是它的受理");
    assert_eq!(status, TurnStatus::Failed(Failure::Cancelled));

    // 它**确实回来了**：回传路照常发过一条 ToolExecuted。没有这条，下面那句
    // 「没写进去」对一个根本没回来的结果也成立。
    let events = events.borrow();
    assert!(
        events.iter().any(|e| matches!(
            &e.event,
            RunnerEvent::ToolExecuted { tool, is_error: false, .. } if &**tool == REMOTE_TOOL
        )),
        "远端回传该真的走完（发过 ToolExecuted）——否则没测到闸：{events:#?}"
    );

    // 而它没有被写进世界。**子还活着**（下面这条），所以挡住它的只可能是 epoch
    // 闸——活性闸在这条路上被排除了。
    assert!(session.is_live(&child), "子该还活着：活性闸不该是这条测试的解释");
    assert!(
        !any_message_mentions(&session, &[AgentId::root(), child.clone()], "GHOSTANSWER"),
        "幽灵结果被写进了已经回滚掉的世界——回写前的 epoch 比对没挡住（红线 6）：{:#?}",
        session.messages_of(&child)
    );
}

/// 孪生对照：**不** bump 世代，同一次回传该老老实实落进子的历史。
#[test]
fn and_the_very_same_result_lands_when_the_epoch_still_matches() {
    let (mut session, mut ctx, _events, _server) = park("bg-epoch-control");
    let child = AgentId::new("root/a1");

    let _ = resolve_remote_tool(
        &mut session,
        &mut ctx,
        child.clone(),
        ToolCallId::new("call_child_ask"),
        RemoteToolOutput::Success("GHOSTANSWER 幽灵答案".to_string()),
    )
    .expect("回传该被受理");

    assert!(
        any_message_mentions(&session, std::slice::from_ref(&child), "GHOSTANSWER"),
        "世代没变时同一次回传该落地（否则上一条测试是空跑的）：{:#?}",
        session.messages_of(&child)
    );
}
