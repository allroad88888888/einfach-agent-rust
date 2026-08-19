//! 209 runtime 端到端：超上限的两种入参在**工具层**的行为——
//!
//! - **value 超长**：`Session::set_note` 在 core 层是硬拒（`notes_indep_deny.rs`
//!   的 `a_value_over_the_cap_is_refused_at_the_core_layer` 断的就是这个），但
//!   issue 209「做什么」2 点名工具层要**截断并如实说**（照 004 工具结果上限的
//!   同款处理）——所以工具层必须在调 `set_note` 之前自己先截一刀，两层行为不同，
//!   这份测试断的是工具层那一半。
//! - **key 超长**：core 直接拒 `NoteDenied::KeyTooLong`，issue 原文没有点名工具层
//!   要单独处理，这里断言这个拒绝原样穿透成一次 `is_error` 的 tool_result（拒绝
//!   显式可见，不是把调用悄悄吞掉、也不是把 key 截断存一份四不像）。
//!
//! 黑盒来源：docs/issues/209-notes-slot.md「做什么」2、「验收」（「超上限」一条）、
//! `NOTE_VALUE_CAP`/`NOTE_KEY_CAP` 公开常量。**实现体一行没读**（见
//! `notes_indep_spec.rs` 顶部）。

use std::time::Duration;

use agent_core::{AgentId, NOTE_KEY_CAP, NOTE_VALUE_CAP, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::notes_indep_support::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_call, temp_dir, tool_result, wire_tool_name,
};

/// value 超过 `NOTE_VALUE_CAP`：写入调用本身**不该报错**（工具层截断之后再存），
/// 而且要如实说——响应文本里得出现某种「截断/超限」的字样，不能悄悄截了却不说。
/// 存下来的东西经 core 校验必须真的落在上限以内，且短于原始输入（真的被截过，
/// 不是原样存了一份超限值）。
#[test]
fn an_overlong_value_is_truncated_by_the_tool_layer_and_reported_honestly() {
    let dir = temp_dir("notes-truncation-value");
    let set_wire = wire_tool_name(agent_runtime::NOTES_SET_TOOL);

    let long_value = "v".repeat(NOTE_VALUE_CAP + 500);
    let input = serde_json::json!({"key": "big", "value": long_value}).to_string();

    let server = RoutedServer::start(vec![
        Route {
            needle: "call_set",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("记完了"),
        },
        Route {
            needle: "kickoff-notes-value-cap",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_set", &set_wire, &input),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin().with_notes();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff-notes-value-cap 写一条超长的")
        .expect("这条流程不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();
    let (set_result, set_error) = tool_result(&session, &root, "call_set");
    assert!(
        !set_error,
        "超长 value 该被工具层截断之后写入成功，不是当场报错：{set_result}"
    );
    assert!(
        ["截断", "超限", "上限", "truncat"]
            .iter()
            .any(|n| set_result.contains(n)),
        "响应该如实说这条被截断了，不能悄悄截了却什么都不提：{set_result}"
    );

    // 正控：真的存进去了，且真的被截过——core 层校验，不是猜工具层文案。
    let stored = session.notes_of(&root);
    let stored_value = stored
        .get("big")
        .unwrap_or_else(|| panic!("截断之后这条该存在，不该整条丢掉：{stored:?}"));
    assert!(
        stored_value.len() <= NOTE_VALUE_CAP,
        "存下来的字节数该落在上限以内：{}",
        stored_value.len()
    );
    assert!(
        stored_value.len() < long_value.len(),
        "存下来的该比原始输入短——真的被截过，不是原样存了一份超限值"
    );
}

/// key 超过 `NOTE_KEY_CAP`：core 直接拒，工具层如实传回一次 `is_error`——不是
/// 把调用悄悄吞掉（模型看不到任何反馈），也不是把 key 截断之后存一份对不上模型
/// 原意的四不像。
#[test]
fn an_overlong_key_surfaces_as_an_explicit_tool_error_not_a_silent_truncation() {
    let dir = temp_dir("notes-truncation-key");
    let set_wire = wire_tool_name(agent_runtime::NOTES_SET_TOOL);

    let long_key = "k".repeat(NOTE_KEY_CAP + 50);
    let input = serde_json::json!({"key": long_key, "value": "正文"}).to_string();

    let server = RoutedServer::start(vec![
        Route {
            needle: "call_set",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("知道了"),
        },
        Route {
            needle: "kickoff-notes-key-cap",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_set", &set_wire, &input),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin().with_notes();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff-notes-key-cap 用一个超长 key")
        .expect("单个工具调用失败不该是 source failure——错误该作为 tool_result 进下一轮");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();
    let (set_result, set_error) = tool_result(&session, &root, "call_set");
    assert!(
        set_error,
        "超长 key 该被显式拒绝，tool_result 该带 is_error：{set_result}"
    );

    // 正控：core 里真的什么都没存——不是拒绝了却偷偷落了一条截断过的 key。
    assert!(
        session.notes_of(&root).is_empty(),
        "被拒的写入不该在 Notes 槽位里留下任何条目（无论是完整的还是截断过的 key）"
    );
}
