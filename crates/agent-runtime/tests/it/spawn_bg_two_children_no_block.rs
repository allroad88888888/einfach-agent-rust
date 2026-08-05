//! 052 验收 1/2/3：**后台 spawn 不挡父**，两个后台子并发跑，而且它们落终态时
//! **不回写父**（结果进 stash，父的历史里不多出任何一条）。
//!
//! # 时序焊死
//!
//! - root 一跳吐**两个** `spawn(background=true)` → 两条 `{"agent_id":...}` 当场
//!   回到父的两个槽 → 父不被挡，立刻发第二跳（脚本让它慢，700ms）。
//! - 两个子各 250ms 答完 → 它们落终态的时刻**明显早于**父的第二跳答完。
//!
//! 于是「父没被挡」有两条互相独立的硬证据：
//!
//! 1. 服务器侧记的时间——父第二跳的**到达时刻早于两个子答完的时刻**（阻塞
//!    spawn 下这不可能：父那时还卡在 `ToolsPending`）；
//! 2. 父那两条 tool_result 的正文是 `{"agent_id":...}`，不是子的回答。
//!
//! 而「不回写父」这条要数着断言：父的 tool_result **恰好两条**（不是四条），
//! 且子的回答正文一个字都没进父的历史。

mod spawn_bg_support;

use std::time::Duration;

use agent_core::{AgentId, Session, TurnStatus};
use agent_runtime::run_turn;

use spawn_bg_support::{
    Route, RoutedServer, any_message_mentions, build_ctx, sse_text, sse_tool_calls, temp_dir,
    tool_results, warned_about, wire_tool_name,
};

const CHILD: Duration = Duration::from_millis(250);
const ROOT_HOP2: Duration = Duration::from_millis(700);

#[test]
fn two_background_children_run_while_the_parent_keeps_going_and_never_write_back() {
    let dir = temp_dir("bg-two");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        // 最具体的先判：父的第二跳请求体里带着两个 call_id（tool_call_id 回填），
        // 别的请求都不带。
        Route {
            needle: "call_bg_a",
            delay: ROOT_HOP2,
            status: 200,
            lines: sse_text("我不等它们，先答了"),
        },
        Route { needle: "TASKBGA", delay: CHILD, status: 200, lines: sse_text("ANSWERBGA 子甲的答案") },
        Route { needle: "TASKBGB", delay: CHILD, status: 200, lines: sse_text("ANSWERBGB 子乙的答案") },
        Route {
            needle: "kickoff",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_calls(&[
                ("call_bg_a", &spawn_wire, r#"{"task":"TASKBGA 后台干活甲","background":true}"#),
                ("call_bg_b", &spawn_wire, r#"{"task":"TASKBGB 后台干活乙","background":true}"#),
            ]),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin().with_spawn(agent_core::AgentLimits::default());
    let (mut ctx, events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff 两个后台子");

    assert_eq!(status, TurnStatus::Done { truncated: false });

    // --- 父没被挡：时间上的硬证据 ---
    let a = server.call("TASKBGA").expect("子甲该被调用");
    let b = server.call("TASKBGB").expect("子乙该被调用");
    let hop2 = server.call("call_bg_a").expect("父的第二跳该发出去");
    assert!(
        hop2.start < a.end && hop2.start < b.end,
        "父的第二跳该在两个子答完之前就发出去（阻塞 spawn 下这不可能）：hop2.start={:?} a.end={:?} b.end={:?}",
        hop2.start,
        a.end,
        b.end,
    );
    assert!(server.overlapped("TASKBGA", "TASKBGB"), "两个后台子该并发跑");

    // --- 父那两个槽收敛成了 agent_id，不是子的回答 ---
    let root = AgentId::root();
    let results = tool_results(&session, &root);
    assert_eq!(results.len(), 2, "父该正好有两条 tool_result（两次 spawn），一条不多：{results:#?}");
    let children: Vec<AgentId> =
        session.live_agents().into_iter().filter(|a| a != &root).collect();
    assert_eq!(children.len(), 2, "两个后台子都该还在活名单上：{children:?}");
    for (call_id, content, is_error) in &results {
        assert!(!is_error, "后台 spawn 该立刻成功收敛：{call_id} {content}");
        assert!(content.contains("agent_id"), "正文该是 {{\"agent_id\":...}}：{content}");
        assert!(
            children.iter().any(|child| content.contains(child.as_str())),
            "正文该点名一个真实存在的子 agent：{content}，活的是 {children:?}"
        );
    }
    assert_ne!(results[0].1, results[1].1, "两次 spawn 该拿到两个不同的 agent_id");

    // --- 子落终态时**没有**回写父（结果进了 stash） ---
    assert!(
        !any_message_mentions(&session, std::slice::from_ref(&root), "ANSWERBGA"),
        "子甲的回答不该出现在父的历史里（父那个槽早就收敛了）：{:#?}",
        session.messages_of(&root)
    );
    assert!(
        !any_message_mentions(&session, std::slice::from_ref(&root), "ANSWERBGB"),
        "子乙的回答不该出现在父的历史里：{:#?}",
        session.messages_of(&root)
    );
    // 而子自己那边确实答完了 —— 上面那两条不是因为「子根本没跑」而绿的。
    for child in &children {
        assert_eq!(session.status_of(child), TurnStatus::Done { truncated: false });
    }
    assert!(
        children.iter().any(|c| any_message_mentions(&session, std::slice::from_ref(c), "ANSWERBGA")),
        "子甲的回答该落在它自己的历史里"
    );
    assert!(
        children.iter().any(|c| any_message_mentions(&session, std::slice::from_ref(c), "ANSWERBGB")),
        "子乙的回答该落在它自己的历史里"
    );

    // --- 跑完没人领 = 轮末告警，不静默 ---
    let events = events.borrow();
    for child in &children {
        assert!(
            warned_about(&events, child.as_str()),
            "跑完没人领的后台子该在轮末留一条可见告警：{child:?}"
        );
    }
}
