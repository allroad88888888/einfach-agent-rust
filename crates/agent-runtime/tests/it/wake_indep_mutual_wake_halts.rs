//! 214 独立验收 · 第 4 条：**互相喊话会停**——两个 agent 一来一回地互相唤醒，
//! 这一轮仍然有界地结束，不会挂死。
//!
//! `runner.rs` 模块文档「214 的修正」写得很直白：唤醒边一加，「没有任何 agent
//! 能把别人重新拉起来」这条让泵在 052 之前天然有限的结构性论证就没了；新的
//! 上界改成**每个 agent 自己的 `MaxTurns`**。这条独立测试要的不是信这句话，
//! 是自己真的搭一对会互相拉扯的 agent，看泵会不会被拖死。
//!
//! # 拓扑：为什么是 root ↔ 前台子，不是两个后台子
//!
//! 子 agent 的 `max_turns` 永远是默认的 32（`ChildConfig` 没有 `max_turns`
//! 字段，公开 API 里只有 `Session::set_max_turns` 能把 **root** 的预算调小）。
//! 要在测试的时间尺度内看到「预算耗尽 → 停止互相唤醒」，至少要有一方的预算是
//! 小数，所以 root 必须是被拉扯的一方，而不是旁观者。
//!
//! 其次，唤醒 root 的那一方必须在 root 已经落终态之后**还活着**——`orphan::reap`
//! 会在 root 落终态的下一次检查点把**后台**（detached）子直接拆掉（`orphan.rs`
//! 模块文档：「父发了一个后台子就不管了……子还在飞时被拆」），所以后台子根本
//! 撑不到「root 已终态、我再唤醒它一次」那个时间点。前台（阻塞）spawn 的子
//! 从来不进 `reap` 的名单（同一份文档：「前台 spawn 的子……从来不在 detached
//! 名单上，reap 一直看不见它们」），会一直活到会话结束，正合适。
//!
//! # 这一轮里真正发生的事
//!
//! 靠 `RunnerEvent` 的时间线核实过（临时插过 `eprintln!` 看实际顺序，验证完
//! 就撤掉了），比最初凭空设想的顺序早一步：`send` 这个工具**在它自己的一次
//! 执行里**先派发出去（先出 `Dispatched::Events` 里的 `ToolResult`，再出
//! `Event::Wake`——`send_indep_turn_end.rs` 顶部已经点过这个顺序），所以
//! root 收敛（进而撞顶）跟 a1 被唤醒，两件事挨得很近，但**root 先**：
//!
//! 1. root 第 1 跳（`turns_used=1`）：前台 spawn 子 a1。
//! 2. a1 第 1 跳：直接答完，落终态。
//! 3. root 第 2 跳（`turns_used=2`，**root 的预算=2，这一跳正好用完**）：调用
//!    `send`，往已经终态的 a1 投一条 `now`。这一次工具执行同时做了两件事：
//!    a1 被唤醒（`Event::Wake` 排进去了）；紧接着 root 自己那次 `send` 的
//!    `ToolResult` 收敛，`try_call_provider` 一看 `turns_used(2) >= max_turns(2)`
//!    ——撞顶，**不发新请求**，直接落 `Done{truncated:true}`。到这一步 root
//!    已经是终态了，比 a1 真正开始处理 PING 还早。
//! 4. a1 第 2 跳（被唤醒，这时才真的发起 HTTP 请求）：读到 PING，**它也往
//!    root 那儿投一条 `now`** 想把 root 拉回来——但 root 此刻已经是撞顶之后
//!    的终态，`on_wake` 的撞顶分支让这次唤醒尝试什么都不做（`wake_indep_
//!    turn_cap.rs` 单独钉着这一格），PONG 就原样留在 root 的收件箱里。
//! 5. a1 收到自己那次 `send` 的 tool_result，第 3 跳：直接答完，落终态。
//!
//! 泵在这之后无事可做（两边都没有在飞的调用了），有界地收工。**「互相喊话」
//! 被挡住的地方不是「root 还在等所以没触发唤醒」（最初设想的顺序），而是
//! 「等 a1 真的喊回来的时候，root 早已经因为撞顶不再理会任何唤醒尝试」——
//! 两条路殊途同归，都在 `on_wake` 的撞顶分支上停下，但时序细节值得记录，
//! 免得下一个人对着源码对不上号。
//!
//! **调用总数的界**：root 最多 2 跳（它的 `max_turns`），a1 在这份脚本里被
//! 精确写死成 3 跳——这两个数都是这份测试自己控制的，不是靠 a1 撞它自己
//! 32 的默认上限撞出来的（真撞 32 跳不现实）。这里断言的是「有界」与「跟
//! 脚本设计的跳数吻合」，不是重新推导 `runner.rs` 那条通用公式。
//!
//! 夹具复用 `send_indep_support`。**没有读**
//! `crates/agent-core/src/command/transitions/wake.rs` 与
//! `crates/agent-runtime/src/send_tool.rs`。

use std::time::{Duration, Instant};

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::send_indep_support::{
    Route, RoutedServer, SEND_WIRE, build_ctx, sse_text, sse_tool_call, temp_dir, wire_tool_name,
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
fn a_root_and_its_child_pinging_each_other_still_ends_the_turn_and_stays_bounded() {
    let dir = temp_dir("wake-mutual");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        // a1 第 3 跳：读到自己那次 send(PONG) 的 tool_result 之后直接收工。
        no_delay("call_a_pong", sse_text("A1DONE-mutual 第二次也收工了")),
        // a1 第 2 跳（被 root 唤醒）：读到 PING，往 root 回一条 PONG。
        no_delay(
            "PING-mutual-该被a1读到",
            sse_tool_call(
                "call_a_pong",
                SEND_WIRE,
                r#"{"to":"root","text":"PONG-mutual-该被root读到","when":"now"}"#,
            ),
        ),
        // root 第 2 跳：spawn 的 tool_result 收敛之后，往已经终态的 a1 投 PING。
        // 这一跳正好用掉 root 的第 2 份（也是最后一份）预算。
        no_delay(
            "call_spawn_mutual",
            sse_tool_call(
                "call_root_ping",
                SEND_WIRE,
                r#"{"to":"root/a1","text":"PING-mutual-该被a1读到","when":"now"}"#,
            ),
        ),
        // a1 第 1 跳：接到任务直接答完，落终态。
        no_delay("A1TASK-mutual", sse_text("A1READY-mutual 先答完一次")),
        no_delay(
            "kickoff-mutual",
            sse_tool_call(
                "call_spawn_mutual",
                &spawn_wire,
                r#"{"task":"A1TASK-mutual 先答一句"}"#,
            ),
        ),
    ]);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_send();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());
    session.set_max_turns(2);

    let start = Instant::now();
    let status = run_turn(&mut session, &mut ctx, "kickoff-mutual 开始互相喊话")
        .expect("互相喊话不该是 source failure");
    let elapsed = start.elapsed();

    // ① 这一轮真的结束了，而且是在有界时间内——没有真实的网络等待，几个
    //   本地假 HTTP 往返正常应该在毫秒级完成；给足余量防抖动，但绝不是在赌
    //   一个会一直空转的循环碰巧被谁掐断。
    assert!(
        elapsed < Duration::from_secs(10),
        "该在有界时间内收工，实际用了 {elapsed:?}——如果这条超时，说明泵挂死了"
    );

    // ② root 自己撞顶截断——它没有被 a1 的 PONG 救回去，`try_call_provider`
    //   在预算用完之后落的是 truncated:true，不是被唤醒机制静默改写。
    assert_eq!(
        status,
        TurnStatus::Done { truncated: true },
        "root 的预算用完之后该被截断，而不是继续被 a1 拉着走"
    );
    assert_eq!(session.turns_used(), 2, "root 精确用掉它的两份预算，一份没多");

    let root = AgentId::root();
    let a1 = root.child(1);

    // ③ a1 被 root 唤醒过一次，也答完了两轮（第一次任务 + 被唤醒那次），
    //   自己落终态——它没有被 root 拖着无限循环。
    assert_eq!(
        session.status_of(&a1),
        TurnStatus::Done { truncated: false },
        "a1 该正常答完，不是被截断或者卡在半路"
    );

    // ④ 调用总数跟脚本设计的跳数吻合，是一个具体的有限数字，不是「测不出来
    //   所以随便断言个很大的上限」。
    assert_eq!(
        server.calls().len(),
        5,
        "root 2 跳（spawn、send-ping） + a1 3 跳（首答、被唤醒回 pong、收尾）：{:?}",
        server.calls().iter().map(|c| c.needle).collect::<Vec<_>>()
    );

    // ⑤ a1 那条「想把 root 拉回来」的 PONG 确实到达了——它没有被拒绝投递、
    //   也没有神奇地让 root 复活：root 早在 a1 收到 PING 之前就已经因为撞顶
    //   落了 `Done{truncated:true}`（`events` 里的时间线为证：root 的
    //   `TurnStatusChanged{Done{truncated:true}}` 排在 a1 的 wake 之前），
    //   所以这条 PONG 到达时 root 已经是终态、且预算已经耗尽——`on_wake` 的
    //   撞顶分支（`wake_indep_turn_cap.rs` 单独钉着）意味着它不会再被读进
    //   `Messages`。这正是「互相喊话被挡住」的可观测证据：a1 确实喊了，
    //   但 root 确实没有再理它，PONG 原样躺在 root 的收件箱里。
    //
    //   **没有断言 `RunnerEvent::UnreadMessages`**：`unread_inbox::report()`
    //   只报一次，而这一次发生在 root **刚**撞顶截断的那一刻——早于 a1 收到
    //   PING、更早于 a1 把 PONG 发出来，所以那一次报告天然看到的是空收件箱。
    //   这条时序巧合（root 先天撞顶，而不是被 a1 唤醒后才撞顶）让这条 PONG
    //   成了 214 讨论范围之外的一个边界情况：一条消息投给了一个「终态、但
    //   往后再也不会被唤醒」的 agent，且投递发生在轮末盘点**之后**。见本文件
    //   顶部黑盒来源之外的报告——这不是 214 §验收要求的行为，写成断言只会
    //   把「我以为它该这样」钉成契约。
    let root_inbox = session.inbox_of(&root);
    assert_eq!(
        root_inbox.len(),
        1,
        "a1 的 PONG 该原样留在 root 的收件箱里，没被神奇地消费掉：{root_inbox:?}"
    );
    assert_eq!(&*root_inbox[0].text, "PONG-mutual-该被root读到");
}
