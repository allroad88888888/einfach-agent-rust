//! 211 独立验收 · 第 7 条：**喊得停**。
//!
//! 自驱动跑到中途（第 1 节自开的那一轮已经正常收尾，第 2 节自开的那一轮还在等
//! provider 应答）置上取消标志——手法照抄 `cancel.rs`：假服务器的这一跳先写完
//! 响应头再挂住不发数据，一个后台线程模拟 Ctrl-C，超时预算拉得远大于这条测试
//! 的时间尺度，观察到的结果必须是取消标志起的作用，不是我们自己的超时机制
//! 抢跑撞上同一个结果。
//!
//! 断言：有界时间内收工；报一条 `AutoTurnHeld{Cancelled}`；已经跑完的那一轮
//! 不被判成失败；`pending_next_turn_mail` 显示还有留言没被读到（喊停没有把
//! 「还欠一轮」这件事悄悄抹掉）。
//!
//! 黑盒来源与「实现体没读」的声明见 `auto_turn_indep_support/mod.rs` 顶部。

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::{AutoTurnHold, pending_next_turn_mail, run_auto_turns};

use crate::auto_turn_indep_support::{
    Leg, RoutedServer, auto_turn_held, build_ctx, chain_routes_with_extra, no_delay,
    sse_tool_call, temp_dir, wire_tool_name,
};

const KICKOFF: &str = "KICKOFF-cancel 先正常跑一节";

#[test]
fn cancelling_mid_way_through_an_auto_turn_holds_without_failing_the_ones_already_done() {
    let dir = temp_dir("auto-turn-cancel");

    let leg0 = Leg {
        trigger_needle: KICKOFF,
        spawn_call_id: "call_spawn_0cancel",
        task_needle: "TASK-A1cancel",
        send_call_id: "call_send_1cancel",
        note_text: "CANCELNOTE-1",
        child_final_text: "A1-DONE-cancel",
        root_final_text: "ROOT-T0-DONE-cancel",
    };
    let leg1 = Leg {
        trigger_needle: "CANCELNOTE-1",
        spawn_call_id: "call_spawn_1cancel",
        task_needle: "TASK-A2cancel",
        send_call_id: "call_send_2cancel",
        note_text: "CANCELNOTE-2",
        child_final_text: "A2-DONE-cancel",
        root_final_text: "ROOT-T1-DONE-cancel",
    };
    // 第 3 节：root 看到 `CANCELNOTE-2` 之后该决定 spawn 一个新子——但这一跳
    // 先写完响应头就挂住不吐数据，给取消标志留出起作用的窗口。它会不会真的被
    // 用来拼出一个 tool_call 不重要：这条测试要看到的是「等不到它」，不是
    // 「等到了之后发生了什么」。**必须走 `chain_routes_with_extra`**，不能直接
    // `Vec::insert(0, ..)`：`CANCELNOTE-2` 同时也是 a2 自己第二跳请求体里的
    // 一部分（它自己 `send` 调用的入参），插错层会连 a2 正常收尾那一跳也一起
    // 挂住，「第 1 节自开轮正常收尾」这个前提就不成立了。
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);
    let mut hang_route = no_delay(
        "CANCELNOTE-2",
        sse_tool_call(
            "call_spawn_2cancel_unused",
            &spawn_wire,
            r#"{"task":"TASK-A3cancel-should-not-arrive"}"#,
        ),
    );
    hang_route.delay = Duration::from_secs(8);
    let routes = chain_routes_with_extra(&[leg0, leg1], hang_route);
    let server = RoutedServer::start(routes);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_send();
    let (ctx, events) = build_ctx(server.port, &dir, tools);
    // 超时预算拉得远大于这条测试的时间尺度（同 `cancel.rs` 的理由）。
    let mut ctx = ctx.with_provider_timeout(Duration::from_secs(5));
    let mut session = Session::new(AgentId::root());

    session.set_agent_limits(AgentLimits {
        max_auto_turns: 3,
        ..AgentLimits::default()
    });

    let status0 = agent_runtime::run_turn(&mut session, &mut ctx, KICKOFF)
        .expect("kickoff 不是 source failure");
    assert_eq!(status0, TurnStatus::Done { truncated: false });

    let cancel = ctx.cancel_flag();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        cancel.store(true, Ordering::Relaxed);
    });

    let start = Instant::now();
    let result = run_auto_turns(&mut session, &mut ctx);
    let elapsed = start.elapsed();

    // ① 有界时间内收工——不是靠 8s 的挂起跳数或 5s 的超时预算撞出来的。
    assert!(
        elapsed < Duration::from_secs(3),
        "该在置位之后的几个 poll 间隔内收尾，不该等到挂起的那一跳，实际 {elapsed:?}"
    );

    // ② 已经跑完的第 1 节自开轮不被判成失败：不管 `run_auto_turns` 整体是
    // `Ok` 还是 `Err`，事件流里都不该出现「第 1 节失败了」这种说法——判据落在
    // 下面对 `AutoTurnHeld` 的断言上，这里只确认没有 panic、没有裸露的错误。
    if let Ok(statuses) = &result {
        assert!(
            !statuses.is_empty(),
            "第 1 节该已经正常收尾，不该被取消影响到"
        );
        assert_eq!(
            statuses[0],
            TurnStatus::Done { truncated: false },
            "第 1 节该是正常终态，取消发生在它之后：{statuses:?}"
        );
    }

    // ③ 报了一条 `AutoTurnHeld{Cancelled}`——喊停之后必须说清「为什么没继续」。
    //
    // **这条目前会红，而且红得不是这份测试的锅**：真机跑一遍能看到
    // `run_auto_turns` 在这条链路上返回的是
    // `Ok([Done{truncated:false}, Failed(Cancelled)])`——取消发生在第 2 节自开
    // 轮**自己的 provider 调用中途**，那次调用直接落成了 `TurnStatus::Failed
    // (Cancelled)`（跟任何一次普通 `run_turn` 被取消时一模一样），事件流里
    // 完全没有 `AutoTurnHeld` 这个变体，`session.inbox_of(&root)` 是空的
    // （`CANCELNOTE-2` 已经在这一轮的 `drain_next_turn` 里被读进历史，取消发生
    // 在那**之后**）。
    //
    // 对照 `crates/agent-wasm/src/turn.rs`（非禁读文件）的浏览器宿主循环：它
    // 用的是更底层的 `try_one_auto_turn_async`，并且**自己**在看到
    // `Failed(Cancelled)` 时显式调 `agent_runtime::undo::undo_turn` 把那半轮
    // 退掉（该文件原话：「自开的一轮被取消 → 跟用户那一轮同一条路（undo_turn
    // 丢弃半轮）」）——退掉之后笔记才会回到收件箱。但这条测试用的是文档说的
    // 「native 同步壳」`run_auto_turns`（`agent-cli::repl` 用的也是这一个），
    // 它*没有*替调用方做这一步；CLI 自己的 `repl.rs` 对**用户那一轮**的取消有
    // `undo::after_cancelled_turn`，但对 `run_auto_turns` 吐出来的
    // `statuses` 里混进的 `Failed(Cancelled)` 完全没有对应处理。
    //
    // 换句话说：211 §验收第 7 条承诺的「报 AutoTurnHeld{Cancelled} + 留言还在
    // 收件箱」只有走 `try_one_auto_turn_async` 手动挡 + 手动 undo 才兑现得了；
    // 走 `run_auto_turns` 拿到的是一截半成品状态、一个不吭声的
    // `Failed(Cancelled)`、和一条已经被读走却再也不会被回答的笔记。这条断言
    // 照验收标准原文写，让它如实地红。
    let held = auto_turn_held(&events.borrow());
    assert!(
        held.iter()
            .any(|(_, reason)| *reason == AutoTurnHold::Cancelled),
        "该报一条理由是 Cancelled 的 AutoTurnHeld，实际一条都没有：{held:?}（\
         `run_auto_turns` 返回的是 {result:?}）"
    );

    // ④ 喊停没有把「还欠一轮」这件事悄悄抹掉：`CANCELNOTE-2` 触发的这一节还没
    // 跑完，它对应的留言该还算在「待读」里。**同③一样，目前会红**——见③的
    // 长注释：笔记已经被 `drain_next_turn` 读进了历史，`run_auto_turns` 没有
    // 替调用方把这半轮 undo 掉，所以这里量到的是 0，不是 ≥1。
    assert!(
        pending_next_turn_mail(&session) >= 1,
        "取消之后收件箱该还有留言等着，实际 pending_next_turn_mail={}（笔记已被读进历史，\
         没有半轮回滚机制把它退回来）",
        pending_next_turn_mail(&session)
    );
}
