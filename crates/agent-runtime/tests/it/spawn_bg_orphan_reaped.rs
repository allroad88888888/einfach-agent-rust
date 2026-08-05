//! 052 验收「孤儿收尾」的四条断言。
//!
//! 父发了一个后台子就**不管了**，直接产出最终答案想收尾。子还在飞（900ms）。
//!
//! 1. `run_turn` 真的返回了（不永久空转、不 panic），而且有界；
//! 2. 返回的是 **root 的正常终态**，不是 `Failed(Cancelled)` —— 这条专门钉
//!    「没走会话级取消」：走了 `Effect::CancelInFlight` 这里立刻红（会话级取消
//!    没有 agent 字段，而且会把这一轮判成取消，可 root 明明答成功了）；
//! 3. 树里孤儿已经非活（`despawn_child` 拆掉了）；
//! 4. 孤儿那条**迟到的在飞结果**没写进任何消息历史 —— 它确实回来了
//!    （流式增量为证），但撞上 `Session::step` 的活性闸被丢掉。
//!
//! 外加一条：轮末留了可见告警（模型 spawn 了后台子却没收尾就走了），不静默。

mod spawn_bg_support;

use std::time::{Duration, Instant};

use agent_core::{AgentId, Session, TurnStatus, UndoReport};
use agent_runtime::run_turn;

use spawn_bg_support::{
    Route, RoutedServer, any_message_mentions, build_ctx, sse_text, sse_tool_call, streamed_text,
    temp_dir, warned_about, wire_tool_name,
};

/// 子答得比父慢得多——父收尾时它还在飞。
const CHILD: Duration = Duration::from_millis(900);

#[test]
fn an_uncollected_background_child_is_despawned_and_the_turn_still_ends_normally() {
    let dir = temp_dir("bg-orphan");
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
            lines: sse_text("LATEORPHANANSWER 迟到的孤儿答案"),
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

    let tools = agent_runtime::ToolTable::builtin().with_spawn(agent_core::AgentLimits::default());
    let (mut ctx, events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let start = Instant::now();
    let status = run_turn(&mut session, &mut ctx, "kickoff 一个后台子然后不管它");
    let elapsed = start.elapsed();

    // ① 真的返回了，而且有界（泵还要等那条在飞的凭据落地，所以 >= CHILD 是对的，
    //    但不该久到看起来像挂住）。
    assert!(
        elapsed < Duration::from_secs(8),
        "该在有界时间内收尾：实际 {elapsed:?}"
    );

    // ② root 的正常终态，**不是** Failed(Cancelled)。
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "root 答成功了，这一轮就该是 Done —— 判成 Failed(Cancelled) 说明走了会话级取消"
    );

    // ③ 孤儿已经非活：树上没了，活名单上也没了。
    let root = AgentId::root();
    let orphan = AgentId::new("root/a1");
    assert!(
        !session.is_live(&orphan),
        "没人领的后台子该被 despawn_child 拆掉"
    );
    assert_eq!(
        session.live_agents(),
        vec![root.clone()],
        "活名单上该只剩 root"
    );
    assert!(
        !session.agent_tree().nodes.iter().any(|n| n.id == orphan),
        "活树上不该还有这个孤儿：{:#?}",
        session.agent_tree()
    );

    // ④ 迟到的结果**确实回来了**，但没写进任何消息历史。
    let events = events.borrow();
    let streamed = streamed_text(&events, &orphan);
    assert!(
        streamed.contains("LATEORPHANANSWER"),
        "孤儿那条在飞的结果该真的回来过（否则这条测试没测到闸）：{streamed:?}"
    );
    assert!(
        !any_message_mentions(
            &session,
            &[root.clone(), orphan.clone()],
            "LATEORPHANANSWER"
        ),
        "迟到的孤儿结果被写进了已经收尾的世界 —— 活性闸没挡住：root={:#?} orphan={:#?}",
        session.messages_of(&root),
        session.messages_of(&orphan),
    );

    // 轮末告警：不静默。
    assert!(
        warned_about(&events, orphan.as_str()),
        "被拆掉的孤儿该留一条可见告警：{events:#?}"
    );

    // undo 照旧连带整棵子树：spawn 和 teardown 都记在这一轮里（turn_id 继承），
    // 一次 `undo_turn` 全退光。
    drop(events);
    let before = session.history_len();
    match session.undo_turn() {
        UndoReport::Applied { entries, turn_id } => {
            assert_eq!(turn_id, 1);
            assert_eq!(
                entries, before,
                "这一轮的全部 entry（含 despawn 的 teardown）该一次退光"
            );
        }
        other => panic!("期望 Applied，拿到 {other:?}"),
    }
    assert_eq!(session.live_agents(), vec![root]);
    assert!(session.messages().is_empty());
}
