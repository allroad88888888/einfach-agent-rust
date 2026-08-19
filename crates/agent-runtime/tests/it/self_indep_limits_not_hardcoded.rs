//! 208 验收第 3 条：改过 `AgentLimits`（`--max-agent-depth` / `--max-children`
//! 这两个进程级参数，在 core 侧落点就是 `Session::set_agent_limits`，决策 32）
//! 之后，`self` 回的是**配的那组数**，不是 `DEFAULT_MAX_AGENT_DEPTH`/
//! `DEFAULT_MAX_CHILDREN`（3/8）两个字面量。
//!
//! # 怎么测「不是硬编码的 3/8」而不知道正文格式
//!
//! 跟 `self_indep_turns_used_changes.rs` 同一个手法：不解析字段，比较**两次调用
//! 整段正文的字节**。root 在默认档下调一次 `self`（第一轮的第 1 跳），改
//! `agent_limits` 之后 root 再调一次（第二轮的第 1 跳）。两次调用都发生在各自
//! 新轮次的第 1 跳，因此 `turns_used`/`retries_used`/depth/工具表/是否压缩过
//! 全都相同——两次之间唯一变了的状态就是 `AgentLimits`。写死 3/8 的实现会让
//! 两段正文逐字节相同，这条测试就是那个反例的看门狗。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::self_indep_support::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_call, temp_dir, tool_result, wire_tool_name,
};

#[test]
fn self_reflects_a_reconfigured_agent_limits_not_the_default_3_and_8() {
    let dir = temp_dir("self-limits-not-hardcoded");
    let self_wire = wire_tool_name(agent_runtime::SELF_TOOL);

    // needle 必须按「最后才会出现在请求体里的」排最前：请求体是累积的整段历史，
    // 第二轮的每一跳请求体里都还带着第一轮的 "call_before"/"kickoff-limits-before"
    // ——不按这个顺序排会被更早、更粗的 needle 抢先认领（本文件第一版就栽在这里：
    // 第二轮的第 1 跳被 "call_before" 那条路由错误地认领成了第一轮的第 2 跳）。
    let server = RoutedServer::start(vec![
        Route {
            needle: "call_after",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("turn two done"),
        },
        Route {
            needle: "kickoff-limits-after",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_after", &self_wire, "{}"),
        },
        Route {
            needle: "call_before",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("turn one done"),
        },
        Route {
            needle: "kickoff-limits-before",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_before", &self_wire, "{}"),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin().with_self();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();

    // 第一轮：默认档（深度 ≤3、子数 ≤8）。
    assert_eq!(session.agent_limits(), AgentLimits::default());
    let status = run_turn(&mut session, &mut ctx, "kickoff-limits-before 默认档下问一次自己")
        .expect("第一轮不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });
    let (before, before_error) = tool_result(&session, &root, "call_before");
    assert!(!before_error, "纯读不该失败：{before}");

    // 改配置：跟默认值都不一样，才能把「退回默认」和「读到新值」区分开。
    let reconfigured = AgentLimits {
        max_depth: 5,
        max_children: 2,
        ..AgentLimits::default()
    };
    assert_ne!(reconfigured, AgentLimits::default());
    session.set_agent_limits(reconfigured);
    session.begin_turn();

    let status = run_turn(&mut session, &mut ctx, "kickoff-limits-after 改档之后再问一次自己")
        .expect("第二轮不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });
    let (after, after_error) = tool_result(&session, &root, "call_after");
    assert!(!after_error, "纯读不该失败：{after}");

    assert_ne!(
        before, after,
        "改了 agent_limits 之后正文却逐字节相同——像是回了一份写死的 3/8，\
         而不是每次读一遍 session.agent_limits()"
    );

    // 前提校验：两轮的形状完全一样（各自都是「第 1 跳问自己、第 2 跳收尾」），
    // 跑完之后 turns_used 落在同一个数上——唯一真的变了的状态确实只有
    // agent_limits，上面那条「不相等」断言测的才是它，不是别的巧合。
    assert_eq!(
        session.turns_used_of(&root),
        2,
        "第二轮该跟第一轮同形状：第 1 跳问自己、第 2 跳收尾，跑完落在 2"
    );
}
