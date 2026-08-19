//! 211 独立验收 · 第 1/2/9 条：**能自己往下跑** + **停得下来** + **`remaining`
//! 逐轮递减、减在开轮之前**。
//!
//! 预算 3，一条自我续写的留言链（root spawn 一个子 → 子给 root 留一条
//! `next_turn` 笔记 → 子收尾 → root 收尾；下一节的触发点就是这一节留下的笔记）
//! →一次 `run_auto_turns` 调用，**没有任何用户输入**，该连开 3 轮，第 4 节留下
//! 的笔记该原地留在收件箱里，不多开一轮，也不 panic、不空转。
//!
//! 黑盒来源与「实现体没读」的声明见 `auto_turn_indep_support/mod.rs` 顶部。

use std::time::{Duration, Instant};

use agent_core::{AgentId, AgentLimits, Deliver, Session, TurnStatus};
use agent_runtime::{AutoTurnHold, run_auto_turns, run_turn};

use crate::auto_turn_indep_support::{
    Leg, auto_turn_held, auto_turn_started_remaining, build_ctx, chain_routes, temp_dir,
};

const KICKOFF: &str = "KICKOFF-chain 从这里开始一条自我续写的链";

/// 四节链：第 1 节是真实用户那一轮（root spawn a1），第 2~4 节是三次自驱动轮
/// （budget=3 正好够开三次）。第 4 节留下的笔记没人读——预算见底之前只够开到
/// 「留笔记」这一步的**上一节**，第 4 节本身就是**用光最后一格预算**开出来的
/// 那一轮，它留的笔记才是「见底之后原地躺着」的那一条。
fn legs() -> Vec<Leg> {
    vec![
        Leg {
            trigger_needle: KICKOFF,
            spawn_call_id: "call_spawn_0chain",
            task_needle: "TASK-A1chain",
            send_call_id: "call_send_1chain",
            note_text: "CHAINNOTE-1",
            child_final_text: "A1-DONE-chain",
            root_final_text: "ROOT-T0-DONE-chain",
        },
        Leg {
            trigger_needle: "CHAINNOTE-1",
            spawn_call_id: "call_spawn_1chain",
            task_needle: "TASK-A2chain",
            send_call_id: "call_send_2chain",
            note_text: "CHAINNOTE-2",
            child_final_text: "A2-DONE-chain",
            root_final_text: "ROOT-T1-DONE-chain",
        },
        Leg {
            trigger_needle: "CHAINNOTE-2",
            spawn_call_id: "call_spawn_2chain",
            task_needle: "TASK-A3chain",
            send_call_id: "call_send_3chain",
            note_text: "CHAINNOTE-3",
            child_final_text: "A3-DONE-chain",
            root_final_text: "ROOT-T2-DONE-chain",
        },
        Leg {
            trigger_needle: "CHAINNOTE-3",
            spawn_call_id: "call_spawn_3chain",
            task_needle: "TASK-A4chain",
            send_call_id: "call_send_4chain",
            note_text: "CHAINNOTE-4",
            child_final_text: "A4-DONE-chain",
            root_final_text: "ROOT-T3-DONE-chain",
        },
    ]
}

#[test]
fn a_self_continuing_chain_runs_exactly_the_budgeted_number_of_auto_turns_then_holds() {
    let dir = temp_dir("auto-turn-chain");
    let server = crate::auto_turn_indep_support::RoutedServer::start(chain_routes(&legs()));

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_send();
    let (mut ctx, events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();

    // 部署方配 3（`DEFAULT_MAX_AUTO_TURNS` 也是 3，这里显式写出来，不依赖默认值
    // 恰好相等这件事）。
    session.set_agent_limits(AgentLimits {
        max_auto_turns: 3,
        ..AgentLimits::default()
    });

    // ---- 真实用户输入那一轮：只有它能把预算加满，且它本身就是第 1 节。----
    let status0 = run_turn(&mut session, &mut ctx, KICKOFF).expect("第 1 节不是 source failure");
    assert_eq!(status0, TurnStatus::Done { truncated: false });
    assert_eq!(
        session.auto_turn_budget(),
        3,
        "真实用户输入该把预算加满到配置的上限"
    );
    assert_eq!(
        session.inbox_of(&root).len(),
        1,
        "第 1 节该给 root 留下一条笔记等着被自动读"
    );

    // ---- 之后**没有任何用户输入**：一次 `run_auto_turns` 调用该连开 3 轮。----
    let start = Instant::now();
    let statuses = run_auto_turns(&mut session, &mut ctx).expect("自驱动不是 source failure");
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(10),
        "该在有界时间内收工，实际用了 {elapsed:?}——超时说明自驱动挂死或空转了"
    );

    // ① 能自己往下跑：连开 3 轮，一轮不多一轮不少，每轮都正常收尾。
    assert_eq!(
        statuses,
        vec![
            TurnStatus::Done { truncated: false },
            TurnStatus::Done { truncated: false },
            TurnStatus::Done { truncated: false },
        ],
        "预算 3 该连开 3 轮，一次真实用户输入换来的是 `run_auto_turns` 返回的 3 个终态"
    );

    // ② 停得下来：预算见底之后不再多开，第 4 节留下的笔记原地留在收件箱里。
    assert_eq!(
        session.auto_turn_budget(),
        0,
        "跑完 3 轮之后预算该恰好见底"
    );
    let inbox = session.inbox_of(&root);
    assert_eq!(
        inbox.len(),
        1,
        "第 4 节留的笔记没人读，该原地留着，不丢弃：{inbox:?}"
    );
    assert_eq!(inbox[0].when, Deliver::NextTurn);
    assert_eq!(&*inbox[0].text, "CHAINNOTE-4");

    // 没有第 5 节：脚本只配了 4 节 16 跳，服务器一共只该被问过这么多次——
    // 多问一次就是「预算见底了还在自己开」的直接证据。
    assert_eq!(
        server.calls().len(),
        16,
        "4 节 × 4 跳 = 16，实际：{:?}",
        server.calls().iter().map(|c| c.needle).collect::<Vec<_>>()
    );

    // ③ `remaining` 逐轮递减，且减在开轮之前：预算 3 开 3 轮，该看到 2、1、0，
    // 不是 3、2、1（那会是「报的是开轮前的值」）。
    assert_eq!(
        auto_turn_started_remaining(&events.borrow()),
        vec![2, 1, 0],
        "第一轮报的该是 n-1 而不是 n——减在开轮之前"
    );

    // 见底之后该说清「为什么没开」：留言还在，理由是预算见底，不是别的。
    assert_eq!(
        auto_turn_held(&events.borrow()),
        vec![(1, AutoTurnHold::BudgetExhausted)],
        "见底之后该报恰好一条 AutoTurnHeld，pending=1（第 4 节的笔记），理由是预算见底"
    );

    // `begin_turn` 不碰预算：跑完 3 轮之后再手动开一轮，预算不该被这一下重置
    // 回上限（写成重置的话，这条必红）。
    session.begin_turn();
    assert_eq!(
        session.auto_turn_budget(),
        0,
        "begin_turn 不该把已经花掉的预算重置回配置的上限"
    );
}
