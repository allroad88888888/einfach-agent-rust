//! 208 验收第 2 条（前半）：子 agent 调 `self`，`turns_used` 是**它自己的**，
//! 不是 root 的。
//!
//! # 怎么在不知道正文格式的前提下证明「不是 root 的」
//!
//! 让子 agent 在自己的一轮里连叫两次 `self`（中间隔一跳）。子 agent 在跑的这段
//! 时间里 root 正**阻塞等着**（前台 spawn 没有回来），root 自己的
//! `turns_used_of(root)` 在这整段窗口里纹丝不动。于是：
//!
//! - 如果实现正确（读 `turns_used_of(&子)`），子的两次调用之间跳数真的往前走
//!   了，两段正文该不相等；
//! - 如果实现读错成不带参数的 `turns_used()`（恒读 root 的槽位，见
//!   `agent-core` 对这四个 per-agent 读口的模块文档），子的两次调用看到的会是
//!   同一个冻结不动的 root 数字，两段正文会**逐字节相同**——这正是 208 交付要求
//!   注入验证的那个 bug 的形状，本文件就是它的看门狗。
//!
//! 顺带留一个可比对的事实：全程跑完之后，子的 `turns_used_of` 和 root 的
//! `turns_used_of` 该是两个不同的数——两本账真的是分开记的，不是巧合地凑巧
//! 相等。

use std::time::Duration;

use agent_core::{AgentId, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::self_indep_support::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_call, temp_dir, tool_result, wire_tool_name,
};

#[test]
fn a_child_agents_turns_used_moves_independently_of_the_frozen_parent_count() {
    let dir = temp_dir("self-child-turns-used");
    let self_wire = wire_tool_name(agent_runtime::SELF_TOOL);
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        // 子的第二次 self 调用之后，子收尾。
        Route {
            needle: "call_c2",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("child branch done"),
        },
        // 子的第一次 self 调用之后，子再叫一次 self。
        Route {
            needle: "call_c1",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_c2", &self_wire, "{}"),
        },
        // root 的 spawn 拿到子的结果之后，root 收尾——这一跳的请求体里带着子的
        // 回答文本 "child branch done"，用它当 needle 认领，不跟子自己的
        // call_c1/call_c2 冲突。
        Route {
            needle: "child branch done",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("root wrap done"),
        },
        // 子 agent 的第一条 user 消息（它的 task 文本）：一上来就连叫两次 self。
        Route {
            needle: "TASKCHILD",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_c1", &self_wire, "{}"),
        },
        // root 的第一跳：把整段任务 spawn 给一个前台子 agent（阻塞，等它跑完）。
        Route {
            needle: "kickoff-child-turns-used",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call(
                "call_spawn",
                &spawn_wire,
                r#"{"task":"TASKCHILD 连叫两次 self"}"#,
            ),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(agent_core::AgentLimits::default())
        .with_self();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(
        &mut session,
        &mut ctx,
        "kickoff-child-turns-used spawn 一个子去连叫两次 self",
    )
    .expect("这条流程不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();
    let child = root.child(1);

    let (first, first_error) = tool_result(&session, &child, "call_c1");
    assert!(!first_error, "纯读不该失败：{first}");
    let (second, second_error) = tool_result(&session, &child, "call_c2");
    assert!(!second_error, "纯读不该失败：{second}");

    assert_ne!(
        first, second,
        "子 agent 两次自读之间它自己的跳数已经往前走了，正文却逐字节相同——\
         像是拿了 root 那份冻结不动的 turns_used 而不是自己的"
    );

    // 两本账是分开记的：全程跑完之后，子和 root 的 turns_used 是两个不同的数
    // （子 3 跳收尾，root 2 跳收尾），不是恰好相等。
    let child_turns = session.turns_used_of(&child);
    let root_turns = session.turns_used_of(&root);
    assert_eq!(child_turns, 3, "子该恰好跑了 3 跳（两次 self + 收尾文本）");
    assert_eq!(root_turns, 2, "root 该恰好跑了 2 跳（spawn 那一跳 + 收尾文本）");
    assert_ne!(
        child_turns, root_turns,
        "两本账凑巧相等的话，上面「不相等」那条正文断言就测不出「读错成 root 的账」这类 bug"
    );
}
