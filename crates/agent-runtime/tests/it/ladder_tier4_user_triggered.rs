//! 108 验收：第 4 档（清窗口）是用户动作，不受阈值管。
//!
//! - 阈值远未到达时，用户直接调 `Session::advance_boundary(agent, len, None)`：
//!   正常生效——证明它不受阈值管，不需要先把 usage 顶上去才能用。
//! - 阈值到达但用户没动作时：第 4 档不触发——自动阶梯里只有第 2、3 档
//!   （108「不参与自动阶梯的三档」），第 4 档必须由显式调用触发，不能被
//!   `next_action` 自己叫出来。
//!
//! 第二条的判据：自动阶梯每次真的动了边界，`SendPlan::summary()` 都该是
//! `Some(..)`（走的是第 3 档，边界推进的同时一定带一份摘要引用——107
//! `apply_summary` 的形状）。「边界动了但没有摘要」正是第 4 档「清窗口」独有
//! 的指纹（104：`summary` 传 `None`）；只要自动运行全程从没出现这个指纹，
//! 就证明第 4 档没有被自动阶梯误触发过。

use agent_core::{AgentId, Session, TurnStatus};
use agent_runtime::{ToolTable, run_turn};

use crate::ladder_support::{SUMMARY_PROMPT_NEEDLE, build_ctx, text_response};
use crate::support;
use crate::support::routed::{Route, RoutedServer};

const WINDOW: u32 = 1000;
const LOW: u32 = 100; // 10%，远低于 85%
const HIGH: u32 = 900; // 90%，冲过 85%

/// 阈值远未到达时，用户直接调用第 4 档：正常生效，不受阈值管。
#[test]
fn tier4_fires_on_direct_user_call_even_when_far_below_the_trigger_line() {
    let dir = support::temp_dir("ladder-tier4-user-triggered");
    let routes = vec![
        Route::sse("R1", text_response("继续", LOW)),
        Route::sse("R2", text_response("继续", LOW)),
    ];
    let server = RoutedServer::start(routes);
    let (mut ctx, _events) = build_ctx(server.port, &dir, ToolTable::builtin(), Some(WINDOW));
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();

    assert_eq!(
        run_turn(&mut session, &mut ctx, "R1 你好").expect("不该是 source failure"),
        TurnStatus::Done { truncated: false }
    );
    assert_eq!(
        session.send_plan_of(&root).boundary(),
        0,
        "前提：usage 远低于阈值，自动阶梯不该已经动过边界"
    );

    let history_len = session.messages_of(&root).len();
    assert!(history_len > 0);
    session.begin_turn();
    session
        .advance_boundary(&root, history_len, None)
        .expect("用户直接触发第 4 档，阈值再低也该正常生效");

    let plan = session.send_plan_of(&root);
    assert_eq!(plan.boundary(), history_len, "边界该推到用户指定的位置");
    assert_eq!(plan.summary(), None, "清窗口不留摘要，这是第 4 档的字面定义");

    // 第 4 档生效之后会话照常往下跑。
    assert_eq!(
        run_turn(&mut session, &mut ctx, "R2 继续").expect("不该是 source failure"),
        TurnStatus::Done { truncated: false }
    );
}

/// 阈值到达但用户没有主动调用第 4 档：自动阶梯里只有第 2、3 档，第 4 档不该
/// 被自动叫出来——用「边界动了但没有摘要」这个第 4 档独有的指纹来判定它
/// 从未在自动运行期间出现过。
#[test]
fn tier4_never_fires_automatically_even_once_the_trigger_line_is_reached() {
    let dir = support::temp_dir("ladder-tier4-not-auto");
    let routes = vec![
        Route::sse(SUMMARY_PROMPT_NEEDLE, text_response("SUMMARYG 自动摘要", 5)),
        Route::sse("R6", text_response("继续", HIGH)),
        Route::sse("R5", text_response("继续", HIGH)),
        Route::sse("R4", text_response("继续", HIGH)),
        Route::sse("R3", text_response("继续", HIGH)),
        Route::sse("R2", text_response("继续", HIGH)),
        Route::sse("R1", text_response("继续", HIGH)),
    ];
    let server = RoutedServer::start(routes);
    let (mut ctx, _events) = build_ctx(server.port, &dir, ToolTable::builtin(), Some(WINDOW));
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();

    for i in 1..=6 {
        // 第 2 轮起，每一轮开跑之前都要显式 `begin_turn`（026 判断 13）——漏了
        // 不报错，新的 `UserInput` 撞上上一轮的 `Done` 会被判成
        // `Notice::ProtocolViolation`，这一轮根本没发生过任何请求，下面「边界
        // 动了却没有摘要」的指纹检查会因为边界压根没动过而永远不触发，变成一
        // 条测不出任何东西的空判断。
        if i > 1 {
            session.begin_turn();
        }
        let text = format!("R{i} 继续聊");
        let status = run_turn(&mut session, &mut ctx, &text)
            .unwrap_or_else(|e| panic!("第 {i} 轮不该是 source failure：{e:?}"));
        assert_eq!(status, TurnStatus::Done { truncated: false }, "第 {i} 轮");

        let plan = session.send_plan_of(&root);
        if plan.boundary() > 0 {
            assert!(
                plan.summary().is_some(),
                "第 {i} 轮之后边界动了却没有摘要引用——这是第 4 档（清窗口）\
                 独有的指纹，出现了就说明自动阶梯误触发了第 4 档：{plan:?}"
            );
        }
    }

    // 正向证据：这条测试不是「自动阶梯全程没干过任何事所以指纹检查从没被
    // 触发」——6 轮里 usage 一直冲线，边界该真的被第 3 档动过（`ladder_tier3_*`
    // 已经验过这个方向），上面的指纹检查因此不是空判断。
    assert!(
        session.send_plan_of(&root).boundary() > 0,
        "6 轮里 usage 一直冲线，边界该真的被自动阶梯动过——否则上面\
         「边界动了却没有摘要」这条指纹检查从没被触发过，测不出第 4 档有没有\
         被误触发"
    );
}
