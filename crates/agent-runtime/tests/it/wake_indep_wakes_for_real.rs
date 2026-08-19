//! 214 独立验收 · 第 1 条：**唤醒真的发生**——目标 `Done` 之后投一条 `now`，
//! 它真的又发起了一次 provider 调用，那次请求的 prompt 里有投给它的那句话，
//! 而且它真的答了。
//!
//! 跟 `agent-core` 那五个 `wake_indep_*` 不同，这条必须走完整的 `run_turn` +
//! 真实（本地假）HTTP 往返——`agent-core` 的独立测试只能证明「`on_wake` 这个
//! 转移函数自己的契约对」，证不了「`srv:agent/send` 投给一个终态 agent 时，
//! runtime 真的把那条转移接上了」。214 §缘起 说得很清楚：这条 issue 存在的
//! 全部理由就是 `agent-runtime` 一个人办不到——所以独立验收也得在
//! `agent-runtime` 这一层跑一遍真实的泵。
//!
//! 形状：root 前台（阻塞）spawn 一个子，子先答完一次话（落终态）；root 收到
//! spawn 的结果之后，再用 `srv:agent/send` 给这个已经答完的子投一条 `now`；
//! 断言子对这条消息**又发起了一次 HTTP 请求**、请求体里含着投给它的原文、
//! 它的回复也进了它自己的历史。
//!
//! 夹具复用 `send_indep_support`（206 独立测试留下的并发假服务器 + `RunnerCtx`
//! 装配，见该模块顶部的黑盒来源声明），这份测试自己只贡献脚本与断言。
//! **没有读** `crates/agent-core/src/command/transitions/wake.rs` 与
//! `crates/agent-runtime/src/send_tool.rs`；黑盒来源另见
//! `docs/issues/214-wake-a-terminal-agent.md` §验收（跳过「实做记录」）。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::send_indep_support::{
    Route, RoutedServer, SEND_WIRE, build_ctx, calls_matching, index_of, sse_text, sse_tool_call,
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
fn sending_now_to_a_done_child_makes_it_call_the_provider_again_with_that_message_and_answer() {
    let dir = temp_dir("wake-real");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        // root 的第三跳（body 里带着 send 那个 call_id）：收尾。**必须排在
        // root 第二跳的路由之前**——root 第三跳的 body 是累积的，仍然含着
        // 第二跳那个 `call_spawn_r1`，先匹配到的路由才算数（`Route` 的文档：
        // 「按声明顺序首次匹配，所以越具体的 needle 越要排在前面」），排反了
        // 会让 root 第三跳误命中第二跳那条，又发一次 spawn 而不是收尾。
        no_delay("call_send_r1", sse_text("ROOTDONE-real")),
        // 子被唤醒之后那一跳：body 里含被投递的原文——这条路由能命中，本身就是
        // 「prompt 里有那条消息」的证据。**必须排在子第一跳的路由之前**：不然
        // 会命中「首次匹配」的第一跳规则，重复回同一句 A1FIRST，「它真的又答了
        // 一次不一样的话」就测不出来了。
        no_delay("WAKENOW-真的醒了吗", sse_text("A1SECOND-读到了才有这句")),
        // root 的第二跳（body 里带着 spawn 那个 call_id）：拿到子已经答完的
        // 消息之后，往它那儿投一条 `now`。
        no_delay(
            "call_spawn_r1",
            sse_tool_call(
                "call_send_r1",
                SEND_WIRE,
                r#"{"to":"root/a1","text":"WAKENOW-真的醒了吗","when":"now"}"#,
            ),
        ),
        // 子的第一跳：接了任务就直接答完，落终态。
        no_delay("A1JOB-real", sse_text("A1FIRST-第一次就答完了")),
        no_delay(
            "kickoff-wake-real",
            sse_tool_call(
                "call_spawn_r1",
                &spawn_wire,
                r#"{"task":"A1JOB-real 先答一句再说"}"#,
            ),
        ),
    ]);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_send();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff-wake-real 先派活再唤醒")
        .expect("这条链路不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let a1 = AgentId::root().child(1);

    // ① 它真的又发了一次 provider 调用——唤醒之前只答过一次。
    assert_eq!(
        calls_matching(&server, "A1JOB-real"),
        1,
        "子的第一次任务只该被服务一次"
    );
    assert_eq!(
        calls_matching(&server, "WAKENOW-真的醒了吗"),
        1,
        "唤醒之后那次请求该恰好发生一次"
    );

    // ② 那次请求的 body 里确实带着投给它的原文——不是巧合命中别的路由。
    let woken_call = server
        .call("WAKENOW-真的醒了吗")
        .expect("唤醒那次调用该被服务器记录下来");
    assert!(
        woken_call.body.contains("WAKENOW-真的醒了吗"),
        "prompt 里该有投给它的那句话：{}",
        woken_call.body
    );

    // ③ 它真的答了：新的回复进了它自己的历史，状态回到终态。
    assert!(
        index_of(&session, &a1, "A1SECOND-读到了才有这句").is_some(),
        "被唤醒之后的回复该进历史：{:#?}",
        session.messages_of(&a1)
    );
    assert_eq!(
        session.status_of(&a1),
        TurnStatus::Done { truncated: false },
        "答完之后该又落回终态"
    );
}
