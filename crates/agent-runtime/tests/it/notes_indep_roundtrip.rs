//! 209 runtime 端到端：模型真的调一次 `srv:agent/notes/set` 再调一次
//! `srv:agent/notes`，第二次的 tool_result 里有第一次记的东西——照
//! `self_indep_turns_used_changes.rs` / `status_indep_whole_tree.rs` 的夹具写法
//! （假 SSE 服务器按请求体路由，`run_turn` 真的跑一遍泵）。
//!
//! 顺带覆盖「乱序写入、读出来是 key 升序」（红线 11）与「覆盖」在**工具层**也成立
//! ——`notes_indep_basic.rs` 断的是 core 层的 `set_note`/`notes_of`，这份断的是
//! 模型只能看到的那一面：tool_result 正文。**实现体一行没读**（见
//! `notes_indep_spec.rs` 顶部）。

use std::time::Duration;

use agent_core::{AgentId, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::notes_indep_support::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_call, temp_dir, tool_result, wire_tool_name,
};

/// 一次写、一次读：第二次调用的 tool_result 里有第一次记的东西。
#[test]
fn a_note_written_by_one_call_shows_up_in_the_next_read_call() {
    let dir = temp_dir("notes-roundtrip-basic");
    let set_wire = wire_tool_name(agent_runtime::NOTES_SET_TOOL);
    let read_wire = wire_tool_name(agent_runtime::NOTES_TOOL);

    let server = RoutedServer::start(vec![
        Route {
            needle: "call_read",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("看到了"),
        },
        Route {
            needle: "call_set",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_read", &read_wire, "{}"),
        },
        Route {
            needle: "kickoff-notes",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call(
                "call_set",
                &set_wire,
                r#"{"key":"todo","value":"buy milk"}"#,
            ),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin().with_notes();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff-notes 记一条再读回来")
        .expect("这条流程不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();
    let (set_result, set_error) = tool_result(&session, &root, "call_set");
    assert!(!set_error, "写入不该失败：{set_result}");

    let (read_result, read_error) = tool_result(&session, &root, "call_read");
    assert!(!read_error, "读取不该失败：{read_result}");
    assert!(
        read_result.contains("todo"),
        "第二次读该看到第一次记的 key：{read_result}"
    );
    assert!(
        read_result.contains("buy milk"),
        "第二次读该看到第一次记的 value：{read_result}"
    );

    // 直接查 core 状态做正控：工具层看到的东西跟 core 里存的一致。
    assert_eq!(
        session.notes_of(&root).get("todo").map(|v| &**v),
        Some("buy milk")
    );
}

/// 乱序写两个 key（先 "zulu" 后 "alpha"）→ 读回来的正文里 "alpha" 排在 "zulu"
/// 前面——红线 11 在工具层的落点：模型看到的顺序也是 key 升序，不是写入顺序。
#[test]
fn keys_written_out_of_order_read_back_in_ascending_order() {
    let dir = temp_dir("notes-roundtrip-order");
    let set_wire = wire_tool_name(agent_runtime::NOTES_SET_TOOL);
    let read_wire = wire_tool_name(agent_runtime::NOTES_TOOL);

    let server = RoutedServer::start(vec![
        Route {
            needle: "call_read",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("看到了"),
        },
        Route {
            needle: "call_set2",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_read", &read_wire, "{}"),
        },
        Route {
            needle: "call_set1",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_set2", &set_wire, r#"{"key":"alpha","value":"AAA"}"#),
        },
        Route {
            needle: "kickoff-notes-order",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_set1", &set_wire, r#"{"key":"zulu","value":"ZZZ"}"#),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin().with_notes();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff-notes-order 乱序记两条")
        .expect("这条流程不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();
    let (read_result, read_error) = tool_result(&session, &root, "call_read");
    assert!(!read_error, "读取不该失败：{read_result}");

    let alpha_pos = read_result
        .find("alpha")
        .unwrap_or_else(|| panic!("该看到 alpha：{read_result}"));
    let zulu_pos = read_result
        .find("zulu")
        .unwrap_or_else(|| panic!("该看到 zulu：{read_result}"));
    assert!(
        alpha_pos < zulu_pos,
        "写入顺序是 zulu 先、alpha 后，正文里该是 alpha 排在 zulu 前面（key 升序）：\
         {read_result}"
    );
}

/// 同一个 key 写两次（覆盖）→ 读回来只看到新值，旧值不该再出现。
#[test]
fn overwriting_a_key_makes_the_read_show_only_the_new_value() {
    let dir = temp_dir("notes-roundtrip-overwrite");
    let set_wire = wire_tool_name(agent_runtime::NOTES_SET_TOOL);
    let read_wire = wire_tool_name(agent_runtime::NOTES_TOOL);

    let server = RoutedServer::start(vec![
        Route {
            needle: "call_read",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("看到了"),
        },
        Route {
            needle: "call_set2",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_read", &read_wire, "{}"),
        },
        Route {
            needle: "call_set1",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call(
                "call_set2",
                &set_wire,
                r#"{"key":"todo","value":"updated plan"}"#,
            ),
        },
        Route {
            needle: "kickoff-notes-overwrite",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call(
                "call_set1",
                &set_wire,
                r#"{"key":"todo","value":"original plan"}"#,
            ),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin().with_notes();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff-notes-overwrite 写两次同一个 key")
        .expect("这条流程不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();
    let (read_result, read_error) = tool_result(&session, &root, "call_read");
    assert!(!read_error, "读取不该失败：{read_result}");
    assert!(
        read_result.contains("updated plan"),
        "该看到覆盖之后的新值：{read_result}"
    );
    assert!(
        !read_result.contains("original plan"),
        "旧值不该再出现——覆盖不是追加：{read_result}"
    );

    assert_eq!(session.notes_of(&root).len(), 1, "同一个 key 只留一条");
}
