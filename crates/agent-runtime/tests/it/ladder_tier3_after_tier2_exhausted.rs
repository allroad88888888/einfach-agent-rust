//! 108 验收：造一个「工具返回全清光仍然超」的场景 → 第 3 档触发，且**在第 2 档
//! 之后**。同时兼任「反向锁：epoch 对得上时确实调了、边界真的动了」——`apply_summary`
//! 真的被调用、`SendPlan` 的边界真的推进、摘要正文真的能读回来。
//!
//! 用内容路由的 [`RoutedServer`]（不是严格顺序）：第 3 档触发时会在**同一次**
//! `run_turn` 调用内部多打一跳给压缩子 agent（106：摘要子零工具，结构上单轮，
//! 且 `CompactSlots` 不跨 `run_turn`——收割必须在触发它的那次调用里完成），
//! 这一跳具体落在服务器连接序列的第几位不是这条测试该关心的事，按内容认领
//! 才不会把「测试对时序的猜测」和「阶梯到底对不对」耦合在一起。
//!
//! 路由按「离现在最近的先声明」排列（`Route` 文档：越具体的 needle 越要排在
//! 前面）——历史是累积的，每一轮的请求体都包含它之前所有轮次的文本，不这样排
//! 旧路由会把新一轮的请求错认成自己。

use agent_core::{AgentId, Session, TurnStatus};
use agent_runtime::{RunnerEvent, ToolTable, run_turn};

use crate::ladder_support::{
    SUMMARY_PROMPT_NEEDLE, build_ctx, text_response, tool_call_response,
};
use crate::support;
use crate::support::routed::{Route, RoutedServer};

const WINDOW: u32 = 1000;
const HIGH: u32 = 900; // 90%，冲过 85%
const SUMMARY_TEXT: &str = "SUMMARYTEXT_UNIQUE 这是压缩子写的摘要正文";

fn routes() -> Vec<Route> {
    vec![
        Route::sse(SUMMARY_PROMPT_NEEDLE, text_response(SUMMARY_TEXT, 5)),
        Route::sse("ROUND6MARK", text_response("收到", HIGH)),
        Route::sse("ROUND5MARK", text_response("继续", HIGH)),
        Route::sse("ROUND4MARK", text_response("继续", HIGH)),
        Route::sse("ROUND3MARK", text_response("继续", HIGH)),
        Route::sse("ROUND2MARK", text_response("继续", HIGH)),
        // round1 hop2：工具结果回来之后的收敛跳，凭工具调用 id 认领。
        Route::sse(
            "call_c1",
            text_response("读完了", HIGH),
        ),
        // round1 hop1：还没有 call_c1，凭这一轮的开场白认领。
        Route::sse(
            "ROUND1MARK",
            tool_call_response("call_c1", "srv_3Afs_2Fread", r#"{"path": "seed.txt"}"#, HIGH),
        ),
    ]
}

#[test]
fn tier3_fires_only_after_tier2_is_exhausted_and_the_boundary_really_moves() {
    let dir = support::temp_dir("ladder-tier3-after-tier2");
    std::fs::write(dir.join("seed.txt"), b"SEEDX-CONTENT").unwrap();
    let server = RoutedServer::start(routes());
    let (mut ctx, events) = build_ctx(server.port, &dir, ToolTable::builtin(), Some(WINDOW));
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();

    let run = |session: &mut Session, ctx: &mut agent_runtime::RunnerCtx, text: &str| {
        run_turn(session, ctx, text).unwrap_or_else(|e| panic!("{text} 不该是 source failure：{e:?}"))
    };

    assert_eq!(
        run(&mut session, &mut ctx, "ROUND1MARK 读一下 seed.txt"),
        TurnStatus::Done { truncated: false }
    );
    // 第 2 轮起，每一轮开跑之前都要显式 `begin_turn`（026 判断 13）——漏了不
    // 报错：新的 `UserInput` 会撞上上一轮的 `Done` 状态被判成
    // `Notice::ProtocolViolation`，这一轮根本没发生任何请求，而
    // `assert_eq!(status, Done)` 这种断言在「真的跑完」与「压根没发生」两种
    // 情况下都成立，测不出这个坑。
    session.begin_turn();
    assert_eq!(
        run(&mut session, &mut ctx, "ROUND2MARK 继续聊"),
        TurnStatus::Done { truncated: false }
    );
    session.begin_turn();
    assert_eq!(
        run(&mut session, &mut ctx, "ROUND3MARK 继续聊"),
        TurnStatus::Done { truncated: false }
    );

    // 第 4 轮末：usage 冲线，且第 1 轮的工具结果这时刚好滑出保护区
    // （保护区=最近 3 轮=2/3/4，第 1 轮在外面）——第 2 档该在这里开火。
    session.begin_turn();
    assert_eq!(
        run(&mut session, &mut ctx, "ROUND4MARK 继续聊"),
        TurnStatus::Done { truncated: false }
    );
    let after_tier2 = session.send_plan_of(&root);
    assert!(
        !after_tier2.cleared().is_empty(),
        "第 4 轮末该已经触发第 2 档，清过第 1 轮的工具结果"
    );
    assert_eq!(
        after_tier2.boundary(),
        0,
        "第 2 档只清工具结果，不该动边界——这是第 3 档之前的状态"
    );

    // 第 5 轮末：第 2 档已经没有新东西可清（第 1 轮清过了，第 2/3 轮从来没有
    // 工具调用），usage 仍然冲线——第 3 档该接手，且这一步是同一次 run_turn
    // 调用内部完成的（压缩子被真实 spawn 并驱动到底）。
    session.begin_turn();
    assert_eq!(
        run(&mut session, &mut ctx, "ROUND5MARK 继续聊"),
        TurnStatus::Done { truncated: false }
    );

    let after_tier3 = session.send_plan_of(&root);
    assert!(
        after_tier3.boundary() > 0,
        "第 3 档该已经推进边界——反向锁：epoch 对得上时真的动了状态"
    );
    let summary_id = after_tier3
        .summary()
        .expect("边界推进的同时该有一份摘要引用");
    assert_eq!(
        session.summary_text(&root, summary_id).as_deref(),
        Some(SUMMARY_TEXT),
        "摘要正文该原样读得回来"
    );

    // 顺序断言：第 2 档确实发生在第 3 档之前（第 4 轮末已经清过，第 5 轮末才摘要）。
    assert_eq!(
        after_tier2.cleared(),
        after_tier3.cleared(),
        "第 3 档触发之后已清列表不该再变——第 2 档在它之前就已经清完了"
    );

    // 第 6 轮：压缩之后会话照常往下跑，证明「父不卡死」。
    session.begin_turn();
    assert_eq!(
        run(&mut session, &mut ctx, "ROUND6MARK 收尾"),
        TurnStatus::Done { truncated: false }
    );

    // 反向锁的另一半：这次回执确实是 CompactDone（Notice::CompactionSummaryReceived），
    // 不是 CompactFailed——只测「边界动了」的话，一个总是把 upto 写死的错误实现
    // 也可能凑巧看起来对，Notice 是 108/105 定死的可观测信号。
    let events = events.borrow();
    let root_notices: Vec<_> = events
        .iter()
        .filter(|e| e.agent == root)
        .filter_map(|e| match &e.event {
            RunnerEvent::Notice(n) => Some(n.clone()),
            _ => None,
        })
        .collect();
    assert!(
        root_notices
            .iter()
            .any(|n| matches!(n, agent_core::Notice::CompactionSummaryReceived)),
        "该有一条 CompactionSummaryReceived：{root_notices:?}"
    );
    assert!(
        !root_notices
            .iter()
            .any(|n| matches!(n, agent_core::Notice::CompactionFailed)),
        "不该有 CompactionFailed：{root_notices:?}"
    );

    // 压缩子干完活该被回收（108 裁决：收割之后 despawn），不占着槽位。
    assert!(
        session.children_of(&root).is_empty(),
        "压缩子该在收割之后被 despawn，不该继续挂在树上"
    );
}
