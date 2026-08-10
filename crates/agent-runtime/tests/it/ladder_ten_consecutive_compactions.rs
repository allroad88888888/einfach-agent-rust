//! 108 验收 ⚠️：连续 10 次自动压缩全部成功——不因摘要子 agent 占满槽位而从
//! 第 9 次开始失败。
//!
//! `AgentLimits::default().max_children == 8`（[`agent_core::DEFAULT_MAX_CHILDREN`]）。
//! 108 的裁决是「收割之后 despawn 摘要子 agent」——不这么做，第 9 次自动压缩
//! 触发 spawn 时活着的压缩子已经堆到 8 个，`SpawnRefused::TooManyChildren` 当场
//! 拒绝，回执变成 `CompactFailed`，这条测试就会在第 9 轮红。
//!
//! 场景刻意简化到**没有任何工具调用**：整段历史只有纯文本轮次，`tool_results_to_clear`
//! 从第一轮起就永远是空的（没有工具结果可清），于是只要过了保护区门槛
//! （前 3 轮）、usage 一直冲线，**每一轮末都直接轮到第 3 档**——不需要先摆一轮
//! 工具调用、等它滑出保护区，13 轮纯文本正好给出 10 次连续压缩
//! （第 4～13 轮各触发一次）。

use agent_core::{AgentId, DEFAULT_MAX_CHILDREN, Session, TurnStatus};
use agent_runtime::{RunnerEvent, ToolTable, run_turn};

use crate::ladder_support::{SUMMARY_PROMPT_NEEDLE, build_ctx, text_response};
use crate::support;
use crate::support::routed::{Route, RoutedServer};

const WINDOW: u32 = 1000;
const HIGH: u32 = 900; // 90%，冲过 85%，且没有工具结果可清，永远直通第 3 档
const ROUNDS: usize = 13; // 前 3 轮垫保护区，后 10 轮各触发一次压缩

fn routes() -> Vec<Route> {
    let mut routes = vec![Route::sse(
        SUMMARY_PROMPT_NEEDLE,
        text_response("SUMMARY10 十连压缩用的摘要正文", 5),
    )];
    // 从最后一轮往第一轮声明——历史累积，晚声明的窄条件必须排在前面才不会被
    // 早声明的宽条件截胡（`Route` 文档：越具体越要排前面）。
    for i in (1..=ROUNDS).rev() {
        let needle: &'static str = Box::leak(format!("R{i}MARK").into_boxed_str());
        routes.push(Route::sse(needle, text_response("继续", HIGH)));
    }
    routes
}

#[test]
fn ten_consecutive_automatic_compactions_all_succeed() {
    let dir = support::temp_dir("ladder-ten-consecutive");
    let server = RoutedServer::start(routes());
    let (mut ctx, events) = build_ctx(server.port, &dir, ToolTable::builtin(), Some(WINDOW));
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();

    let mut boundaries = Vec::new();
    for i in 1..=ROUNDS {
        // 第 2 轮起，每一轮开跑之前都要显式 `begin_turn`（026 判断 13）——漏了
        // 不报错，新的 `UserInput` 撞上上一轮的 `Done` 会被判成
        // `Notice::ProtocolViolation`，这一轮根本没发生过任何请求。
        if i > 1 {
            session.begin_turn();
        }
        let text = format!("R{i}MARK 继续聊");
        let status = run_turn(&mut session, &mut ctx, &text)
            .unwrap_or_else(|e| panic!("第 {i} 轮不该是 source failure：{e:?}"));
        assert_eq!(status, TurnStatus::Done { truncated: false }, "第 {i} 轮");
        boundaries.push(session.send_plan_of(&root).boundary());

        // 每一轮之后都检查：活着的压缩子数量不该逼近 max_children——这是本条
        // 验收的直接度量，裁决不落地的话这个数字会一路涨到 8 然后拒绝 spawn。
        assert!(
            session.children_of(&root).len() < DEFAULT_MAX_CHILDREN,
            "第 {i} 轮之后活着的子 agent 数 {} 不该逼近上限 {}——收割之后该 despawn",
            session.children_of(&root).len(),
            DEFAULT_MAX_CHILDREN
        );
    }

    // 前 3 轮历史太短，边界该纹丝不动；第 4～13 轮（10 轮）各触发一次压缩，
    // 边界应该单调递增。
    assert_eq!(boundaries[0], 0, "第 1 轮：历史太短，不该触发");
    assert_eq!(boundaries[1], 0, "第 2 轮：历史太短，不该触发");
    assert_eq!(boundaries[2], 0, "第 3 轮：历史太短，不该触发");
    for i in 3..ROUNDS {
        assert!(
            boundaries[i] > boundaries[i - 1],
            "第 {} 轮的边界（{}）该严格大于上一轮（{}）——每一轮都该触发一次压缩",
            i + 1,
            boundaries[i],
            boundaries[i - 1]
        );
    }

    // 收工之后压缩子该全部被回收，不留任何一个活着。
    assert!(
        session.children_of(&root).is_empty(),
        "10 次压缩收工之后不该有任何压缩子还挂在树上：{:?}",
        session.children_of(&root)
    );

    // 十次都是 CompactDone，一次 CompactFailed 都不该有——裁决要保证的是「不因为
    // 槽位满而失败」，不是「失败了但恰好不影响别的断言」。
    let events = events.borrow();
    let root_notices: Vec<_> = events
        .iter()
        .filter(|e| e.agent == root)
        .filter_map(|e| match &e.event {
            RunnerEvent::Notice(n) => Some(n.clone()),
            _ => None,
        })
        .collect();
    let received = root_notices
        .iter()
        .filter(|n| matches!(n, agent_core::Notice::CompactionSummaryReceived))
        .count();
    let failed = root_notices
        .iter()
        .filter(|n| matches!(n, agent_core::Notice::CompactionFailed))
        .count();
    assert_eq!(received, 10, "该恰好 10 次成功回执：{root_notices:?}");
    assert_eq!(failed, 0, "一次失败都不该有：{root_notices:?}");
}
