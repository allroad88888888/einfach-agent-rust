//! **被叫醒的后台子，`collect` 领回的必须是它醒来之后说的话。**
//!
//! # 这条是真机 dogfood 逮出来的，逮了两次
//!
//! 现场（真 provider，两个后台兄弟）：B 把算出来的 `13` 用 `send(when="now")`
//! 发给 A，A 被 214 叫醒、重新答了「13 乘以 7 等于 91」——而 root `collect` 领回
//! 来的是**叫醒之前**那句「我在等数字」。没有任何东西报错。
//!
//! 根因是 214（唤醒）和 053（stash + collect）在交接处没人测过：后台子落终态时
//! `Subtree::harvest_detached` 把它**从 detached 名单划掉**、结果进 stash，所以
//! 它醒来后再落终态时不再被收割。
//!
//! **第一版修法在真机上又错了一次**，这才是这条测试真正要钉死的东西：光把它从
//! stash 挪回 detached 不够——`send` 叫醒它的那一刻 `Event::Wake` 还排在待办队列
//! 里没被 `step`，它**此刻仍然是终态**，于是下一圈收割立刻把那份旧答案原样打回
//! stash 并再次划掉它。真机上表现得跟没修一样。
//!
//! 所以断言分两半，缺一不可：
//!
//! 1. 子确实被叫醒并说了**新的一句**（`A1SECOND`）；
//! 2. `collect` 领回的正文是**那一句**，不是叫醒之前的 `A1FIRST`。
//!
//! 只断言第 1 条的话，两版实现（错的和对的）都会绿——第一版修法就是这么骗过
//! 「唤醒真的发生」那条既有测试的。
//!
//! 夹具复用 `send_indep_support`（206 留下的并发假服务器 + `RunnerCtx` 装配）。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::send_indep_support::{
    Route, RoutedServer, SEND_WIRE, build_ctx, sse_text, sse_tool_call, temp_dir, tool_result,
    wire_tool_name,
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
fn collect_after_a_wake_returns_what_the_child_said_after_waking_not_before() {
    let dir = temp_dir("wake-collect");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);
    let collect_wire = wire_tool_name(agent_runtime::COLLECT_TOOL);

    let server = RoutedServer::start(vec![
        // ── root 的四跳。**越具体的 needle 越靠前**（`Route` 的「按声明顺序
        // 首次匹配」）：root 的请求体是累积的，后面几跳仍然含着前面那些 call_id。
        //
        // 第四跳：collect 的结果回来了 → 收尾。
        no_delay("call_collect_r1", sse_text("ROOTDONE")),
        // 第三跳：send 已经投出去 → 领结果。
        no_delay("call_send_r1", sse_tool_call("call_collect_r1", &collect_wire, r#"{"id":"root/a1"}"#)),
        // 第二跳：后台 spawn 回了 agent_id → 往那个已经答完的子投一条 `now`。
        //
        // **这一跳必须慢**（唯一用到 `Route::delay` 的地方）：后台 spawn 当场就
        // 收敛了父的槽，所以 root 会立刻往下走，而子的第一跳还在飞——那时它不是
        // 终态，`send` 就不会叫醒任何人（206 的行为，正确）。延一下让子先落终态，
        // 这条测试要的是**终态之后被叫醒**那条路。
        Route {
            needle: "call_spawn_r1",
            delay: Duration::from_millis(400),
            status: 200,
            lines:
                sse_tool_call(
                    "call_send_r1",
                    SEND_WIRE,
                    r#"{"to":"root/a1","text":"WAKENOW 醒了就说句新的"}"#,
                ),
        },
        // 子被叫醒之后那一跳（body 里含被投递的原文）。**排在子第一跳之前**，
        // 否则会命中首次匹配的第一跳规则、重复回同一句话。
        no_delay("WAKENOW 醒了就说句新的", sse_text("A1SECOND-醒来之后说的")),
        // 子的第一跳。
        no_delay("TASKWAIT", sse_text("A1FIRST-醒来之前说的")),
        // root 的第一跳：后台 spawn 一个子。
        no_delay(
            "kickoff-wake-collect",
            sse_tool_call(
                "call_spawn_r1",
                &spawn_wire,
                r#"{"task":"TASKWAIT 先答一句就停","background":true}"#,
            ),
        ),
    ]);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_collect()
        .with_send();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff-wake-collect 派个后台子再叫醒它")
        .expect("唤醒 + 领取不是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();
    let a1 = root.child(1);

    // ① 前提：它真的被叫醒并说了**新的一句**。这一条挡的是「测试其实什么都没
    //    发生」——脚本写错时它会先红。
    let said: Vec<String> = session
        .messages_of(&a1)
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            agent_core::ContentBlock::Text(t) => Some(t.to_string()),
            _ => None,
        })
        .collect();
    assert!(
        said.iter().any(|t| t.contains("A1SECOND")),
        "子该被叫醒并说出新的一句：{said:?}"
    );

    // ② **本条的靶心**：collect 领回的是醒来之后那句，不是之前那句。
    let (collected, is_error) = tool_result(&session, &root, "call_collect_r1");
    assert!(!is_error, "collect 该成功：{collected}");
    assert!(
        collected.contains("A1SECOND"),
        "collect 领回的该是它**醒来之后**说的话，实际是：{collected}"
    );
    assert!(
        !collected.contains("A1FIRST"),
        "领回了叫醒之前那份——stash 没被刷新（`Subtree::rearm_after_wake` / `woken` 标记）：{collected}"
    );
}
