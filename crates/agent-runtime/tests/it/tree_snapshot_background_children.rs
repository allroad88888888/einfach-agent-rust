//! 054 第 1 项的验收：**活树面板在后台子 agent 场景下已经是对的，零代码。**
//!
//! 面板（web `render/agent_tree.ts` / CLI `/agents`）是 `Session::agent_tree()`
//! 快照的**哑渲染**（OBSERVABILITY §「snapshot，不是 reconstruct」）——它整棵
//! 重画，不从事件流重建任何状态机。所以「后台子在面板上对不对」这个问题，等价于
//! 「`with_tree_events` 推出去的那串快照对不对」，而那串快照是 core 的纯派生读。
//! 本文件钉的就是这一等价关系的前半截：**推出去的快照真的把后台并发画对了**。
//!
//! 两条，各钉一个 M7 时不存在的现象：
//!
//! 1. 有一帧是 **root `Thinking` + 两个子同时 `Thinking`**。这一帧在阻塞 spawn 下
//!    **结构上不可能**：那时 root 卡在 `ToolsPending`（面板上是 `Working(spawn,
//!    spawn)`）直到子收敛。它是 M8 相对 M7 的新现象本身。
//! 2. 被轮末清算拆掉的孤儿**从树上消失**——最后一帧只剩 root。052 在 `reap` 之后
//!    补的那次 `maybe_emit_tree` 就是为这条服务的（不补的话面板会永远停在「有一个
//!    子 agent 在跑」的旧帧上，而它已经不存在了；048 真机逮到过同一类漏投影）。
//!
//! 被 collect 领走的子**留在树上并转 `Done`**（collect 不拆人）也在第一条里一并
//! 断言：最后一帧三个节点全 `Done`。

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use agent_core::{AgentActivity, AgentId, AgentLimits, AgentTree, Session, TurnStatus};
use agent_runtime::{run_turn, RunnerCtx, ToolTable};

use crate::spawn_bg_support::{
    build_ctx, sse_text, sse_tool_call, sse_tool_calls, temp_dir, wire_tool_name, Route,
    RoutedServer,
};

/// 两个后台子都跑这么久——比 root 的后续几跳（零延迟）慢得多，于是「root 已经
/// 在想下一步、两个子还在跑」这一刻真的存在。
const CHILD: Duration = Duration::from_millis(300);

/// 收一串 `RunnerCtx::with_tree_events` 的快照。跟 `tree_snapshot_emits_on_change.rs`
/// 同款装配——面板拿到的就是这一串，一帧不多一帧不少。
fn collect_trees(ctx: RunnerCtx) -> (RunnerCtx, Rc<RefCell<Vec<AgentTree>>>) {
    let trees = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&trees);
    let ctx = ctx.with_tree_events(Box::new(move |tree| sink.borrow_mut().push(tree)));
    (ctx, trees)
}

fn is_running(activity: &AgentActivity) -> bool {
    matches!(
        activity,
        AgentActivity::Thinking | AgentActivity::Working { .. }
    )
}

#[test]
fn two_background_children_are_on_the_tree_at_once_while_the_parent_thinks() {
    let dir = temp_dir("tree-bg-two");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);
    let collect_wire = wire_tool_name(agent_runtime::COLLECT_TOOL);

    let server = RoutedServer::start(vec![
        // 越靠后发生的 call_id 越靠前判（root 每一跳都带着此前全部 call_id）。
        Route {
            needle: "call_c2",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("两个都领回来了"),
        },
        Route {
            needle: "call_c1",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_c2", &collect_wire, r#"{"id":"root/a2"}"#),
        },
        Route {
            needle: "call_bg_a",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_c1", &collect_wire, r#"{"id":"root/a1"}"#),
        },
        Route {
            needle: "TASKBGA",
            delay: CHILD,
            status: 200,
            lines: sse_text("ANSWERBGA 子甲"),
        },
        Route {
            needle: "TASKBGB",
            delay: CHILD,
            status: 200,
            lines: sse_text("ANSWERBGB 子乙"),
        },
        Route {
            needle: "kickoff",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_calls(&[
                (
                    "call_bg_a",
                    &spawn_wire,
                    r#"{"task":"TASKBGA 后台干活甲","background":true}"#,
                ),
                (
                    "call_bg_b",
                    &spawn_wire,
                    r#"{"task":"TASKBGB 后台干活乙","background":true}"#,
                ),
            ]),
        },
    ]);

    let tools = ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_status()
        .with_collect();
    let (ctx, _events) = build_ctx(server.port, &dir, tools);
    let (mut ctx, trees) = collect_trees(ctx);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff 两个后台子，随后各领各的");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let trees = trees.borrow();
    assert!(!trees.is_empty(), "开了 with_tree_events 就该收到快照");

    // ① M8 的新现象：root 自己在想下一步（`Thinking`），而两个后台子**同时**在跑。
    //    阻塞 spawn 下这一帧结构上不可能——那时 root 一定停在 `ToolsPending`
    //    （面板上是 `Working(...)`）等子收敛。
    let both_running = trees
        .iter()
        .find(|t| {
            t.nodes.len() == 3
                && t.nodes[0].activity == AgentActivity::Thinking
                && is_running(&t.nodes[1].activity)
                && is_running(&t.nodes[2].activity)
        })
        .unwrap_or_else(|| panic!("该有一帧是 root 在想、两个后台子同时在跑：{trees:#?}"));
    assert_eq!(both_running.nodes[1].id, AgentId::root().child(1));
    assert_eq!(both_running.nodes[2].id, AgentId::root().child(2));
    assert_eq!(both_running.nodes[1].depth, 1);
    assert_eq!(both_running.nodes[2].depth, 1);
    assert_eq!(both_running.nodes[1].parent, Some(AgentId::root()));
    // 面板上区分得开谁是谁：两个同层子的 task 文本各是自己的那一句。
    assert_eq!(
        both_running.nodes[1].task.as_deref(),
        Some("TASKBGA 后台干活甲")
    );
    assert_eq!(
        both_running.nodes[2].task.as_deref(),
        Some("TASKBGB 后台干活乙")
    );

    // ② 被 collect 领走的子**留在树上并转 `Done`**（collect 只读结果，不拆人）。
    let last = trees.last().expect("至少一帧");
    assert_eq!(last.nodes.len(), 3, "领完的两个子该还在树上：{last:#?}");
    for node in &last.nodes {
        assert!(
            matches!(node.activity, AgentActivity::Done { .. }),
            "{last:#?}"
        );
    }

    // ③ 快照就是 `agent_tree()`（哑渲染的前提）——最后一帧跟此刻现读逐字节相同。
    assert_eq!(
        *last,
        session.agent_tree(),
        "推出去的最后一帧该等于此刻现读的树"
    );
}

#[test]
fn a_reaped_orphan_disappears_from_the_pushed_tree() {
    let dir = temp_dir("tree-bg-orphan");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        // 父的第二跳：立刻答完收尾，压根不管那个后台子。
        Route {
            needle: "call_bg",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("我自己答完了"),
        },
        Route {
            needle: "ORPHANTASK",
            delay: CHILD,
            status: 200,
            lines: sse_text("LATEANSWER"),
        },
        Route {
            needle: "kickoff",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call(
                "call_bg",
                &spawn_wire,
                r#"{"task":"ORPHANTASK 后台慢活","background":true}"#,
            ),
        },
    ]);

    let tools = ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_status()
        .with_collect();
    let (ctx, _events) = build_ctx(server.port, &dir, tools);
    let (mut ctx, trees) = collect_trees(ctx);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff 一个后台子然后不管它");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let trees = trees.borrow();
    let orphan = AgentId::root().child(1);

    // 它**确实上过树**——否则下面那条「不在了」会因为它从没出现过而白绿。
    assert!(
        trees.iter().any(|t| t.nodes.iter().any(|n| n.id == orphan)),
        "后台子该在树上出现过：{trees:#?}"
    );

    // 拆掉之后面板收到的**最后一帧**只剩 root：`orphan::reap` 之后补的那次
    // `maybe_emit_tree` 就是这一帧（那条路不经过 `session.step`，A 段的变化检测
    // 看不见它）。少了它，面板会永远停在「有一个子 agent 在跑」的旧帧上。
    let last = trees.last().expect("至少一帧");
    assert_eq!(
        last.nodes.len(),
        1,
        "孤儿被拆掉后树上只该剩 root：{last:#?}"
    );
    assert_eq!(last.nodes[0].id, AgentId::root());
    assert!(
        matches!(last.nodes[0].activity, AgentActivity::Done { .. }),
        "{last:#?}"
    );
    assert_eq!(
        *last,
        session.agent_tree(),
        "推出去的最后一帧该等于此刻现读的树"
    );
}
