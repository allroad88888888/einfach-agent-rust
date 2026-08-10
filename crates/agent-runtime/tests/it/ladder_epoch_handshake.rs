//! 108 验收 ⚠️ epoch 握手：摘要在飞时取消 → 迟到的 `CompactDone` 被 `step` 挡下
//! → `apply_summary` 一次都不该被调用，状态一个字节不变。
//!
//! 反向锁在同一个文件里（`matching_epoch_...`）：同样的场景，不取消，
//! `apply_summary` 确实被调、边界确实推进——`ladder_tier3_after_tier2_exhausted.rs`
//! 已经验过一次这个方向，这里再摆一遍是因为 108 把它跟 epoch 握手列在同一条
//! 验收里，独立可读。
//!
//! 取消的机制跟既有的 `cancel.rs` / `spawn_indep_cancel_tree.rs` 同一套手法：
//! 假服务器收到压缩子的请求之后**挂住不回**（`Route::after` 拉长延迟），一个
//! 后台线程 200ms 后置位 `ctx.cancel_flag()`——这不是靠 sleep 赌毫秒赌出来的巧合，
//! 超时预算被拉得远大于这条测试的时间尺度，观察到的结果只能是取消标志起的作用。

use std::sync::atomic::Ordering;
use std::time::Duration;

use agent_core::{AgentId, Session, TurnStatus};
use agent_runtime::ToolTable;

use crate::ladder_support::{
    SUMMARY_PROMPT_NEEDLE, build_ctx, text_response, tool_call_response,
};
use crate::support;
use crate::support::routed::{Route, RoutedServer};

const WINDOW: u32 = 1000;
const HIGH: u32 = 900;

fn setup_routes(compaction_route: Route) -> Vec<Route> {
    vec![
        compaction_route,
        Route::sse("ROUND4MARK", text_response("继续", HIGH)),
        Route::sse("ROUND3MARK", text_response("继续", HIGH)),
        Route::sse("ROUND2MARK", text_response("继续", HIGH)),
        Route::sse("call_e1", text_response("读完了", HIGH)),
        Route::sse(
            "ROUND1MARK",
            tool_call_response("call_e1", "srv_3Afs_2Fread", r#"{"path": "seed.txt"}"#, HIGH),
        ),
    ]
}

/// 跑到「第 2 档刚清完、第 3 档即将开火」那一刻之前的四轮，返回 session/ctx/server
/// 供各自的第 5 轮场景接着摆。
fn build_up_to_tier3_trigger(
    tag: &str,
    compaction_route: Route,
) -> (Session, agent_runtime::RunnerCtx, RoutedServer) {
    let dir = support::temp_dir(tag);
    std::fs::write(dir.join("seed.txt"), b"SEEDE-CONTENT").unwrap();
    let server = RoutedServer::start(setup_routes(compaction_route));
    let (mut ctx, _events) = build_ctx(server.port, &dir, ToolTable::builtin(), Some(WINDOW));
    let mut session = Session::new(AgentId::root());

    for (i, text) in [
        "ROUND1MARK 读一下 seed.txt",
        "ROUND2MARK 继续聊",
        "ROUND3MARK 继续聊",
        "ROUND4MARK 继续聊", // 第 2 档在这一轮末开火
    ]
    .into_iter()
    .enumerate()
    {
        // 第 2 轮起，每一轮开跑之前都要显式 `begin_turn`（026 判断 13）——漏了
        // 不报错，会话停在上一轮的 `Done`，新的 `UserInput` 被判成
        // `Notice::ProtocolViolation`，这一轮根本没发生过任何请求。
        if i > 0 {
            session.begin_turn();
        }
        let status = agent_runtime::run_turn(&mut session, &mut ctx, text)
            .unwrap_or_else(|e| panic!("{text} 不该是 source failure：{e:?}"));
        assert_eq!(status, TurnStatus::Done { truncated: false }, "{text}");
    }
    let root = AgentId::root();
    assert!(
        !session.send_plan_of(&root).cleared().is_empty(),
        "前提：第 2 档该已经开火"
    );
    assert_eq!(session.send_plan_of(&root).boundary(), 0, "前提：边界还没动");

    (session, ctx, server)
}

/// 取消：压缩子的请求挂住，200ms 后置位取消标志。第 5 轮末该轮到第 3 档，
/// 但摘要没能落地——状态一个字节不变。
#[test]
fn cancel_during_the_summary_child_drops_the_late_result_and_leaves_state_untouched() {
    let compaction_route = Route::sse(SUMMARY_PROMPT_NEEDLE, text_response("不该落地的摘要", 5))
        .after(Duration::from_secs(20));
    let (mut session, ctx, _server) =
        build_up_to_tier3_trigger("ladder-epoch-cancel", compaction_route);
    let root = AgentId::root();
    let before_boundary = session.send_plan_of(&root).boundary();
    let before_cleared = session.send_plan_of(&root).cleared().to_vec();

    let mut ctx = ctx.with_provider_timeout(Duration::from_secs(5));
    let cancel = ctx.cancel_flag();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        cancel.store(true, Ordering::Relaxed);
    });

    session.begin_turn();
    // 触发第 3 档：这一次调用内部会 spawn 压缩子、发起它的第一跳，然后被取消。
    // 压缩发生在**父自己的这一轮已经收尾之后**（108「turn 结束拿到 usage 时
    // 判」），所以取消压缩子不会把父这一轮的 `TurnStatus` 从 `Done` 拉回
    // `Failed`——跟 `spawn_indep_cancel_tree.rs` 那种「取消时子还在父的关键路径
    // 上」不是同一种形状，父这一轮该照样是 `Done`（下面就断言这个）。
    //
    // 正向证据换成**耗时**：如果压缩子的请求真的发生过、真的被取消标志截断，
    // 这次调用该花上至少 200ms（等到置位那一刻）、但远少于 5s 的 provider 超时
    // 预算或 20s 的挂住时长——落回 `cancel.rs` 的同一套判据。要是这一轮其实
    // 什么都没发生（比如又漏了一处 `begin_turn`），耗时会趋近于 0，这条断言
    // 立刻能抓到。
    let start = std::time::Instant::now();
    let status = agent_runtime::run_turn(&mut session, &mut ctx, "ROUND5MARK 继续聊")
        .expect("取消不是 source failure");
    let elapsed = start.elapsed();

    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "父自己的这一轮不受压缩子取消的影响，该正常收尾"
    );
    assert!(
        elapsed >= Duration::from_millis(200) && elapsed < Duration::from_secs(2),
        "该在置位之后的几个 poll 间隔内收尾，不该趋近 0（没发生过）也不该拖到 \
         5s/20s 的预算，实际 {elapsed:?}"
    );

    // 核心断言：apply_summary 一次都没被调用——状态一个字节不变。
    assert_eq!(
        session.send_plan_of(&root).boundary(),
        before_boundary,
        "被取消的摘要不该推进边界"
    );
    assert_eq!(session.send_plan_of(&root).summary(), None, "不该有摘要引用");
    assert_eq!(
        session.send_plan_of(&root).cleared().to_vec(),
        before_cleared,
        "第 2 档已清列表也不该被这次取消动到"
    );
}

/// 反向锁：epoch 对得上（没有取消），第 5 轮末第 3 档正常落地——边界真的动了、
/// 摘要正文真的能读回来。
#[test]
fn matching_epoch_lets_the_summary_apply_and_the_boundary_really_moves() {
    let compaction_route = Route::sse(SUMMARY_PROMPT_NEEDLE, text_response("SUMMARYE 落地的摘要", 5));
    let (mut session, mut ctx, _server) =
        build_up_to_tier3_trigger("ladder-epoch-reverse", compaction_route);
    let root = AgentId::root();

    session.begin_turn();
    let status = agent_runtime::run_turn(&mut session, &mut ctx, "ROUND5MARK 继续聊")
        .expect("正常路径不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let plan = session.send_plan_of(&root);
    assert!(plan.boundary() > 0, "没有取消时边界该真的推进");
    let id = plan.summary().expect("该有摘要引用");
    assert_eq!(
        session.summary_text(&root, id).as_deref(),
        Some("SUMMARYE 落地的摘要"),
        "摘要正文该原样读得回来"
    );
}
