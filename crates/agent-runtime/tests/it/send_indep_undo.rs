//! 206：`/undo` 掉包含一次 `send` 的那一轮 → 收件箱与两边 `Messages` 全部回到
//! 投递之前，而且**不产生屏障**——投递和排空都是纯状态（`Undoability::StateOnly`），
//! `/undo` 不该停下来问「这一步撤不回去，确定吗」。
//!
//! 形状：第一轮跑一句普通问答并留下快照；第二轮里子 agent 给 root 投一条、root
//! 在下一次组装请求之前把它排空、答完收工。然后一次 `/undo` 把第二轮整个退掉，
//! 逐值比对回第一轮结束时的那份快照。
//!
//! 用 `agent_runtime::undo::undo_turn`（带钩子表的那一档，201）而不是
//! `Session::undo_turn`：三个宿主的 `/undo` 走的就是这条，钩子路上出问题这里才看得见。
//!
//! 黑盒来源与「实现体没读」的声明见 `send_indep_support/mod.rs` 顶部。

use std::time::Duration;

use agent_core::{
    AgentId, AgentLimits, AgentValue, AtomKey, Session, TurnStatus, UndoReport, Undoability,
};
use agent_runtime::run_turn;

use crate::send_indep_support::{
    Route, RoutedServer, SEND_WIRE, build_ctx, index_of, injected, sse_text, sse_tool_call,
    temp_dir, wire_tool_name,
};

fn no_delay(needle: &'static str, lines: Vec<String>) -> Route {
    Route {
        needle,
        delay: Duration::ZERO,
        status: 200,
        lines,
    }
}

#[test]
fn undoing_the_turn_that_contained_a_send_restores_both_sides_without_a_barrier() {
    let dir = temp_dir("send-undo");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        no_delay("call_a_send", sse_text("AAADONE")),
        no_delay("call_r1", sse_text("ROUND2DONE")),
        no_delay(
            "TASKUNDO",
            sse_tool_call(
                "call_a_send",
                SEND_WIRE,
                r#"{"to":"root","text":"UNDOME 中途汇报一句"}"#,
            ),
        ),
        no_delay(
            "SECONDTURN",
            sse_tool_call("call_r1", &spawn_wire, r#"{"task":"TASKUNDO 边干边汇报"}"#),
        ),
        no_delay("kickoff-undo", sse_text("ROUND1DONE")),
    ]);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_send();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();

    // --- 第一轮：留一份「投递之前」的完整快照 ---
    let status = run_turn(&mut session, &mut ctx, "kickoff-undo 先聊一句")
        .expect("普通一轮不是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });
    let primitives_before = session.primitives();
    let root_messages_before = session.messages_of(&root);
    assert!(session.inbox_of(&root).is_empty());

    // --- 第二轮：子 agent 投一条，root 排空之后接着答 ---
    session.begin_turn();
    let status =
        run_turn(&mut session, &mut ctx, "SECONDTURN 这次派个活").expect("投递不是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let a1 = root.child(1);
    let (_, note) = injected(&session, &root, "UNDOME");
    assert!(note.contains(a1.as_str()), "认得出是谁投的：{note}");
    assert!(
        server
            .call("call_r1")
            .expect("root 该发过第二跳")
            .body
            .contains("UNDOME"),
        "投来的那条该真的进了 root 下一次请求的 prompt"
    );

    // --- 这一轮里没有一条 entry 是屏障 ---
    let this_turn = session.turn_id();
    let undoabilities: Vec<_> = session
        .history()
        .entries()
        .filter(|e| e.meta.turn_id == this_turn)
        .map(|e| (e.meta.label, e.meta.undoability))
        .collect();
    assert!(
        !undoabilities.is_empty(),
        "这一轮该留下 entry，否则下面的断言是空跑的"
    );
    assert!(
        undoabilities
            .iter()
            .all(|(_, u)| *u == Undoability::StateOnly),
        "投递/排空没碰外部世界，这一轮不该有任何非 StateOnly 的 entry：{undoabilities:?}"
    );

    // --- 一次 `/undo` 全退光，且不被拦下来问 ---
    match agent_runtime::undo::undo_turn(&mut session, &mut ctx) {
        UndoReport::Applied { turn_id, .. } => assert_eq!(turn_id, this_turn),
        other => panic!("纯状态的一轮不该产生屏障，得到 {other:?}"),
    }

    // root 那半边逐值比对。**只比 root 的槽位**：一轮 spawn 被 undo 之后，子
    // agent 的 atom 键仍然留在 family 上（值全部回到默认档），这是 spawn 的既有
    // 行为（`spawn_indep_undo_subtree.rs` 同样只断言活名单与消息），不是 206 的事
    // ——子那半边下面按「回到默认」单独断。
    let root_slots = |snap: &[(AtomKey, AgentValue)]| -> Vec<(AtomKey, AgentValue)> {
        snap.iter()
            .filter(|(k, _)| matches!(k, AtomKey::Agent(id, _) if id == &root))
            .cloned()
            .collect()
    };
    assert_eq!(
        root_slots(&session.primitives()),
        root_slots(&primitives_before),
        "root 的全部 primitive（收件箱也在里面）该逐值回到投递之前"
    );
    assert_eq!(
        session.messages_of(&root),
        root_messages_before,
        "root 的 `Messages` 回到投递之前"
    );
    assert!(
        index_of(&session, &root, "UNDOME").is_none(),
        "被投递的那条不该还留在对话里"
    );
    assert!(
        session.inbox_of(&root).is_empty(),
        "收件箱也该回到投递之前（空的）：{:?}",
        session.inbox_of(&root)
    );
    assert!(!session.is_live(&a1), "那一轮 spawn 的子也一起退掉了");
    assert!(
        session.messages_of(&a1).is_empty(),
        "子那半边的 `Messages` 同样回到投递之前"
    );
    assert!(
        session.inbox_of(&a1).is_empty(),
        "子那半边的收件箱同理：{:?}",
        session.inbox_of(&a1)
    );
}
