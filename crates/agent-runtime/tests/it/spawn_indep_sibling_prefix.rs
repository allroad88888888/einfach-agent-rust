//! 独立测试覆盖点 8：红线 11 兄弟前缀。
//!
//! 两个子都不带显式 `tools`（缺省 = 父的工具子集），拿到的是同一份工具
//! 子集。断言两个子各自请求体里、**task 文本出现之前**的那一段前缀逐字节
//! 相同——这正是「固定模板不含 task」的实检：子 agent 的系统模板只依赖
//! `AgentLimits`，task 文本只进第一条 user 消息，于是 `[Tools][System]`
//! 前缀（不管 wire 上具体怎么排列字段）在 task 出现之前必须逐字节一致，
//! 前缀缓存才可能在兄弟之间命中（029 判断 11）。
//!
//! 用「task 文本出现的字节位置」而不是假设某个具体的 JSON 字段顺序：这样
//! 断言不依赖对 encode() 输出结构的额外假设，只依赖「前缀不含 task」这一
//! 条被测的性质本身。

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::spawn_indep_support::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_calls, temp_dir, wire_tool_name,
};

#[test]
fn two_siblings_with_the_default_tool_subset_share_a_byte_identical_prefix_before_their_task_text()
{
    let dir = temp_dir("sibling-prefix");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let task_a = "SIBA task text unique to child A";
    let task_b = "SIBB task text unique to child B";

    let server = RoutedServer::start(vec![
        Route {
            needle: "call_a",
            delay: Default::default(),
            status: 200,
            lines: sse_text("both siblings reported"),
        },
        Route {
            needle: "SIBA",
            delay: Default::default(),
            status: 200,
            lines: sse_text("sibling A done"),
        },
        Route {
            needle: "SIBB",
            delay: Default::default(),
            status: 200,
            lines: sse_text("sibling B done"),
        },
        Route {
            needle: "kickoff5",
            delay: Default::default(),
            status: 200,
            lines: sse_tool_calls(&[
                ("call_a", &spawn_wire, &format!(r#"{{"task":"{task_a}"}}"#)),
                ("call_b", &spawn_wire, &format!(r#"{{"task":"{task_b}"}}"#)),
            ]),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin().with_spawn(AgentLimits::default());
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = agent_runtime::block_on(run_turn(
        &mut session,
        &mut ctx,
        "kickoff5 spawn two siblings with the default tool subset",
    ));
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();
    let a_id = root.child(1);
    let b_id = root.child(2);
    // 两个子都省了 `tools`，缺省该都等于父当时的工具子集——先验一下它们
    // 拿到的确实是同一份（不只是长度碰巧相等）。
    let a_tools = session
        .tools_allowed_of(&a_id)
        .expect("a 该有 ToolsAllowed");
    let b_tools = session
        .tools_allowed_of(&b_id)
        .expect("b 该有 ToolsAllowed");
    assert_eq!(a_tools, b_tools, "两个子都省了 tools，缺省子集该完全一样");

    let body_a = &server
        .call("SIBA")
        .expect("child A must have been called")
        .body;
    let body_b = &server
        .call("SIBB")
        .expect("child B must have been called")
        .body;

    let idx_a = body_a
        .find(task_a)
        .expect("task 文本该逐字出现在子 A 的请求体里");
    let idx_b = body_b
        .find(task_b)
        .expect("task 文本该逐字出现在子 B 的请求体里");

    assert_eq!(
        idx_a, idx_b,
        "task 文本出现的字节位置该完全相同——前面的前缀长度必须一样长"
    );
    assert_eq!(
        &body_a[..idx_a],
        &body_b[..idx_b],
        "task 文本出现之前的前缀该逐字节相同：\nA[..{idx_a}]={:?}\nB[..{idx_b}]={:?}",
        &body_a[..idx_a],
        &body_b[..idx_b]
    );

    // 前缀确实非空——不是两条空字符串巧合相等蒙混过关。
    assert!(
        idx_a > 20,
        "前缀该有实质内容（工具表 + 固定模板），不是几乎贴着请求体开头就出现 task：idx_a={idx_a}"
    );
}
