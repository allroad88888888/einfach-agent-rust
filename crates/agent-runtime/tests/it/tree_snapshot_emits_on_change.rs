//! 048 独立测试点：`RunnerCtx::with_tree_events` 的 emit-on-change 断言。
//!
//! 两条：
//! 1. spawn 一个子 agent 之后，回调该收到一棵**带着那个子 agent**的树（`AgentTree`
//!    是 `Session::agent_tree()` 的投影，不是我们自己拼的——直接断言收到的
//!    `AgentNode` 的 `parent`/`depth`/`task`/`activity`）。
//! 2. 树没变的 step **不该**重复推同一棵树——用 `timeout.rs` 同款的「provider
//!    挂起 + 重试」脚本：重试那一步（`Event::Timeout` 且预算没耗尽）状态原地
//!    留在 `Thinking`，`agent_tree()` 跟上一次逐字节相同，change 检测必须挡住它。

use crate::support;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use agent_core::{
    AgentActivity, AgentId, AgentLimits, AgentTree, ErrorClass, Failure, Session, TurnStatus,
};
use agent_runtime::{ToolTable, run_turn};

use crate::support::routed::{Route, RoutedServer};

const USAGE_STOP: &str = r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":10,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}"#;

fn text_reply(needle: &'static str, content: &str) -> Route {
    Route::sse(
        needle,
        vec![
            format!(
                r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":"{content}"}},"finish_reason":null}}]}}"#
            ),
            USAGE_STOP.to_string(),
            "data: [DONE]".to_string(),
        ],
    )
}

/// 收集一条 `RunnerCtx::with_tree_events` 的树快照序列，供两条测试共用装配。
fn collect_trees(
    ctx: agent_runtime::RunnerCtx,
) -> (agent_runtime::RunnerCtx, Rc<RefCell<Vec<AgentTree>>>) {
    let trees = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&trees);
    let ctx = ctx.with_tree_events(Box::new(move |tree| sink.borrow_mut().push(tree)));
    (ctx, trees)
}

#[test]
fn spawning_a_child_emits_a_tree_snapshot_that_includes_it() {
    let dir = support::temp_dir("tree-snapshot-spawn");
    let server = RoutedServer::start(vec![
        // 最具体先判：root 第二跳请求体里带着子的回答文本。
        text_reply("答案X", "最终结论：晴天。"),
        // 子 agent 首跳请求体里带着它自己的 task 文本。
        text_reply("任务X", "答案X：今天晴天。"),
        // 兜底：root 首跳（一次 spawn 调用）。
        Route::sse(
            "",
            vec![
                r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"srv_3Aagent_2Fspawn","arguments":"{\"task\": \"任务X：查天气\"}"}}]}}]}"#,
                r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":100}}"#,
                "data: [DONE]",
            ],
        ),
    ]);

    let tools = ToolTable::builtin().with_spawn(AgentLimits::default());
    let (ctx, _events) = support::build_ctx_agent_aware(server.port, &dir, tools);
    let (mut ctx, trees) = collect_trees(ctx);
    let mut session = Session::new(AgentId::root());

    let status =
        agent_runtime::block_on(run_turn(&mut session, &mut ctx, "帮我spawn个子agent查天气"));
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let trees = trees.borrow();
    assert!(
        !trees.is_empty(),
        "开了 with_tree_events 就该至少收到一棵树"
    );

    // 该有一帧恰好在子 agent 刚生出来、还没落终态那一刻：root 在等（ToolsPending
    // 的呈现层投影是 `Working`），子已经在树上、正在 `Thinking`。
    let child = AgentId::root().child(1);
    let with_live_child = trees
        .iter()
        .find(|t| t.nodes.len() == 2 && t.nodes[1].activity == AgentActivity::Thinking)
        .unwrap_or_else(|| panic!("该有一棵树看到子 agent 刚生出来还在 Thinking：{trees:#?}"));
    assert_eq!(with_live_child.nodes[0].id, AgentId::root());
    assert_eq!(with_live_child.nodes[1].id, child);
    assert_eq!(with_live_child.nodes[1].parent, Some(AgentId::root()));
    assert_eq!(with_live_child.nodes[1].depth, 1);
    assert_eq!(
        with_live_child.nodes[1].task.as_deref(),
        Some("任务X：查天气")
    );

    // 最后一帧是收尾终态：两个都 Done。
    let last = trees.last().expect("至少有一帧");
    assert_eq!(last.nodes.len(), 2);
    assert!(
        matches!(last.nodes[0].activity, AgentActivity::Done { .. }),
        "{last:#?}"
    );
    assert!(
        matches!(last.nodes[1].activity, AgentActivity::Done { .. }),
        "{last:#?}"
    );

    // change 检测生效的直接证据：连续两帧不该完全相同（每一帧都代表一次真实
    // 变化，不是无脑逐 step 重推）。
    assert!(
        trees.windows(2).all(|w| w[0] != w[1]),
        "不该有连续两帧完全相同的树——change 检测该挡掉重复：{trees:#?}"
    );
}

#[test]
fn a_provider_timeout_retry_that_leaves_status_unchanged_does_not_re_emit() {
    // 照抄 `timeout.rs`：两次 `HangAfterHeaders` + `max_retries = 1`——恰好一次
    // provider 超时重试（状态原地留在 `Thinking`）,第二次超时耗尽预算落 `Failed`。
    let dir = support::temp_dir("tree-snapshot-timeout-noop");
    let port = support::spawn_scripted_server(vec![
        support::ScriptedResponse::HangAfterHeaders,
        support::ScriptedResponse::HangAfterHeaders,
    ]);
    let (ctx, _events) = support::build_ctx(port, &dir);
    let ctx = ctx.with_provider_timeout(Duration::from_millis(150));
    let (mut ctx, trees) = collect_trees(ctx);

    let mut session = Session::new(AgentId::root());
    session.set_max_retries(1);

    let status = agent_runtime::block_on(run_turn(&mut session, &mut ctx, "你好"));
    assert_eq!(
        status,
        TurnStatus::Failed(Failure::Provider(ErrorClass::Retryable))
    );

    let trees = trees.borrow();
    // 该恰好两帧：`UserInput` 把 root 推进 `Thinking`（第一帧），重试那次
    // `Timeout` 状态原地不动（不该多推一帧），耗尽预算的第二次 `Timeout` 把
    // root 推进 `Failed`（第二帧）。多于两帧就说明「没变的 step」也被推了。
    assert_eq!(
        trees.len(),
        2,
        "该恰好两帧（Thinking 一次、Failed 一次），重试那步不该重复推同一棵树：{trees:#?}"
    );
    assert_eq!(trees[0].nodes.len(), 1);
    assert_eq!(trees[0].nodes[0].activity, AgentActivity::Thinking);
    assert_eq!(trees[1].nodes.len(), 1);
    assert!(
        matches!(trees[1].nodes[0].activity, AgentActivity::Failed { .. }),
        "{:?}",
        trees[1]
    );
}
