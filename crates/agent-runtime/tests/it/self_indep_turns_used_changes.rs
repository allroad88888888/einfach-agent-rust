//! 208 验收第 1 条：跑满 `max_turns` 之前调一次 `self`、之后再调一次，
//! `turns_used` **确实变了**（不是回一份写死的默认值）。
//!
//! # 怎么测「不是写死的」而不知道正文长什么样
//!
//! `srv:agent/self` 的渲染格式是 `self_render.rs` 的实现细节——这份独立测试按
//! 硬规矩看不到那个文件。所以这里不断言某个字段等于某个数字，改断言**两次调用
//! 整段正文的字节不相等**：在同一个 agent、同一张工具表、零重试、没有压缩、
//! 没有新 spawn 的前提下，两次调用之间唯一合法会变的状态就是 `turns_used`——
//! 写死答案的实现会让两次调用逐字节相同，这条测试就是那个反例的看门狗。
//!
//! `max_turns` 压到 3，让「快用完了」这件事在一轮 3 跳里就能自然发生，对应
//! 验收原文「跑满 max_turns 之前调一次、之后再调一次」的字面场景。

use std::time::Duration;

use agent_core::{AgentId, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::self_indep_support::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_call, temp_dir, tool_result, wire_tool_name,
};

#[test]
fn turns_used_moves_between_an_early_call_and_a_later_call_in_the_same_turn() {
    let dir = temp_dir("self-turns-used");
    let self_wire = wire_tool_name(agent_runtime::SELF_TOOL);

    // needle 越具体越靠前：hop3 的请求体里同时有 "call_2" 和 "call_1"，反过来写
    // 会被 "call_1" 那条路由抢先命中。
    let server = RoutedServer::start(vec![
        Route {
            needle: "call_2",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("all turns spent"),
        },
        Route {
            needle: "call_1",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_2", &self_wire, "{}"),
        },
        Route {
            needle: "kickoff-turns-used",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_1", &self_wire, "{}"),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin().with_self();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());
    session.set_max_turns(3);

    let status = run_turn(
        &mut session,
        &mut ctx,
        "kickoff-turns-used 连叫两次 self，中间隔一跳",
    )
    .expect("这条流程不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();

    let (early, early_error) = tool_result(&session, &root, "call_1");
    assert!(!early_error, "纯读不该失败：{early}");
    let (late, late_error) = tool_result(&session, &root, "call_2");
    assert!(!late_error, "纯读不该失败：{late}");

    assert_ne!(
        early, late,
        "两次调用之间跳数已经往前走了，正文却逐字节相同——像是回了一份写死的默认值，\
         而不是每次读一遍当场的 turns_used"
    );

    // 前提校验：这一轮确实发生了 3 跳（call_1 那一跳 + call_2 那一跳 + 收尾文本
    // 那一跳），对不上就说明上面那条「不相等」断言没有真的驱动过 turns_used。
    assert_eq!(
        session.turns_used_of(&root),
        3,
        "这一轮该恰好用满 3 跳（等于 set_max_turns 压的那个上限），\
         对不上就说明测试没有真的把 turns_used 推动到位"
    );
}
