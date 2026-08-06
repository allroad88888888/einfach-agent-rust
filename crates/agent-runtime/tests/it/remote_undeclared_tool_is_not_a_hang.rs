//! 060 验收第一条：模型编出来的 `web:` 名字**不进等待槽**。
//!
//! `location` 是纯按名字推的（`tool_table::location_of`：`web:` 前缀 →
//! `Location::Web`），所以在 060 之前，模型只要吐一个工具表里根本没有的
//! `web:nope/x`，dispatch 就会替它开一个等待槽然后 `Dispatched::Nothing`——泵撞
//! 「在飞表空」收工，`run_turn` 返回 `ToolsPending`，宿主回去等一个**永远不会来**
//! 的 `POST /tool_result`。会话挂死，且全程不报错。
//!
//! 补上 `declares()` 之后它跟别的不存在的工具走同一条路：`ctx.fs.execute` 的
//! `unknown_tool` → `is_error` 的 tool_result → 模型自纠（决策 20 的兜底）。
//!
//! 对照组同在这个文件里：**真在表里**的 `browser_action`（也是 `Location::Web`）
//! 照旧进槽——证明这道闸判的是「声明了没有」，不是「远端一律不许挂起」。

use agent_core::{AgentId, ContentBlock, Session, TurnStatus};
use agent_providers::wire_name;
use agent_runtime::{run_turn, ToolTable};

use crate::support::{build_ctx_with, spawn_scripted_server, sse_text, sse_tool_call, temp_dir};

fn tool_results(session: &Session) -> Vec<(String, bool)> {
    session
        .messages()
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => Some((content.to_string(), *is_error)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_web_name_the_table_never_declared_gets_an_is_error_result_instead_of_a_waiting_slot() {
    let dir = temp_dir("remote-undeclared");
    let ghost = "web:nope/x";
    // 工具表是 `standard()`：它**有**三个真的远端工具（ask_user_question /
    // browser_action / save_file），唯独没有 `web:nope/x`。所以这条测试测的是
    // 「声明了没有」，不是「这张表压根不支持远端」。
    let tools = ToolTable::standard();
    assert!(!tools.declares(ghost), "这条测试的前提就是表里没有这个名字");
    assert!(tools.declares("browser_action"), "对照组得真的在表里");

    let port = spawn_scripted_server(vec![
        sse_tool_call(
            "call_ghost",
            &wire_name::to_wire(ghost),
            r#"{\"any\": \"thing\"}"#,
        ),
        sse_text("那个工具不存在，我换个办法。"),
    ]);
    let (mut ctx, _events) = build_ctx_with(port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "调一个不存在的 web 工具")
        .expect("undeclared tool is represented in the turn status");

    // 1. 有界返回、loop 继续（003 哲学：工具失败不中止 loop）。
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "该跑完第二跳收敛，而不是停在 ToolsPending"
    );
    // 2. **没进等待槽**——060 的核心断言。
    assert_eq!(
        ctx.pending_remote_tool_count(),
        0,
        "编出来的 web: 名字不该占一个永远等不到回传的槽"
    );
    assert_eq!(ctx.next_remote_deadline(), None, "没有槽就没有截止线");
    // 3. 模型拿到的是 `is_error`，看得见、能自纠。
    let results = tool_results(&session);
    assert_eq!(results.len(), 1, "该正好有一条 tool_result: {results:#?}");
    assert!(
        results[0].1,
        "未声明的工具名该落 is_error（unknown_tool 语义）: {results:#?}"
    );
    assert!(
        results[0].0.contains("unknown_tool"),
        "错误里该说清是「不认识这个工具」: {results:#?}"
    );
}

/// 对照组：**真在表里**的远端工具照旧挂起等回传——这道闸没有把远端通道判死。
#[test]
fn a_declared_web_tool_still_parks_in_the_waiting_slot() {
    let dir = temp_dir("remote-declared");
    let port = spawn_scripted_server(vec![sse_tool_call(
        "call_real",
        "browser_action",
        r#"{\"action\": \"render_card\"}"#,
    )]);
    let (mut ctx, _events) = build_ctx_with(port, &dir, ToolTable::standard());
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "渲染一张卡片")
        .expect("remote dispatch should not be a source failure");

    assert_eq!(
        status,
        TurnStatus::ToolsPending,
        "声明过的远端工具该等宿主回传"
    );
    assert_eq!(ctx.pending_remote_tool_count(), 1, "它该正好占一个等待槽");
    assert!(
        ctx.next_remote_deadline().is_some(),
        "060：占了槽就该有截止线"
    );
    // 脚本只挂了一跳：如果闸判错把它也当未知工具，泵会去要第二跳，
    // 服务器没有下一条脚本，这一轮不会是 `ToolsPending`。
}
