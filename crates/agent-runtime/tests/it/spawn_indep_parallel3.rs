//! 独立测试覆盖点 1：并行重叠的硬证据（三个子，不止两个）。
//!
//! root 一跳吐三个并行的 `srv:agent/spawn` 调用，脚本让最慢的那个（A）压尾
//! （B/C 明显更快）。断言：
//! - 服务器侧记录的三个子请求的到达时间窗有公共交叠（不是两两偶然重叠）；
//! - 总耗时 < 三者延迟之和的 60%（时间断言留了宽容度，见下面的数字选择）；
//! - 父确实等到最慢的 A 完成才发第二跳（B/C 先完工不代表父提前继续）；
//! - 消息树完整（3 个 ToolUse + 3 个 ToolResult + 最终文本）；
//! - 整棵子树只在一个 turn 里——用 `undo_turn` 一次性退光作为「turn_id 全树
//!   一致」的操作证据（029 没有暴露原始日志迭代口，这是公开 API 能给到的
//!   最强证据：如果子 agent 的 entry 带着不同的 turn_id，`undo_turn` 不会
//!   一次性吞掉整棵树）。

use std::time::{Duration, Instant};

use agent_core::{AgentId, ContentBlock, Session, TurnStatus, UndoReport};
use agent_runtime::run_turn;

use crate::spawn_indep_support::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_calls, temp_dir, wire_tool_name,
};

const SLOW: Duration = Duration::from_millis(350);
const FAST: Duration = Duration::from_millis(250);

#[test]
fn three_children_overlap_and_the_parent_waits_for_the_slowest() {
    let dir = temp_dir("parallel3");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let root_call_a = r#"{"task":"TASKA parallel work alpha"}"#;
    let root_call_b = r#"{"task":"TASKB parallel work beta"}"#;
    let root_call_c = r#"{"task":"TASKC parallel work gamma"}"#;

    let server = RoutedServer::start(vec![
        // 最具体的先判：hop2 的请求体里带着子的 call_id（tool_call_id 回填）
        // 和三个 task 文本，hop1 都不带；顺序反了 hop1 会被下面更宽的路由抢先
        // 命中，hop2 也会被 "TASKA" 那条路由抢先命中（它一样出现在 hop2 里）。
        Route {
            needle: "call_a",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("all three are back, summary done"),
        },
        Route {
            needle: "TASKA",
            delay: SLOW,
            status: 200,
            lines: sse_text("answer A"),
        },
        Route {
            needle: "TASKB",
            delay: FAST,
            status: 200,
            lines: sse_text("answer B"),
        },
        Route {
            needle: "TASKC",
            delay: FAST,
            status: 200,
            lines: sse_text("answer C"),
        },
        Route {
            needle: "kickoff",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_calls(&[
                ("call_a", &spawn_wire, root_call_a),
                ("call_b", &spawn_wire, root_call_b),
                ("call_c", &spawn_wire, root_call_c),
            ]),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin().with_spawn(agent_core::AgentLimits::default());
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let start = Instant::now();
    let status = agent_runtime::block_on(run_turn(
        &mut session,
        &mut ctx,
        "kickoff please split into three parallel workers",
    ));
    let elapsed = start.elapsed();

    assert_eq!(status, TurnStatus::Done { truncated: false });

    // --- 并行的硬证据 ---
    let a = server.call("TASKA").expect("child A must have been called");
    let b = server.call("TASKB").expect("child B must have been called");
    let c = server.call("TASKC").expect("child C must have been called");

    let common_start = [a.start, b.start, c.start].into_iter().max().unwrap();
    let common_end = [a.end, b.end, c.end].into_iter().min().unwrap();
    assert!(
        common_start < common_end,
        "三个子请求该有一段公共重叠窗口：a=[{:?},{:?}] b=[{:?},{:?}] c=[{:?},{:?}]",
        a.start,
        a.end,
        b.start,
        b.end,
        c.start,
        c.end
    );

    let serial_sum = SLOW + FAST + FAST;
    assert!(
        elapsed < serial_sum.mul_f64(0.6),
        "总耗时该明显小于三者延迟之和：elapsed={elapsed:?}, serial_sum*0.6={:?}",
        serial_sum.mul_f64(0.6)
    );

    assert!(b.end < a.end, "B 该比压尾的 A 先完工");
    assert!(c.end < a.end, "C 该比压尾的 A 先完工");

    let hop2_call = server
        .call("call_a")
        .expect("root's second hop must have been called");
    assert!(
        hop2_call.start > a.end,
        "父该等到最慢的 A 完工才发第二跳: hop2.start={:?} a.end={:?}",
        hop2_call.start,
        a.end
    );

    // --- 消息树完整 ---
    let messages = session.messages();
    let tool_uses: Vec<_> = messages
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolUse { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(tool_uses.len(), 3, "该有三次 spawn 调用: {messages:#?}");
    assert!(tool_uses.iter().all(|n| &**n == agent_runtime::SPAWN_TOOL));

    let tool_results: Vec<_> = messages
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
        tool_results.len(),
        3,
        "三个子都该回一条 tool_result: {messages:#?}"
    );
    assert!(
        tool_results.iter().all(|(_, is_error)| !is_error),
        "三个子都成功，不该有 is_error"
    );
    for expect in ["answer A", "answer B", "answer C"] {
        assert!(
            tool_results
                .iter()
                .any(|(content, _)| content.contains(expect)),
            "该找到子的回答 {expect:?}: {tool_results:#?}"
        );
    }

    let final_text: Vec<_> = messages
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert!(
        final_text.iter().any(|t| t.contains("summary done")),
        "父的收尾文本该在场: {final_text:#?}"
    );

    assert_eq!(
        session.live_agents().len(),
        4,
        "root + 三个子都该在活名单上"
    );

    // --- turn_id 全树一致：唯一可从公开 API 拿到的强证据是 undo_turn 一次
    // 性吞光整棵树——如果子的 entry 带着别的 turn_id，游标会在 turn 边界停下，
    // 不会一路清空。
    let history_len_before = session.history_len();
    let report = session.undo_turn();
    match report {
        UndoReport::Applied { entries, turn_id } => {
            assert_eq!(
                turn_id, 1,
                "只跑过一轮，没调过 begin_turn，turn_id 该恒为 1"
            );
            assert_eq!(
                entries, history_len_before,
                "一次 undo_turn 该吞掉整棵树在这一轮里留下的全部 entry: {report:?}"
            );
        }
        other => panic!("期望 Applied，拿到 {other:?}"),
    }
    assert_eq!(
        session.live_agents(),
        vec![AgentId::root()],
        "undo 之后三个子都该从活名单上消失"
    );
    assert!(
        session.messages().is_empty(),
        "undo 之后 root 的消息也该清空"
    );
}
