//! 108 验收（从 106 移交）：摘要失败/超时 → `CompactFailed` → 父不卡死、边界
//! 不动、下一轮照常跑完。反向锁：成功路径回执确实是 `CompactDone` 而不是
//! `CompactFailed`——只测失败路径的话，一个「永远失败」的实现照样把前一条
//! 走绿（106「行为验收移交」原话）。
//!
//! 失败走**超时**：压缩子的请求连接上但永远不应答（`Route::after` 拉一个远超
//! `with_provider_timeout` 的延迟）。这不是取消——没有人翻 `cancel_flag`，是
//! 压缩子自己没能按时说完话，跟 [`crate::ladder_epoch_handshake`] 测的「取消」
//! 是两回事：那条测的是 epoch 闸，这条测的是「失败是正常事件，不是异常路径」。

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use agent_core::{AgentId, Notice, Session, TurnStatus};
use agent_runtime::{AgentEvent, RunnerEvent, ToolTable, run_turn};

use crate::ladder_support::{SUMMARY_PROMPT_NEEDLE, build_ctx, text_response};
use crate::support;
use crate::support::routed::{Route, RoutedServer};

const WINDOW: u32 = 1000;
const HIGH: u32 = 900;

fn routes(compaction_route: Route) -> Vec<Route> {
    vec![
        compaction_route,
        Route::sse("ROUND5MARK", text_response("收尾", HIGH)),
        Route::sse("ROUND4MARK", text_response("继续", HIGH)),
        Route::sse("ROUND3MARK", text_response("继续", HIGH)),
        Route::sse("ROUND2MARK", text_response("继续", HIGH)),
        Route::sse("ROUND1MARK", text_response("继续", HIGH)),
    ]
}

/// 跑到第 3 轮末（历史刚好够长，第 4 轮末就会触发第 3 档）为止，纯文本没有
/// 任何工具调用——`tool_results_to_clear` 永远是空的，过了保护区门槛就直通
/// 第 3 档，不需要先摆一轮工具调用。
fn build_up_to_tier3_trigger(
    tag: &str,
    compaction_route: Route,
    provider_timeout: Duration,
) -> (
    Session,
    agent_runtime::RunnerCtx,
    RoutedServer,
    Rc<RefCell<Vec<AgentEvent>>>,
) {
    let dir = support::temp_dir(tag);
    let server = RoutedServer::start(routes(compaction_route));
    let (ctx, events) = build_ctx(server.port, &dir, ToolTable::builtin(), Some(WINDOW));
    let mut ctx = ctx.with_provider_timeout(provider_timeout);
    let mut session = Session::new(AgentId::root());

    for (i, text) in ["ROUND1MARK 继续聊", "ROUND2MARK 继续聊", "ROUND3MARK 继续聊"]
        .into_iter()
        .enumerate()
    {
        // 第 2 轮起，每一轮开跑之前都要显式 `begin_turn`（026 判断 13）——漏了
        // 不报错，新的 `UserInput` 撞上上一轮的 `Done` 会被判成
        // `Notice::ProtocolViolation`，这一轮根本没发生过任何请求。
        if i > 0 {
            session.begin_turn();
        }
        let status = run_turn(&mut session, &mut ctx, text)
            .unwrap_or_else(|e| panic!("{text} 不该是 source failure：{e:?}"));
        assert_eq!(status, TurnStatus::Done { truncated: false }, "{text}");
    }
    let root = AgentId::root();
    assert_eq!(
        session.send_plan_of(&root).boundary(),
        0,
        "前提：前 3 轮历史太短，压缩还不该触发"
    );

    (session, ctx, server, events)
}

fn root_notices(events: &Rc<RefCell<Vec<AgentEvent>>>, root: &AgentId) -> Vec<Notice> {
    events
        .borrow()
        .iter()
        .filter(|e| &e.agent == root)
        .filter_map(|e| match &e.event {
            RunnerEvent::Notice(n) => Some(n.clone()),
            _ => None,
        })
        .collect()
}

/// 压缩子超时：第 4 轮末触发第 3 档，压缩子的请求永远不应答——父不该卡死，
/// 那一轮该正常收尾（`Done`），边界不该动，下一轮（第 5 轮）该照常跑完，
/// 且回执只该是 `CompactionFailed`，不该混进 `CompactionSummaryReceived`。
#[test]
fn a_timed_out_summary_child_yields_compact_failed_and_the_parent_keeps_going() {
    let compaction_route = Route::sse(SUMMARY_PROMPT_NEEDLE, text_response("不该被用到", 5))
        .after(Duration::from_secs(20));
    let (mut session, mut ctx, _server, events) = build_up_to_tier3_trigger(
        "ladder-compact-failed",
        compaction_route,
        Duration::from_millis(150),
    );
    let root = AgentId::root();

    session.begin_turn();
    // 第 4 轮末触发第 3 档；压缩子超时，父这一轮本身照样正常收尾。
    let status = run_turn(&mut session, &mut ctx, "ROUND4MARK 继续聊")
        .expect("压缩子超时不该让这一轮本身变成 source failure");
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "父不该卡死：这一轮该正常收尾，不是被压缩子的超时拖住"
    );

    let plan = session.send_plan_of(&root);
    assert_eq!(plan.boundary(), 0, "压缩没做成，边界不该动");
    assert_eq!(plan.summary(), None);

    let notices = root_notices(&events, &root);
    assert!(
        notices.iter().any(|n| matches!(n, Notice::CompactionFailed)),
        "该有一条 CompactionFailed：{notices:?}"
    );
    assert!(
        !notices
            .iter()
            .any(|n| matches!(n, Notice::CompactionSummaryReceived)),
        "不该混进 CompactionSummaryReceived：{notices:?}"
    );

    // 下一轮（第 5 轮）照常跑完——不是「这一轮凑巧没死，其实已经卡死了」。
    session.begin_turn();
    let status =
        run_turn(&mut session, &mut ctx, "ROUND5MARK 收尾").expect("下一轮不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    assert!(
        session.children_of(&root).is_empty(),
        "失败的压缩子也不该继续挂在树上"
    );
}

/// 反向锁：同样的场景，压缩子这次正常应答——边界真的推进，回执确实是
/// `CompactionSummaryReceived`（`CompactDone`），不是 `CompactionFailed`。
#[test]
fn and_the_success_path_really_is_compact_done_not_compact_failed() {
    let compaction_route = Route::sse(SUMMARY_PROMPT_NEEDLE, text_response("SUMMARYF 正常落地", 5));
    let (mut session, mut ctx, _server, events) = build_up_to_tier3_trigger(
        "ladder-compact-failed-reverse",
        compaction_route,
        Duration::from_secs(5),
    );
    let root = AgentId::root();

    session.begin_turn();
    let status = run_turn(&mut session, &mut ctx, "ROUND4MARK 继续聊")
        .expect("正常路径不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let plan = session.send_plan_of(&root);
    assert!(plan.boundary() > 0, "正常路径边界该真的推进");

    let notices = root_notices(&events, &root);
    assert!(
        notices
            .iter()
            .any(|n| matches!(n, Notice::CompactionSummaryReceived)),
        "该有一条 CompactionSummaryReceived：{notices:?}"
    );
    assert!(
        !notices.iter().any(|n| matches!(n, Notice::CompactionFailed)),
        "不该混进 CompactionFailed：{notices:?}"
    );
}
