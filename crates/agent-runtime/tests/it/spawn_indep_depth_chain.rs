//! 独立测试覆盖点 2：深度链。
//!
//! root → L1 → L2 → L3 逐级 spawn（L3 落在 `depth() == 3`，`AgentLimits::
//! default()` 的 `max_depth == 3`，正好合法）。L3 自己再尝试 spawn 一个
//! depth 4 的孩子——超限，`Session::spawn_child` 在校验闸就拒绝，压根不会
//! 发出任何 provider 请求；L3 收到 `is_error` 的 tool_result 之后照常收尾，
//! 结果沿链一路汇总回 root。
//!
//! 断言：全树完成（`Done`）；depth 4 从未真的长出来（活名单只有四个
//! agent：root/L1/L2/L3）；L3（那个尝试者，也就是「孙的父」）收到的
//! tool_result 是 `is_error`；`turn_id` 全树一致（用跟覆盖点 1 同一个
//! `undo_turn` 单次吞光作证据——029 没有暴露原始日志迭代口）。

mod spawn_indep_support;

use agent_core::{AgentId, AgentLimits, ContentBlock, Session, TurnStatus, UndoReport};
use agent_runtime::run_turn;

use spawn_indep_support::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_call, temp_dir, wire_tool_name,
};

#[test]
fn a_depth_three_chain_completes_and_the_depth_four_attempt_is_refused() {
    let dir = temp_dir("depth-chain");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    // 每一跳都靠自己那次 spawn 的 call_id 去认第二跳的请求（第二跳的请求体
    // 里会带着自己第一跳里说过的 task 文本，光靠 task 文本区分不了两跳）。
    let server = RoutedServer::start(vec![
        Route {
            needle: "call_root",
            delay: Default::default(),
            status: 200,
            lines: sse_text("root done, chain complete"),
        },
        Route {
            needle: "call_l1",
            delay: Default::default(),
            status: 200,
            lines: sse_text("L1 done"),
        },
        Route {
            needle: "call_l2",
            delay: Default::default(),
            status: 200,
            lines: sse_text("L2 done"),
        },
        // L3 的第二跳：收到 depth 4 被拒的 is_error 之后，自己照常收尾。
        Route {
            needle: "call_l3_attempt",
            delay: Default::default(),
            status: 200,
            lines: sse_text("L3 done despite the refusal"),
        },
        Route {
            needle: "L1MARK",
            delay: Default::default(),
            status: 200,
            lines: sse_tool_call(
                "call_l1",
                &spawn_wire,
                r#"{"task":"L2MARK go two levels down"}"#,
            ),
        },
        Route {
            needle: "L2MARK",
            delay: Default::default(),
            status: 200,
            lines: sse_tool_call(
                "call_l2",
                &spawn_wire,
                r#"{"task":"L3MARK go three levels down"}"#,
            ),
        },
        // L3 合法（depth 3），它自己尝试再 spawn 一个 depth 4 的孩子——这个
        // 名字（L4MARK）不会被任何 Route 认领：闸挡在 `spawn_child` 里，压根
        // 不会有 HTTP 请求发出来。
        Route {
            needle: "L3MARK",
            delay: Default::default(),
            status: 200,
            lines: sse_tool_call(
                "call_l3_attempt",
                &spawn_wire,
                r#"{"task":"L4MARK should never be reached"}"#,
            ),
        },
        Route {
            needle: "startchain",
            delay: Default::default(),
            status: 200,
            lines: sse_tool_call(
                "call_root",
                &spawn_wire,
                r#"{"task":"L1MARK go one level down"}"#,
            ),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin().with_spawn(AgentLimits::default());
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "startchain please go deep");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    // --- 树的形状：四个 agent，没有第五个 ---
    let mut live = session.live_agents();
    live.sort();
    let root = AgentId::root();
    let l1 = root.child(1);
    let l2 = l1.child(1);
    let l3 = l2.child(1);
    let mut expected = vec![root.clone(), l1.clone(), l2.clone(), l3.clone()];
    expected.sort();
    assert_eq!(
        live, expected,
        "该恰好四个 agent：root/L1/L2/L3，没有 depth 4 的第五个"
    );
    assert_eq!(
        l3.depth(),
        3,
        "L3 该落在 depth 3——AgentLimits::default() 的上限"
    );

    // --- L3（尝试者，也是被拒的 depth-4 孩子的父）收到 is_error ---
    let l3_messages = session.messages_of(&l3);
    let l3_tool_results: Vec<_> = l3_messages
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => Some((content.clone(), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(
        l3_tool_results.len(),
        1,
        "L3 只发起过一次（被拒的）spawn: {l3_messages:#?}"
    );
    assert!(
        l3_tool_results[0].1,
        "depth 4 超限该是 is_error: {l3_tool_results:#?}"
    );

    // L3 自己照常收尾（003 哲学：工具失败不中止 loop）。
    let l3_final_text: Vec<_> = l3_messages
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert!(
        l3_final_text
            .iter()
            .any(|t| t.contains("despite the refusal")),
        "L3 该在拒绝之后照常给出结论: {l3_final_text:#?}"
    );

    // --- 全树汇总回 root ---
    let root_final_text: Vec<_> = session
        .messages()
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert!(
        root_final_text.iter().any(|t| t.contains("chain complete")),
        "root 该收到链条汇总: {root_final_text:#?}"
    );

    // --- turn_id 全树一致：同覆盖点 1 的证据手法。 ---
    assert_eq!(
        session.turn_id(),
        1,
        "全程没调过 begin_turn，turn_id 该恒为 1"
    );
    let history_len_before = session.history_len();
    match session.undo_turn() {
        UndoReport::Applied { entries, turn_id } => {
            assert_eq!(turn_id, 1);
            assert_eq!(
                entries, history_len_before,
                "四层子树该在同一个 turn 里被一次性吞光"
            );
        }
        other => panic!("期望 Applied，拿到 {other:?}"),
    }
    assert_eq!(session.live_agents(), vec![root], "undo 之后只剩 root");
}
