//! 206 的拒绝面：六种发不出去的情形都回 `is_error` 的 tool_result，**这一轮
//! 继续跑完**（决策 20 的哲学，跟 spawn / status / collect 一致——不 panic、
//! 不卡这一轮，让模型自己收敛）。
//!
//! 六种一次发出去，靠 call_id 各取各的结果：
//!
//! | call_id | 发的是什么 | 为什么该被拒 |
//! |---|---|---|
//! | `call_send_self` | `to` 是自己 | 自己给自己发没有意义 |
//! | `call_send_dead` | `to` 不在活树上 | 投给一个不存在的收件箱 |
//! | `call_send_empty` | `text` 是空串 | 空正文 |
//! | `call_send_noto` | 没有 `to` | 缺必填参数 |
//! | `call_send_badwhen` | `when` 写成别的字 | 只有两档 |
//! | `call_send_nextchild` | `next_turn` 投给子 agent | **子 agent 活不到下一轮** |
//!
//! 最后一条的拒绝文本要**直说「要留话就留给 root」**（206「做什么」§1），
//! 否则模型只会换个 id 再试一次——所以这里断的是那层意思，不是「不允许」四个字。
//!
//! 黑盒来源与「实现体没读」的声明见 `send_indep_support/mod.rs` 顶部。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::send_indep_support::{
    Route, RoutedServer, SEND_WIRE, build_ctx, index_of, sse_text, sse_tool_call, sse_tool_calls,
    temp_dir, tool_result, wire_tool_name,
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
fn six_kinds_of_undeliverable_send_are_error_results_and_the_turn_carries_on() {
    let dir = temp_dir("send-refusals");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        no_delay("call_send_self", sse_text("ROOTFINISHED 被拒了也照样收工")),
        no_delay(
            "call_r1",
            sse_tool_calls(&[
                (
                    "call_send_self",
                    SEND_WIRE,
                    r#"{"to":"root","text":"自言自语"}"#,
                ),
                (
                    "call_send_dead",
                    SEND_WIRE,
                    r#"{"to":"root/a9","text":"喂喂喂"}"#,
                ),
                (
                    "call_send_empty",
                    SEND_WIRE,
                    r#"{"to":"root/a1","text":""}"#,
                ),
                ("call_send_noto", SEND_WIRE, r#"{"text":"忘了写收信人"}"#),
                (
                    "call_send_badwhen",
                    SEND_WIRE,
                    r#"{"to":"root/a1","text":"什么时候","when":"tomorrow"}"#,
                ),
                (
                    "call_send_nextchild",
                    SEND_WIRE,
                    r#"{"to":"root/a1","text":"下一轮再聊","when":"next_turn"}"#,
                ),
            ]),
        ),
        no_delay("TASKALIVE", sse_text("AAADONE")),
        no_delay(
            "kickoff-refuse",
            sse_tool_call(
                "call_r1",
                &spawn_wire,
                r#"{"task":"TASKALIVE 一句话就答完"}"#,
            ),
        ),
    ]);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_send();
    let (mut ctx, events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(
        &mut session,
        &mut ctx,
        "kickoff-refuse 一口气发六条发不出去的",
    )
    .expect("被拒的 send 不该变成 source failure");
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "被拒的调用不该把这一轮弄停"
    );

    let root = AgentId::root();
    let a1 = root.child(1);
    assert!(
        session.is_live(&a1),
        "root/a1 该真的活着，否则几条断言测的是别的事"
    );

    // --- 六条全是 is_error ---
    let mut bodies = Vec::new();
    for call in [
        "call_send_self",
        "call_send_dead",
        "call_send_empty",
        "call_send_noto",
        "call_send_badwhen",
        "call_send_nextchild",
    ] {
        let (body, is_error) = tool_result(&session, &root, call);
        assert!(is_error, "{call} 该是 is_error 的 tool_result：{body}");
        assert!(!body.trim().is_empty(), "{call} 的拒绝文本不该是空的");
        bodies.push((call, body));
    }
    let body_of = |call: &str| {
        bodies
            .iter()
            .find(|(c, _)| *c == call)
            .map(|(_, b)| b.clone())
            .unwrap()
    };

    // 不活的那个：点名是哪个 id，并给出「你现在能发给谁」。
    let dead = body_of("call_send_dead");
    assert!(dead.contains("root/a9"), "该点名是哪个 id：{dead}");
    assert!(
        dead.contains(a1.as_str()),
        "拒绝文本该把现在能发给谁一并给出：{dead}"
    );

    // `next_turn` 投给子 agent：要说到「留给 root」这层意思，不是干巴巴一句不允许。
    let next_child = body_of("call_send_nextchild");
    assert!(
        next_child.contains("root"),
        "该指出下一轮活着的只有 root：{next_child}"
    );
    assert!(
        next_child.contains("下一轮"),
        "该说清是「活不到下一轮」而不是别的什么限制：{next_child}"
    );

    // --- 一条都没真的投出去 ---
    assert!(
        session.inbox_of(&a1).is_empty(),
        "被拒的三条冲着 root/a1 去的，一条都不该落进它的收件箱：{:?}",
        session.inbox_of(&a1)
    );
    assert!(
        session.inbox_of(&root).is_empty(),
        "给自己发的那条也不该落进自己的收件箱：{:?}",
        session.inbox_of(&root)
    );

    // --- loop 照常往下走 ---
    assert!(
        index_of(&session, &root, "ROOTFINISHED").is_some(),
        "被拒之后 root 该照常收尾：{:#?}",
        session.messages_of(&root)
    );
    assert_eq!(
        server.calls().len(),
        4,
        "四跳 = root 3 + 子 1，被拒不该多花一次调用：{:?}",
        server.calls().iter().map(|c| c.needle).collect::<Vec<_>>()
    );
    drop(events);
}
